//! The live tier: what changes from second to second, read natively (no PowerShell).
//! - Claude Code's open sessions (`~/.claude/sessions/<pid>.json`: status busy / shell / idle / waiting),
//! - the work running under each session (a process snapshot: first non-shell process under a shell),
//! - Kitten Assistant's lines (`kitten-assistant status --json`, only when its files changed).
//! A thread polls every second and sends a JSON snapshot to the page whenever it differs from the last one.
//! The minute-by-minute data script still supplies git, PRs, titles and task labels.

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const SHELLS: [&str; 11] =
    ["cmd.exe", "powershell.exe", "pwsh.exe", "bash.exe", "sh.exe", "conhost.exe", "wscript.exe", "cscript.exe", "chcp.com", "openconsole.exe", "agentmon-run.exe"];
const SCRIPT_HOSTS: [&str; 6] = ["powershell.exe", "pwsh.exe", "bash.exe", "sh.exe", "wscript.exe", "cscript.exe"];
const NOT_WORK: [&str; 4] = ["agentmon.exe", "msedgewebview2.exe", "kitten-assistant.exe", "timeout.exe"];

#[derive(Clone)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub exe: String,
}

/// Every process: pid, parent pid, exe name (lowercase). One Toolhelp snapshot, a few milliseconds.
pub fn processes() -> Vec<Proc> {
    let mut out = Vec::with_capacity(512);
    // SAFETY: a snapshot handle we close below; PROCESSENTRY32W is plain data with dwSize set as required.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            out.push(Proc { pid: e.th32ProcessID, ppid: e.th32ParentProcessID, exe: String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase() });
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

/// A process's command line, read natively (no WMI): NtQueryInformationProcess(ProcessCommandLineInformation).
pub fn cmdline(pid: u32) -> Option<String> {
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation};
    use windows_sys::Win32::Foundation::UNICODE_STRING;
    // SAFETY: the handle is checked and closed; the buffer is u64-aligned (UNICODE_STRING needs 8) and the kernel
    // writes a UNICODE_STRING whose Buffer points inside it, at most `len` bytes long.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = vec![0u64; 512];
        let mut len = 0u32;
        let mut st = NtQueryInformationProcess(h, ProcessCommandLineInformation, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut len);
        if st < 0 && len as usize > buf.len() * 8 && len < 1 << 20 {
            buf = vec![0u64; (len as usize).div_ceil(8)];
            st = NtQueryInformationProcess(h, ProcessCommandLineInformation, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut len);
        }
        CloseHandle(h);
        if st < 0 {
            return None;
        }
        let us = &*(buf.as_ptr() as *const UNICODE_STRING);
        if us.Buffer.is_null() || us.Length == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(us.Buffer, us.Length as usize / 2)))
    }
}

/// "cargo build --release -j 4 -p skate-game" -> "cargo build skate-game": the program and its first two words
/// that are not options or numbers.
pub fn task_label(exe: &str, cmd: Option<&str>) -> String {
    let name = exe.trim_end_matches(".exe");
    let Some(cmd) = cmd else { return name.to_string() };
    if SCRIPT_HOSTS.contains(&format!("{name}.exe").as_str()) {
        // A script host: name the script it runs ("powershell push-at-1730.ps1"), or a Claude shell command.
        const EXTS: [&str; 7] = [".ps1", ".sh", ".py", ".vbs", ".js", ".cmd", ".bat"];
        let script = cmd.split(['"', ' ']).find(|w| EXTS.iter().any(|e| w.to_lowercase().ends_with(e)));
        if let Some(s) = script {
            return format!("{name} {}", s.rsplit(['\\', '/']).next().unwrap_or(s));
        }
        let encoded = cmd.split_whitespace().any(|w| {
            let w = w.to_lowercase();
            w.len() >= 2 && "-encodedcommand".starts_with(&w)
        });
        if encoded || cmd.contains("__claudeCodeScript") {
            return format!("{name} command");
        }
    }
    let rest = cmd.trim_start();
    let rest = if let Some(r) = rest.strip_prefix('"') { r.split_once('"').map(|x| x.1).unwrap_or("") } else { rest.split_once(' ').map(|x| x.1).unwrap_or("") };
    let words: Vec<&str> = rest.split_whitespace().filter(|w| !w.starts_with('-') && !w.chars().all(|c| c.is_ascii_digit())).take(2).collect();
    let s = std::iter::once(name).chain(words).collect::<Vec<_>>().join(" ");
    if s.chars().count() > 60 { s.chars().take(57).collect::<String>() + "..." } else { s }
}

