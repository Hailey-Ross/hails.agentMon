# agentMon

A small Windows desktop widget that shows what every Claude Code session on your PC is doing, in a hive theme: each
open session is a honeycomb cell, and working sessions have a bee buzzing in them.

## Use it

- Start **agentMon** from the Start menu (or run `%LOCALAPPDATA%\agentMon\agentMon.exe`).
- Drag the top strip to move it. Drag the bottom-right corner to resize it. It remembers where you left it.
- **Right-click anywhere** for the menu: Always on top, window size (Compact, Tall, Wide, Full dashboard), resize by
  dragging, theme, Refresh now, Minimize, Close.
- The pin button in the top strip also toggles Always on top.

What it shows:

- **Hive**: sessions that need your answer, sessions working (including builds or tests running in the background),
  sessions ready for your reply, things to look at (failing CI, a stuck kitten line, branches behind), and your
  projects.
- **Sessions**: every open Claude Code window, what it is running, your last message and Claude's last reply.
- **Projects**: pick one for its sessions, git and worktrees, PRs and CI, and where you left off.
- **PRs**: open pull requests and each repo's latest CI run (checked every 10 minutes through `gh`).

It only reads, and nothing is sent anywhere:

- Claude Code's session files in `%USERPROFILE%\.claude` (open sessions, titles, your last prompt, the last reply).
- The processes running under each session, so a background build counts as work.
- `git` in each project, and `gh` for pull requests and CI (if `gh` is installed and signed in).
- [Kitten Assistant](https://github.com/Hailey-Ross/hails.Kitten-Assistant), if installed: each session's status
  line, mood and progress bar (`status --json`).

## Install

Needs Windows 10 or 11 with the Microsoft Edge WebView2 Runtime (part of Windows 11), Rust (stable, MSVC) and the
Windows SDK (for `rc.exe`, which embeds the icon).

```powershell
git clone https://github.com/Hailey-Ross/hails.agentMon
cd hails.agentMon
powershell -NoProfile -ExecutionPolicy Bypass -File tools\rebuild-install.ps1
```

The script builds a release exe at low priority (waiting first until no other cargo build runs), copies it to
`%LOCALAPPDATA%\agentMon`, adds a Start menu shortcut, and restarts the widget if it was open. Run it again after
pulling changes. Build output goes to `.\target` (or `-TargetDir <folder>`).

## How it is built

- `src/main.rs`: the window (tao) and WebView2 page (wry), the right-click and title-strip actions, the minute-by-minute
  data refresh, window position memory, one instance at a time.
- `build.rs`: draws the app icon in code (tiny-skia) at every Windows size and embeds it in the exe.
- `index.html`, `board.js`, `board.css`, `fonts/`: the hive UI, built into the exe. `index.html` also works in a
  browser if `agentmon.ps1` has written `board-data.js` next to it.
- `agentmon.ps1`: collects the data (read-only) and writes `board-data.js`; built into the exe and run in the
  background every minute.

Data and the window position live in `%LOCALAPPDATA%\agentMon`.

Fonts: Bricolage Grotesque, Figtree and JetBrains Mono under the SIL Open Font License (`fonts/OFL-*.txt`).
