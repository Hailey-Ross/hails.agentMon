//! The slow tier, native: every project with Claude Code activity in the last N days, its recent sessions
//! (title, last prompt, last reply), git (branch, changes, worktrees), open PRs and the latest CI run (gh),
//! and where you left off (memory resume lines, todo count). Runs once a minute on its own thread and skips
//! whatever cannot have changed: log summaries are cached by size and time, git is re-read only when the
//! project's sessions wrote something, its index / HEAD moved, or 10 minutes passed, and gh is cached for
//! 10 minutes per repo. Times are Unix milliseconds.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
const MIN: u64 = 60_000;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn mtime_ms(p: &Path) -> u64 {
    std::fs::metadata(p).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Collapse whitespace and cut to `n` characters.
pub fn short(s: &str, n: usize) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > n { s.chars().take(n - 3).collect::<String>() + "..." } else { s }
}

/// Run a program quietly (no window, below-normal priority) and return its stdout when it succeeds.
fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn read_head(p: &Path, n: usize) -> String {
    let mut buf = vec![0u8; n];
    let got = std::fs::File::open(p).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    String::from_utf8_lossy(&buf[..got]).into_owned()
}

fn read_tail(p: &Path, n: u64) -> String {
    let Ok(mut f) = std::fs::File::open(p) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(n)));
    let mut buf = Vec::with_capacity(n.min(len) as usize);
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

#[derive(Clone, Default)]
struct Summary {
    title: String,
    ai_title: String,
    last_prompt: String,
    last_said: String,
}

/// The newest title, prompt and reply in a session log's last 400 KB.
fn summarize(log: &Path) -> Summary {
    let tail = read_tail(log, 400_000);
    let lines: Vec<&str> = tail.lines().collect();
    let last = |kind: &str, field: &str| -> String {
        let pat = format!("\"type\":\"{kind}\"");
        lines.iter().rev().filter(|l| l.contains(&pat)).find_map(|l| serde_json::from_str::<Value>(l).ok()).and_then(|v| v[field].as_str().map(str::to_string)).unwrap_or_default()
    };
    let custom = last("custom-title", "customTitle");
    let ai = last("ai-title", "aiTitle");
    let mut said = String::new();
    for l in lines.iter().rev().filter(|l| l.contains("\"type\":\"assistant\"") && l.contains("\"type\":\"text\"")).take(6) {
        if let Ok(v) = serde_json::from_str::<Value>(l) {
            let t: Vec<&str> = v["message"]["content"].as_array().map(|a| a.iter().filter(|c| c["type"] == "text").filter_map(|c| c["text"].as_str()).collect()).unwrap_or_default();
            let t = t.join(" ");
            if !t.trim().is_empty() {
                said = t;
                break;
            }
        }
    }
    Summary {
        title: if custom.is_empty() { ai.clone() } else { custom },
        ai_title: ai,
        last_prompt: short(&last("last-prompt", "lastPrompt"), 400),
        last_said: short(&said, 500),
    }
}

struct GitInfo {
    at: u64,
    signal: u64,
    value: Value,
}

struct GhInfo {
    at: u64,
    prs: Vec<Value>,
    run: Option<Value>,
}

pub struct Scanner {
    claude: PathBuf,
    days: u64,
    cwds: HashMap<PathBuf, String>,
    roots: HashMap<String, Option<String>>,
    sums: HashMap<PathBuf, (u64, u64, Summary)>,
    git: HashMap<String, GitInfo>,
    worktrees: HashMap<String, (u64, u64)>,
    remotes: HashMap<String, (u64, Vec<(String, String)>)>,
    gh: HashMap<String, GhInfo>,
    have_gh: bool,
    quick: bool,
}

