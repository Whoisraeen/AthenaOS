@echo off
REM get-bootlog.bat - double-click to pull BOOTLOG.TXT off the AthenaOS USB stick.
REM Approve the UAC prompt; the log opens in Notepad on your Desktop. No typing.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0get-bootlog.ps1"
