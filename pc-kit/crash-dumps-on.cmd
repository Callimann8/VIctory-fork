@echo off
rem Turns on Windows crash dumps for victory.exe (asks for admin). Double-click.
powershell -NoProfile -Command "Start-Process powershell -Verb RunAs -ArgumentList '-NoExit -ExecutionPolicy Bypass -File "%~dp0crash-dumps.ps1"'"
