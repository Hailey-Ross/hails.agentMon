<#
agentMon data: feeds the agentMon widget (and index.html in a browser) with every project that had Claude Code
activity in the last N days. Per project: open + recent Claude sessions (with the work running under each),
kitten status lines, git state (incl. worktrees), where you left off, open PRs and the latest CI run (gh, cached).
Read-only. Writes only board-data.js and gh-cache.json, into -OutDir (default: next to this script).
Usage: powershell -NoProfile -File agentmon.ps1 [-OutDir <folder>] [-Days 30] [-GhMinutes 10] [-Open]
#>
param(
    [string]$OutDir = '',
    [int]$Days = 30,
    [int]$GhMinutes = 10,
    [switch]$Open
)

$ErrorActionPreference = 'Continue'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
if ($OutDir) { [void](New-Item -ItemType Directory -Force -Path $OutDir); $here = $OutDir }
$cacheFile = Join-Path $here 'gh-cache.json'
# Scan cache: summaries of unchanged session logs, log cwds and worktree git counts carry over between runs
# (only entries used this run are kept, so it cannot grow without end).
$scanFile = Join-Path $here 'scan-cache.json'
$scan = @{}
$scanNext = @{}
if (Test-Path $scanFile) {
    try { (Get-Content $scanFile -Raw | ConvertFrom-Json).PSObject.Properties | ForEach-Object { $scan[$_.Name] = $_.Value } } catch {}
}
$claudeDir = Join-Path $env:USERPROFILE '.claude'
$kittenDir = Join-Path $env:LOCALAPPDATA 'KittenAssistant'
$now = Get-Date
try { [System.Diagnostics.Process]::GetCurrentProcess().PriorityClass = 'BelowNormal' } catch {}

