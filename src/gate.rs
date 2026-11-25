//! The build gate: keeps heavy work (builds, tests, games) from piling up on the machine.
//!
//! - `agentmon-run -- <command>` takes a ticket in the queue folder, waits until the gate admits it (first come,
//!   first served; a slot free; CPU and memory to spare), then runs the command.
//! - Heavy work started any other way still counts against the slots. While the widget runs it pauses such a
//!   build (suspends it) when the machine is full, and resumes it when a slot frees, when the widget closes, or
//!   on the widget's next start. Games are never paused.
//! - One heavy job at a time by default; gate.json "maxSlots" sets another number, or "auto" to scale with the
//!   machine (about one per 8 cores and 16 GB, 1 to 4).
//!
//! Everything lives in %LOCALAPPDATA%\agentMon: queue\*.json (tickets), gate.json (settings, optional),
//! gate-paused.json (what the widget paused).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{GetSystemTimes, OpenThread, ResumeThread, SuspendThread, THREAD_SUSPEND_RESUME};

use crate::live::{started_ms, Proc};

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("agentMon")
}

/// Settings, with defaults; gate.json may override any of them.
pub struct Config {
    /// Build tools: count as heavy and may be paused when started outside the gate.
    pub builds: Vec<String>,
    /// Games and other heavy apps: count as heavy, never paused.
    pub games: Vec<String>,
    pub max_cpu: f64,
    pub min_free_gb: f64,
    pub max_slots: Option<usize>,
    /// Seconds between two admissions, so the load of the last one shows before the next starts.
    pub ramp_secs: u64,
    pub pause_outsiders: bool,
}

impl Config {
    pub fn load() -> Self {
        let v: Value = std::fs::read(dir().join("gate.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        let list = |k: &str, d: &[&str]| -> Vec<String> {
            v[k].as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_lowercase())).collect())
                .unwrap_or_else(|| d.iter().map(|s| s.to_string()).collect())
        };
        Config {
            builds: list("builds", &["cargo.exe", "rustc.exe", "msbuild.exe", "cmake.exe", "ninja.exe", "cl.exe", "link.exe", "devenv.exe", "gradle.exe"]),
            games: list("games", &["skate3rust*", "skate3recomp*"]),
            max_cpu: v["maxCpu"].as_f64().unwrap_or(70.0),
            min_free_gb: v["minFreeGb"].as_f64().unwrap_or(6.0),
            // One heavy job at a time unless gate.json says otherwise: a number, or "auto" to scale with the machine.
            max_slots: if v["maxSlots"] == "auto" { None } else { Some(v["maxSlots"].as_u64().unwrap_or(1).max(1) as usize) },
            ramp_secs: v["rampSecs"].as_u64().unwrap_or(30),
            pause_outsiders: v["pauseOutsiders"].as_bool().unwrap_or(true),
        }
    }
}

fn matches(name: &str, pats: &[String]) -> bool {
    pats.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == p,
    })
}

pub struct Machine {
    pub cores: usize,
    pub total_gb: f64,
    pub free_gb: f64,
}

pub fn machine() -> Machine {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    // SAFETY: MEMORYSTATUSEX is plain data with dwLength set as the call requires.
    let (total, free) = unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) != 0 { (m.ullTotalPhys, m.ullAvailPhys) } else { (0, 0) }
    };
    Machine { cores, total_gb: total as f64 / 1e9, free_gb: free as f64 / 1e9 }
}

pub fn slots(cfg: &Config, m: &Machine) -> usize {
    cfg.max_slots.unwrap_or_else(|| (m.cores / 8).min((m.total_gb / 16.0) as usize).clamp(1, 4))
}

/// System CPU load between two samples, in percent.
#[derive(Default)]
pub struct CpuMeter {
    last: Option<(u64, u64)>,
    pub load: f64,
}

