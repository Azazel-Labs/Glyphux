# Profiles one of the workspace's binaries with samply, without a UAC prompt (after the
# one-time .vscode/samply-setup.ps1), and opens the result in the Firefox Profiler.
#
#   .vscode/profile.ps1 [-Bin text_demo] [-- program arguments]
#
# The binary is built with the `profiling` profile (release + line tables) and runs
# normally, unelevated, while the elevated task records the whole system: in the profiler,
# pick the program's process track.
param(
    [string]$Bin = 'text_demo',
    [Parameter(ValueFromRemainingArguments)][string[]]$ProgramArgs
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$work = Join-Path $env:LOCALAPPDATA 'samply-task'
if (-not (Get-ScheduledTask -TaskName 'samply-record' -ErrorAction SilentlyContinue)) {
    throw 'The samply-record task is missing: run .vscode/samply-setup.ps1 once first.'
}
if (-not $ProgramArgs -and $Bin -eq 'text_demo') {
    # 600 frames of the demo scene, not saved ("-"): PNG encoding would dominate the profile.
    $ProgramArgs = @("$root/examples/web-demo/assets/Inter-Regular.otf", '1280', '720', '-', '600')
}

Push-Location $root
try {
    cargo build --profile profiling --bin $Bin
    if ($LASTEXITCODE -ne 0) { throw 'build failed' }
} finally {
    Pop-Location
}
$exe = Join-Path $root "target/profiling/$Bin.exe"

New-Item -ItemType Directory -Force $work | Out-Null
Get-ChildItem $work | Remove-Item
$file = { param($name) Join-Path $work $name }
$finished = {
    $done = Get-Content (& $file 'done.txt') -Raw -ErrorAction SilentlyContinue
    if ($done) { $done.Trim() }
}

Set-Content (& $file 'request.txt') 'start'
Start-ScheduledTask -TaskName 'samply-record'
$deadline = (Get-Date).AddMinutes(1)
while (-not (Test-Path (& $file 'ready.txt'))) {
    if ($result = & $finished) { throw "samply failed: $result" }
    if ((Get-Date) -gt $deadline) { throw 'samply did not start (see the samply-record task)' }
    Start-Sleep -Milliseconds 100
}

try {
    & $exe @ProgramArgs
} finally {
    # Stop recording even if the program failed.
    Set-Content (& $file 'stop.txt') 'stop'
}
Write-Host 'Writing the profile...'
while (-not ($result = & $finished)) { Start-Sleep -Milliseconds 200 }
if ($result -ne '0') {
    Get-Content (& $file 'err.txt') -ErrorAction SilentlyContinue | Write-Host
    throw "samply failed: $result"
}
samply load (& $file 'profile.json.gz')
