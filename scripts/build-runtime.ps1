<#
.SYNOPSIS
  Build a trimmed, machine-targeted llama-server package from a pinned upstream
  llama.cpp commit, instead of forking llama.cpp.

.DESCRIPTION
  Why a build recipe and not a fork:
  - The inference kernels are identical; a fork cannot make them faster. Speed
    comes from the launch flags Companion already sets (measured in
    docs/PERFORMANCE.md) and from the request path (prompt-cache alignment,
    early stop, streaming).
  - What a custom build DOES buy: a much smaller package (one CUDA architecture
    instead of every architecture; only the server, no CLI/bench/quantize/
    perplexity/tts tools), a CPU backend compiled for the host's instruction set
    when portability is not required, and reproducible provenance (commit hash
    recorded next to the binary).
  - Upstream moves daily (new model architectures, new speculative modes). A
    pin + recipe follows upstream with one variable change; a fork rots.

  This script needs: git, CMake >= 3.21, Ninja (optional), the MSVC "Desktop
  development with C++" workload, and for GPU builds the CUDA Toolkit whose
  driver is installed. None of these are installed on the machine this recipe
  was written on, so it is a documented, reviewed procedure, not a tested one.

.PARAMETER Commit
  Upstream llama.cpp commit or tag to build. Default: the commit of the bundled
  binary (`llama-server --version` reports it), so a rebuild reproduces what
  shipped.

.PARAMETER Backend
  cuda | vulkan | cpu. Vulkan runs on Intel/AMD/NVIDIA GPUs including the
  integrated Arc GPU in Core Ultra laptops; CUDA is fastest on NVIDIA.

.PARAMETER CudaArch
  CUDA architectures to compile, e.g. "120" (RTX 50 series), "89" (RTX 40),
  "86" (RTX 30). Fewer architectures = smaller ggml-cuda.dll and faster load.

.PARAMETER Native
  Compile the CPU backend for THIS machine's instruction set (AVX2/AVX-512 as
  present). Faster on the build host, not portable to other CPUs. Off by
  default: the portable build ships one CPU backend per instruction set and
  picks the best at startup, which is what the official releases do.

.EXAMPLE
  .\scripts\build-runtime.ps1 -Backend cuda -CudaArch 120
  .\scripts\build-runtime.ps1 -Backend vulkan
  .\scripts\build-runtime.ps1 -Backend cpu -Native
#>
param(
    [string]$Commit = '5266f24da',
    [ValidateSet('cuda', 'vulkan', 'cpu')]
    [string]$Backend = 'cuda',
    [string]$CudaArch = '120',
    [switch]$Native,
    [string]$Output = (Join-Path (Split-Path -Parent $PSScriptRoot) 'models\bin-custom')
)

$ErrorActionPreference = 'Stop'
$work = Join-Path $env:TEMP "llama-cpp-build-$Commit"

foreach ($tool in 'git', 'cmake') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "$tool is required. Install it and open a new terminal."
    }
}
if ($Backend -eq 'cuda' -and -not (Get-Command nvcc -ErrorAction SilentlyContinue)) {
    throw 'The CUDA Toolkit (nvcc) is required for a CUDA build. Use -Backend vulkan or cpu otherwise.'
}

if (-not (Test-Path $work)) {
    git clone --filter=blob:none https://github.com/ggml-org/llama.cpp.git $work
}
Push-Location $work
try {
    git fetch --all --tags
    git checkout --detach $Commit
    $resolved = git rev-parse --short HEAD

    $flags = @(
        '-DCMAKE_BUILD_TYPE=Release',
        '-DBUILD_SHARED_LIBS=ON',
        '-DLLAMA_BUILD_TESTS=OFF',
        '-DLLAMA_BUILD_EXAMPLES=OFF',
        '-DLLAMA_BUILD_TOOLS=ON',
        '-DLLAMA_CURL=OFF',
        '-DGGML_LTO=ON'
    )
    if ($Native) {
        $flags += '-DGGML_NATIVE=ON'
    }
    else {
        # Portable CPU backends: one DLL per instruction set, chosen at runtime.
        $flags += '-DGGML_NATIVE=OFF', '-DGGML_BACKEND_DL=ON', '-DGGML_CPU_ALL_VARIANTS=ON'
    }
    switch ($Backend) {
        'cuda' {
            $flags += '-DGGML_CUDA=ON', "-DCMAKE_CUDA_ARCHITECTURES=$CudaArch", '-DGGML_CUDA_FA_ALL_QUANTS=ON'
        }
        'vulkan' { $flags += '-DGGML_VULKAN=ON' }
        'cpu' { }
    }

    $build = "build-$Backend"
    cmake -S . -B $build @flags
    cmake --build $build --config Release --target llama-server -j
    if ($LASTEXITCODE -ne 0) { throw 'Build failed.' }

    # Ship only what llama-server needs at runtime.
    New-Item -ItemType Directory -Force $Output | Out-Null
    Get-ChildItem -Recurse -Path $build -Include 'llama-server.exe', 'llama-server-impl.dll', 'llama.dll', 'llama-common.dll', 'mtmd.dll', 'ggml*.dll', 'libomp.dll' |
        Where-Object { $_.Name -notlike 'ggml-rpc*' } |
        ForEach-Object { Copy-Item $_.FullName -Destination $Output -Force }
    if ($Backend -eq 'cuda') {
        # The CUDA runtime redistributables (cudart64_*.dll, cublas64_*.dll,
        # cublasLt64_*.dll) come from the toolkit's bin folder; they are not
        # built. cuBLAS is still required for f16 matrix paths.
        $cudaBin = Join-Path (Split-Path -Parent (Split-Path -Parent (Get-Command nvcc).Source)) 'bin'
        Get-ChildItem $cudaBin -Include 'cudart64_*.dll', 'cublas64_*.dll', 'cublasLt64_*.dll' -Recurse | Copy-Item -Destination $Output -Force
    }
    Set-Content -Path (Join-Path $Output 'RUNTIME-BUILD.txt') -Value @(
        "llama.cpp commit: $resolved",
        "backend: $Backend",
        "cuda architectures: $(if ($Backend -eq 'cuda') { $CudaArch } else { 'n/a' })",
        "native cpu: $($Native.IsPresent)",
        "flags: $($flags -join ' ')",
        "built: $(Get-Date -Format o)"
    )
    Write-Host "Runtime package written to $Output. Point COMPANION_LLAMA_SERVER_BIN at its llama-server.exe or replace models\bin."
}
finally {
    Pop-Location
}
