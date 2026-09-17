# Local LLM PC Companion

A privacy-first AI assistant that runs entirely on your PC: chat, code questions and agent tasks on
local GGUF models, served by [llama.cpp](https://github.com/ggml-org/llama.cpp). No cloud API key is
needed and nothing leaves your machine unless you turn on web search.

The llama.cpp runtime is **part of this project**: it is built from a pinned upstream commit with the
backends your PC can use (CPU always; Vulkan and CUDA when the hardware and SDKs are present). Model
weights are not included.

More documentation: `docs/ARCHITECTURE.md`, `docs/PERFORMANCE.md`, and the audit records in
`docs/validation/` and `docs/research/`.

## Layout

```
local-llm-companion/
  backend/    Rust (Axum) API: chat, models, tools, agent, search, vision, documents, memory
  frontend/   React + TypeScript + Vite UI, served by the backend
  runtime/    llama.cpp.lock.json (the pinned commit); bin/ is built here (not in Git)
  scripts/    build-runtime.ps1 / build-runtime.sh, benchmarks, backups, end-to-end checks
  models/     your GGUF models, one folder per model; two come with the repository (Git LFS); modeldownloader.py
  plugins/    plugin manifests
  docs/       architecture, performance, research and validation records
  run.ps1     build (first run) and start the app on Windows (PowerShell)
  run.bat     the same from Command Prompt
  run.sh      the same on Linux and macOS
```

## 1. Prerequisites

### Windows

Install these, then open a **new** PowerShell so PATH changes take effect.

| Tool | Needed for | Notes |
| --- | --- | --- |
| [Git](https://git-scm.com/downloads/win) | cloning; fetching llama.cpp | |
| [Node.js 24 LTS](https://nodejs.org/en/download) with npm | the UI and its tests | tests import TypeScript directly (Node 24+) |
| [Rust](https://rust-lang.org/tools/install/) (stable, MSVC toolchain) | the backend | `rustup update stable` if already installed |
| [Visual Studio 2022 or Build Tools](https://visualstudio.microsoft.com/downloads/) with **Desktop development with C++** | the backend and llama.cpp | provides the compiler, Windows SDK, CMake and Ninja |
| **C++ Clang tools for Windows** (a Visual Studio Installer component: *C++ Clang Compiler for Windows*) | *recommended*: faster CPU inference | the runtime build uses clang automatically when it is installed; see the note below |
| [Vulkan SDK](https://vulkan.lunarg.com/sdk/home) | *optional*: the Vulkan GPU backend | any GPU vendor, including integrated GPUs |
| [CUDA Toolkit](https://developer.nvidia.com/cuda-downloads) 12 or 13 | *optional*: the CUDA backend | NVIDIA GPUs only; the display driver is separate and not required from this installer |
| Python 3 | *optional*: `models/modeldownloader.py`, `scripts/backup-data.py` | standard library only |

Check:

```powershell
git --version; node --version; npm --version; rustc --version; cargo --version
```

**Why clang is preferred.** llama.cpp's own Windows releases build the CPU modules with clang. Built
with Microsoft's compiler (MSVC) from the same commit, CPU prompt processing measured 7% slower
(generation speed and GPU speed were the same). When Visual Studio's clang component is installed,
`scripts/build-runtime.ps1` builds the way the official release does: clang builds the CPU modules
and the tools, and MSVC builds only the CUDA and Vulkan modules (NVIDIA supports only MSVC as CUDA's
host compiler on Windows). Without clang, MSVC builds everything. `runtime/bin/BUILD_INFO.json`
records which compiler built which part. If you add clang later, run the build script again.

### Linux / macOS

`git`, `cmake` 3.21+, a C/C++ compiler (`ninja` is used when present), Rust, Node.js 24+.
Optional: the Vulkan SDK or `libvulkan-dev` + `glslc` (Vulkan), the CUDA Toolkit (CUDA, Linux).
macOS builds Metal automatically. Start the app with `run.sh` (see "Run" below).

## 2. Get the project

```powershell
git clone https://github.com/Insomniac-Coder/local-llm-companion.git
cd local-llm-companion
```

Run the remaining commands from this directory.

## 3. Build the llama.cpp runtime

The runtime is built once into `runtime/bin/` from the commit in `runtime/llama.cpp.lock.json`.
`run.ps1` (Windows) and `run.sh` (Linux, macOS) do this automatically the first time, or run it yourself:

```powershell
.\scripts\build-runtime.ps1                         # Windows: CPU + every GPU backend this PC can use
.\scripts\build-runtime.ps1 -Backends cpu           # CPU only
.\scripts\build-runtime.ps1 -Backends cpu,cuda -CudaArchitectures 120   # smaller CUDA build for one GPU generation
```

```bash
scripts/build-runtime.sh                            # Linux / macOS
scripts/build-runtime.sh --dry-run                  # print the plan, build nothing
```

What the automatic build (`auto`, the default) chooses:

- **CPU**: always, as one module per x86 instruction set; the fastest one your processor supports is
  loaded at startup.
- **Vulkan**: when the PC has a GPU **and** the Vulkan SDK is installed.
- **CUDA**: when the PC has an **NVIDIA GPU** **and** the CUDA Toolkit is installed. On a PC without
  an NVIDIA GPU the CUDA backend and its runtime libraries are not built or copied at all, even if a
  toolkit is installed. (Asking for `-Backends cuda` explicitly still builds it, with a warning, for
  preparing a runtime for another PC.)
- The CUDA runtime libraries (cuBLAS, cudart, nvJitLink) are copied next to the build, so the PC that
  runs it needs only the NVIDIA display driver, not the toolkit.

The build takes about 10 minutes with CUDA on a fast desktop CPU (much less without it). It fetches
llama.cpp into `build/llama.cpp/`, builds there, then replaces `runtime/bin/` in one step and writes
`runtime/bin/BUILD_INFO.json` (commit, backends, compiler, modules). Rebuild after changing
`runtime/llama.cpp.lock.json` or installing a GPU SDK.

Check the result:

```powershell
.\runtime\bin\llama-server.exe --version
.\runtime\bin\llama-server.exe --list-devices     # CUDA0 / Vulkan0 lines when those backends work
```

```bash
runtime/bin/llama-server --version                  # Linux / macOS
runtime/bin/llama-server --list-devices
```

To use a llama.cpp you built or installed elsewhere, set `COMPANION_LLAMA_SERVER_BIN` to its
`llama-server` executable; the app looks there first, then `runtime/bin/`, then `PATH`.

## 4. Add models

Two models come with the repository, split into parts under 2 GB and stored with Git LFS: Gemma 4 E4B
(`models/gemma-4-e4b-it-qat/`, 4.2 GB) and Gemma 4 E2B (`models/gemma-4-e2b-it/`, 3.1 GB). Install
[Git LFS](https://git-lfs.com) (`git lfs install`) before cloning, or Git fetches small placeholder files
instead of the models. Every download counts against the repository owner's monthly Git LFS bandwidth, so
fetch only the model you need:

```
git clone -c "lfs.fetchinclude=models/gemma-4-e4b-it-qat/*" <repository URL>
git lfs pull --include "models/gemma-4-e2b-it/*"
```

The first line clones with only the E4B; the second, run later inside the clone, fetches the E2B. In an
existing clone, `git config lfs.fetchinclude "models/gemma-4-e4b-it-qat/*"` before `git pull` does the
same. The app does not list a model whose folder holds only placeholders.

Put each model in its own folder under `models/`, e.g. `models/my-model/my-model-Q4_K_M.gguf`.
Split GGUF files need all their parts in the same folder; a vision model's projector (`mmproj-*.gguf`)
goes in the same folder as its weights.

The included downloader does this for you, verifies the file against the size and SHA-256 the source
publishes, and checks that the runtime can load it:

```powershell
python models\modeldownloader.py hf <owner>/<repository> <file>.gguf
python models\modeldownloader.py hf <owner>/<repository> <file>.gguf --mmproj <projector>.gguf
python models\modeldownloader.py ollama <model>:<tag>
```

On Linux and macOS use `python3 models/modeldownloader.py` with the same arguments.

Prefer the upstream GGUF from Hugging Face. Ollama's own registry files are packaged for Ollama (it
repairs some of them in memory at load), so some load in Ollama but not in stock llama.cpp; the
downloader then keeps the file with an `.incompatible` suffix and writes the reason next to it.

Files added while the app is open appear after **Scan** in the model library. Deleted model folders
disappear from the list on the next refresh.

## 5. Run

### Windows

```powershell
.\run.ps1
```

Or, from Command Prompt (for example when PowerShell's execution policy blocks `run.ps1`):

```bat
run.bat
```

Then open <http://localhost:5173>. The first run builds the runtime if `runtime/bin/llama-server.exe`
is missing (skipped when `COMPANION_LLAMA_SERVER_BIN` is set), installs UI dependencies, builds the UI
and starts the backend; later runs start in seconds. Keep the terminal open and press **Ctrl+C** once
to stop the app and the model server cleanly.

### Linux / macOS

```bash
./run.sh
```

The same steps as `run.ps1`: it builds the runtime with `scripts/build-runtime.sh` the first time, installs
the UI dependencies, builds the UI and starts the backend. Then open <http://localhost:5173>, and press
**Ctrl+C** once in the terminal to stop. If the shell reports `Permission denied`, run
`chmod +x run.sh scripts/build-runtime.sh` once, or start it with `bash run.sh`.

Use a separate clone for each system. The UI dependencies and the runtime are built for the system
that installed them, so one folder shared between Windows and WSL cannot run both `run.ps1` and
`run.sh` (the runtime build refuses to replace a Windows runtime).

### By hand (Linux / macOS)

```bash
scripts/build-runtime.sh                 # once
cd frontend && npm ci && npm run build && cd ..
cd backend && COMPANION_ADDR=127.0.0.1:5173 cargo run --release --bin companion-backend
```

For UI development with hot reload, start the backend on its default port (3877) and run
`npm run dev` in `frontend/` (Vite proxies `/api` to it; set `COMPANION_API_TARGET` to proxy to
another backend).

### First steps in the app

1. **Models**: select a model and **Load**. The app sizes the context and places the model on your
   hardware automatically (see below).
2. **Chat** for conversation and attachments; **Code** for questions about a linked project folder,
   plans and agent tasks.
3. **Settings > Performance** to choose how hardware is used.

## Performance on your hardware

Settings > Performance offers **Auto**, **Fastest**, **Balanced**, **Light** and **Manual**:

- **Auto** (default) chooses for each model at load, aiming for the highest output speed: every layer
  on the GPU if any cache precision (f16 or 8-bit) and GPU memory reserve achieves it, otherwise as
  much of the model on the GPU as fits. Mixture-of-experts models keep expert weights in RAM when the
  GPU is too small, which is much faster than splitting whole layers. The runtime's own memory fit
  decides, so it is exact for every architecture llama.cpp loads.
- **Fastest / Balanced / Light** apply a profile measured for that model on your PC: open the model's
  details on the Models page and choose **Calibrate** (it unloads the current model and takes a minute
  or two). Balanced keeps nearly the fastest generation with fewer cores busy; Light leaves the most
  room for other programs.
- **Manual** sets threads, GPU layers, batch size, cache and waiting behaviour yourself. Combinations
  that cannot work (for example an 8-bit cache with Flash Attention off) cannot be selected.

A model whose chat template has no tool support is detected at load: the model list shows whether
**Tools** are confirmed, and such models create plain-text documents (`.txt`, `.md`, `.csv`, `.html`,
`.json`) instead of Word, PowerPoint, Excel or PDF files.

### PCs without a GPU

Build the runtime as usual; it contains only the CPU backend. Auto mode loads models on the CPU with
all physical cores and a context cap suited to the free RAM. Prefer 4B–8B models at Q4_K_M, or a
mixture-of-experts model with few active parameters, and leave Reasoning off unless you need it.
If a GPU start fails on a PC that has one, the app retries once on the CPU.

## Working modes

- **Chat** for conversation and attachments.
- **Code / Ask** for read-only project questions (default).
- **Code / Plan** for inspection and a proposed implementation plan without edits.
- **Code / Agent** for changes and verification. **Ask** requests action approvals; explicitly
  selecting **Auto** permits registered actions, including commands and deletion, without per-action
  prompts. File boundaries and tool safety limits still apply; shell commands are not process-sandboxed
  by their working directory. Web search requires the task's Search switch and is blocked when its
  saved policy is Deny.
- **Ctrl+K** finds a session or action. The inspector can be resized by dragging its edge or with the
  arrow keys on its separator.

Code questions can use up to 24 read-only actions per reply (search, read any line range, list
directories); results carry evidence IDs the model can keep or release as context fills.

## Storage, privacy and configuration

- Conversations, settings and records live in `data/` (or an existing `backend/data/`). Set
  `COMPANION_DATA_DIR` to choose a directory explicitly. Back it up before moving an installation
  (`scripts/backup-data.py`).
- **Settings > Privacy & boundaries > Keep a record of model requests** (on by default) stores what was
  sent to the model and what it returned, capped at the latest 300 requests, for diagnosing wrong or
  broken answers. The records stay on this PC and are included when you export a conversation; they
  can contain file contents the assistant read.

| Variable | Purpose | Default |
| --- | --- | --- |
| `COMPANION_ADDR` | address the backend listens on | `127.0.0.1:3877` (`run.ps1` and `run.sh` use `:5173`) |
| `COMPANION_DATA_DIR` | data directory | `data/` |
| `COMPANION_MODELS_DIR` | models directory | `models/` |
| `COMPANION_FRONTEND_DIR` | compiled UI | `frontend/dist/` |
| `COMPANION_LLAMA_SERVER_BIN` | a specific `llama-server` | `runtime/bin/`, then `PATH` |

## Checks

```powershell
cd backend;  cargo test
cd ..\frontend;  npm test;  npm run build
```

```bash
cd backend && cargo test
cd ../frontend && npm test && npm run build
```

## Troubleshooting

- **`cargo`, `node` or `npm` not recognised**: finish installing and open a new terminal.
- **`link.exe`, MSVC or Windows SDK errors**: install Visual Studio's *Desktop development with C++*
  workload, then use a new terminal.
- **The runtime build skips CUDA or Vulkan**: the build prints why (no matching GPU, or the SDK was not
  found). Install the SDK, open a new terminal so its environment variables are visible, and run the
  build again.
- **`llama-server` not found**: run `scripts\build-runtime.ps1` (Windows) or `scripts/build-runtime.sh`
  (Linux, macOS), or set `COMPANION_LLAMA_SERVER_BIN`.
- **A model does not load**: the error names the cause (for example a file packaged for Ollama, or an
  architecture this llama.cpp version does not know). Check the model's folder for `INCOMPATIBLE.txt`
  when it came from the downloader.
- **No models listed**: models must be `.gguf` files (not archives) inside `models/`; press Scan.
- **Port 5173 or 3877 in use**: stop the previous instance with Ctrl+C in its terminal.
- **PowerShell blocks scripts**: follow your organisation's execution policy; do not disable security
  policies globally to run the launcher.

## License

This project is public domain under [The Unlicense](LICENSE). Anyone may clone, fork, copy, modify, publish,
compile, sell or distribute it, for any purpose, commercial or not, with no conditions and no attribution
required.

The third-party software it builds on keeps its own licenses. The llama.cpp runtime (MIT) is built into
`runtime/` by the build script and is not stored in this repository. The Rust and npm dependencies are
fetched at build time. Models you download come with their own terms.
