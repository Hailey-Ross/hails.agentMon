//! Bring a Claude Code session's window to the front: the visible top-level window owned by the session's
//! process or one of its parents (VS Code, Windows Terminal, a console host), preferring a title that names
//! the project.

use std::collections::{HashMap, HashSet};

use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SetForegroundWindow, ShowWindow, GW_OWNER, SW_RESTORE,
};

use crate::live::processes;

struct Found {
    hwnd: HWND,
    pid: u32,
    title: String,
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> i32 {
    // SAFETY: lparam is the &mut Vec<Found> passed to EnumWindows below, alive for the whole enumeration.
    let list = unsafe { &mut *(lparam as *mut Vec<Found>) };
    unsafe {
        if IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() {
            return 1;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return 1;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        list.push(Found { hwnd, pid, title: String::from_utf16_lossy(&buf[..n.max(0) as usize]) });
    }
    1
}

/// The part of a window title before the first " - ", without a leading status glyph ("◐ SkateRust Engine").
fn lead(title: &str) -> String {
    let first = title.split(" - ").next().unwrap_or("");
    first.trim_start_matches(|c: char| !c.is_alphanumeric()).trim().to_lowercase()
}

/// Focus the window for session process `pid`. Between several windows of one app, a title led by the session's
/// `name` wins (VS Code shows "<session> - <folder> - Visual Studio Code"; two sessions can share a folder), then
/// one naming the project `folder`. Returns whether a window was found.
pub fn focus_session(pid: u32, name: &str, folder: &str) -> bool {
    let parent: HashMap<u32, u32> = processes().into_iter().map(|p| (p.pid, p.ppid)).collect();
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut cur = pid;
    while cur != 0 && seen.insert(cur) && chain.len() < 16 {
        chain.push(cur);
        cur = parent.get(&cur).copied().unwrap_or(0);
    }

    let mut windows: Vec<Found> = Vec::new();
    // SAFETY: the callback only touches the Vec behind lparam, which outlives the call.
    unsafe { EnumWindows(Some(collect), &mut windows as *mut Vec<Found> as LPARAM) };

    let name = name.trim().to_lowercase();
    let folder = folder.to_lowercase();
    // Nearest ancestor first; within it, the session's own title, then the project's, then any.
    let best = chain.iter().find_map(|p| {
        let mine: Vec<&Found> = windows.iter().filter(|w| w.pid == *p).collect();
        mine.iter()
            .find(|w| !name.is_empty() && lead(&w.title) == name)
            .or_else(|| mine.iter().find(|w| !folder.is_empty() && w.title.to_lowercase().contains(&folder)))
            .or(mine.first())
            .map(|w| w.hwnd)
    });
    let Some(hwnd) = best else { return false };
    // SAFETY: hwnd came from EnumWindows just now; these calls fail harmlessly if it has closed since.
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::lead;

    #[test]
    fn the_session_name_leads_a_vs_code_title() {
        assert_eq!(lead("◐ SkateRust Engine - skate-3-rust-engine - Visual Studio Code"), "skaterust engine");
        assert_eq!(lead("✳ agentMon - skate-3-rust-engine - Visual Studio Code"), "agentmon");
        assert_eq!(lead("skate-3-rust-engine - Visual Studio Code"), "skate-3-rust-engine");
    }
}
