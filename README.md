# agentMon

A small Windows desktop widget that shows what every AI agent on your PC is doing, in a hive theme: each open Claude
Code session is a honeycomb cell it builds while it works, with a bee buzzing in it, and every sub-agent is a worker
bee helping out. It also keeps heavy builds from piling up on the machine (the build gate).

All native Rust and lean: the data is read natively, pushed into a small WebView2 page, and the animations move
GPU layers instead of redrawing.

## Use it

- Start **agentMon** from the Start menu (or run `%LOCALAPPDATA%\agentMon\agentMon.exe`).
- Drag the top strip to move it. Drag the bottom-right corner to resize it. It remembers where you left it.
- **Right-click anywhere** for the menu: Always on top, window size (Compact, Tall, Wide, Full dashboard), resize by
  dragging, theme, Refresh now, Check for updates, Minimize, Close.
- Click a session (its row or its cell) to bring its window to the front.
- It updates itself from this repo's stable releases (checked after start and every 6 hours).

What it shows:

- **Hive**: sessions that need your answer, sessions working (including builds or tests running in the background),
  sessions ready for your reply, the build queue, other agents and background jobs, things to look at (failing CI,
  something that looks stuck, a job waiting on you, branches behind), and your projects.
- **Time**: how long each turn and task has run against how long it usually takes ("about 3 min left", "running
  long", "much longer than usual"), and signs of being stuck (a build using no CPU for minutes, a session writing
  nothing for 10 minutes).
- **Sessions**: every open Claude Code window, its sub-agents, what it is running, your last message and Claude's
  last reply.
- **Projects**: pick one for its sessions, git and worktrees, PRs and CI, and where you left off.
- **PRs**: open pull requests and each repo's latest CI run (checked every 10 minutes through `gh`).

The agents it sees: Claude Code sessions and their sub-agents, Claude runs without a window (headless, SDK),
background jobs, and other agent tools (Codex, Gemini CLI, Cursor Agent, OpenCode, Aider, Copilot CLI, Goose, Amp,
Qwen Code, Crush, Cline).

It only reads (plus the build gate below), and nothing is sent anywhere:

- Claude Code's files in `%USERPROFILE%\.claude` (open sessions, sub-agents, jobs, titles, last prompt and reply).
- The processes on the machine (to see the work under each session and other agent tools).
- `git` in each project, and `gh` for pull requests and CI (if `gh` is installed and signed in).
- [Kitten Assistant](https://github.com/Hailey-Ross/hails.Kitten-Assistant), if installed: each session's status
  line, mood and progress bar (`status --json`).

## The build gate

Run heavy commands through `agentmon-run` so they take turns instead of fighting over the machine:

```powershell
agentmon-run --session "my-project build" -- cargo build --release
```

It waits its turn (first come, first served; one heavy job at a time by default), then runs the command with your
console and exits with its
exit code. `--wait-max <minutes>` gives up after a while.

Taking turns for a whole series of steps, so nothing slips in between them:

```powershell
agentmon-run hold --for 45m                    # wait for a slot, then keep it for this session
agentmon-run -- cargo test -p skate-core       # starts at once, inside the held slot
agentmon-run -- cargo test -p skate-game       # so does this; no one took the slot in between
agentmon-run release                           # give it back (or it ends after 45 min / when the session closes)

agentmon-run batch -- "cargo test -p a" "cargo test -p b"   # one slot, commands back to back (--keep-going)
agentmon-run --exclusive -- my-perf-test.exe               # waits for a clear machine, keeps it clear
agentmon-run --lock auto -- cargo test                     # never alongside another job in this repo
agentmon-run status                                        # the queue
```

A hold is tied to the Claude Code session that took it (or the terminal, outside Claude Code): everything that
session runs counts as part of the reservation and is never paused. `--lock NAME` works with any name two jobs share
(a target folder, a device, a test database).

Builds started any other way still count. While the widget runs, a build tool started outside the gate while the
machine is full (cargo, rustc, MSBuild, CMake, Ninja, cl, link) is paused until a slot frees up, and resumed after 20
minutes at most, when the widget closes, or on its next start. Games are never paused. The Build queue card shows the
gauges, what holds a slot and what waits.

Settings (optional) go in `%LOCALAPPDATA%\agentMon\gate.json`, for example
`{ "maxSlots": 1, "maxCpu": 70, "minFreeGb": 6, "pauseOutsiders": true, "games": ["mygame*"] }`. `"maxSlots": "auto"`
lets several jobs run at once on a big machine (about one per 8 cores and 16 GB, 1 to 4, while CPU load is under
`maxCpu` and `minFreeGb` is free).

## Install

Needs Windows 10 or 11 with the Microsoft Edge WebView2 Runtime (part of Windows 11), Rust (stable, MSVC) and the
Windows SDK (for `rc.exe`, which embeds the icon).

```powershell
git clone https://github.com/Hailey-Ross/hails.agentMon
cd hails.agentMon
powershell -NoProfile -ExecutionPolicy Bypass -File tools\rebuild-install.ps1
```

The script builds at low priority (through the build gate once it is installed), copies `agentMon.exe` and
`agentmon-run.exe` to `%LOCALAPPDATA%\agentMon`, puts that folder on your PATH, adds a Start menu shortcut, and
restarts the widget if it was open. Run it again after pulling changes. Build output goes to `.\target` (or
`-TargetDir <folder>`).

## How it is built

- `src/main.rs`: the window (tao) and WebView2 page (wry), title strip and right-click actions, window position
  memory, one instance at a time, a lean WebView2 (one renderer, no background services, low memory while minimized).
- `src/live.rs`: every second, natively: open sessions, their sub-agents and work, other agents, jobs, the kitten.
- `src/scan.rs`: every minute: projects, recent sessions, git (only re-read when something may have changed), PRs.
- `src/gate.rs`, `src/bin/agentmon-run.rs`: the build gate.
- `src/focus.rs`: bring a session's window to the front. `src/update.rs`: self-update from releases.
- `build.rs`: draws the app icon in code (tiny-skia) at every Windows size and embeds it in the exe.
- `index.html`, `board.js`, `board.css`, `fonts/`: the hive UI, built into the exe.

Fonts: Bricolage Grotesque, Figtree and JetBrains Mono under the SIL Open Font License (`fonts/OFL-*.txt`).