impl Scanner {
    pub fn new() -> Self {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
        Scanner {
            claude: home.join(".claude"),
            days: 30,
            cwds: HashMap::new(),
            roots: HashMap::new(),
            sums: HashMap::new(),
            git: HashMap::new(),
            worktrees: HashMap::new(),
            remotes: HashMap::new(),
            gh: HashMap::new(),
            have_gh: run("gh", &["--version"]).is_some(),
            quick: false,
        }
    }

    /// The main repo of a folder (a worktree maps to its main checkout); None when the folder is gone.
    fn root_of(&mut self, cwd: &str) -> Option<String> {
        if let Some(r) = self.roots.get(cwd) {
            return r.clone();
        }
        let r = if Path::new(cwd).exists() {
            match run("git", &["-C", cwd, "rev-parse", "--path-format=absolute", "--git-common-dir"]) {
                Some(out) => {
                    let common = out.trim().replace('/', "\\");
                    let p = Path::new(&common);
                    if p.file_name().is_some_and(|n| n == ".git") {
                        p.parent().map(|x| x.display().to_string()).or(Some(cwd.to_string()))
                    } else {
                        Some(cwd.to_string())
                    }
                }
                None => Some(cwd.to_string()),
            }
        } else {
            None
        };
        self.roots.insert(cwd.to_string(), r.clone());
        r
    }

