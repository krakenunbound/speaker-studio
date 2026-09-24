@echo off
if not exist "%~dp0.engine\Scripts\python.exe" goto setup
if not exist "%~dp0.engine\speaker-studio-ready" goto setup
if not exist "%~dp0Speaker Studio.exe" goto setup
start "" "%~dp0Speaker Studio.exe"
exit /b 0

:setup
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1"
exit /b %ERRORLEVEL%
