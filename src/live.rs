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
const SHELLS: [&str; 10] =
    ["cmd.exe", "powershell.exe", "pwsh.exe", "bash.exe", "sh.exe", "conhost.exe", "wscript.exe", "cscript.exe", "chcp.com", "openconsole.exe"];
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
                queue.push(c.pid);
                continue;
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
        let mut out = Vec::new();
        let mut live_tasks = Vec::new();
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
                live_tasks.push(tp);
            }
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
            out.push(json!({
                "id": s["sessionId"], "pid": pid, "cwd": s["cwd"], "status": s["status"], "name": s["name"],
                "startedAt": start, "since": s["statusUpdatedAt"], "activity": activity, "tasks": t,
            }));
        }
        self.cpu.retain(|pid, _| live_tasks.contains(pid));

        // The kitten: re-read when its files changed, and every 10 s for expiring lines.
        if self.kitten_exe.exists() {
            let ksig = format!("{}#{}", signature(&self.kitten_dir.join("sessions"), "txt"), signature(&self.kitten_dir, "txt"));
            let stale = self.kitten_read.is_none_or(|t| t.elapsed() > Duration::from_secs(10));
            if ksig != self.kitten_sig || stale {
                self.kitten_sig = ksig;
                self.kitten = kitten_json(&self.kitten_exe);
                self.kitten_read = Some(Instant::now());
            }
        }

        let snapshot = json!({ "sessions": out, "kitten": self.kitten });
        let text = snapshot.to_string();
        if text == self.last_sent {
            return None;
        }
        self.last_sent = text.clone();
        Some(json!({ "t": now_ms(), "data": snapshot }).to_string())
    }
}