function Read-Tail([string]$path, [int]$bytes) {
    $fs = [IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
    try {
        $start = [Math]::Max(0, $fs.Length - $bytes)
        [void]$fs.Seek($start, 'Begin')
        $buf = New-Object byte[] ($fs.Length - $start)
        $n = $fs.Read($buf, 0, $buf.Length)
        [Text.Encoding]::UTF8.GetString($buf, 0, $n)
    } finally { $fs.Dispose() }
}

function Read-Head([string]$path, [int]$bytes) {
    $fs = [IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
    try {
        $buf = New-Object byte[] ([Math]::Min($bytes, $fs.Length))
        $n = $fs.Read($buf, 0, $buf.Length)
        [Text.Encoding]::UTF8.GetString($buf, 0, $n)
    } finally { $fs.Dispose() }
}

function Get-LastEntry([string[]]$lines, [string]$type) {
    $l = $lines | Where-Object { $_ -like "*`"type`":`"$type`"*" } | Select-Object -Last 1
    if ($l) { try { return ($l | ConvertFrom-Json) } catch {} }
    $null
}

function Get-LastAssistantText([string[]]$lines) {
    $cands = @($lines | Where-Object { $_ -like '*"type":"assistant"*' -and $_ -like '*"type":"text"*' })
    for ($i = $cands.Count - 1; $i -ge 0 -and $i -ge $cands.Count - 6; $i--) {
        try {
            $o = $cands[$i] | ConvertFrom-Json
            $t = @($o.message.content | Where-Object { $_.type -eq 'text' } | ForEach-Object { $_.text }) -join ' '
            if ($t.Trim()) { return $t.Trim() }
        } catch {}
    }
    ''
}

function Short([string]$s, [int]$n) {
    if (-not $s) { return '' }
    $s = ($s -replace '\s+', ' ').Trim()
    if ($s.Length -gt $n) { $s.Substring(0, $n - 3) + '...' } else { $s }
}

function HtmlEnc([string]$s) { [System.Net.WebUtility]::HtmlEncode($s) }

function Age([datetime]$t) {
    $d = $now - $t
    if ($d.TotalMinutes -lt 1) { 'just now' }
    elseif ($d.TotalMinutes -lt 60) { '{0} min ago' -f [int]$d.TotalMinutes }
    elseif ($d.TotalHours -lt 24) { '{0} h ago' -f [int]$d.TotalHours }
    else { '{0} d ago' -f [int]$d.TotalDays }
}

$rootCache = @{}
function Get-ProjectRoot([string]$cwd) {
    if ($rootCache.ContainsKey($cwd)) { return $rootCache[$cwd] }
    $root = $cwd
    if (Test-Path -LiteralPath $cwd) {
        $common = git -C $cwd rev-parse --path-format=absolute --git-common-dir 2>$null
        if ($LASTEXITCODE -eq 0 -and $common) {
            $common = $common.Trim() -replace '/', '\'
            if ((Split-Path -Leaf $common) -eq '.git') { $root = Split-Path -Parent $common }
        }
    } else { $root = $null }
    $rootCache[$cwd] = $root
    $root
}

# ---- Claude sessions (project folders + running session registry) ----
$cutoff = $now.AddDays(-$Days)
$groups = @{}
function Get-Group([string]$root) {
    if (-not $groups.ContainsKey($root)) {
        $groups[$root] = [ordered]@{
            Root = $root; Name = (Split-Path -Leaf $root); Sessions = @{}; MemoryDirs = @(); LastActive = [datetime]::MinValue
        }
    }
    $groups[$root]
}

foreach ($pd in Get-ChildItem (Join-Path $claudeDir 'projects') -Directory -ErrorAction SilentlyContinue) {
    $logs = @(Get-ChildItem $pd.FullName -Filter *.jsonl -File -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTime -ge $cutoff } | Sort-Object LastWriteTime -Descending)
    if (-not $logs) { continue }
    # A log's first lines never change, so its cwd is read once and kept in the scan cache.
    $cwdKey = 'cwd|' + $logs[0].FullName
    $cwd = $scan[$cwdKey]
    if (-not $cwd) {
        $m = [regex]::Match((Read-Head $logs[0].FullName 131072), '"cwd":"((?:[^"\\]|\\.)*)"')
        if (-not $m.Success) { continue }
        $cwd = ('"' + $m.Groups[1].Value + '"' | ConvertFrom-Json)
    }
    $scanNext[$cwdKey] = $cwd
    $root = Get-ProjectRoot $cwd
    if (-not $root) { continue }
    $g = Get-Group $root
    $mem = Join-Path $pd.FullName 'memory'
    if (Test-Path $mem) { $g.MemoryDirs += $mem }
    foreach ($log in ($logs | Select-Object -First 4)) {
        $id = [IO.Path]::GetFileNameWithoutExtension($log.Name)
        # Unchanged log (same length and time) -> the summary from the last run; else read its tail.
        $sumKey = 'log|' + $log.FullName
        $sig = "$($log.Length)|$($log.LastWriteTimeUtc.Ticks)"
        $sum = $scan[$sumKey]
        if (-not $sum -or $sum.sig -ne $sig) {
            $lines = (Read-Tail $log.FullName 400000) -split "`n"
            $ct = Get-LastEntry $lines 'custom-title'
            $at = Get-LastEntry $lines 'ai-title'
            $lp = Get-LastEntry $lines 'last-prompt'
            $sum = [pscustomobject]@{
                sig = $sig
                title = $(if ($ct) { "$($ct.customTitle)" } elseif ($at) { "$($at.aiTitle)" } else { '' })
                aiTitle = $(if ($at) { "$($at.aiTitle)" } else { '' })
                lastPrompt = $(if ($lp) { "$($lp.lastPrompt)" } else { '' })
                lastSaid = "$(Get-LastAssistantText $lines)"
            }
        }
        $scanNext[$sumKey] = $sum
        $g.Sessions[$id] = [ordered]@{
            Id = $id; Cwd = $cwd; Title = $sum.title; AiTitle = $sum.aiTitle; LastPrompt = $sum.lastPrompt
            LastSaid = $sum.lastSaid; LastWrite = $log.LastWriteTime
            Running = $false; Status = ''; Name = ''; Pid = 0
        }
        if ($log.LastWriteTime -gt $g.LastActive) { $g.LastActive = $log.LastWriteTime }
    }
}

# Work running under a session: a session whose turn ended can still own a background build or
# test. Walk each session's process tree; a "task" is the first non-shell process under a shell
# (or under claude itself). Helpers that start with the session (MCP servers) are left out.
$procs = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue)
$children = @{}
foreach ($p in $procs) {
    $pp = [int]$p.ParentProcessId
    if (-not $children.ContainsKey($pp)) { $children[$pp] = New-Object System.Collections.ArrayList }
    [void]$children[$pp].Add($p)
}
$shellNames = '^(cmd|powershell|pwsh|bash|sh|conhost|wscript|cscript|chcp|timeout|OpenConsole)\.exe$'
function Get-TaskLabel($p) {
    $name = [IO.Path]::GetFileNameWithoutExtension($p.Name)
    $cmd = "$($p.CommandLine)"
    $rest = ($cmd -replace '^\s*("[^"]*"|\S+)\s*', '')
    $words = @($rest -split '\s+' | Where-Object { $_ -and $_ -notmatch '^-' -and $_ -notmatch '^\d+$' } | Select-Object -First 2)
    $label = (@($name) + $words) -join ' '
    Short $label 60
}
function Get-SessionTasks([int]$sessionPid, [datetime]$sessionStart) {
    $tasks = @()
    $queue = New-Object System.Collections.Queue
    $queue.Enqueue($sessionPid)
    $seen = @{}
    while ($queue.Count) {
        $id = $queue.Dequeue()
        if ($seen.ContainsKey($id) -or -not $children.ContainsKey($id)) { continue }
        $seen[$id] = $true
        foreach ($c in $children[$id]) {
            if ($c.Name -match $shellNames) { $queue.Enqueue([int]$c.ProcessId); continue }
            $started = $c.CreationDate
            if ($started -and ($started - $sessionStart).TotalSeconds -lt 60) { continue }
            if ("$($c.CommandLine)" -match '(?i)\bmcp\b|modelcontextprotocol|kitten-assistant|agentmon\.exe|msedgewebview2') { continue }
            $tasks += [ordered]@{ label = (Get-TaskLabel $c); started = $started; pid = [int]$c.ProcessId }
        }
    }
    @($tasks | Sort-Object { $_.started })
}

foreach ($sf in Get-ChildItem (Join-Path $claudeDir 'sessions') -Filter *.json -File -ErrorAction SilentlyContinue) {
    try { $j = Get-Content -LiteralPath $sf.FullName -Raw | ConvertFrom-Json } catch { continue }
    if (-not (Get-Process -Id $j.pid -ErrorAction SilentlyContinue)) { continue }
    $root = Get-ProjectRoot $j.cwd
    if (-not $root) { continue }
    $g = Get-Group $root
    if (-not $g.Sessions.ContainsKey($j.sessionId)) {
        $g.Sessions[$j.sessionId] = [ordered]@{
            Id = $j.sessionId; Cwd = $j.cwd; Title = ''; AiTitle = ''; LastPrompt = ''; LastSaid = ''; LastWrite = $sf.LastWriteTime
        }
    }
    $s = $g.Sessions[$j.sessionId]
    $s.Running = $true; $s.Status = "$($j.status)"; $s.Name = "$($j.name)"; $s.Pid = $j.pid; $s.Cwd = $j.cwd
    $sessStart = if ($j.startedAt) { [DateTimeOffset]::FromUnixTimeMilliseconds([long]$j.startedAt).LocalDateTime } else { [datetime]::MinValue }
    $s.Tasks = @(Get-SessionTasks ([int]$j.pid) $sessStart)
    if ($sf.LastWriteTime -gt $g.LastActive) { $g.LastActive = $sf.LastWriteTime }
}

# ---- Kitten Assistant status lines ----
# Preferred: the kitten's own `status --json` (its mood, how long the text has been up, progress, expiry).
# Fallback for an older kitten: read its status files and guess the mood from the first word.
$kitten = @()
$statusFile = Join-Path $kittenDir 'status.txt'
function Get-Mood([string]$t) {
    if ($t -match '^(Failed|Error|Problem|Blocked)\b') { 'problem' } elseif ($t -match '^Waiting\b') { 'waiting' } else { 'work' }
}
function Get-ToneOfKittenMood([string]$mood, [string]$text) {
    if ($mood -match '(?i)fail|problem|error|worried') { 'problem' } elseif ($mood -match '(?i)wait') { 'waiting' } elseif ($mood) { 'work' } else { Get-Mood $text }
}
$kittenExe = Join-Path $kittenDir 'kitten-assistant.exe'
$kittenJson = $null
if (Test-Path -LiteralPath $kittenExe) {
    try { $kittenJson = ((& $kittenExe status --json 2>$null | Write-Output) -join '') | ConvertFrom-Json } catch { $kittenJson = $null }
}
if ($kittenJson -and $kittenJson.PSObject.Properties['lines']) {
    foreach ($l in @($kittenJson.lines)) {
        if (-not $l -or -not $l.text) { continue }
        $script:kitten += [ordered]@{
            Name = "$($l.session)"; Text = "$($l.text)"; Time = $now.AddSeconds(-[double]$l.age_secs); Kind = "$($l.mood)"
            Progress = $l.progress; ProgressLabel = $l.progress_label; ExpiresIn = $l.expires_in_secs
        }
    }
} else {
    function Add-Kitten([string]$path, [string]$name) {
        if (-not (Test-Path -LiteralPath $path)) { return }
        $text = (Get-Content -LiteralPath $path -Raw -Encoding UTF8 -ErrorAction SilentlyContinue)
        if ($text) { $text = ($text -split "`r?`n")[0].Trim() }
        if (-not $text -or $text -ieq 'idle') { return }
        $t = (Get-Item -LiteralPath $path).LastWriteTime
        if ($name -and ($now - $t).TotalHours -ge 3) { return }
        $script:kitten += [ordered]@{ Name = $name; Text = $text; Time = $t; Kind = '' }
    }
    Add-Kitten $statusFile ''
    foreach ($kf in Get-ChildItem (Join-Path $kittenDir 'sessions') -Filter *.txt -File -ErrorAction SilentlyContinue) {
        Add-Kitten $kf.FullName $kf.BaseName
    }
}
function Find-KittenGroup([string]$kname) {
    if (-not $kname) { return $null }
    foreach ($g in $groups.Values) {
        if ($g.Name -ieq $kname -or $kname -like "*$($g.Name)*") { return $g.Root }
        foreach ($s in $g.Sessions.Values) { if ($s.Name -and $s.Name -ieq $kname) { return $g.Root } }
    }
    $null
}

# ---- git ----
function Get-GitInfo([string]$root) {
    if (-not (Test-Path -LiteralPath (Join-Path $root '.git'))) { return $null }
    $info = [ordered]@{ Branch = ''; Ahead = 0; Behind = 0; Changed = 0; Upstream = ''; Worktrees = @() }
    foreach ($l in (git -C $root status --porcelain=v2 --branch 2>$null)) {
        if ($l -like '# branch.head *') { $info.Branch = $l.Substring(14) }
        elseif ($l -like '# branch.upstream *') { $info.Upstream = $l.Substring(18) }
        elseif ($l -match '^# branch\.ab \+(\d+) -(\d+)') { $info.Ahead = [int]$matches[1]; $info.Behind = [int]$matches[2] }
        elseif ($l -notlike '#*' -and $l) { $info.Changed++ }
    }
    $wt = $null
    foreach ($l in (git -C $root worktree list --porcelain 2>$null)) {
        if ($l -like 'worktree *') { $wt = [ordered]@{ Path = ($l.Substring(9) -replace '/', '\'); Branch = '(detached)'; Changed = 0 } }
        elseif ($l -like 'branch *') { $wt.Branch = $l.Substring(7) -replace '^refs/heads/', '' }
        elseif ($l -eq '' -and $wt) { if ($wt.Path -ne $root) { $info.Worktrees += $wt }; $wt = $null }
    }
    if ($wt -and $wt.Path -ne $root) { $info.Worktrees += $wt }
    # Worktrees: `git status` on each is the slowest part of a run, so each one is rechecked at most every
    # 3 minutes, and at most 4 per run (the stalest first); the others show their last count.
    $budget = 4
    $wts = @($info.Worktrees | Select-Object -First 12)
    $byAge = $wts | Sort-Object { $e = $scan['wt|' + $_.Path]; if ($e) { [long]$e.at } else { 0 } }
    foreach ($w in $byAge) {
        $key = 'wt|' + $w.Path
        $e = $scan[$key]
        $fresh = $e -and ($now.Ticks - [long]$e.at) -lt [TimeSpan]::FromMinutes(3).Ticks
        if ((-not $e -or (-not $fresh -and $budget -gt 0)) -and (Test-Path -LiteralPath $w.Path)) {
            if ($e) { $budget-- }
            $e = [pscustomobject]@{ at = $now.Ticks; changed = @(git -C $w.Path status --porcelain 2>$null).Count }
        }
        if ($e) { $w.Changed = [int]$e.changed; $scanNext[$key] = $e }
    }
    $info
}

function Get-GhRepos([string]$root) {
    $r = @()
    foreach ($l in (git -C $root remote -v 2>$null)) {
        if ($l -match '^(\S+)\s+\S*github\.com[:/]([^/\s]+/[^/\s]+?)(\.git)?\s+\(fetch\)') {
            $r += [ordered]@{ Remote = $matches[1]; Repo = $matches[2] }
        }
    }
    $r
}

# ---- gh (cached) ----
$cache = @{}
if (Test-Path $cacheFile) {
    try { (Get-Content $cacheFile -Raw | ConvertFrom-Json).PSObject.Properties | ForEach-Object { $cache[$_.Name] = $_.Value } } catch {}
}
$haveGh = [bool](Get-Command gh -ErrorAction SilentlyContinue)
function Get-GhInfo([string]$repo, [bool]$mineOnly) {
    $key = "$repo|$mineOnly"
    $c = $cache[$key]
    if ($c -and ($now - [datetime]$c.Fetched).TotalMinutes -lt $GhMinutes) { return $c }
    if (-not $haveGh) { return $c }
    $ghArgs = @('pr', 'list', '-R', $repo, '--state', 'open', '--limit', '10', '--json', 'number,title,url,headRefName,isDraft,statusCheckRollup')
    if ($mineOnly) { $ghArgs += @('--author', '@me') }
    $prs = gh @ghArgs 2>$null
    if ($LASTEXITCODE -ne 0) { return $c }
    $runs = gh run list -R $repo -L 1 --json status,conclusion,workflowName,headBranch,createdAt,url 2>$null
    $v = [pscustomobject]@{
        Fetched = $now.ToString('o')
        Prs = @(try { ($prs -join "`n") | ConvertFrom-Json | ForEach-Object { $_ } } catch { })
        Run = @(try { ($runs -join "`n") | ConvertFrom-Json | ForEach-Object { $_ } } catch { }) | Select-Object -First 1
    }
    $cache[$key] = $v
    $v
}
function Get-Checks($pr) {
    $roll = @($pr.statusCheckRollup)
    if (-not $roll.Count) { return 'none' }
    $states = $roll | ForEach-Object { if ($_.conclusion) { "$($_.conclusion)" } elseif ($_.state) { "$($_.state)" } else { "$($_.status)" } }
    if ($states | Where-Object { $_ -match 'FAILURE|ERROR|TIMED_OUT|CANCELLED|ACTION_REQUIRED' }) { 'fail' }
    elseif ($states | Where-Object { $_ -match 'PENDING|QUEUED|IN_PROGRESS|EXPECTED|WAITING' }) { 'running' }
    else { 'pass' }
}

# ---- where you left off ----
function Get-LeftOff($g) {
    $lines = @()
    foreach ($m in $g.MemoryDirs) {
        $idx = Join-Path $m 'MEMORY.md'
        if (Test-Path $idx) {
            $lines += Get-Content $idx -Encoding UTF8 |
                Where-Object { $_ -match '(?i)resume|left off|next step|in progress' -and $_ -notmatch '(?i)STANDING|no resume' } |
                ForEach-Object { $_ -replace '^\s*-\s*', '' -replace '\[([^\]]+)\]\([^)]*\)\s*:?\s*', '$1: ' } |
                Select-Object -First 2
        }
    }
    $todo = Join-Path $g.Root '.claude\todo'
    $count = 0
    if (Test-Path $todo) { $count = @(Get-ChildItem $todo -Filter *.md -File | Where-Object { $_.Name -ne 'README.md' }).Count }
    [ordered]@{ Lines = @($lines | Select-Object -Unique); TodoCount = $count }
}

# ---- data out (board-data.js, read by index.html) ----
function Iso($t) { if ($t -and $t -ne [datetime]::MinValue) { ([datetime]$t).ToString('o') } else { $null } }

$projects = @()
foreach ($g in $groups.Values) {
    $sessions = @($g.Sessions.Values | Sort-Object @{ e = { -not $_.Running } }, @{ e = { $_.LastWrite }; Descending = $true } | Select-Object -First 6 | ForEach-Object {
        [ordered]@{
            id = $_.Id; name = "$($_.Name)"; title = "$($_.Title)"; aiTitle = "$($_.AiTitle)"
            where = $(if ($_.Cwd -and $_.Cwd -ne $g.Root) { Split-Path -Leaf $_.Cwd } else { '' })
            running = [bool]$_.Running; status = "$($_.Status)"; pid = [int]$_.Pid; lastWrite = (Iso $_.LastWrite)
            lastPrompt = (Short $_.LastPrompt 400); lastSaid = (Short $_.LastSaid 500)
            tasks = @($_.Tasks | Where-Object { $_ } | ForEach-Object { [ordered]@{ label = $_.label; started = (Iso $_.started); pid = $_.pid } })
        }
    })
    $git = Get-GitInfo $g.Root
    $gitOut = $null
    if ($git) {
        $gitOut = [ordered]@{
            branch = $git.Branch; ahead = $git.Ahead; behind = $git.Behind; upstream = $git.Upstream; changed = $git.Changed
            worktrees = @($git.Worktrees | ForEach-Object { [ordered]@{ leaf = (Split-Path -Leaf $_.Path); path = $_.Path; branch = $_.Branch; changed = $_.Changed } })
        }
    }
    $prs = @(); $runs = @()
    if ($git) {
        foreach ($r in (Get-GhRepos $g.Root)) {
            $info = Get-GhInfo $r.Repo ($r.Remote -ne 'origin')
            if (-not $info) { continue }
            foreach ($pr in @($info.Prs | ForEach-Object { $_ })) {
                if (-not $pr -or -not $pr.number) { continue }
                $prs += [ordered]@{ repo = $r.Repo; number = $pr.number; title = $pr.title; url = $pr.url; branch = $pr.headRefName; draft = [bool]$pr.isDraft; checks = (Get-Checks $pr) }
            }
            $run = $info.Run
            if ($run -and $run.createdAt) {
                $res = if ($run.status -ne 'completed') { 'running' } elseif ($run.conclusion -eq 'success') { 'pass' } else { 'fail' }
                $runs += [ordered]@{ repo = $r.Repo; workflow = $run.workflowName; branch = $run.headBranch; created = (Iso ([datetime]$run.createdAt)); url = $run.url
                    state = $(if ($run.status -ne 'completed') { "$($run.status)" } else { "$($run.conclusion)" }); result = $res }
            }
        }
    }
    $lo = Get-LeftOff $g
    $projects += [ordered]@{
        name = $g.Name; root = $g.Root; lastActive = (Iso $g.LastActive)
        sessions = $sessions; git = $gitOut; prs = @($prs); runs = @($runs)
        leftOff = [ordered]@{ lines = @($lo.Lines | ForEach-Object { Short $_ 300 }); todoCount = $lo.TodoCount }
    }
}

$kittenOut = @($kitten | Sort-Object Time -Descending | ForEach-Object {
    $root = Find-KittenGroup $_.Name
    $prog = $null
    if ($null -ne $_.Progress) { try { $prog = [Math]::Max([double]0.0, [Math]::Min([double]1.0, [double]$_.Progress)) } catch {} }
    [ordered]@{
        name = $_.Name; text = $_.Text; time = (Iso $_.Time); mood = (Get-ToneOfKittenMood $_.Kind $_.Text); kind = $_.Kind
        progress = $prog; progressLabel = "$($_.ProgressLabel)"; expiresIn = $_.ExpiresIn
        project = $(if ($root) { Split-Path -Leaf $root } else { '' })
    }
})

$data = [ordered]@{ generated = (Iso $now); days = $Days; kitten = $kittenOut; projects = @($projects) }
$json = ConvertTo-Json $data -Depth 10 -Compress

$enc = New-Object Text.UTF8Encoding($false)
$outData = Join-Path $here 'board-data.js'
[IO.File]::WriteAllText("$outData.tmp", "window.BOARD_DATA = $json;`n", $enc)
Move-Item -LiteralPath "$outData.tmp" -Destination $outData -Force
[IO.File]::WriteAllText($cacheFile, ($cache | ConvertTo-Json -Depth 8), $enc)
[IO.File]::WriteAllText($scanFile, ($scanNext | ConvertTo-Json -Depth 4 -Compress), $enc)
if ($Open) { Start-Process (Join-Path $here 'index.html') }