/// Other AI agent tools on the machine: by program name, or a node / python / bun process running one.
const AGENT_EXES: [(&str, &str); 11] = [
    ("codex.exe", "Codex"), ("gemini.exe", "Gemini CLI"), ("cursor-agent.exe", "Cursor Agent"), ("opencode.exe", "OpenCode"),
    ("aider.exe", "Aider"), ("copilot.exe", "Copilot CLI"), ("goose.exe", "Goose"), ("amp.exe", "Amp"), ("qwen.exe", "Qwen Code"),
    ("crush.exe", "Crush"), ("cline.exe", "Cline"),
];
const AGENT_SCRIPTS: [(&str, &str); 10] = [
    ("@openai/codex", "Codex"), ("@google/gemini-cli", "Gemini CLI"), ("gemini-cli", "Gemini CLI"), ("cursor-agent", "Cursor Agent"),
    ("opencode", "OpenCode"), ("aider", "Aider"), ("@github/copilot", "Copilot CLI"), ("@sourcegraph/amp", "Amp"), ("qwen-code", "Qwen Code"),
    ("@anthropic-ai/claude-code", "Claude Code"),
];

/// CPU time a process has used so far (kernel + user), in milliseconds.
pub fn cpu_ms(pid: u32) -> Option<u64> {
    // SAFETY: the handle is checked and closed; the FILETIME outputs are plain data.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let (mut c, mut x, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = std::mem::zeroed();
        let ok = GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) != 0;
        CloseHandle(h);
        if !ok {
            return None;
        }
        let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
        Some((t(k) + t(u)) / 10_000)
    }
}

/// When a process started, in Unix milliseconds.
pub fn started_ms(pid: u32) -> Option<u64> {
    // SAFETY: the handle is checked and closed; the FILETIME outputs are plain data.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let (mut c, mut x, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = std::mem::zeroed();
        let ok = GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) != 0;
        CloseHandle(h);
        if !ok {
            return None;
        }
        let ticks = ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64;
        // FILETIME counts 100 ns from 1601; Unix time starts 11644473600 s later.
        (ticks / 10_000).checked_sub(11_644_473_600_000)
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The work under one session: the first non-shell process below a shell (or below claude itself), leaving out
/// helpers that started with the session (MCP servers) and known non-work programs.
fn tasks(session_pid: u32, session_start: u64, kids: &HashMap<u32, Vec<&Proc>>, starts: &mut HashMap<u32, Option<u64>>) -> Vec<Value> {
    let mut out = Vec::new();
    let mut queue = vec![session_pid];
    let mut seen = 0;
    while let Some(id) = queue.pop() {
        seen += 1;
        if seen > 200 {
            break;
        }
        for c in kids.get(&id).map(|v| v.as_slice()).unwrap_or(&[]) {
            if SHELLS.contains(&c.exe.as_str()) {
                // A script host with nothing under it is running PowerShell / shell code itself (a sleep, a wait
                // loop, a scheduled push): that is the task. Otherwise look through it.
                let leaf = kids.get(&c.pid).is_none_or(|k| k.iter().all(|g| g.exe == "conhost.exe"));
                if !(leaf && SCRIPT_HOSTS.contains(&c.exe.as_str())) {
                    queue.push(c.pid);
                    continue;
                }
            }
            if NOT_WORK.contains(&c.exe.as_str()) {
                continue;
            }
            let st = *starts.entry(c.pid).or_insert_with(|| started_ms(c.pid));
            if let Some(st) = st {
                if session_start > 0 && st < session_start + 60_000 {
                    continue;
                }
            }
            out.push(json!({ "pid": c.pid, "exe": c.exe.trim_end_matches(".exe"), "started": st }));
        }
    }
    out
}

/// Files whose names and change times say whether something may have changed.
fn signature(dir: &Path, ext: &str) -> String {
    let mut parts: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == ext))
                .map(|e| {
                    let m = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok());
                    format!("{}:{}", e.file_name().to_string_lossy(), m.map(|d| d.as_millis()).unwrap_or(0))
                })
                .collect()
        })
        .unwrap_or_default();
    parts.sort();
    parts.join("|")
}

