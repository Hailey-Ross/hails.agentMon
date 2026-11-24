//! agentMon: a small always-available desktop widget that shows what every Claude Code session is
//! doing. The hive UI (index.html, board.js, board.css, fonts) and the data script (agentmon.ps1)
//! are built into the exe; data lives in %LOCALAPPDATA%\agentMon.
//!
//! Two tiers feed the page: the live tier (live.rs, native, every second: open sessions, their work,
//! the kitten) is pushed straight into the page; the data script (every minute: git, PRs, titles,
//! where you left off) writes board-data.js and the page is told to reload it.
#![windows_subsystem = "windows"]

mod focus;
mod live;
mod update;

use std::borrow::Cow;
use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::windows::IconExtWindows;
use tao::window::{Icon, ResizeDirection, UserAttentionType, Window, WindowBuilder};
use wry::http::{header::CONTENT_TYPE, Request, Response, StatusCode};
use wry::{WebContext, WebViewBuilder};

const INDEX: &str = include_str!("../index.html");
const BOARD_JS: &str = include_str!("../board.js");
const BOARD_CSS: &str = include_str!("../board.css");
const DATA_SCRIPT: &str = include_str!("../agentmon.ps1");
const FONTS: [(&str, &[u8]); 3] = [
    ("/fonts/bricolage-grotesque-latin.woff2", include_bytes!("../fonts/bricolage-grotesque-latin.woff2")),
    ("/fonts/figtree-latin.woff2", include_bytes!("../fonts/figtree-latin.woff2")),
    ("/fonts/jetbrains-mono-latin.woff2", include_bytes!("../fonts/jetbrains-mono-latin.woff2")),
];

const REFRESH_EVERY: Duration = Duration::from_secs(60);
const LIVE_EVERY: Duration = Duration::from_millis(1000);
const UPDATE_EVERY: Duration = Duration::from_secs(6 * 3600);
const SAVE_DELAY: Duration = Duration::from_millis(600);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

enum UserEvent {
    Drag,
    Resize(ResizeDirection),
    Minimize,
    Close,
    Pin(bool),
    Size(f64, f64),
    /// A live-tier snapshot (JSON) for the page.
    Live(String),
    /// The data script wrote a new board-data.js.
    DataReady,
    /// Whether a session's window could be brought to the front.
    Focused(bool),
    /// Flash the taskbar button (the page decides when).
    Attention,
    /// An update is staged ("0.2.0 build 42"), or the check said none / failed.
    UpdateStaged(String),
    UpdateNote(String),
    /// Swap the staged update in and restart.
    UpdateNow,
}

/// "size:440x760" -> logical width and height, within sane bounds.
fn parse_size(m: &str) -> Option<(f64, f64)> {
    let (w, h) = m.strip_prefix("size:")?.split_once('x')?;
    let (w, h) = (w.parse::<f64>().ok()?, h.parse::<f64>().ok()?);
    ((340.0..=3840.0).contains(&w) && (300.0..=2160.0).contains(&h)).then_some((w, h))
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("agentMon")
}

/// Runs the data script now, then every minute, or sooner when the page asks through `wake`.
fn start_refresher(dir: PathBuf, wake: mpsc::Receiver<()>, proxy: EventLoopProxy<UserEvent>) {
    let script = dir.join("agentmon.ps1");
    if fs::read_to_string(&script).ok().as_deref() != Some(DATA_SCRIPT) {
        let _ = fs::write(&script, DATA_SCRIPT);
    }
    thread::spawn(move || loop {
        let ok = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .arg("-OutDir")
            .arg(&dir)
            .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)
            .status()
            .is_ok_and(|s| s.success());
        if ok && proxy.send_event(UserEvent::DataReady).is_err() {
            return;
        }
        // Several quick wake-ups in a row (sessions opening) collapse into one run.
        match wake.recv_timeout(REFRESH_EVERY) {
            Ok(()) => while wake.try_recv().is_ok() {},
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    });
}

