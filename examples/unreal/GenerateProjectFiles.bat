@echo off
rem Generates the demo's Visual Studio solution (GlyphuxDemo.sln), as right-clicking the
rem .uproject > Generate Visual Studio project files does.
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

call "%ENGINE%\Engine\Build\BatchFiles\Build.bat" -projectfiles -project="%PROJECT%" -game -rocket -progress
if errorlevel 1 (
	pause
	exit /b 1
)
