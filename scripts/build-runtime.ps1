<#
.SYNOPSIS
  Build the llama.cpp inference runtime from the pinned upstream commit into
  runtime\bin, with every backend this machine can compile.

.DESCRIPTION
  The app runs llama-server as a separate process. This script produces that
  runtime from source, so it is part of the project rather than a download:
  the commit comes from runtime\llama.cpp.lock.json, and the result records
  exactly what was built in runtime\bin\BUILD_INFO.json.

  One build contains all selected backends. They are compiled as separately
  loaded modules (GGML_BACKEND_DL): one CPU module per x86 instruction set,
  chosen at startup for the processor it runs on, plus Vulkan and CUDA modules
  when their SDKs are present. A machine without a GPU, or without a CUDA
  driver, simply never loads those modules.

  Requirements, all detected rather than assumed:
    git; Visual Studio 2022 or Build Tools with "Desktop development with C++"
    (provides the compiler, CMake and Ninja);
    for Vulkan: the Vulkan SDK (VULKAN_SDK);
    for CUDA: the CUDA Toolkit (nvcc / CUDA_PATH). Its runtime libraries are
    copied next to the build so machines without the toolkit can run it.
  The compiler used is recorded. As in the official llama.cpp Windows release,
  clang (Visual Studio's "C++ Clang tools for Windows") builds the CPU modules
  and the tools when installed, and MSVC the GPU modules; without clang, MSVC
  builds everything.

.PARAMETER Backends
  auto (default): cpu plus every GPU backend whose SDK is installed.
  Or any of: cpu, vulkan, cuda.

.PARAMETER CudaArchitectures
  CUDA compute capabilities to compile, e.g. "120" (compute capability 12.0) or
  "86;89;120". Empty: llama.cpp's default list (larger, runs on more GPUs).

.EXAMPLE
  .\scripts\build-runtime.ps1
  .\scripts\build-runtime.ps1 -Backends cpu,vulkan
  .\scripts\build-runtime.ps1 -Backends cpu,cuda -CudaArchitectures 120
#>
param(
    [string[]]$Backends = @('auto'),
    [string]$CudaArchitectures = '',
    [int]$Jobs = 0
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$lock = Get-Content (Join-Path $repoRoot 'runtime\llama.cpp.lock.json') -Raw | ConvertFrom-Json
$sourceDir = Join-Path $repoRoot 'build\llama.cpp\src'
# Set once the compiler is chosen: one build folder per compiler, because
# CMake cannot switch compilers inside an existing build folder.
$buildDir = $null
$outputDir = Join-Path $repoRoot 'runtime\bin'
$stagingDir = Join-Path $repoRoot 'runtime\bin.staging'

function Step($message) { Write-Host "==> $message" -ForegroundColor Cyan }

# Run a native program and fail only on its exit code. Windows PowerShell 5.1
# turns anything a native program writes to stderr (progress, warnings) into an
# error record when output is redirected, which under 'Stop' aborted the build
# on harmless messages from vcvars, git and CMake.
function Invoke-Native {
    param([string]$File, [string[]]$Arguments, [string]$Failure)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $File @Arguments 2>&1 | ForEach-Object { "$_" }
        $code = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previous
    }
    if ($Failure -and $code -ne 0) { throw "$Failure (exit $code)" }
}

# ---------------------------------------------------------------------------
# Toolchain
# ---------------------------------------------------------------------------
if (-not (Get-Command git -ErrorAction SilentlyContinue)) { throw 'git is required.' }

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) {
    throw 'Visual Studio 2022 or Build Tools with "Desktop development with C++" is required.'
}
$vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vsPath) { throw 'No Visual Studio installation with the C++ x64 tools was found.' }

