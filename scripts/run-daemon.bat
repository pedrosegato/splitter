@echo off
setlocal

if "%1"=="" (
    set PEER_NAME=bob
) else (
    set PEER_NAME=%1
)

if "%2"=="" (
    set PORT=7001
) else (
    set PORT=%2
)

set IDENTITY_DIR=%USERPROFILE%\.splitter\%PEER_NAME%
set CMAKE_POLICY_VERSION_MINIMUM=3.5

echo === Splitter daemon launcher ===
echo peer_name: %PEER_NAME%
echo port:      %PORT%
echo identity:  %IDENTITY_DIR%
echo.

if not exist "%IDENTITY_DIR%" mkdir "%IDENTITY_DIR%"

echo [1/3] git pull...
git pull

echo.
echo [2/3] cargo build --release...
cargo build -p splitter-cli --release
if errorlevel 1 (
    echo build failed.
    pause
    exit /b 1
)

echo.
echo === Local audio devices ===
.\target\release\splitter-cli.exe devices
echo.

echo [3/3] launching daemon...
echo press Ctrl+C inside daemon to graceful shutdown.
echo.
.\target\release\splitter-cli.exe daemon --signaling-port %PORT% --peer-name %PEER_NAME% --identity-dir "%IDENTITY_DIR%"

endlocal
