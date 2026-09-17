#!/usr/bin/env bash
# Build the llama.cpp inference runtime from the pinned upstream commit into
# runtime/bin, with every backend this machine can compile.
#
# The Linux and macOS counterpart of scripts/build-runtime.ps1 (Windows). Same
# lock file, same output layout, same BUILD_INFO.json, so the app finds and
# describes the runtime the same way on every platform.
#
# Backends are compiled as separately loaded modules (GGML_BACKEND_DL): one CPU
# module per instruction set on x86-64, chosen at startup, plus a GPU module per
# available SDK. macOS builds Metal into the library instead (Apple Silicon has
# one GPU API and no module choice to make).
#
# Requirements, all detected rather than assumed:
#   git, cmake >= 3.21, a C/C++ compiler; ninja when present (make otherwise)
#   Vulkan: glslc plus the Vulkan headers/loader (e.g. the Vulkan SDK, or
#           libvulkan-dev + glslc packages)
#   CUDA:   the CUDA Toolkit (nvcc on PATH, or CUDA_HOME / CUDA_PATH)
#   Metal:  macOS with Xcode command line tools
#
# Usage:
#   scripts/build-runtime.sh                       # cpu + every available GPU backend
#   scripts/build-runtime.sh --backends cpu,vulkan
#   scripts/build-runtime.sh --backends cpu,cuda --cuda-architectures "86;89;120"
#   scripts/build-runtime.sh --dry-run             # print the plan, build nothing
set -euo pipefail

backends="auto"
cuda_architectures=""
jobs=""
dry_run=0

while [ $# -gt 0 ]; do
    case "$1" in
        --backends) backends="$2"; shift 2 ;;
        --cuda-architectures) cuda_architectures="$2"; shift 2 ;;
        --jobs|-j) jobs="$2"; shift 2 ;;
        --dry-run) dry_run=1; shift ;;
        -h|--help) sed -n '2,27p' "$0"; exit 0 ;;
        *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
lock="$repo_root/runtime/llama.cpp.lock.json"
source_dir="$repo_root/build/llama.cpp/src"
build_dir="$repo_root/build/llama.cpp/build"
output_dir="$repo_root/runtime/bin"
staging_dir="$repo_root/runtime/bin.staging"