    fn git_status(&mut self, root: &str, activity: u64, now: u64) -> Option<Value> {
        let gitdir = Path::new(root).join(".git");
        if !gitdir.exists() {
            return None;
        }
        // Re-read only when something may have changed: the project's sessions wrote, the index or HEAD moved.
        let signal = activity.max(mtime_ms(&gitdir.join("index"))).max(mtime_ms(&gitdir.join("HEAD")));
        if let Some(g) = self.git.get(root) {
            if g.signal == signal && now - g.at < 10 * MIN {
                let mut v = g.value.clone();
                self.refresh_worktrees(&mut v, now);
                return Some(v);
            }
        }
        let mut branch = String::new();
        let (mut ahead, mut behind, mut changed) = (0i64, 0i64, 0i64);
        let mut upstream = String::new();
        for l in run("git", &["-C", root, "status", "--porcelain=v2", "--branch"]).unwrap_or_default().lines() {
            if let Some(b) = l.strip_prefix("# branch.head ") {
                branch = b.to_string();
            } else if let Some(u) = l.strip_prefix("# branch.upstream ") {
                upstream = u.to_string();
            } else if let Some(ab) = l.strip_prefix("# branch.ab ") {
                let mut it = ab.split_whitespace();
                ahead = it.next().and_then(|x| x.trim_start_matches('+').parse().ok()).unwrap_or(0);
                behind = it.next().and_then(|x| x.trim_start_matches('-').parse().ok()).unwrap_or(0);
            } else if !l.starts_with('#') && !l.is_empty() {
                changed += 1;
            }
        }
        let mut worktrees = Vec::new();
        let (mut path, mut wbranch) = (String::new(), String::from("(detached)"));
        let list = run("git", &["-C", root, "worktree", "list", "--porcelain"]).unwrap_or_default();
        for l in list.lines().chain(std::iter::once("")) {
            if let Some(p) = l.strip_prefix("worktree ") {
                path = p.replace('/', "\\");
                wbranch = "(detached)".into();
            } else if let Some(b) = l.strip_prefix("branch ") {
                wbranch = b.trim_start_matches("refs/heads/").to_string();
            } else if l.is_empty() && !path.is_empty() {
                if !path.eq_ignore_ascii_case(root) {
                    let leaf = Path::new(&path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    worktrees.push(json!({ "leaf": leaf, "path": path, "branch": wbranch, "changed": 0 }));
                }
                path.clear();
            }
        }
        worktrees.truncate(12);
        let mut v = json!({ "branch": branch, "ahead": ahead, "behind": behind, "upstream": upstream, "changed": changed, "worktrees": worktrees });
        self.refresh_worktrees(&mut v, now);
        self.git.insert(root.to_string(), GitInfo { at: now, signal, value: v.clone() });
        Some(v)
    }

    /// Worktree change counts: each one re-read at most every 3 minutes, at most 4 per scan (never-read first).
    fn refresh_worktrees(&mut self, v: &mut Value, now: u64) {
        let Some(list) = v["worktrees"].as_array_mut() else { return };
        let mut order: Vec<usize> = (0..list.len()).collect();
        order.sort_by_key(|&i| self.worktrees.get(list[i]["path"].as_str().unwrap_or("")).map(|e| e.0).unwrap_or(0));
        let mut budget = 4;
        for i in order {
            let path = list[i]["path"].as_str().unwrap_or("").to_string();
            let known = self.worktrees.get(&path).copied();
            let stale = known.is_none_or(|(at, _)| now - at > 3 * MIN);
            if stale && (known.is_none() || budget > 0) && Path::new(&path).exists() {
                if known.is_some() {
                    budget -= 1;
                }
                let n = run("git", &["-C", &path, "status", "--porcelain"]).map(|o| o.lines().filter(|l| !l.is_empty()).count() as u64).unwrap_or(0);
                self.worktrees.insert(path.clone(), (now, n));
            }
            if let Some((_, n)) = self.worktrees.get(&path) {
                list[i]["changed"] = json!(n);
            }
        }
    }

    /// GitHub repos behind a checkout's remotes: (remote name, owner/repo).
    fn gh_repos(&mut self, root: &str, now: u64) -> Vec<(String, String)> {
        if let Some((at, r)) = self.remotes.get(root) {
            if now - at < 10 * MIN {
                return r.clone();
            }
        }
        let mut out = Vec::new();
        for l in run("git", &["-C", root, "remote", "-v"]).unwrap_or_default().lines() {
            if !l.ends_with("(fetch)") {
                continue;
            }
            let mut it = l.split_whitespace();
            let (Some(name), Some(url)) = (it.next(), it.next()) else { continue };
            if let Some(pos) = url.find("github.com") {
                let rest = url[pos + 10..].trim_start_matches([':', '/']).trim_end_matches(".git");
                if rest.split('/').count() == 2 {
                    out.push((name.to_string(), rest.to_string()));
                }
            }
        }
        self.remotes.insert(root.to_string(), (now, out.clone()));
        out
    }

    fn gh_info(&mut self, repo: &str, mine_only: bool, now: u64) -> Option<(Vec<Value>, Option<Value>)> {
        let key = format!("{repo}|{mine_only}");
        if let Some(g) = self.gh.get(&key) {
            if now - g.at < 10 * MIN || !self.have_gh || self.quick {
                return Some((g.prs.clone(), g.run.clone()));
            }
        }
        // A quick scan never waits on the network; the full scan right after fills PRs and CI in.
        if !self.have_gh || self.quick {
            return None;
        }
        let mut args = vec!["pr", "list", "-R", repo, "--state", "open", "--limit", "10", "--json", "number,title,url,headRefName,isDraft,statusCheckRollup"];
        if mine_only {
            args.extend(["--author", "@me"]);
        }
        let prs: Vec<Value> = run("gh", &args).and_then(|o| serde_json::from_str(&o).ok()).unwrap_or_default();
        let runs: Vec<Value> = run("gh", &["run", "list", "-R", repo, "-L", "1", "--json", "status,conclusion,workflowName,headBranch,createdAt,url"])
            .and_then(|o| serde_json::from_str(&o).ok())
            .unwrap_or_default();
        let run0 = runs.into_iter().next();
        self.gh.insert(key, GhInfo { at: now, prs: prs.clone(), run: run0.clone() });
        Some((prs, run0))
    }

    /// A scan without GitHub (local files and git only): fast enough for the first paint.
    pub fn scan_quick(&mut self) -> Value {
        self.quick = true;
        let v = self.scan();
        self.quick = false;
        v
    }

    /// One full scan, as the page's data object.
    pub fn scan(&mut self) -> Value {
        let now = now_ms();
        let cutoff = now.saturating_sub(self.days * 24 * 60 * MIN);
        // Project folders -> root, logs, memory index.
        struct Group {
            root: String,
            logs: Vec<(PathBuf, u64, u64)>,
            memory: Vec<PathBuf>,
        }
        let mut groups: HashMap<String, Group> = HashMap::new();
        let projects_dir = self.claude.join("projects");
        let dirs: Vec<PathBuf> = std::fs::read_dir(&projects_dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
        for pd in dirs {
            let mut logs: Vec<(PathBuf, u64, u64)> = std::fs::read_dir(&pd)
                .map(|rd| {
                    rd.flatten()
                        .filter_map(|e| {
                            let p = e.path();
                            if p.extension().is_none_or(|x| x != "jsonl") {
                                return None;
                            }
                            let m = e.metadata().ok()?;
                            let t = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64;
                            (t >= cutoff).then_some((p, t, m.len()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if logs.is_empty() {
                continue;
            }
            logs.sort_by(|a, b| b.1.cmp(&a.1));
            let newest = logs[0].0.clone();
            let cwd = match self.cwds.get(&newest) {
                Some(c) => c.clone(),
                None => {
                    let head = read_head(&newest, 131_072);
                    let Some(at) = head.find("\"cwd\":\"") else { continue };
                    let rest = &head[at + 6..];
                    let Some(end) = rest[1..].find(|c| c == '"').map(|e| e + 2) else { continue };
                    let Ok(c) = serde_json::from_str::<String>(&rest[..end]) else { continue };
                    self.cwds.insert(newest.clone(), c.clone());
                    c
                }
            };
            let Some(root) = self.root_of(&cwd) else { continue };
            let g = groups.entry(root.to_lowercase()).or_insert_with(|| Group { root: root.clone(), logs: Vec::new(), memory: Vec::new() });
            logs.truncate(4);
            g.logs.extend(logs.into_iter());
            let mem = pd.join("memory").join("MEMORY.md");
            if mem.exists() {
                g.memory.push(mem);
            }
        }

        let mut projects = Vec::new();
        let mut seen_logs = Vec::new();
        for g in groups.into_values() {
            let mut logs = g.logs;
            logs.sort_by(|a, b| b.1.cmp(&a.1));
            logs.truncate(6);
            let last_active = logs.first().map(|l| l.1).unwrap_or(0);
            let mut sessions = Vec::new();
            for (path, t, len) in &logs {
                seen_logs.push(path.clone());
                let sum = match self.sums.get(path) {
                    Some((l, m, s)) if *l == *len && *m == *t => s.clone(),
                    _ => {
                        let s = summarize(path);
                        self.sums.insert(path.clone(), (*len, *t, s.clone()));
                        s
                    }
                };
                let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                sessions.push(json!({ "id": id, "name": "", "title": sum.title, "aiTitle": sum.ai_title, "where": "",
                    "running": false, "status": "", "pid": 0, "lastWrite": t, "lastPrompt": sum.last_prompt, "lastSaid": sum.last_said, "tasks": [] }));
            }
            let root = g.root;
            let name = Path::new(&root).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| root.clone());
            let git = self.git_status(&root, last_active, now);
            let (mut prs, mut runs) = (Vec::new(), Vec::new());
            if git.is_some() {
                for (remote, repo) in self.gh_repos(&root, now) {
                    let Some((p, r)) = self.gh_info(&repo, remote != "origin", now) else { continue };
                    for pr in p {
                        prs.push(json!({ "repo": repo, "number": pr["number"], "title": pr["title"], "url": pr["url"], "branch": pr["headRefName"],
                            "draft": pr["isDraft"].as_bool().unwrap_or(false), "checks": checks(&pr) }));
                    }
                    if let Some(r) = r {
                        let done = r["status"] == "completed";
                        let result = if !done { "running" } else if r["conclusion"] == "success" { "pass" } else { "fail" };
                        runs.push(json!({ "repo": repo, "workflow": r["workflowName"], "branch": r["headBranch"], "created": r["createdAt"], "url": r["url"],
                            "state": if done { r["conclusion"].clone() } else { r["status"].clone() }, "result": result }));
                    }
                }
            }
            projects.push(json!({ "name": name, "root": root, "lastActive": last_active, "sessions": sessions, "git": git, "prs": prs, "runs": runs,
                "leftOff": left_off(&root, &g.memory) }));
        }
        self.sums.retain(|p, _| seen_logs.contains(p));
        json!({ "generated": now, "days": self.days, "kitten": [], "projects": projects })
    }
}

/// A PR's checks: fail, running, pass or none.
fn checks(pr: &Value) -> &'static str {
    let roll = pr["statusCheckRollup"].as_array().cloned().unwrap_or_default();
    if roll.is_empty() {
        return "none";
    }
    let states: Vec<String> = roll
        .iter()
        .map(|c| c["conclusion"].as_str().filter(|s| !s.is_empty()).or(c["state"].as_str()).or(c["status"].as_str()).unwrap_or("").to_uppercase())
        .collect();
    if states.iter().any(|s| ["FAILURE", "ERROR", "TIMED_OUT", "CANCELLED", "ACTION_REQUIRED"].contains(&s.as_str())) {
        "fail"
    } else if states.iter().any(|s| ["PENDING", "QUEUED", "IN_PROGRESS", "EXPECTED", "WAITING"].contains(&s.as_str())) {
        "running"
    } else {
        "pass"
    }
}

/// Where you left off: resume lines from the memory index, and how many todo files there are.
fn left_off(root: &str, memory: &[PathBuf]) -> Value {
    let mut lines = Vec::new();
    for m in memory {
        let text = std::fs::read_to_string(m).unwrap_or_default();
        for l in text.lines() {
            let low = l.to_lowercase();
            let hit = ["resume", "left off", "next step", "in progress"].iter().any(|k| low.contains(k));
            if !hit || l.contains("STANDING") || low.contains("no resume") {
                continue;
            }
            // "- [Title](file.md) : text" -> "Title: text"
            let mut s = l.trim().trim_start_matches("- ").to_string();
            if s.starts_with('[') {
                if let (Some(a), Some(b)) = (s.find("]("), s.find(')')) {
                    let title = s[1..a].to_string();
                    let rest = s[b + 1..].trim_start().trim_start_matches(':').trim_start().to_string();
                    s = format!("{title}: {rest}");
                }
            }
            let s = short(&s, 300);
            if !lines.contains(&s) {
                lines.push(s);
            }
            if lines.len() >= 2 {
                break;
            }
        }
    }
    let todo = Path::new(root).join(".claude").join("todo");
    let count = std::fs::read_dir(todo)
        .map(|rd| rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "md") && e.file_name() != "README.md").count())
        .unwrap_or(0);
    json!({ "lines": lines, "todoCount": count })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_collapses_and_cuts() {
        assert_eq!(short("a  b\n c", 10), "a b c");
        assert_eq!(short("abcdefghijk", 8), "abcde...");
    }

    #[test]
    fn pr_checks_read_the_rollup() {
        assert_eq!(checks(&json!({ "statusCheckRollup": [] })), "none");
        assert_eq!(checks(&json!({ "statusCheckRollup": [{ "conclusion": "SUCCESS" }, { "conclusion": "FAILURE" }] })), "fail");
        assert_eq!(checks(&json!({ "statusCheckRollup": [{ "conclusion": "", "status": "IN_PROGRESS" }] })), "running");
        assert_eq!(checks(&json!({ "statusCheckRollup": [{ "conclusion": "SUCCESS" }, { "state": "SUCCESS" }] })), "pass");
    }
}
