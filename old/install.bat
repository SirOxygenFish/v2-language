@echo off
REM V2 Language installer (Windows x64). Double-click to install.
echo Installing the V2 language...
powershell -ExecutionPolicy Bypass -NoProfile -File "%~dp0install.ps1" %*
echo.
pause