step() { printf '==> %s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }
fail() { echo "error: $*" >&2; exit 1; }

# A project folder also used from Windows (for example through WSL) holds the
# Windows runtime in runtime/bin. Replacing it would move it aside and break
# run.ps1; each system needs its own clone.
if [ -e "$output_dir/llama-server.exe" ]; then
    fail "runtime/bin holds a Windows runtime (this folder is also used from Windows). Clone the project separately for this system, or set COMPANION_LLAMA_SERVER_BIN."
fi

# Read one string field from the lock file without depending on jq.
lock_field() {
    sed -n "s/^[[:space:]]*\"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$lock" | head -n 1
}
repository="$(lock_field repository)"
tag="$(lock_field tag)"
commit="$(lock_field commit)"
[ -n "$repository" ] && [ -n "$commit" ] || fail "could not read repository/commit from $lock"

# ---------------------------------------------------------------------------
# Toolchain
# ---------------------------------------------------------------------------
os="$(uname -s)"
have git || fail "git is required"
have cmake || fail "cmake >= 3.21 is required"

generator=()
if have ninja; then generator=(-G Ninja); fi

# Toolchains are found through PATH, their SDK environment variables and
# pkg-config, never through fixed install locations.
vulkan_available=0
if have glslc && { [ -n "${VULKAN_SDK:-}" ] || { have pkg-config && pkg-config --exists vulkan; }; }; then
    vulkan_available=1
fi

nvcc_path=""
if have nvcc; then
    nvcc_path="$(command -v nvcc)"
else
    for home in "${CUDA_HOME:-}" "${CUDA_PATH:-}"; do
        if [ -n "$home" ] && [ -x "$home/bin/nvcc" ]; then nvcc_path="$home/bin/nvcc"; break; fi
    done
fi
cuda_available=0
[ -n "$nvcc_path" ] && cuda_available=1

metal_available=0
[ "$os" = "Darwin" ] && metal_available=1

# GPU hardware in this PC. An automatic build includes a GPU backend only for
# hardware that is present: a PC without an NVIDIA GPU never builds or ships
# the CUDA module and its runtime libraries, even when a CUDA Toolkit is
# installed. An explicit --backends request still builds it (for another PC).
# Linux: PCI display controllers (class 0x03xxxx) from sysfs; NVIDIA is vendor
# 0x10de.
any_gpu=0
nvidia_gpu=0
if [ "$os" = "Linux" ]; then
    for device in /sys/bus/pci/devices/*; do
        [ -r "$device/class" ] || continue
        case "$(cat "$device/class")" in
            0x03*)
                any_gpu=1
                [ "$(cat "$device/vendor" 2>/dev/null)" = "0x10de" ] && nvidia_gpu=1
                ;;
        esac
    done
elif [ "$os" = "Darwin" ]; then
    any_gpu=1
fi

selected=(cpu)
add() { case " ${selected[*]} " in *" $1 "*) ;; *) selected+=("$1") ;; esac; }
IFS=',' read -r -a requested <<< "$backends"
for backend in ${requested[@]+"${requested[@]}"}; do
    backend="$(echo "$backend" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
    case "$backend" in
        "") ;;
        auto)
            [ $vulkan_available -eq 1 ] && [ $any_gpu -eq 1 ] && [ "$os" != "Darwin" ] && add vulkan
            [ $cuda_available -eq 1 ] && [ $nvidia_gpu -eq 1 ] && add cuda
            [ $metal_available -eq 1 ] && add metal
            ;;
        cpu) ;;
        vulkan) [ $vulkan_available -eq 1 ] || fail "Vulkan requested but glslc/Vulkan headers were not found"; add vulkan ;;
        cuda)
            [ $cuda_available -eq 1 ] || fail "CUDA requested but the CUDA Toolkit (nvcc) was not found"
            [ $nvidia_gpu -eq 1 ] || echo "warning: this PC has no NVIDIA GPU; building the CUDA backend anyway because it was requested explicitly" >&2
            add cuda
            ;;
        metal) [ $metal_available -eq 1 ] || fail "Metal is only available on macOS"; add metal ;;
        *) fail "unknown backend '$backend' (use cpu, vulkan, cuda, metal or auto)" ;;
    esac
done
contains() { case " ${selected[*]} " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }
on_off() { if contains "$1"; then echo ON; else echo OFF; fi; }

step "Backends: ${selected[*]}   platform: $os $(uname -m)"
if [ "$os" = "Linux" ] && [ $any_gpu -eq 0 ]; then
    echo "    No GPU found in this PC: the Vulkan and CUDA backends are not built."
else
    [ $vulkan_available -eq 1 ] || [ "$os" = "Darwin" ] || echo "    Vulkan toolchain not found: the Vulkan backend is skipped."
    if [ $nvidia_gpu -eq 0 ]; then
        [ "$os" = "Linux" ] && echo "    No NVIDIA GPU found in this PC: the CUDA backend is not built."
    elif [ $cuda_available -eq 0 ]; then
        echo "    CUDA Toolkit not found: the CUDA backend is skipped."
    fi
fi

# Empty arrays are expanded with ${a[@]+...}: bash before 4.4 (macOS ships 3.2)
# treats "${a[@]}" of an empty array as unset under `set -u` and stops.
flags=(
    ${generator[@]+"${generator[@]}"}
    -DCMAKE_BUILD_TYPE=Release
    -DBUILD_SHARED_LIBS=ON
    -DGGML_NATIVE=OFF
    -DLLAMA_BUILD_SERVER=ON
    -DLLAMA_BUILD_TOOLS=ON
    -DLLAMA_BUILD_EXAMPLES=OFF
    -DLLAMA_BUILD_TESTS=OFF
    -DLLAMA_CURL=OFF
    -DGGML_RPC=OFF
    "-DGGML_VULKAN=$(on_off vulkan)"
    "-DGGML_CUDA=$(on_off cuda)"
    "-DGGML_METAL=$(on_off metal)"
)
# The binaries look for their libraries next to themselves, as in upstream's
# release build, not at the absolute build-folder path CMake records by
# default: runtime/bin keeps working after build/ is deleted or the project
# folder moves.
if [ "$os" = "Darwin" ]; then
    flags+=(-DCMAKE_INSTALL_RPATH=@loader_path -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON)
else
    flags+=('-DCMAKE_INSTALL_RPATH=$ORIGIN' -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON)
fi
if [ "$os" != "Darwin" ]; then
    # Runtime-selected CPU modules exist for x86-64; elsewhere one CPU backend is built.
    flags+=(-DGGML_BACKEND_DL=ON)
    case "$(uname -m)" in x86_64|amd64) flags+=(-DGGML_CPU_ALL_VARIANTS=ON) ;; esac
fi
if contains cuda; then
    flags+=("-DCMAKE_CUDA_COMPILER=$nvcc_path")
    [ -n "$cuda_architectures" ] && flags+=("-DCMAKE_CUDA_ARCHITECTURES=$cuda_architectures")
fi

if [ $dry_run -eq 1 ]; then
    step "Dry run: llama.cpp $tag ($commit)"
    echo "    source: $source_dir"
    echo "    cmake -S $source_dir -B $build_dir ${flags[*]}"
    echo "    output: $output_dir"
    exit 0
fi

# ---------------------------------------------------------------------------
# Source at the pinned commit
# ---------------------------------------------------------------------------
step "Fetching llama.cpp $tag ($commit)"
if [ ! -d "$source_dir/.git" ]; then
    mkdir -p "$source_dir"
    git -C "$source_dir" init --quiet
    git -C "$source_dir" remote add origin "$repository"
fi
if [ "$(git -C "$source_dir" rev-parse HEAD 2>/dev/null || true)" != "$commit" ]; then
    git -C "$source_dir" fetch --depth 1 origin "$commit"
    git -C "$source_dir" checkout --quiet --force FETCH_HEAD
fi
resolved="$(git -C "$source_dir" rev-parse HEAD)"
[ "$resolved" = "$commit" ] || fail "checked out $resolved, expected $commit"

# ---------------------------------------------------------------------------
# Configure and build
# ---------------------------------------------------------------------------
step "Configuring"
cmake -S "$source_dir" -B "$build_dir" "${flags[@]}"

step "Building (this takes a while; CUDA adds the most)"
# Makefiles (no ninja) build on one core unless told otherwise.
[ -n "$jobs" ] || jobs="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
build_args=(--build "$build_dir" --config Release -j "$jobs")
cmake "${build_args[@]}"

# ---------------------------------------------------------------------------
# Stage, verify, then replace runtime/bin in one step
# ---------------------------------------------------------------------------
if pgrep -f "$output_dir/llama-server" >/dev/null 2>&1; then
    fail "a llama-server from runtime/bin is running; stop the model in the app and run this again"
fi

step "Staging"
rm -rf "$staging_dir"
mkdir -p "$staging_dir"
# Executables plus every shared library the build produced (llama, ggml and its
# backend modules, mtmd); .so on Linux, .dylib on macOS. Versioned libraries
# are a file plus links (libllama.so.0 -> libllama.so.0.0.N), and the binaries
# ask for the link's name, so links are copied as links.
find "$build_dir/bin" -maxdepth 1 -type f \( -perm -u+x -o -name '*.so*' -o -name '*.dylib' \) -exec cp -p {} "$staging_dir/" \;
find "$build_dir/bin" -maxdepth 1 -type l \( -name '*.so*' -o -name '*.dylib' \) -exec cp -P {} "$staging_dir/" \;
find "$build_dir" -type f \( -name 'lib*.so*' -o -name 'lib*.dylib' \) -not -path "$build_dir/bin/*" -not -path "$staging_dir/*" -exec cp -p {} "$staging_dir/" \;
find "$build_dir" -type l \( -name 'lib*.so*' -o -name 'lib*.dylib' \) -not -path "$build_dir/bin/*" -not -path "$staging_dir/*" -exec cp -P {} "$staging_dir/" \;
# Only the backends selected now: a build folder reused from an earlier build
# can still hold another backend's module, which must not be shipped.
contains cuda || rm -f "$staging_dir"/*ggml-cuda*
contains vulkan || rm -f "$staging_dir"/*ggml-vulkan*

if contains cuda && [ "$os" = "Linux" ]; then
    # The CUDA runtime libraries the ggml-cuda module links against, so a machine
    # with only the NVIDIA driver can run the build.
    cuda_lib="$(dirname "$(dirname "$nvcc_path")")/lib64"
    for lib in libcudart.so* libcublas.so* libcublasLt.so*; do
        for match in "$cuda_lib"/$lib; do [ -e "$match" ] && cp -P "$match" "$staging_dir/"; done
    done
fi

server="$staging_dir/llama-server"
[ -x "$server" ] || fail "the build produced no llama-server"
version="$(LD_LIBRARY_PATH="$staging_dir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" DYLD_LIBRARY_PATH="$staging_dir" "$server" --version 2>&1 | tr '\n' ' ' | sed 's/"/\\"/g')"
# Off Windows every module is lib-prefixed, so `ggml-*` matches nothing; with
# pipefail that status alone used to end the script here without a message.
modules="$(cd "$staging_dir" && { ls libggml-* ggml-* 2>/dev/null || true; } | sed -E 's/(\.[0-9]+)*\.(so|dylib|dll)(\.[0-9]+)*$//' | sort -u | sed 's/.*/"&"/' | paste -sd, -)"
backend_list="$(printf '"%s",' "${selected[@]}" | sed 's/,$//')"

cat > "$staging_dir/BUILD_INFO.json" <<EOF
{
  "tag": "$tag",
  "commit": "$resolved",
  "backends": [$backend_list],
  "compiler": "$(cmake -S "$source_dir" -B "$build_dir" -LA -N 2>/dev/null | sed -n 's/^CMAKE_CXX_COMPILER:[^=]*=//p' | head -n 1)",
  "cuda_architectures": "$( contains cuda && echo "${cuda_architectures:-llama.cpp default}" )",
  "modules": [$modules],
  "version": "$version",
  "built_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF

if [ -d "$output_dir" ]; then
    rm -rf "$output_dir.previous"
    mv "$output_dir" "$output_dir.previous"
fi
mv "$staging_dir" "$output_dir"
step "Runtime ready in $output_dir"
echo "    $version"