fn kitten_json(exe: &Path) -> Value {
    Command::new(exe)
        .args(["status", "--json"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
        .map(|v| v["lines"].clone())
        .unwrap_or(Value::Null)
}

/// Total CPU of a process and everything under it (a build's compilers, a test's children).
fn subtree_cpu(root: u32, kids: &HashMap<u32, Vec<&Proc>>) -> u64 {
    let mut total = 0;
    let mut stack = vec![root];
    let mut n = 0;
    while let Some(p) = stack.pop() {
        n += 1;
        if n > 400 {
            break;
        }
        total += cpu_ms(p).unwrap_or(0);
        if let Some(ch) = kids.get(&p) {
            stack.extend(ch.iter().map(|c| c.pid));
        }
    }
    total
}

pub struct Watcher {
    claude_projects: PathBuf,
    transcripts: HashMap<String, PathBuf>,
    /// task pid -> (CPU ms of its subtree, when that last went up by a real amount)
    cpu: HashMap<u32, (u64, u64)>,
    claude_sessions: PathBuf,
    kitten_dir: PathBuf,
    kitten_exe: PathBuf,
    session_sig: String,
    sessions: Vec<Value>,
    kitten_sig: String,
    kitten: Value,
    kitten_read: Option<Instant>,
    starts: HashMap<u32, Option<u64>>,
    last_sent: String,
    /// The build gate's widget side (pauses outside builds when the machine is full).
    pub keeper: crate::gate::Keeper,
    /// Command lines read so far (by pid; a process's command line never changes).
    cmds: HashMap<u32, Option<String>>,
    /// Sub-agents per session id, refreshed every few seconds.
    subagents: HashMap<String, Value>,
    meta_cache: HashMap<PathBuf, (u64, Value)>,
    sub_read: Option<Instant>,
    jobs: Value,
    jobs_read: Option<Instant>,
}

/// A session's live sub-agents: `<session folder>/subagents/agent-*.meta.json` not finished, with a transcript
/// written in the last 20 minutes.
fn read_subagents(dir: &Path, cache: &mut HashMap<PathBuf, (u64, Value)>, now: u64) -> Value {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return Value::Array(out) };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(id) = name.strip_suffix(".meta.json") else { continue };
        let mt = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
        let meta = match cache.get(&p) {
            Some((t, v)) if *t == mt => v.clone(),
            _ => {
                let v = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).unwrap_or(Value::Null);
                cache.insert(p.clone(), (mt, v.clone()));
                v
            }
        };
        if meta["finished"].as_bool().unwrap_or(false) {
            continue;
        }
        let log = dir.join(format!("{id}.jsonl"));
        let act = std::fs::metadata(&log).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(mt);
        if now.saturating_sub(act) > 20 * 60_000 {
            continue;
        }
        out.push(json!({ "id": id, "description": meta["description"], "type": meta["agentType"],
            "background": meta["requestShape"] == "background", "activity": act / 15_000 * 15_000 }));
    }
    Value::Array(out)
}

/// Background jobs (`~/.claude/jobs/*/state.json`) touched in the last 3 days and not finished.
fn read_jobs(dir: &Path, now: u64) -> Value {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let f = e.path().join("state.json");
            let Ok(m) = std::fs::metadata(&f) else { continue };
            let t = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
            if now.saturating_sub(t) > 3 * 86_400_000 {
                continue;
            }
            let Some(v) = std::fs::read(&f).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
            let state = v["state"].as_str().unwrap_or("").to_lowercase();
            if ["done", "finished", "complete", "completed", "cancelled", "canceled"].contains(&state.as_str()) {
                continue;
            }
            out.push(json!({ "id": e.file_name().to_string_lossy(), "state": state, "detail": v["detail"], "needs": v["needs"], "updated": t }));
        }
    }
    Value::Array(out)
}

impl Watcher {
    pub fn new() -> Self {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
        let kitten_dir = local.join("KittenAssistant");
        Watcher {
            claude_projects: home.join(".claude").join("projects"),
            transcripts: HashMap::new(),
            cpu: HashMap::new(),
            claude_sessions: home.join(".claude").join("sessions"),
            kitten_exe: kitten_dir.join("kitten-assistant.exe"),
            kitten_dir,
            session_sig: String::new(),
            sessions: Vec::new(),
            kitten_sig: String::new(),
            kitten: Value::Null,
            kitten_read: None,
            starts: HashMap::new(),
            last_sent: String::new(),
            keeper: crate::gate::Keeper::new(),
            cmds: HashMap::new(),
            subagents: HashMap::new(),
            meta_cache: HashMap::new(),
            sub_read: None,
            jobs: Value::Array(Vec::new()),
            jobs_read: None,
        }
    }

