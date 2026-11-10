@echo off
rem Builds the demo's editor target (which runs cargo for the plugin's Rust) and opens the
rem demo in the Unreal Editor. Building here first shows the errors, where the editor would
rem only offer to rebuild.
setlocal
rem The engine: UE_ROOT if set, else the 5.8 an engine install registers, else the launcher's
rem default folder.
set "ENGINE=%UE_ROOT%"
if not defined ENGINE for /f "tokens=2,*" %%A in ('reg query "HKLM\SOFTWARE\EpicGames\Unreal Engine\5.8" /v InstalledDirectory 2^>nul ^| find "InstalledDirectory"') do set "ENGINE=%%B"
if not defined ENGINE set "ENGINE=C:\Program Files\Epic Games\UE_5.8"
if not exist "%ENGINE%\Engine\Build\BatchFiles\Build.bat" (
	echo No Unreal Engine 5.8 in "%ENGINE%": set UE_ROOT to the engine's folder.
	pause
	exit /b 1
)
set "PROJECT=%~dp0GlyphuxDemo.uproject"

call "%ENGINE%\Engine\Build\BatchFiles\Build.bat" GlyphuxDemoEditor Win64 Development -Project="%PROJECT%" -WaitMutex
if errorlevel 1 (
	pause
	exit /b 1
)
start "" "%ENGINE%\Engine\Binaries\Win64\UnrealEditor.exe" "%PROJECT%"
