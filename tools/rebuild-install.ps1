<#
Rebuilds agentMon and reinstalls it into %LOCALAPPDATA%\agentMon, then restarts it.
The build goes through agentMon's own build gate when agentmon-run is installed (it waits for a slot);
otherwise it waits (up to -WaitMinutes) until no build or game in -Heavy runs. Builds at Idle priority with
-j 4, closes the running widget gracefully (its window position is saved), installs agentMon.exe and
agentmon-run.exe, puts the install folder on the user PATH, and starts the widget again.
#>
param(
    [int]$WaitMinutes = 30,
    [string[]]$Heavy = @('cargo', 'rustc', 'skate3rust*'),
    [string]$TargetDir = ''
)

$root = Split-Path -Parent $PSScriptRoot
# Build output: -TargetDir, else K:\VS-Code Output\agentMon\main when that drive folder exists, else .\target.
$target = if ($TargetDir) { $TargetDir } elseif (Test-Path 'K:\VS-Code Output') { 'K:\VS-Code Output\agentMon\main' } else { Join-Path $root 'target' }
$inst = Join-Path $env:LOCALAPPDATA 'agentMon'
$gateExe = Join-Path $inst 'agentmon-run.exe'
$useGate = Test-Path $gateExe
if (-not $useGate) {
    $deadline = (Get-Date).AddMinutes($WaitMinutes)
    $clear = 0
    while ($clear -lt 2) {
        if ((Get-Date) -gt $deadline) { 'gave up: other builds or a game still running'; exit 2 }
        if (Get-Process $Heavy -ErrorAction SilentlyContinue) { $clear = 0 } else { $clear++ }
        Start-Sleep -Seconds 10
    }
}

$env:CARGO_TARGET_DIR = $target
# Build number: the newest CI run's (so this build keeps itself until a newer stable release exists, then
# updates to it), or 1 when GitHub cannot be asked.
if (-not $env:AGENTMON_BUILD) {
    $n = $null
    if (Get-Command gh -ErrorAction SilentlyContinue) {
        $n = gh api 'repos/Hailey-Ross/hails.agentMon/actions/runs?per_page=1' --jq '.workflow_runs[0].run_number' 2>$null
    }
    $env:AGENTMON_BUILD = if ("$n" -match '^\d+$') { "$n" } else { '1' }
}
$log = Join-Path (Split-Path $target) 'build.log'
$cargoArgs = @('build', '--release', '-j', '4')
$p = if ($useGate) {
    Start-Process $gateExe -ArgumentList (@('--session', 'agentMon-rebuild', '--wait-max', "$WaitMinutes", '--', 'cargo') + $cargoArgs) -WorkingDirectory $root -NoNewWindow -PassThru -RedirectStandardError $log
} else {
    Start-Process cargo -ArgumentList $cargoArgs -WorkingDirectory $root -NoNewWindow -PassThru -RedirectStandardError $log
}
Start-Sleep -Milliseconds 300
# Idle priority; the build's processes inherit it.
try { $p.PriorityClass = 'Idle' } catch {}
$p.WaitForExit()
$ok = Select-String -Path $log -Pattern 'Finished `release`' -Quiet
Get-Content $log -Tail 3
if (-not $ok) { 'build failed (or gave up waiting for a slot)'; exit 1 }
Remove-Item $log -ErrorAction SilentlyContinue

$running = Get-Process agentMon -ErrorAction SilentlyContinue
foreach ($r in $running) { [void]$r.CloseMainWindow() }
foreach ($r in $running) { if (-not $r.WaitForExit(5000)) { Stop-Process -Id $r.Id } }
[void](New-Item -ItemType Directory -Force -Path $inst)
Copy-Item (Join-Path $target 'release\agentmon.exe') (Join-Path $inst 'agentMon.exe') -Force
# agentmon-run may be running (an agent's build waiting in the queue); a running exe can be renamed, not replaced.
$runNew = Join-Path $target 'release\agentmon-run.exe'
if (Test-Path $runNew) {
    try { Copy-Item $runNew $gateExe -Force -ErrorAction Stop } catch {
        Remove-Item "$gateExe.old" -Force -ErrorAction SilentlyContinue
        Rename-Item $gateExe "$gateExe.old" -ErrorAction SilentlyContinue
        Copy-Item $runNew $gateExe -Force
    }
}
# The install folder on the user PATH, so every terminal and agent can call agentmon-run.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $inst) {
    [Environment]::SetEnvironmentVariable('Path', ($userPath.TrimEnd(';') + ';' + $inst), 'User')
    'added to the user PATH: ' + $inst + ' (new terminals see it)'
}
$lnk = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\agentMon.lnk'
if (-not (Test-Path $lnk)) {
    $sc = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
    $sc.TargetPath = Join-Path $inst 'agentMon.exe'
    $sc.WorkingDirectory = $inst
    $sc.Description = 'Hive monitor for Claude Code sessions'
    $sc.Save()
}
if ($running) { Start-Process (Join-Path $inst 'agentMon.exe') -WorkingDirectory $inst }
'installed' + $(if ($running) { ' and restarted' } else { '' })
