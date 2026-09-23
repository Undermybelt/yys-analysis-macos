@echo off
setlocal

rem Start the read-only desktop memory probe; Python may request administrator permission.
where py >nul 2>nul
if errorlevel 1 (
  echo Python launcher py.exe was not found. Please install Python 3.8 or newer.
  exit /b 2
)

py -3 "%~dp0desktop-memory-probe.py" %*
exit /b %errorlevel%
