$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$frontendDir = Join-Path $repoRoot 'frontend'
$backendDir = Join-Path $repoRoot 'backend'

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
