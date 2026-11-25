//! agentmon-run: take turns on the machine through agentMon's build gate.
//!
//!   agentmon-run [OPTIONS] -- <command> [args...]        run one heavy command when a slot is free
//!   agentmon-run batch [OPTIONS] -- "<cmd>" "<cmd>" ...  one slot for several commands, run back to back
//!   agentmon-run hold [OPTIONS] [--for 30m]               reserve a slot for this session's next commands
//!   agentmon-run release                                  give this session's reserved slot back
//!   agentmon-run status                                   print the queue
//!
//! OPTIONS: --session NAME   who this is (default: KITTEN_SESSION, else the folder name)
//!          --exclusive      keep every other job out while this runs (timing-sensitive work)
//!          --lock NAME      never run at the same time as another job with this lock (repeatable);
//!                           --lock auto = the git repo of the current folder
//!          --wait-max MIN   give up after waiting this many minutes (exit 3)
//!          --keep-going     (batch) run the rest after a failure
//!
//! While a session holds a slot, every agentmon-run from that same Claude Code session starts at once, inside
//! the reservation, so a series of tests cannot lose its turn between steps. A hold ends with `release`, when
//! its time is up, or when the session closes. Waiting lines go to stderr; commands keep this console.
#![allow(dead_code)]

#[path = "../gate.rs"]
mod gate;
#[path = "../live.rs"]
mod live;

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use gate::{Config, CpuMeter, Ticket};

fn usage() -> ExitCode {
    eprintln!("usage: agentmon-run [--session NAME] [--exclusive] [--lock NAME] [--wait-max MIN] -- <command> [args...]");
    eprintln!("       agentmon-run batch [OPTIONS] [--keep-going] -- \"<cmd>\" \"<cmd>\" ...");
    eprintln!("       agentmon-run hold [OPTIONS] [--for 30m]");
    eprintln!("       agentmon-run release | status");
    ExitCode::from(2)
}

fn ordinal(n: usize) -> String {
    let s = match (n % 10, n % 100) {
        (1, x) if x != 11 => "st",
        (2, x) if x != 12 => "nd",
        (3, x) if x != 13 => "rd",
        _ => "th",
    };
    format!("{n}{s}")
}

/// "30m", "90", "1h30m", "45s" -> milliseconds (a bare number = minutes).
fn parse_duration(s: &str) -> Option<u64> {
    if let Ok(m) = s.parse::<u64>() {
        return Some(m * 60_000);
    }
    let (mut total, mut num) = (0u64, String::new());
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            let n: u64 = num.parse().ok()?;
            num.clear();
            total += match c {
                'h' => n * 3_600_000,
                'm' => n * 60_000,
                's' => n * 1_000,
                _ => return None,
            };
        }
    }
    num.is_empty().then_some(total)
}

struct Opts {
    session: String,
    exclusive: bool,
    locks: Vec<String>,
    wait_max: Option<Duration>,
    keep_going: bool,
    hold_for: u64,
}

