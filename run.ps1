$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$frontendDir = Join-Path $repoRoot 'frontend'
$backendDir = Join-Path $repoRoot 'backend'

# The llama.cpp runtime is part of the project: built once from the pinned
# commit (runtime\llama.cpp.lock.json), then reused. An explicit override
# (COMPANION_LLAMA_SERVER_BIN) skips the build.
$runtimeServer = Join-Path $repoRoot 'runtime\bin\llama-server.exe'
if (-not $env:COMPANION_LLAMA_SERVER_BIN -and -not (Test-Path $runtimeServer)) {
    Write-Host 'The llama.cpp runtime is not built yet; building it now (one time).'
    & (Join-Path $repoRoot 'scripts\build-runtime.ps1')
    if (-not (Test-Path $runtimeServer)) { throw 'The runtime build did not produce llama-server.exe. See the output above.' }
}

Push-Location $frontendDir
try {
    if (-not (Test-Path (Join-Path $frontendDir 'node_modules'))) {
        npm ci
        if ($LASTEXITCODE -ne 0) { throw 'Frontend dependency installation failed.' }
    }
    npm run build
    if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed. The backend was not started with stale UI files.' }
}
finally {
    Pop-Location
}

# The backend serves the compiled UI and API from one process/port.
Push-Location $backendDir
$previousAddr = $env:COMPANION_ADDR
$env:COMPANION_ADDR = '127.0.0.1:5173'
try {
    cargo run --bin companion-backend
    if ($LASTEXITCODE -ne 0) { throw 'Companion stopped with an error. See the diagnostic above.' }
}
finally {
    if ($null -eq $previousAddr) {
        Remove-Item Env:COMPANION_ADDR -ErrorAction SilentlyContinue
    }
    else {
        $env:COMPANION_ADDR = $previousAddr
    }
    Pop-Location
}