impl CpuMeter {
    pub fn sample(&mut self) -> f64 {
        let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
        // SAFETY: three FILETIME outputs, plain data.
        let (idle, kernel, user) = unsafe {
            let (mut i, mut k, mut u): (FILETIME, FILETIME, FILETIME) = std::mem::zeroed();
            if GetSystemTimes(&mut i, &mut k, &mut u) == 0 {
                return self.load;
            }
            (ft(i), ft(k), ft(u))
        };
        let busy_total = (kernel + user, idle);
        if let Some((t0, i0)) = self.last {
            let (dt, di) = (busy_total.0.saturating_sub(t0), busy_total.1.saturating_sub(i0));
            if dt > 0 {
                let now = 100.0 * (dt.saturating_sub(di)) as f64 / dt as f64;
                // Smooth over a few seconds so one spike does not open or close the gate.
                self.load = if self.load == 0.0 { now } else { self.load * 0.7 + now * 0.3 };
            }
        }
        self.last = Some(busy_total);
        self.load
    }
}

/// A queue ticket: one `agentmon-run` command or batch, or a hold (a slot reserved for a session's series of
/// commands). `pid` keeps it alive: the runner for a command, the owning Claude session (or shell) for a hold.
#[allow(dead_code)]
#[derive(Clone, Default)]
pub struct Ticket {
    pub path: PathBuf,
    pub pid: u32,
    pub created: u64,
    pub session: String,
    pub cwd: String,
    pub cmd: String,
    pub running: bool,
    pub started: u64,
    /// "run", "batch" or "hold".
    pub kind: String,
    /// Keeps every other job out while it runs (timing-sensitive work).
    pub exclusive: bool,
    /// Resources it uses; two jobs with a lock in common never run at once.
    pub locks: Vec<String>,
    /// When a hold ends on its own (Unix ms; 0 = no end).
    pub expires: u64,
    /// The agentmon-run process waiting for this ticket's turn (for a hold it differs from `pid`). A waiting
    /// ticket whose waiter is gone is dropped, or nothing would ever admit it and it would block the line.
    pub waiter: u32,
}

pub fn queue_dir() -> PathBuf {
    dir().join("queue")
}

pub fn read_tickets(alive: &HashMap<u32, &Proc>) -> Vec<Ticket> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(queue_dir()) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Some(v) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
        let pid = v["pid"].as_u64().unwrap_or(0) as u32;
        let expires = v["expires"].as_u64().unwrap_or(0);
        let waiter = v["waiter"].as_u64().unwrap_or(0) as u32;
        let waiting = v["state"].as_str() != Some("running");
        if !alive.contains_key(&pid) || (expires > 0 && now_ms() > expires) || (waiting && waiter != 0 && !alive.contains_key(&waiter)) {
            // Its runner or owner is gone (finished, killed, closed), the hold ran out, or nobody waits for it
            // any more: the ticket goes too.
            let _ = std::fs::remove_file(&p);
            continue;
        }
        out.push(Ticket {
            path: p,
            pid,
            created: v["created"].as_u64().unwrap_or(0),
            session: v["session"].as_str().unwrap_or("").to_string(),
            cwd: v["cwd"].as_str().unwrap_or("").to_string(),
            cmd: v["cmd"].as_str().unwrap_or("").to_string(),
            running: v["state"].as_str() == Some("running"),
            started: v["started"].as_u64().unwrap_or(0),
            kind: v["kind"].as_str().unwrap_or("run").to_string(),
            exclusive: v["exclusive"].as_bool().unwrap_or(false),
            locks: v["locks"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            expires,
            waiter,
        });
    }
    out.sort_by_key(|t| (t.created, t.pid));
    out
}

#[allow(dead_code)]
pub fn write_ticket(t: &Ticket) {
    let v = json!({ "pid": t.pid, "created": t.created, "session": t.session, "cwd": t.cwd, "cmd": t.cmd,
        "state": if t.running { "running" } else { "waiting" }, "started": t.started,
        "kind": t.kind, "exclusive": t.exclusive, "locks": t.locks, "expires": t.expires, "waiter": t.waiter });
    let tmp = t.path.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &t.path);
    }
}