fn repo_root(cwd: &str) -> Option<String> {
    let out = Command::new("git").args(["-C", cwd, "rev-parse", "--path-format=absolute", "--git-common-dir"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let common = String::from_utf8_lossy(&out.stdout).trim().replace('/', "\\");
    let p = std::path::Path::new(&common);
    Some(if p.file_name().is_some_and(|n| n == ".git") { p.parent()?.display().to_string() } else { common })
}

fn snapshot() -> (Vec<live::Proc>, Vec<Ticket>) {
    let procs = live::processes();
    let tickets = {
        let alive: HashMap<u32, &live::Proc> = procs.iter().map(|p| (p.pid, p)).collect();
        gate::read_tickets(&alive)
    };
    (procs, tickets)
}

/// Wait in line until the gate admits `ticket` (or give up), then mark it running.
fn wait_turn(ticket: &mut Ticket, opts: &Opts) -> bool {
    let cfg = Config::load();
    let mut cpu = CpuMeter::default();
    cpu.sample();
    let began = Instant::now();
    let (mut last_note, mut last_print) = (String::new(), Instant::now() - Duration::from_secs(60));
    loop {
        std::thread::sleep(Duration::from_millis(1000));
        let load = cpu.sample();
        let m = gate::machine();
        let (procs, tickets) = snapshot();
        let runners: HashSet<u32> = tickets.iter().map(|t| t.pid).collect();
        let paused: HashSet<u32> = gate::read_paused(&gate::paused_file()).into_keys().collect();
        let outs = gate::outsiders(&procs, &cfg, &runners);
        let jobs = tickets.iter().filter(|t| t.running).count() + outs.iter().filter(|o| !paused.contains(&o.pid)).count();
        let ahead = tickets.iter().filter(|t| !t.running && (t.created, t.pid) < (ticket.created, ticket.pid)).count();
        let now = gate::now_ms();
        if ahead == 0 && gate::admissible(ticket, &tickets, &cfg, &m, load, jobs, gate::last_admit(), now) {
            // One admission at a time across all runners: a lock file only one of them can create.
            let lock_path = gate::dir().join("gate-admit.lock");
            if let Ok(meta) = std::fs::metadata(&lock_path) {
                if meta.modified().ok().and_then(|t| t.elapsed().ok()).is_some_and(|e| e > Duration::from_secs(15)) {
                    let _ = std::fs::remove_file(&lock_path);
                }
            }
            if OpenOptions::new().write(true).create_new(true).open(&lock_path).is_ok() {
                ticket.running = true;
                ticket.started = now;
                if ticket.kind == "hold" {
                    ticket.expires = now + opts.hold_for;
                }
                gate::write_ticket(ticket);
                let _ = std::fs::write(gate::last_admit_file(), now.to_string());
                let _ = std::fs::remove_file(&lock_path);
                if began.elapsed() > Duration::from_secs(2) {
                    eprintln!("agentmon-run: slot free after {} s", began.elapsed().as_secs());
                }
                return true;
            }
            continue;
        }
        let why = if tickets.iter().any(|t| t.running && t.exclusive) {
            "an exclusive job is running".to_string()
        } else if ticket.exclusive && jobs > 0 {
            format!("waiting for the machine to clear ({jobs} running)")
        } else if tickets.iter().any(|t| t.running && t.locks.iter().any(|l| ticket.locks.contains(l))) {
            "another job holds the same lock".to_string()
        } else {
            format!("{jobs} heavy job{} running, CPU {load:.0}%, {:.1} GB free", if jobs == 1 { "" } else { "s" }, m.free_gb)
        };
        let note = if ahead > 0 { format!("waiting for a slot: {} in line; {why}", ordinal(ahead + 1)) } else { format!("waiting for a slot: next in line; {why}") };
        // Reprint when the reason changes, not when only the numbers in it move.
        let key: String = note.chars().filter(|c| !c.is_ascii_digit() && *c != '.').collect();
        if key != last_note || last_print.elapsed() > Duration::from_secs(60) {
            eprintln!("agentmon-run: {note}");
            last_note = key;
            last_print = Instant::now();
        }
        if opts.wait_max.is_some_and(|w| began.elapsed() > w) {
            eprintln!("agentmon-run: gave up after waiting {} min", began.elapsed().as_secs() / 60);
            let _ = std::fs::remove_file(&ticket.path);
            return false;
        }
    }
}

fn exit_of(s: std::io::Result<std::process::ExitStatus>, what: &str) -> u8 {
    match s {
        Ok(s) => s.code().map(|c| c.clamp(0, 255) as u8).unwrap_or(1),
        Err(e) => {
            eprintln!("agentmon-run: could not start {what}: {e}");
            127
        }
    }
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match args.first().map(String::as_str) {
        Some("hold" | "release" | "batch" | "status") => args.remove(0),
        _ => "run".to_string(),
    };
    let split = args.iter().position(|a| a == "--");
    let (opt_args, cmd): (Vec<String>, Vec<String>) = match split {
        Some(i) => (args[..i].to_vec(), args[i + 1..].to_vec()),
        None => (args.clone(), Vec::new()),
    };
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let mut o = Opts { session: std::env::var("KITTEN_SESSION").unwrap_or_default(), exclusive: false, locks: Vec::new(), wait_max: None, keep_going: false, hold_for: 30 * 60_000 };
    let mut i = 0;
    while i < opt_args.len() {
        let next = opt_args.get(i + 1).cloned();
        match (opt_args[i].as_str(), next) {
            ("--session", Some(v)) => { o.session = v; i += 1; }
            ("--lock", Some(v)) => {
                if v == "auto" {
                    if let Some(r) = repo_root(&cwd) { o.locks.push(r.to_lowercase()); }
                } else {
                    o.locks.push(v.to_lowercase());
                }
                i += 1;
            }
            ("--wait-max", Some(v)) => { o.wait_max = v.parse::<u64>().ok().map(|m| Duration::from_secs(m * 60)); i += 1; }
            ("--for", Some(v)) => { o.hold_for = parse_duration(&v).unwrap_or(30 * 60_000).clamp(60_000, 8 * 3_600_000); i += 1; }
            ("--exclusive", _) => o.exclusive = true,
            ("--keep-going", _) => o.keep_going = true,
            _ => return usage(),
        }
        i += 1;
    }
    if o.session.is_empty() {
        o.session = std::path::Path::new(&cwd).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    }
    let _ = std::fs::create_dir_all(gate::queue_dir());
    let me = std::process::id();
    let (procs, tickets) = snapshot();
    let owner = gate::owner_of(me, &procs);

    match mode.as_str() {
        "status" => {
            if tickets.is_empty() {
                println!("queue empty");
            }
            for t in &tickets {
                let state = if t.running { "running" } else { "waiting" };
                let extra = [if t.exclusive { "exclusive" } else { "" }, if t.locks.is_empty() { "" } else { "locked" }].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(", ");
                println!("{state:8} {:6} {:24} {}{}", t.kind, t.session, t.cmd, if extra.is_empty() { String::new() } else { format!("  ({extra})") });
            }
            return ExitCode::SUCCESS;
        }
        "release" => {
            let mine: Vec<&Ticket> = tickets.iter().filter(|t| t.kind == "hold" && t.pid == owner).collect();
            for t in &mine {
                let _ = std::fs::remove_file(&t.path);
            }
            eprintln!("agentmon-run: {}", if mine.is_empty() { "no slot held by this session" } else { "slot released" });
            return ExitCode::SUCCESS;
        }
        _ => {}
    }

    // Inside this session's own reservation: start at once.
    let held = tickets.iter().find(|t| t.kind == "hold" && t.running && t.pid == owner).cloned();
    if mode == "hold" {
        if let Some(h) = held {
            // Holding already: extend it.
            let mut h = h;
            h.expires = gate::now_ms() + o.hold_for;
            gate::write_ticket(&h);
            eprintln!("agentmon-run: slot already held; extended by {} min", o.hold_for / 60_000);
            return ExitCode::SUCCESS;
        }
        let created = gate::now_ms();
        let mut t = Ticket {
            path: gate::queue_dir().join(format!("{created}-{owner}-hold.json")),
            pid: owner, created, session: o.session.clone(), cwd, cmd: "(reserved for a series of commands)".into(),
            kind: "hold".into(), exclusive: o.exclusive, locks: o.locks.clone(), waiter: me, ..Default::default()
        };
        gate::write_ticket(&t);
        if !wait_turn(&mut t, &o) {
            return ExitCode::from(3);
        }
        eprintln!("agentmon-run: slot held for {} min; run commands with agentmon-run as usual, then `agentmon-run release`", o.hold_for / 60_000);
        return ExitCode::SUCCESS;
    }

    if cmd.is_empty() {
        return usage();
    }
    let mut ticket = None;
    if held.is_none() {
        let created = gate::now_ms();
        let mut t = Ticket {
            path: gate::queue_dir().join(format!("{created}-{me}.json")),
            pid: me, created, session: o.session.clone(), cwd, cmd: cmd.join(" "),
            kind: mode.clone(), exclusive: o.exclusive, locks: o.locks.clone(), waiter: me, ..Default::default()
        };
        gate::write_ticket(&t);
        if !wait_turn(&mut t, &o) {
            return ExitCode::from(3);
        }
        ticket = Some(t);
    } else if o.exclusive || !o.locks.is_empty() {
        eprintln!("agentmon-run: inside this session's held slot (its own exclusive / lock settings apply)");
    }

    let code = if mode == "batch" {
        let mut first_fail = 0u8;
        for (n, c) in cmd.iter().enumerate() {
            eprintln!("agentmon-run: [{}/{}] {c}", n + 1, cmd.len());
            let code = exit_of(Command::new("cmd").args(["/C", c]).status(), c);
            if code != 0 {
                if first_fail == 0 {
                    first_fail = code;
                }
                if !o.keep_going {
                    eprintln!("agentmon-run: stopped after a failure (exit {code})");
                    break;
                }
            }
        }
        first_fail
    } else {
        exit_of(Command::new(&cmd[0]).args(&cmd[1..]).status(), &cmd[0])
    };
    if let Some(t) = ticket {
        let _ = std::fs::remove_file(&t.path);
    }
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_like_the_kitten() {
        assert_eq!(parse_duration("30"), Some(30 * 60_000));
        assert_eq!(parse_duration("45s"), Some(45_000));
        assert_eq!(parse_duration("1h30m"), Some(90 * 60_000));
        assert_eq!(parse_duration("2x"), None);
    }
}
