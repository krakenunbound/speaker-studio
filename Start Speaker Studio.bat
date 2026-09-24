@echo off
if not exist "%~dp0.engine\Scripts\pythonw.exe" (
  powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1"
  exit /b %ERRORLEVEL%
)
start "" "%~dp0Speaker Studio.exe"
exit /b 0