/// Heavy work outside the gate: the top-most build-tool process of each tree that is not under a gated runner,
/// and every game. (A build's child compilers belong to their root; one build = one job.)
pub struct Outsider {
    pub pid: u32,
    pub exe: String,
    pub started: u64,
    pub game: bool,
}

pub fn outsiders(procs: &[Proc], cfg: &Config, runners: &HashSet<u32>) -> Vec<Outsider> {
    let parent: HashMap<u32, u32> = procs.iter().map(|p| (p.pid, p.ppid)).collect();
    let by: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let heavy = |p: &Proc| matches(&p.exe, &cfg.builds) || matches(&p.exe, &cfg.games);
    let mut out = Vec::new();
    'next: for p in procs {
        if !heavy(p) {
            continue;
        }
        // Skip it when an ancestor is heavy (it is part of that job) or a gated runner (it is that job).
        let mut cur = parent.get(&p.pid).copied().unwrap_or(0);
        let mut hops = 0;
        while cur != 0 && hops < 32 {
            if runners.contains(&cur) {
                continue 'next;
            }
            if let Some(a) = by.get(&cur) {
                if heavy(a) {
                    continue 'next;
                }
            } else {
                break;
            }
            cur = parent.get(&cur).copied().unwrap_or(0);
            hops += 1;
        }
        out.push(Outsider { pid: p.pid, exe: p.exe.clone(), started: started_ms(p.pid).unwrap_or(0), game: matches(&p.exe, &cfg.games) });
    }
    out.sort_by_key(|o| o.started);
    out
}

/// Every process in the tree under `root`, root first.
pub fn tree(root: u32, procs: &[Proc]) -> Vec<u32> {
    let mut kids: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in procs {
        kids.entry(p.ppid).or_default().push(p.pid);
    }
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() && out.len() < 600 {
        if let Some(k) = kids.get(&out[i]) {
            out.extend(k.iter().copied());
        }
        i += 1;
    }
    out
}