    /// One poll. Returns the snapshot JSON when it differs from the last one sent.
    pub fn poll(&mut self) -> Option<String> {
        let sig = signature(&self.claude_sessions, "json");
        if sig != self.session_sig {
            self.session_sig = sig;
            self.sessions = std::fs::read_dir(&self.claude_sessions)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                        .filter_map(|e| std::fs::read(e.path()).ok())
                        .filter_map(|b| serde_json::from_slice::<Value>(&b).ok())
                        .collect()
                })
                .unwrap_or_default();
        }

        let procs = processes();
        let alive: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
        let mut kids: HashMap<u32, Vec<&Proc>> = HashMap::new();
        for p in &procs {
            kids.entry(p.ppid).or_default().push(p);
        }
        self.starts.retain(|pid, _| alive.contains_key(pid));

        let now = now_ms();
        self.cmds.retain(|pid, _| alive.contains_key(pid));
        let refresh_subs = self.sub_read.is_none_or(|t| t.elapsed() > Duration::from_secs(5));
        if refresh_subs {
            self.sub_read = Some(Instant::now());
        }
        let mut out = Vec::new();
        let mut live_tasks = Vec::new();
        let mut session_pids = Vec::new();
        for s in &self.sessions {
            let Some(pid) = s["pid"].as_u64().map(|p| p as u32) else { continue };
            if !alive.get(&pid).is_some_and(|p| p.exe == "claude.exe" || p.exe == "node.exe") {
                continue;
            }
            let start = s["startedAt"].as_u64().unwrap_or(0);
            let mut t = tasks(pid, start, &kids, &mut self.starts);
            // Is each task doing anything? CPU of its whole subtree; "cpuAt" = when it last went up.
            for task in t.iter_mut() {
                let tp = task["pid"].as_u64().unwrap_or(0) as u32;
                let total = subtree_cpu(tp, &kids);
                let e = self.cpu.entry(tp).or_insert((total, now));
                if total > e.0 + 150 {
                    *e = (total, now / 15_000 * 15_000);
                }
                task["cpuAt"] = json!(e.1);
                let exe = task["exe"].as_str().unwrap_or("").to_string();
                let cmd = self.cmds.entry(tp).or_insert_with(|| cmdline(tp)).clone();
                task["label"] = json!(task_label(&exe, cmd.as_deref()));
                live_tasks.push(tp);
            }
            session_pids.push(pid);
            // When the session last wrote to its transcript (a message, a tool call, a result).
            let id = s["sessionId"].as_str().unwrap_or("").to_string();
            if !id.is_empty() && !self.transcripts.contains_key(&id) {
                if let Ok(rd) = std::fs::read_dir(&self.claude_projects) {
                    for d in rd.flatten() {
                        let f = d.path().join(format!("{id}.jsonl"));
                        if f.exists() {
                            self.transcripts.insert(id.clone(), f);
                            break;
                        }
                    }
                }
            }
            let activity = self
                .transcripts
                .get(&id)
                .and_then(|f| std::fs::metadata(f).ok())
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64 / 15_000 * 15_000);
            // Its sub-agents (the session's own folder next to its transcript).
            if refresh_subs {
                if let Some(f) = self.transcripts.get(&id) {
                    let v = read_subagents(&f.with_extension("").join("subagents"), &mut self.meta_cache, now);
                    self.subagents.insert(id.clone(), v);
                }
            }
            out.push(json!({
                "id": s["sessionId"], "pid": pid, "cwd": s["cwd"], "status": s["status"], "name": s["name"],
                "startedAt": start, "since": s["statusUpdatedAt"], "activity": activity, "tasks": t,
                "subagents": self.subagents.get(&id).cloned().unwrap_or(Value::Array(Vec::new())),
            }));
        }
        let live_ids: Vec<String> = out.iter().filter_map(|s| s["id"].as_str().map(str::to_string)).collect();
        self.subagents.retain(|id, _| live_ids.contains(id));

        // Every other agent on the machine: Claude runs without a session window (headless, SDK), and other
        // AI agent tools (by program name, or a node / python / bun process running one). Not their helpers.
        let mut agents = Vec::new();
        let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
        for p in &procs {
            let parent_exe = by_pid.get(&p.ppid).map(|x| x.exe.as_str()).unwrap_or("");
            let kind = if p.exe == "claude.exe" {
                (!session_pids.contains(&p.pid) && parent_exe != "claude.exe").then_some("Claude Code (no window)")
            } else if let Some((_, k)) = AGENT_EXES.iter().find(|(e, _)| *e == p.exe) {
                (parent_exe != p.exe).then_some(*k)
            } else if ["node.exe", "bun.exe", "deno.exe", "python.exe", "pythonw.exe"].contains(&p.exe.as_str()) && parent_exe != p.exe {
                let cmd = self.cmds.entry(p.pid).or_insert_with(|| cmdline(p.pid)).clone().unwrap_or_default().to_lowercase();
                AGENT_SCRIPTS.iter().find(|(pat, _)| cmd.contains(pat)).map(|(_, k)| *k).filter(|k| *k != "Claude Code" || !session_pids.contains(&p.pid))
            } else {
                None
            };
            let Some(kind) = kind else { continue };
            let total = subtree_cpu(p.pid, &kids);
            let e = self.cpu.entry(p.pid).or_insert((total, now));
            if total > e.0 + 150 {
                *e = (total, now / 15_000 * 15_000);
            }
            live_tasks.push(p.pid);
            let started = *self.starts.entry(p.pid).or_insert_with(|| started_ms(p.pid));
            agents.push(json!({ "pid": p.pid, "kind": kind, "started": started, "cpuAt": e.1 }));
        }
        self.cpu.retain(|pid, _| live_tasks.contains(pid));

        // Background jobs, every 10 s.
        if self.jobs_read.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
            self.jobs_read = Some(Instant::now());
            self.jobs = read_jobs(&self.claude_projects.with_file_name("jobs"), now);
        }

        // The kitten: re-read when its files changed, and every 30 s for expiring lines.
        if self.kitten_exe.exists() {
            let ksig = format!("{}#{}", signature(&self.kitten_dir.join("sessions"), "txt"), signature(&self.kitten_dir, "txt"));
            let stale = self.kitten_read.is_none_or(|t| t.elapsed() > Duration::from_secs(30));
            if ksig != self.kitten_sig || stale {
                self.kitten_sig = ksig;
                self.kitten = kitten_json(&self.kitten_exe);
                self.kitten_read = Some(Instant::now());
            }
        }

        // The build gate, with each queued or running job tied to the Claude session it runs under.
        let mut gate = self.keeper.tick(&procs);
        let session_pids: HashMap<u32, Value> =
            out.iter().filter_map(|s| Some((s["pid"].as_u64()? as u32, s["id"].clone()))).collect();
        let parent: HashMap<u32, u32> = procs.iter().map(|p| (p.pid, p.ppid)).collect();
        for list in ["jobs", "waiting"] {
            if let Some(items) = gate[list].as_array_mut() {
                for item in items {
                    let mut cur = item["pid"].as_u64().unwrap_or(0) as u32;
                    for _ in 0..24 {
                        if let Some(id) = session_pids.get(&cur) {
                            item["sid"] = id.clone();
                            break;
                        }
                        cur = parent.get(&cur).copied().unwrap_or(0);
                        if cur == 0 {
                            break;
                        }
                    }
                }
            }
        }
        let snapshot = json!({ "sessions": out, "kitten": self.kitten, "gate": gate, "agents": agents, "jobs": self.jobs });
        let text = snapshot.to_string();
        if text == self.last_sent {
            return None;
        }
        self.last_sent = text.clone();
        Some(json!({ "t": now_ms(), "data": snapshot }).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::task_label;

    #[test]
    fn script_hosts_are_named_by_their_script() {
        let ps = r#""C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File C:\Temp\scratchpad\push-at-1730.ps1"#;
        assert_eq!(task_label("powershell", Some(ps)), "powershell push-at-1730.ps1");
        let claude = r#""powershell.exe" -NoProfile -NonInteractive -Command "$__claudeCodeScript = $env:X""#;
        assert_eq!(task_label("powershell", Some(claude)), "powershell command");
        assert_eq!(task_label("powershell", Some("powershell.exe -NoProfile -EncodedCommand dAByAGEAcAB7AGU")), "powershell command");
        assert_eq!(task_label("pwsh", Some("pwsh -enc dAByAGEA")), "pwsh command");
        assert_eq!(task_label("cargo", Some("cargo build --release -j 4 -p skate-game")), "cargo build skate-game");
    }
}
