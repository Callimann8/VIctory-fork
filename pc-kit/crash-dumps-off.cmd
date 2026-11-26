@echo off
rem Turns crash dumps for victory.exe off again (asks for admin). Double-click.
powershell -NoProfile -Command "Start-Process powershell -Verb RunAs -ArgumentList '-NoExit -ExecutionPolicy Bypass -File "%~dp0crash-dumps.ps1" -Undo'"
