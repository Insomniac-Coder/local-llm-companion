@echo off
rem Build (first run) and start Companion from Command Prompt on Windows: the
rem counterpart of run.ps1, with the same steps in the same order. Keep the
rem window open and press Ctrl+C once to stop the app and the model server
rem (cmd.exe then asks "Terminate batch job (Y/N)?"; either answer is fine).
setlocal
set "ROOT=%~dp0"
set "ROOT=%ROOT:~0,-1%"

where npm >nul 2>nul || (
    echo error: npm was not found on PATH. Install Node.js 24 or newer, which includes npm. See Prerequisites in README.md.
    exit /b 1
)
where cargo >nul 2>nul || (
    echo error: cargo was not found on PATH. Install Rust with rustup and open a new window. See Prerequisites in README.md.
    exit /b 1
)

rem The llama.cpp runtime is part of the project: built once from the pinned
rem commit (runtime\llama.cpp.lock.json), then reused. An explicit override
rem (COMPANION_LLAMA_SERVER_BIN) skips the build. The build script is
rem PowerShell; -ExecutionPolicy Bypass applies to that one process only.
if not defined COMPANION_LLAMA_SERVER_BIN if not exist "%ROOT%\runtime\bin\llama-server.exe" (
    echo The llama.cpp runtime is not built yet; building it now, one time.
    call powershell -NoProfile -ExecutionPolicy Bypass -File "%ROOT%\scripts\build-runtime.ps1"
    if errorlevel 1 (
        echo error: The runtime build failed. See the output above.
        exit /b 1
    )
    if not exist "%ROOT%\runtime\bin\llama-server.exe" (
        echo error: The runtime build did not produce llama-server.exe. See the output above.
        exit /b 1
    )
)

rem npm is npm.cmd: without "call" this script would end when it returns. Every
rem tool is started with "call", which works the same for .exe files.
pushd "%ROOT%\frontend"
if not exist node_modules (
    call npm ci
    if errorlevel 1 (
        popd
        echo error: Frontend dependency installation failed.
        exit /b 1
    )
)
call npm run build
if errorlevel 1 (
    popd
    echo error: Frontend build failed. The backend was not started with stale UI files.
    exit /b 1
)
popd

rem The backend serves the compiled UI and API from one process and port.
rem setlocal keeps COMPANION_ADDR inside this script, as run.ps1 restores it.
pushd "%ROOT%\backend"
set "COMPANION_ADDR=127.0.0.1:5173"
call cargo run --bin companion-backend
set "EXIT_CODE=%ERRORLEVEL%"
popd
if not "%EXIT_CODE%"=="0" (
    echo error: Companion stopped with an error. See the diagnostic above.
    exit /b 1
)
endlocal