Step "Loading the MSVC x64 environment from $vsPath"
$vcvars = Join-Path $vsPath 'VC\Auxiliary\Build\vcvars64.bat'
# vcvars looks for vswhere on PATH and prints a warning when it is missing.
$env:PATH = "$(Split-Path -Parent $vswhere);$env:PATH"
$previousPreference = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
try {
    cmd /c "`"$vcvars`" >nul 2>nul && set" 2>$null | ForEach-Object {
        if ("$_" -match '^([^=]+)=(.*)$') { Set-Item -Path "Env:$($matches[1])" -Value $matches[2] }
    }
}
finally {
    $ErrorActionPreference = $previousPreference
}
if (-not (Get-Command cl -ErrorAction SilentlyContinue)) { throw 'Loading the MSVC environment failed: cl.exe is not on PATH.' }

foreach ($tool in 'cmake', 'ninja') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        $bundled = Get-ChildItem -Path (Join-Path $vsPath 'Common7\IDE\CommonExtensions\Microsoft\CMake') -Recurse -Filter "$tool.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $bundled) { throw "$tool was not found on PATH or in Visual Studio." }
        $env:PATH = "$($bundled.DirectoryName);$env:PATH"
    }
}

# The GNU-style clang driver, as in the official Windows release, not
# clang-cl: ggml gives clang-cl MSVC's /arch flags, which cannot enable the
# instruction sets some CPU variants need (the Alder Lake variant fails on
# AVX-VNNI). Paths, not command objects: Get-Command and Get-Item expose them
# differently.
$clang = (Get-Command clang -ErrorAction SilentlyContinue).Source
if (-not $clang) {
    $candidate = Join-Path $vsPath 'VC\Tools\Llvm\x64\bin\clang.exe'
    if (Test-Path $candidate) { $clang = $candidate }
}
$compiler = if ($clang) { 'clang' } else { 'msvc' }
# The GPU build must not find clang first on PATH: CMake would pick it.
$pathWithoutClang = $env:PATH
if ($clang) {
    # The toolchain file names the compilers without a path.
    $env:PATH = "$(Split-Path -Parent $clang);$env:PATH"
}

# SDK installers set these system-wide, but a terminal (or app) opened before
# the install does not see them until it restarts.
foreach ($name in 'VULKAN_SDK', 'CUDA_PATH') {
    if (-not [Environment]::GetEnvironmentVariable($name, 'Process')) {
        $value = [Environment]::GetEnvironmentVariable($name, 'Machine')
        if (-not $value) { $value = [Environment]::GetEnvironmentVariable($name, 'User') }
        if ($value) { Set-Item -Path "Env:$name" -Value $value }
    }
}
$buildDir = Join-Path $repoRoot "build\llama.cpp\build-$compiler"
# GPU modules are always compiled by MSVC (nvcc on Windows accepts only MSVC as
# its host compiler). With clang they are a second, GPU-only build in this
# folder, which is also the folder of an MSVC-only build, so switching between
# the two does not recompile CUDA.
$gpuBuildDir = Join-Path $repoRoot 'build\llama.cpp\build-msvc'

$vulkanAvailable = [bool]$env:VULKAN_SDK -and (Test-Path (Join-Path $env:VULKAN_SDK 'Bin\glslc.exe'))
$nvcc = (Get-Command nvcc -ErrorAction SilentlyContinue).Source
if (-not $nvcc -and $env:CUDA_PATH) {
    $candidate = Join-Path $env:CUDA_PATH 'bin\nvcc.exe'
    if (Test-Path $candidate) { $nvcc = $candidate }
}
$cudaAvailable = [bool]$nvcc

# GPU hardware in this PC. An automatic build includes a GPU backend only for
# hardware that is present: a PC without an NVIDIA GPU never builds or ships
# the CUDA module and its runtime libraries (hundreds of MB), even when a CUDA
# Toolkit happens to be installed. An explicit -Backends request still builds
# it (for example to prepare a runtime for another PC).
$physicalAdapters = @(Get-CimInstance Win32_VideoController -ErrorAction SilentlyContinue |
    Where-Object { $_.PNPDeviceID -match '^PCI\\VEN_' })
$nvidiaGpu = [bool]($physicalAdapters | Where-Object { $_.PNPDeviceID -match 'VEN_10DE' })
$anyGpu = $physicalAdapters.Count -gt 0

$selected = @()
foreach ($backend in ($Backends | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim().ToLower() } | Where-Object { $_ })) {
    switch ($backend) {
        'auto' {
            $selected += 'cpu'
            if ($vulkanAvailable -and $anyGpu) { $selected += 'vulkan' }
            if ($cudaAvailable -and $nvidiaGpu) { $selected += 'cuda' }
        }
        'cpu' { $selected += 'cpu' }
        'vulkan' {
            if (-not $vulkanAvailable) { throw 'Vulkan was requested but the Vulkan SDK was not found (set VULKAN_SDK).' }
            $selected += 'vulkan'
        }
        'cuda' {
            if (-not $cudaAvailable) { throw 'CUDA was requested but the CUDA Toolkit (nvcc) was not found.' }
            if (-not $nvidiaGpu) { Write-Warning 'This PC has no NVIDIA GPU; building the CUDA backend anyway because it was requested explicitly.' }
            $selected += 'cuda'
        }
        default { throw "Unknown backend '$backend'. Use cpu, vulkan, cuda or auto." }
    }
}
$selected = $selected | Select-Object -Unique
if ($selected -notcontains 'cpu') { $selected = @('cpu') + $selected }
Step "Backends: $($selected -join ', ')   compiler: $compiler"
if (-not $anyGpu) { Write-Host '    No GPU found in this PC: the Vulkan and CUDA backends are not built.' }
elseif (-not $vulkanAvailable) { Write-Host '    Vulkan SDK not found: the Vulkan backend is skipped.' }
if ($anyGpu -and -not $nvidiaGpu) { Write-Host '    No NVIDIA GPU found in this PC: the CUDA backend is not built.' }
elseif ($anyGpu -and -not $cudaAvailable) { Write-Host '    CUDA Toolkit not found: the CUDA backend is skipped.' }

# ---------------------------------------------------------------------------
# Source at the pinned commit
# ---------------------------------------------------------------------------
Step "Fetching llama.cpp $($lock.tag) ($($lock.commit))"
if (-not (Test-Path (Join-Path $sourceDir '.git'))) {
    New-Item -ItemType Directory -Force $sourceDir | Out-Null
    Invoke-Native git @('-C', $sourceDir, 'init', '--quiet') 'git init failed'
    Invoke-Native git @('-C', $sourceDir, 'remote', 'add', 'origin', $lock.repository) 'git remote add failed'
}
$current = (Invoke-Native git @('-C', $sourceDir, 'rev-parse', 'HEAD') $null | Select-Object -Last 1)
if ($current -ne $lock.commit) {
    Invoke-Native git @('-C', $sourceDir, 'fetch', '--depth', '1', 'origin', $lock.commit) 'Fetching the pinned commit failed'
    Invoke-Native git @('-C', $sourceDir, 'checkout', '--quiet', '--force', 'FETCH_HEAD') 'Checking out the pinned commit failed'
}
$resolved = (Invoke-Native git @('-C', $sourceDir, 'rev-parse', 'HEAD') 'git rev-parse failed' | Select-Object -Last 1)
if ($resolved -ne $lock.commit) { throw "Checked out $resolved, expected $($lock.commit)." }

# ---------------------------------------------------------------------------
# Configure and build
# ---------------------------------------------------------------------------
$commonFlags = @(
    '-G', 'Ninja',
    '-DCMAKE_BUILD_TYPE=Release',
    '-DBUILD_SHARED_LIBS=ON',
    '-DGGML_BACKEND_DL=ON',
    '-DGGML_NATIVE=OFF',
    '-DLLAMA_BUILD_SERVER=ON',
    '-DLLAMA_BUILD_TOOLS=ON',
    '-DLLAMA_BUILD_EXAMPLES=OFF',
    '-DLLAMA_BUILD_TESTS=OFF',
    '-DLLAMA_CURL=OFF',
    '-DGGML_RPC=OFF'
)
$gpuFlags = @(
    "-DGGML_VULKAN=$(if ($selected -contains 'vulkan') { 'ON' } else { 'OFF' })",
    "-DGGML_CUDA=$(if ($selected -contains 'cuda') { 'ON' } else { 'OFF' })"
)
if ($selected -contains 'cuda' -and $CudaArchitectures) {
    $gpuFlags += "-DCMAKE_CUDA_ARCHITECTURES=$CudaArchitectures"
}
$gpuSelected = @($selected | Where-Object { $_ -ne 'cpu' })
$gpuTargets = @($gpuSelected | ForEach-Object { "ggml-$_" })

$buildArgs = @('--build', $buildDir, '--config', 'Release')
if ($Jobs -gt 0) { $buildArgs += '-j', $Jobs }

if ($compiler -eq 'clang') {
    # Measured on the same commit: MSVC-built CPU modules read prompts 7% slower
    # than clang-built ones. clang builds the CPU modules, llama-server and the
    # tools; MSVC builds only the GPU modules, which load into it through
    # ggml's C interface, the way the official release combines them.
    $flags = $commonFlags + @(
        "-DCMAKE_TOOLCHAIN_FILE=$(Join-Path $sourceDir 'cmake\x64-windows-llvm.cmake')",
        '-DGGML_CPU_ALL_VARIANTS=ON',
        '-DGGML_VULKAN=OFF',
        '-DGGML_CUDA=OFF'
    )
    $gpuBuildFlags = $commonFlags + @('-DCMAKE_C_COMPILER=cl', '-DCMAKE_CXX_COMPILER=cl', '-DGGML_CPU=OFF') + $gpuFlags
}
else {
    $flags = $commonFlags + @('-DGGML_CPU_ALL_VARIANTS=ON') + $gpuFlags
    $gpuBuildFlags = @()
}

Step "Configuring (CPU modules and tools: $compiler)"
Invoke-Native cmake (@('-S', $sourceDir, '-B', $buildDir) + $flags) 'CMake configuration failed'
Step 'Building the CPU modules and tools'
Invoke-Native cmake $buildArgs 'Build failed'

if ($gpuBuildFlags -and $gpuTargets) {
    # Also covers helper builds CMake starts on its own (the Vulkan shader
    # generator), which take the compiler from PATH rather than these flags.
    $env:PATH = $pathWithoutClang
    Step "Configuring (GPU modules: msvc)"
    Invoke-Native cmake (@('-S', $sourceDir, '-B', $gpuBuildDir) + $gpuBuildFlags) 'CMake configuration of the GPU modules failed'
    Step "Building $($gpuTargets -join ', ') (this takes a while; CUDA the most)"
    $gpuBuildArgs = @('--build', $gpuBuildDir, '--config', 'Release', '--target') + $gpuTargets
    if ($Jobs -gt 0) { $gpuBuildArgs += '-j', $Jobs }
    Invoke-Native cmake $gpuBuildArgs 'Build of the GPU modules failed'
}

# ---------------------------------------------------------------------------
# Stage, verify, then replace runtime\bin in one step
# ---------------------------------------------------------------------------
$running = Get-Process -Name 'llama-server' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($outputDir, [StringComparison]::OrdinalIgnoreCase) }
if ($running) { throw 'A llama-server from runtime\bin is running. Stop the model in the app, then run this again.' }

Step 'Staging'
if (Test-Path $stagingDir) { Remove-Item -Recurse -Force $stagingDir }
New-Item -ItemType Directory -Force $stagingDir | Out-Null
# Only the backends selected now: a build folder reused from an earlier build
# can still hold another backend's module, which must not be shipped.
Get-ChildItem (Join-Path $buildDir 'bin') -File |
    Where-Object { $_.Extension -in '.exe', '.dll' } |
    Where-Object { ($selected -contains 'cuda') -or ($_.BaseName -notlike 'ggml-cuda*') } |
    Where-Object { ($selected -contains 'vulkan') -or ($_.BaseName -notlike 'ggml-vulkan*') } |
    Copy-Item -Destination $stagingDir
if ($gpuBuildFlags) {
    # Only the GPU modules themselves: everything they load (ggml-base) comes
    # from the clang build, at the same commit.
    foreach ($target in $gpuTargets) {
        $module = Join-Path $gpuBuildDir "bin\$target.dll"
        if (-not (Test-Path $module)) { throw "The GPU build produced no $target.dll." }
        Copy-Item $module -Destination $stagingDir
    }
}
if ($compiler -eq 'clang') {
    # The CPU modules link clang's OpenMP runtime. Visual Studio's clang imports
    # it as libomp140.x86_64.dll, which Windows does not always have, so the
    # exact library the modules import is copied next to them.
    $dumpbin = (Get-Command dumpbin -ErrorAction SilentlyContinue).Source
    if (-not $dumpbin) { throw 'dumpbin.exe was not found; it comes with the MSVC tools.' }
    $openmpImports = Get-ChildItem $stagingDir -Filter 'ggml-cpu-*.dll' |
        ForEach-Object { Invoke-Native $dumpbin @('/nologo', '/dependents', $_.FullName) "Reading the imports of $($_.Name) failed" } |
        ForEach-Object { "$_".Trim() } |
        Where-Object { $_ -match '^libomp[\w.]*\.dll$' } |
        Sort-Object -Unique
    foreach ($name in $openmpImports) {
        if (Test-Path (Join-Path $stagingDir $name)) { continue }
        $found = @(
            (Join-Path (Split-Path -Parent $clang) $name)
            Get-ChildItem (Join-Path $vsPath 'VC\Redist\MSVC') -Recurse -Filter $name -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -match '\\x64\\' } | ForEach-Object { $_.FullName }
            (Join-Path $env:SystemRoot "System32\$name")
        ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
        if (-not $found) { throw "The OpenMP runtime $name that the CPU modules import was not found." }
        Copy-Item $found -Destination $stagingDir
        Write-Host "    OpenMP runtime: $name from $found"
    }
}
if ($selected -contains 'cuda') {
    $cudaBin = Split-Path -Parent $nvcc
    # Redistributable CUDA runtime libraries the ggml-cuda module links against
    # (cuBLAS needs nvJitLink). CUDA 13 keeps them in bin\x64, older releases
    # in bin.
    $runtimeDlls = @($cudaBin, (Join-Path $cudaBin 'x64')) |
        Where-Object { Test-Path $_ } |
        ForEach-Object { Get-ChildItem $_ -Filter '*.dll' } |
        Where-Object { $_.Name -match '^(cudart64_|cublas64_|cublasLt64_|nvJitLink_)' }
    if (-not ($runtimeDlls | Where-Object { $_.Name -like 'cublas64_*' })) {
        throw "The CUDA runtime libraries were not found next to $nvcc."
    }
    $runtimeDlls | Copy-Item -Destination $stagingDir
}

$server = Join-Path $stagingDir 'llama-server.exe'
if (-not (Test-Path $server)) { throw 'The build produced no llama-server.exe.' }
$version = ((Invoke-Native $server @('--version') $null) | Out-String).Trim()
$modules = Get-ChildItem $stagingDir -Filter 'ggml-*.dll' | ForEach-Object { $_.BaseName } | Sort-Object

[ordered]@{
    tag = $lock.tag
    commit = $resolved
    backends = $selected
    gpu_present = $anyGpu
    nvidia_gpu_present = $nvidiaGpu
    compiler = $compiler
    gpu_modules_compiler = $(if ($gpuTargets) { 'msvc' } else { $null })
    cuda_architectures = $(if ($selected -contains 'cuda') { if ($CudaArchitectures) { $CudaArchitectures } else { 'llama.cpp default' } } else { $null })
    modules = $modules
    version = $version
    built_at = (Get-Date).ToString('o')
    flags = $flags
    gpu_module_flags = $(if ($gpuBuildFlags) { $gpuBuildFlags } else { $null })
} | ConvertTo-Json -Depth 4 | Set-Content -Path (Join-Path $stagingDir 'BUILD_INFO.json') -Encoding utf8

if (Test-Path $outputDir) {
    $previous = "$outputDir.previous"
    if (Test-Path $previous) { Remove-Item -Recurse -Force $previous }
    Rename-Item $outputDir (Split-Path -Leaf $previous)
}
Rename-Item $stagingDir (Split-Path -Leaf $outputDir)
Step "Runtime ready in $outputDir"
Write-Host "    $version"
Write-Host "    modules: $($modules -join ', ')"
