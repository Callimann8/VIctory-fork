@echo off
rem Marks the test run going on now as noisy (you are using the PC). Double-click it any time during a run.
if not exist "C:\xcl\runs\testbench" mkdir "C:\xcl\runs\testbench"
echo noisy > "C:\xcl\runs\testbench\NOISY"
echo The current test run is marked noisy.
timeout /t 3 >nul
