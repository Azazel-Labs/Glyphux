# One-time setup (asks for administrator rights once) that lets .vscode/profile.ps1 record
# with samply without a UAC prompt each time.
#
# samply records through ETW (xperf from the Windows Performance Toolkit), which needs
# administrator rights. This installs a copy of samply and a small wrapper into
# C:\Program Files\samply (writable by administrators only) and registers a scheduled task,
# "samply-record", that runs the wrapper elevated; starting the task needs no prompt. The
# wrapper only records the whole system until asked to stop, always into the same file: it
# takes no command or arguments from the request, so the task cannot be used to run anything
# else elevated. Run again after updating samply. Remove it all with -Uninstall.
param([switch]$Uninstall, [string]$UserData, [string]$Samply)

$ErrorActionPreference = 'Stop'
$installDir = Join-Path $env:ProgramFiles 'samply'
$taskName = 'samply-record'

$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
    # Re-run elevated (the one UAC prompt), passing this user's paths.
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"",
        '-UserData', "`"$env:LOCALAPPDATA`"", '-Samply', "`"$((Get-Command samply).Source)`"")
    if ($Uninstall) { $argList += '-Uninstall' }
    $p = Start-Process powershell -Verb RunAs -ArgumentList $argList -Wait -PassThru
    exit $p.ExitCode
}

$installedSamply = Join-Path $installDir 'samply.exe'
$xperf = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\Windows Performance Toolkit\xperf.exe'
# A recording that never got its stop request (samply waits for Ctrl+C forever), and the
# trace sessions it had started.
$stale = @(Get-Process samply -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $installedSamply })
if ($stale) {
    $stale | Stop-Process -Force
    foreach ($session in @('-stop'), @('-stop', 'SamplySession')) {
        Start-Process $xperf -ArgumentList $session -Wait -WindowStyle Hidden
    }
}

if ($Uninstall) {
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $installDir -ErrorAction SilentlyContinue
    Write-Host 'Removed the samply task and its files.'
    exit 0
}

New-Item -ItemType Directory -Force $installDir | Out-Null
Copy-Item -Force $Samply $installedSamply

$work = Join-Path $UserData 'samply-task'
# The wrapper the task runs. Everything it executes is fixed here; from the work directory it
# only reads whether to start and when to stop.
$wrapper = @'
$ErrorActionPreference = 'Stop'
$work = 'WORK'
$samply = 'SAMPLY'
$xperf = 'XPERF'
$file = { param($name) Join-Path $work $name }
Add-Type -Namespace Samply -Name Console -MemberDefinition @"
[DllImport("kernel32.dll")] public static extern bool FreeConsole();
[DllImport("kernel32.dll")] public static extern bool AttachConsole(uint pid);
[DllImport("kernel32.dll")] public static extern bool SetConsoleCtrlHandler(System.IntPtr handler, bool add);
[DllImport("kernel32.dll")] public static extern bool GenerateConsoleCtrlEvent(uint ctrlEvent, uint group);
"@
try {
    # Clean up after a recording that never got its stop request: samply, then the trace
    # sessions it had started (only then: other tools may be using the kernel logger).
    $stale = @(Get-Process samply -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $samply })
    if ($stale) {
        $stale | Stop-Process -Force
        foreach ($session in @('-stop'), @('-stop', 'SamplySession')) {
            Start-Process $xperf -ArgumentList $session -Wait -WindowStyle Hidden
        }
    }
    if ((Get-Content (& $file 'request.txt') -Raw).Trim() -ne 'start') { throw 'bad request' }
    $p = Start-Process $samply -PassThru -WindowStyle Hidden `
        -ArgumentList @('record', '--all', '--save-only', '--no-open', '-o', "`"$(& $file 'profile.json.gz')`"") `
        -RedirectStandardOutput (& $file 'log.txt') -RedirectStandardError (& $file 'err.txt')
    $null = $p.Handle  # keeps the exit code readable
    # Ready once xperf records (samply then asks for Ctrl+C).
    $deadline = (Get-Date).AddMinutes(1)
    while (-not ((Get-Content (& $file 'err.txt') -Raw -ErrorAction SilentlyContinue) -match 'Ctrl\+C')) {
        if ($p.HasExited -or (Get-Date) -gt $deadline) { throw 'samply did not start recording' }
        Start-Sleep -Milliseconds 100
    }
    Set-Content (& $file 'ready.txt') 'ready'
    # Until the profile script asks to stop, for an hour at most.
    $deadline = (Get-Date).AddHours(1)
    while (-not (Test-Path (& $file 'stop.txt')) -and -not $p.HasExited -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 100
    }
    # Ctrl+C, as samply expects: it stops xperf and writes the profile. Sent through its
    # console, which this script joins while ignoring the event itself.
    if (-not $p.HasExited) {
        [Samply.Console]::FreeConsole() | Out-Null
        [Samply.Console]::AttachConsole([uint32]$p.Id) | Out-Null
        [Samply.Console]::SetConsoleCtrlHandler([IntPtr]::Zero, $true) | Out-Null
        [Samply.Console]::GenerateConsoleCtrlEvent(0, 0) | Out-Null
        $p.WaitForExit()
    }
    Set-Content (& $file 'done.txt') "$($p.ExitCode)"
} catch {
    Set-Content (& $file 'done.txt') "error: $_"
}
'@
$wrapper = $wrapper.Replace('WORK', $work).Replace('SAMPLY', $installedSamply).Replace('XPERF', $xperf)
Set-Content -Path (Join-Path $installDir 'record.ps1') -Value $wrapper -Encoding UTF8

$action = New-ScheduledTaskAction -Execute 'powershell.exe' `
    -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$installDir\record.ps1`""
$principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit (New-TimeSpan -Hours 2)
Register-ScheduledTask -TaskName $taskName -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Write-Host 'Installed: run .vscode/profile.ps1 to profile without a UAC prompt.'