/// Polls the live tier every second; sends a snapshot only when something changed.
fn start_live(proxy: EventLoopProxy<UserEvent>) {
    thread::spawn(move || {
        let mut w = live::Watcher::new();
        loop {
            if let Some(snapshot) = w.poll() {
                if proxy.send_event(UserEvent::Live(snapshot)).is_err() {
                    return;
                }
            }
            thread::sleep(LIVE_EVERY);
        }
    });
}

/// Checks for a newer stable release a minute after start, then every 6 hours (numbered builds only).
fn start_updater(proxy: EventLoopProxy<UserEvent>, now: mpsc::Receiver<()>) {
    update::clean_up();
    thread::spawn(move || {
        let mut wait = Duration::from_secs(60);
        loop {
            let asked = matches!(now.recv_timeout(wait), Ok(()));
            wait = UPDATE_EVERY;
            if !update::updates_itself() {
                if asked {
                    let _ = proxy.send_event(UserEvent::UpdateNote(format!("{} does not update itself", update::label())));
                }
                continue;
            }
            let ev = match update::check_and_stage() {
                Ok(Some(v)) => UserEvent::UpdateStaged(v),
                Ok(None) => UserEvent::UpdateNote(format!("Up to date ({})", update::label())),
                Err(e) => UserEvent::UpdateNote(format!("Update check failed: {e}")),
            };
            let staged = matches!(ev, UserEvent::UpdateStaged(_));
            if (staged || asked) && proxy.send_event(ev).is_err() {
                return;
            }
        }
    });
}

fn load_state(path: &Path) -> Value {
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}))
}

fn save_state(path: &Path, state: &mut Value, window: &Window) {
    if let Ok(p) = window.outer_position() {
        state["x"] = json!(p.x);
        state["y"] = json!(p.y);
    }
    let s = window.inner_size();
    if s.width > 0 && s.height > 0 {
        state["w"] = json!(s.width);
        state["h"] = json!(s.height);
    }
    let _ = fs::write(path, state.to_string());
}

/// The app icon embedded by build.rs (resource 1), the same one Explorer and the taskbar show.
fn app_icon() -> Option<Icon> {
    Icon::from_resource(1, None).ok()
}

fn serve(dir: &Path, req: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let (body, mime): (Cow<'static, [u8]>, &str) = match req.uri().path() {
        "/" | "/index.html" => (Cow::Borrowed(INDEX.as_bytes()), "text/html; charset=utf-8"),
        "/board.js" => (Cow::Borrowed(BOARD_JS.as_bytes()), "text/javascript; charset=utf-8"),
        "/board.css" => (Cow::Borrowed(BOARD_CSS.as_bytes()), "text/css; charset=utf-8"),
        "/board-data.js" => match fs::read(dir.join("board-data.js")) {
            Ok(b) => (Cow::Owned(b), "text/javascript; charset=utf-8"),
            Err(_) => (Cow::Borrowed(&b"window.BOARD_DATA = window.BOARD_DATA || null;"[..]), "text/javascript"),
        },
        p => match FONTS.iter().find(|(n, _)| *n == p) {
            Some((_, b)) => (Cow::Borrowed(*b), "font/woff2"),
            None => return Response::builder().status(StatusCode::NOT_FOUND).body(Cow::Borrowed(&b""[..])).unwrap(),
        },
    };
    Response::builder().header(CONTENT_TYPE, mime).header("Cache-Control", "no-store").body(body).unwrap()
}