/// Suspend (or resume) every thread of every process in `pids`.
pub fn set_suspended(pids: &[u32], suspend: bool) {
    let want: HashSet<u32> = pids.iter().copied().collect();
    // SAFETY: a thread snapshot we close below; each thread handle is checked and closed.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return;
        }
        let mut e: THREADENTRY32 = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut ok = Thread32First(snap, &mut e) != 0;
        while ok {
            if want.contains(&e.th32OwnerProcessID) {
                let h = OpenThread(THREAD_SUSPEND_RESUME, 0, e.th32ThreadID);
                if !h.is_null() {
                    if suspend {
                        SuspendThread(h);
                    } else {
                        ResumeThread(h);
                    }
                    CloseHandle(h);
                }
            }
            ok = Thread32Next(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
}

pub fn paused_file() -> PathBuf {
    dir().join("gate-paused.json")
}

/// What the widget paused: root pid -> (the pids it suspended, when).
pub fn read_paused(path: &Path) -> HashMap<u32, (Vec<u32>, u64)> {
    let v: Value = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    let mut out = HashMap::new();
    if let Some(a) = v.as_array() {
        for e in a {
            let root = e["root"].as_u64().unwrap_or(0) as u32;
            let pids = e["pids"].as_array().map(|x| x.iter().filter_map(|p| p.as_u64().map(|p| p as u32)).collect()).unwrap_or_default();
            out.insert(root, (pids, e["at"].as_u64().unwrap_or(0)));
        }
    }
    out
}

pub fn write_paused(path: &Path, paused: &HashMap<u32, (Vec<u32>, u64)>) {
    let v: Vec<Value> = paused.iter().map(|(r, (p, at))| json!({ "root": r, "pids": p, "at": at })).collect();
    let _ = std::fs::write(path, Value::Array(v).to_string());
}

/// Resume everything a previous widget paused (it closed or crashed before it could).
pub fn resume_leftovers() {
    let path = paused_file();
    let paused = read_paused(&path);
    for (pids, _) in paused.values() {
        set_suspended(pids, false);
    }
    let _ = std::fs::remove_file(path);
}

/// Whether one more job may start now, given what already runs.
pub fn room(cfg: &Config, m: &Machine, cpu: f64, jobs: usize, last_admit: u64, now: u64) -> bool {
    if jobs == 0 {
        return true;
    }
    jobs < slots(cfg, m) && cpu < cfg.max_cpu && m.free_gb >= cfg.min_free_gb && now.saturating_sub(last_admit) >= cfg.ramp_secs * 1000
}

/// Whether `me` (first in line) may start: nothing exclusive running; if `me` is exclusive, nothing at all
/// running; no running ticket sharing one of its locks; and room on the machine.
pub fn admissible(me: &Ticket, tickets: &[Ticket], cfg: &Config, m: &Machine, cpu: f64, jobs: usize, last: u64, now: u64) -> bool {
    let running: Vec<&Ticket> = tickets.iter().filter(|t| t.running && t.path != me.path).collect();
    if running.iter().any(|t| t.exclusive) {
        return false;
    }
    if me.exclusive {
        return jobs == 0;
    }
    if running.iter().any(|t| t.locks.iter().any(|l| me.locks.contains(l))) {
        return false;
    }
    room(cfg, m, cpu, jobs, last, now)
}

/// The process that owns a command run from `pid`: the nearest Claude Code session above it (so a hold covers
/// everything that session runs), else the shell that started it.
pub fn owner_of(pid: u32, procs: &[Proc]) -> u32 {
    let by: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let first_parent = by.get(&pid).map(|p| p.ppid).unwrap_or(0);
    let mut cur = first_parent;
    for _ in 0..32 {
        let Some(p) = by.get(&cur) else { break };
        if p.exe == "claude.exe" {
            return cur;
        }
        cur = p.ppid;
    }
    first_parent
}

pub fn last_admit_file() -> PathBuf {
    dir().join("gate-last-admit")
}

pub fn last_admit() -> u64 {
    std::fs::read_to_string(last_admit_file()).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// The widget's side of the gate: watches the machine every second, pauses builds started outside the gate
/// while the machine is full, resumes them in order when room frees up, and reports the queue for the page.
pub struct Keeper {
    cfg: Config,
    cfg_read: u64,
    cpu: CpuMeter,
    paused: HashMap<u32, (Vec<u32>, u64)>,
    /// Builds resumed once are never paused again (no flip-flopping a build in and out).
    released: HashSet<u32>,
}

impl Keeper {
    pub fn new() -> Self {
        resume_leftovers();
        Keeper { cfg: Config::load(), cfg_read: now_ms(), cpu: CpuMeter::default(), paused: HashMap::new(), released: HashSet::new() }
    }

    /// Resume everything this keeper paused (the widget is closing).
    pub fn release_all(&mut self) {
        for (pids, _) in self.paused.values() {
            set_suspended(pids, false);
        }
        self.paused.clear();
        let _ = std::fs::remove_file(paused_file());
    }

    pub fn tick(&mut self, procs: &[Proc]) -> Value {
        let now = now_ms();
        if now - self.cfg_read > 30_000 {
            self.cfg = Config::load();
            self.cfg_read = now;
        }
        let load = self.cpu.sample();
        let m = machine();
        let cap = slots(&self.cfg, &m);
        let alive: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
        let tickets = read_tickets(&alive);
        let runners: HashSet<u32> = tickets.iter().map(|t| t.pid).collect();
        let outs = outsiders(procs, &self.cfg, &runners);
        let mut changed = false;
        let before = self.paused.len();
        self.paused.retain(|root, _| alive.contains_key(root));
        self.released.retain(|root| alive.contains_key(root));
        changed |= before != self.paused.len();

        let running_tickets = tickets.iter().filter(|t| t.running).count();
        // An exclusive job or hold keeps the machine to itself: no outside build fits next to it.
        let exclusive = tickets.iter().any(|t| t.running && t.exclusive);
        if self.cfg.pause_outsiders {
            // Pause: in start order, an outside build that does not fit next to what started before it.
            let mut jobs = running_tickets;
            for o in &outs {
                if self.paused.contains_key(&o.pid) {
                    continue;
                }
                // A build resumed once keeps running (it counts, but is never paused again).
                if self.released.contains(&o.pid) {
                    jobs += 1;
                    continue;
                }
                let fits = !exclusive && (jobs == 0 || (jobs < cap && load < self.cfg.max_cpu + 15.0 && m.free_gb >= self.cfg.min_free_gb * 0.75));
                if fits || o.game {
                    jobs += 1;
                } else {
                    let pids = tree(o.pid, procs);
                    set_suspended(&pids, true);
                    self.paused.insert(o.pid, (pids, now));
                    changed = true;
                }
            }
            // Resume: the longest-paused build first, once there is room and no queued ticket was there before it.
            let active = running_tickets + outs.iter().filter(|o| !self.paused.contains_key(&o.pid)).count();
            if let Some((&root, &(_, at))) = self.paused.iter().min_by_key(|(_, (_, at))| *at) {
                let ticket_first = tickets.iter().any(|t| !t.running && t.created < at);
                // A pause never lasts more than 20 minutes, whatever else is running.
                let overdue = now.saturating_sub(at) > 20 * 60_000;
                if overdue || (!ticket_first && room(&self.cfg, &m, load, active, last_admit(), now)) {
                    let pids = tree(root, procs);
                    set_suspended(&pids, false);
                    self.released.insert(root);
                    if let Some((old, _)) = self.paused.remove(&root) {
                        set_suspended(&old, false);
                    }
                    let _ = std::fs::write(last_admit_file(), now.to_string());
                    changed = true;
                }
            }
        } else if !self.paused.is_empty() {
            self.release_all();
            changed = true;
        }
        if changed {
            if self.paused.is_empty() {
                let _ = std::fs::remove_file(paused_file());
            } else {
                write_paused(&paused_file(), &self.paused);
            }
        }

        let jobs: Vec<Value> = tickets
            .iter()
            .filter(|t| t.running)
            .map(|t| json!({ "pid": t.pid, "gated": true, "session": t.session, "cmd": t.cmd, "started": t.started,
                "kind": t.kind, "exclusive": t.exclusive, "locks": t.locks, "expires": t.expires }))
            .chain(outs.iter().map(|o| {
                json!({ "pid": o.pid, "gated": false, "exe": o.exe.trim_end_matches(".exe"), "started": o.started, "game": o.game,
                    "paused": self.paused.get(&o.pid).map(|(_, at)| *at) })
            }))
            .collect();
        let waiting: Vec<Value> = tickets
            .iter()
            .filter(|t| !t.running)
            .map(|t| json!({ "pid": t.pid, "session": t.session, "cmd": t.cmd, "created": t.created, "kind": t.kind, "exclusive": t.exclusive, "locks": t.locks }))
            .collect();
        json!({
            // Rounded so the page only redraws when something worth seeing moved.
            "cpu": (load / 5.0).round() * 5.0, "freeGb": (m.free_gb * 2.0).round() / 2.0, "totalGb": m.total_gb.round(),
            "cores": m.cores, "slots": cap, "maxCpu": self.cfg.max_cpu, "minFreeGb": self.cfg.min_free_gb,
            "pausing": self.cfg.pause_outsiders, "jobs": jobs, "waiting": waiting,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config { builds: vec!["cargo.exe".into()], games: vec!["skate3rust*".into()], max_cpu: 70.0, min_free_gb: 6.0, max_slots: Some(2), ramp_secs: 30, pause_outsiders: true }
    }

    #[test]
    fn an_empty_machine_always_takes_a_job() {
        let m = Machine { cores: 4, total_gb: 8.0, free_gb: 1.0 };
        assert!(room(&cfg(), &m, 99.0, 0, 0, 1));
    }

    #[test]
    fn a_second_job_needs_a_slot_cpu_memory_and_the_ramp() {
        let m = Machine { cores: 32, total_gb: 64.0, free_gb: 20.0 };
        let now = 1_000_000;
        assert!(room(&cfg(), &m, 40.0, 1, now - 31_000, now));
        assert!(!room(&cfg(), &m, 40.0, 2, now - 31_000, now), "slots full");
        assert!(!room(&cfg(), &m, 85.0, 1, now - 31_000, now), "cpu busy");
        assert!(!room(&cfg(), &m, 40.0, 1, now - 5_000, now), "still ramping");
        let low = Machine { free_gb: 3.0, ..m };
        assert!(!room(&cfg(), &low, 40.0, 1, now - 31_000, now), "memory short");
    }

    #[test]
    fn exclusive_jobs_and_locks_take_turns() {
        let m = Machine { cores: 32, total_gb: 64.0, free_gb: 30.0 };
        let now = 10_000_000;
        let t = |name: &str, running: bool, exclusive: bool, locks: &[&str]| Ticket {
            path: PathBuf::from(name), running, exclusive, locks: locks.iter().map(|s| s.to_string()).collect(), ..Default::default()
        };
        let me = t("me", false, false, &["repo-a"]);
        // A plain job next to another plain job: room decides.
        assert!(admissible(&me, &[t("x", true, false, &[])], &cfg(), &m, 20.0, 1, 0, now));
        // Not next to an exclusive one.
        assert!(!admissible(&me, &[t("x", true, true, &[])], &cfg(), &m, 20.0, 1, 0, now));
        // Not next to a job holding the same lock, even with room.
        assert!(!admissible(&me, &[t("x", true, false, &["repo-a"])], &cfg(), &m, 20.0, 1, 0, now));
        assert!(admissible(&me, &[t("x", true, false, &["repo-b"])], &cfg(), &m, 20.0, 1, 0, now));
        // An exclusive job waits for an empty machine.
        let ex = t("ex", false, true, &[]);
        assert!(!admissible(&ex, &[t("x", true, false, &[])], &cfg(), &m, 20.0, 1, 0, now));
        assert!(admissible(&ex, &[], &cfg(), &m, 20.0, 0, 0, now));
    }

    #[test]
    fn slots_scale_with_cores_and_memory() {
        let c = Config { max_slots: None, ..cfg() };
        assert_eq!(slots(&c, &Machine { cores: 8, total_gb: 16.0, free_gb: 8.0 }), 1);
        assert_eq!(slots(&c, &Machine { cores: 32, total_gb: 64.0, free_gb: 8.0 }), 4);
        assert_eq!(slots(&c, &Machine { cores: 24, total_gb: 32.0, free_gb: 8.0 }), 2);
        assert_eq!(slots(&c, &Machine { cores: 4, total_gb: 8.0, free_gb: 4.0 }), 1);
    }

    #[test]
    fn a_builds_compilers_are_part_of_its_job() {
        let p = |pid, ppid, exe: &str| Proc { pid, ppid, exe: exe.into() };
        let procs = vec![p(1, 0, "explorer.exe"), p(10, 1, "powershell.exe"), p(11, 10, "cargo.exe"), p(12, 11, "rustc.exe"), p(20, 1, "skate3rust.exe"), p(30, 1, "agentmon-run.exe"), p(31, 30, "cargo.exe")];
        let c = Config { builds: vec!["cargo.exe".into(), "rustc.exe".into()], ..cfg() };
        let runners: HashSet<u32> = [30].into_iter().collect();
        let o: Vec<u32> = outsiders(&procs, &c, &runners).iter().map(|o| o.pid).collect();
        assert!(o.contains(&11) && o.contains(&20), "{o:?}");
        assert!(!o.contains(&12), "rustc under cargo belongs to the cargo job");
        assert!(!o.contains(&31), "a gated build is not an outsider");
    }
}
