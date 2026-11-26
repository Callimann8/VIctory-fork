@echo off
rem Double-click: play GTA 6 from the very start with an empty data folder (real save untouched).
rem To start over again later: run "play-fresh.cmd -Reset" (empties only C:\xcl\victory-data-fresh).
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0play-fresh.ps1" %*
pause