/// Opens a link from the page in the default browser (https only).
fn open_external(url: &str) {
    if url.starts_with("https://") && !url.contains('"') {
        let _ = Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

/// A JSON string literal for evaluate_script.
fn js_str(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

fn main() {
    let dir = data_dir();
    let _ = fs::create_dir_all(&dir);

    // One agentMon at a time: the lock file stays open, unshared, while the app runs.
    let lock = fs::OpenOptions::new().write(true).create(true).truncate(false).share_mode(0).open(dir.join("instance.lock"));
    let Ok(lock) = lock else { return };

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    let (update_tx, update_rx) = mpsc::channel::<()>();
    start_refresher(dir.clone(), wake_rx, proxy.clone());
    start_live(proxy.clone());
    start_updater(proxy.clone(), update_rx);

    let state_path = dir.join("window.json");
    let mut state = load_state(&state_path);
    let pinned = state["pinned"].as_bool().unwrap_or(false);

    let mut wb = WindowBuilder::new()
        .with_title("agentMon")
        .with_decorations(false)
        .with_always_on_top(pinned)
        .with_window_icon(app_icon())
        .with_min_inner_size(LogicalSize::new(340.0, 300.0))
        .with_inner_size(LogicalSize::new(440.0, 760.0));
    if let (Some(w), Some(h)) = (state["w"].as_u64(), state["h"].as_u64()) {
        wb = wb.with_inner_size(PhysicalSize::new(w as u32, h as u32));
    }
    if let (Some(x), Some(y)) = (state["x"].as_i64(), state["y"].as_i64()) {
        let (x, y) = (x as i32, y as i32);
        // Only restore a position that is still on a connected monitor.
        let on_screen = event_loop.available_monitors().any(|m| {
            let (p, s) = (m.position(), m.size());
            x + 40 >= p.x && x < p.x + s.width as i32 - 40 && y >= p.y && y < p.y + s.height as i32 - 40
        });
        if on_screen {
            wb = wb.with_position(PhysicalPosition::new(x, y));
        }
    }
    let window = wb.build(&event_loop).expect("agentMon: could not open its window");

    let mut web_context = WebContext::new(Some(dir.join("webview")));
    let serve_dir = dir.clone();
    let ipc_proxy = proxy.clone();
    let webview = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_background_color((20, 16, 10, 255))
        .with_initialization_script(&format!(
            "window.AGENTMON_APP = true; window.AGENTMON_PINNED = {pinned}; window.AGENTMON_VERSION = {};",
            js_str(&update::label())
        ))
        .with_custom_protocol("agentmon".into(), move |_id, req| serve(&serve_dir, &req))
        .with_ipc_handler(move |req: Request<String>| {
            let msg = req.body().as_str();
            let ev = match msg {
                "drag" => Some(UserEvent::Drag),
                "min" => Some(UserEvent::Minimize),
                "close" => Some(UserEvent::Close),
                "pin:1" => Some(UserEvent::Pin(true)),
                "pin:0" => Some(UserEvent::Pin(false)),
                "resize:se" => Some(UserEvent::Resize(ResizeDirection::SouthEast)),
                "attention" => Some(UserEvent::Attention),
                "update-check" => {
                    let _ = update_tx.send(());
                    None
                }
                "update-now" => Some(UserEvent::UpdateNow),
                "refresh" => {
                    let _ = wake_tx.send(());
                    None
                }
                m => {
                    if let Some(url) = m.strip_prefix("open:") {
                        open_external(url);
                        None
                    } else if let Some(rest) = m.strip_prefix("focus:") {
                        // focus:<pid>|<project folder name>
                        let (pid, hint) = rest.split_once('|').unwrap_or((rest, ""));
                        pid.parse::<u32>().ok().map(|pid| UserEvent::Focused(focus::focus_session(pid, hint)))
                    } else {
                        parse_size(m).map(|(w, h)| UserEvent::Size(w, h))
                    }
                }
            };
            if let Some(ev) = ev {
                let _ = ipc_proxy.send_event(ev);
            }
        })
        .with_url("http://agentmon.localhost/index.html")
        .build(&window)
        .expect("agentMon: could not start WebView2 (is the Microsoft Edge WebView2 Runtime installed?)");

    let mut save_at: Option<Instant> = None;
    let mut lock = Some(lock);
    let mut hidden = false;
    let mut last_live: Option<String> = None;
    event_loop.run(move |event, _, control_flow| {
        let js = |code: String| {
            let _ = webview.evaluate_script(&code);
        };
        match event {
            Event::WindowEvent { event: WindowEvent::Moved(_), .. } => save_at = Some(Instant::now() + SAVE_DELAY),
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                // Minimized: stop the page from drawing (no animation work while nobody can see it).
                let now_hidden = size.width == 0 || size.height == 0 || window.is_minimized();
                if now_hidden != hidden {
                    hidden = now_hidden;
                    let _ = webview.set_visible(!hidden);
                    if !hidden {
                        if let Some(s) = &last_live {
                            js(format!("window.agentMonLive && window.agentMonLive({s})"));
                        }
                    }
                }
                if !hidden {
                    save_at = Some(Instant::now() + SAVE_DELAY);
                }
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if save_at.take().is_some() {
                    save_state(&state_path, &mut state, &window);
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } | Event::UserEvent(UserEvent::Close) => {
                save_state(&state_path, &mut state, &window);
                *control_flow = ControlFlow::Exit;
                return;
            }
            Event::UserEvent(UserEvent::Drag) => {
                let _ = window.drag_window();
            }
            Event::UserEvent(UserEvent::Resize(d)) => {
                let _ = window.drag_resize_window(d);
            }
            Event::UserEvent(UserEvent::Minimize) => window.set_minimized(true),
            Event::UserEvent(UserEvent::Pin(on)) => {
                window.set_always_on_top(on);
                state["pinned"] = json!(on);
                save_state(&state_path, &mut state, &window);
            }
            Event::UserEvent(UserEvent::Size(w, h)) => {
                window.set_inner_size(LogicalSize::new(w, h));
                // Keep the whole window on its monitor after it grows.
                if let (Some(m), Ok(p)) = (window.current_monitor(), window.outer_position()) {
                    let (mp, ms, os) = (m.position(), m.size(), window.outer_size());
                    let x = p.x.min(mp.x + ms.width as i32 - os.width as i32).max(mp.x);
                    let y = p.y.min(mp.y + ms.height as i32 - os.height as i32).max(mp.y);
                    if (x, y) != (p.x, p.y) {
                        window.set_outer_position(PhysicalPosition::new(x, y));
                    }
                }
                save_at = Some(Instant::now() + SAVE_DELAY);
            }
            Event::UserEvent(UserEvent::Live(snapshot)) => {
                if !hidden {
                    js(format!("window.agentMonLive && window.agentMonLive({snapshot})"));
                }
                last_live = Some(snapshot);
            }
            Event::UserEvent(UserEvent::DataReady) => {
                if !hidden {
                    js("window.agentMonReload && window.agentMonReload()".into());
                }
            }
            Event::UserEvent(UserEvent::Focused(ok)) => js(format!("window.agentMonFocused && window.agentMonFocused({ok})")),
            Event::UserEvent(UserEvent::Attention) => {
                if !window.is_focused() {
                    window.request_user_attention(Some(UserAttentionType::Informational));
                }
            }
            Event::UserEvent(UserEvent::UpdateStaged(v)) => js(format!("window.agentMonUpdate && window.agentMonUpdate({}, true)", js_str(&v))),
            Event::UserEvent(UserEvent::UpdateNote(t)) => js(format!("window.agentMonUpdate && window.agentMonUpdate({}, false)", js_str(&t))),
            Event::UserEvent(UserEvent::UpdateNow) => {
                save_state(&state_path, &mut state, &window);
                match update::swap_in() {
                    Ok(exe) => {
                        // Let go of the single-instance lock, start the new exe, leave.
                        drop(lock.take());
                        let _ = Command::new(exe).spawn();
                        *control_flow = ControlFlow::Exit;
                        return;
                    }
                    Err(e) => js(format!("window.agentMonUpdate && window.agentMonUpdate({}, false)", js_str(&e))),
                }
            }
            _ => {}
        }
        *control_flow = match save_at {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        };
    });
}
