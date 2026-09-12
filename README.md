# Local LLM PC Companion

Privacy-first local AI companion (llama.cpp + GGUF). See `docs/ARCHITECTURE.md`
and [the v5.1 product review](docs/PRODUCT_REVIEW_2026-09-11.md) for implemented improvements, verification and the remaining full-product roadmap.

## Layout (§71)

```
local-llm-companion/
  backend/   # Rust Axum API: chat, models, tools, agent, search, vision, docs, memory, daio, index, knowledge, plugins
  frontend/  # React+TS+Vite: chat/code modes, resources, settings, themes
  plugins/filesystem/plugin.json
  docs/ARCHITECTURE.md
  models/    # GGUF files in root/subfolders; metadata.json optional
```

## Install prerequisites (Windows / PowerShell)

Install these before running the project from source:

1. **Git** — install [Git for Windows](https://git-scm.com/downloads/win) to clone
   the repository and use the app's Git features.
2. **Node.js 24 LTS with npm** — use the [official Node.js installer](https://nodejs.org/en/download).
   Keep the npm option enabled; a separate npm installation is not needed.
   Node.js 24+ is used by this project's tests, which import TypeScript directly.
3. **Rust stable with Cargo** — install through [rustup](https://rust-lang.org/tools/install/)
   and choose the default Windows MSVC toolchain. Cargo is included with Rust.
   If Rust is already installed, run `rustup update stable`.
4. **Visual Studio C++ build tools and Windows SDK** — follow the
   [Rust Windows prerequisites](https://rust-lang.github.io/rustup/installation/windows-msvc.html).
   In the Visual Studio installer, select **Desktop development with C++**, including
   the MSVC x64/x86 tools and a Windows SDK. These supply the linker and compiler
   needed by the backend and its bundled SQLite dependency. VS Code alone is not
   a replacement for these build tools.
5. **llama.cpp's `llama-server` runtime** — required to generate model responses;
   it is not installed by Cargo or npm. See the runtime setup below.
6. **A compatible GGUF model** — download one separately or use the app's model
   downloader. Model weights are not included in this repository.

Open a **new PowerShell terminal** after installation so PATH changes take effect,
then check:

```powershell
git --version
node --version
npm --version
rustc --version
cargo --version
```

The current development setup was verified with Rust/Cargo 1.98.1 and Node.js
24.13.0; these are tested versions, not a declared minimum Rust version.
React, Vite, TypeScript, and Rust libraries are installed automatically by the
project's package managers; do not install them globally. A separate SQLite
server is not required. Python 3 is optional, only for the included
`scripts/backup-data.py` utility, not for building or running the core app.

Allow disk space for build artifacts and model downloads. RAM/VRAM requirements
depend on the model, quantization, and context size; model file size alone is not
the total runtime memory requirement. The first build needs internet access to
download dependencies. Local inference does not require a cloud API key.

## First-time setup

### 1. Clone the project

```powershell
git clone https://github.com/Insomniac-Coder/local-llm-companion.git
cd local-llm-companion
```

Run the remaining commands from this project directory.

### 2. Install the model runtime

Download a Windows build from the official [llama.cpp releases](https://github.com/ggml-org/llama.cpp/releases).
Choose a build appropriate for your machine (CPU, or a supported GPU backend).
Extract `llama-server.exe` **and its accompanying DLLs** into `models/bin/`.
Keep the release's runtime dependencies together; copying only the executable
can cause missing-DLL errors. For GPU builds, follow that release's driver and
runtime requirements. Prebuilt releases avoid having to compile llama.cpp yourself.

The expected layout is:

```text
models/
  .gitkeep
  bin/
    llama-server.exe
    ...DLLs from the matching release...
  my-model.gguf
```

Verify the runtime can start:

```powershell
.\models\bin\llama-server.exe --version
```

Alternatively, install `llama-server` on PATH or set its full path before starting
the app:

```powershell
$env:COMPANION_LLAMA_SERVER_BIN = 'C:\path\to\llama-server.exe'
```

The environment-variable override takes precedence over PATH and `models/bin/`.
The app launches the model server itself; you do not need to run a second server
manually. Its default inference port is `3888`.

### 3. Add a model manually (or download one in the app)

The `models/` directory is included in fresh clones via `.gitkeep`. Place a GGUF
directly inside it, such as `models/my-model.gguf`, or in a model-specific folder,
such as `models/my-model/model.gguf`. For supported models, `metadata.json` is
optional: the app reads GGUF metadata to discover the model. Split GGUF models
need all their shards in the same folder.

If you add files while the app is open, use **Scan** in the model library to
rediscover them, then select and load the model. Models and runtime binaries stay
local and are ignored by Git. Building the app does **not** download model weights.

### 4. Build and run

```powershell
.\run.ps1                           # builds UI and starts one local process
# Open http://localhost:5173
# Press Ctrl+C once to stop the server and llama sidecar cleanly.
```

The launcher runs `npm ci` if `frontend/node_modules` is missing, builds the
frontend, and uses Cargo to build/start the backend. The initial build can take
several minutes; wait for the server's startup output before opening the page.
Keep this terminal open while using the app. On later runs, use the same command.

After launch, open **Models**, scan if needed, and load your GGUF model. The app
can open without model weights/runtime installed, but it cannot generate real
model responses until both are available.

### Common setup issues

- **`cargo`, `node`, or `npm` is not recognized:** finish the relevant installation
  and reopen PowerShell so it sees the updated PATH.
- **`link.exe`, MSVC, or Windows SDK errors:** install the C++ workload and SDK
  above, then retry from a new terminal.
- **`llama-server` not found / missing DLLs:** check `models/bin/`, the override
  variable, and that the complete matching runtime package was extracted.
- **No models detected:** check the files are actual `.gguf` weights, not ZIP
  archives or download links, and scan again.
- **Port `5173` or `3888` already in use:** stop the previous Companion/runtime
  instance before launching another one. Use Ctrl+C in its terminal for shutdown.
- **PowerShell blocks script execution:** follow your machine or organization's
  script-execution policy. Do not globally disable security policies just to run
  the launcher.

For frontend development with hot reload, run Vite separately from `frontend/`;
the normal user runtime is the single backend process, which serves both the UI
and `/api/*` on port 5173. The backend still supports `COMPANION_ADDR` when
started directly, defaulting to port 3877.

## Working modes

- **Chat** for conversation and attachments.
- **Code / Ask** for read-only project questions (default).
- **Code / Plan** for inspection and a proposed implementation plan without edits.
- **Code / Agent** for changes and verification. **Ask** requests action approvals; explicitly selecting **Auto** permits registered actions, including commands and deletion, without per-action prompts. File boundaries and tool safety limits still apply; shell commands are not process-sandboxed by their working directory. Web search requires the task's Search switch and is blocked when its saved policy is Deny.
- Use **Ctrl+K** to find a session or action. The inspector can be resized by dragging its edge or focusing the separator and using arrow keys.
- Models, Resources, Runtime & diagnostics, Tools & plugins and Settings are grouped at the bottom left. There is no duplicate configuration menu in the header.

## Source exploration

Code questions can use up to 24 read-only actions per reply. The model is guided
to search for named symbols, inspect source and tests, and verify documentation
against implementation instead of treating documentation as a complete audit.

`read_file` accepts a workspace-relative `path` and optional 1-based, inclusive
`start_line`/`end_line`. Any line is addressable, including lines beyond 17,000.
Reads default to 200 lines, capped at 500 lines and approximately 12,000 content
characters per chunk. Results include total lines, numbered content, and exact
continuation coordinates; `start_column` handles exceptionally long lines.
Search can target a directory or one file and no longer skips source files
merely because they exceed 1 MB. Broad searches remain capped at 50 matches;
narrow the path or query when that limit is reported.

For read-only chat exploration, each result has an evidence ID. The model can
use `manage_context` with `keep` and `release` ID arrays to select evidence after
reading it. Under context pressure, unmarked chunks with fewer matches to the
recent user requests are released first (oldest first for ties). This relevance
score is a heuristic, not semantic certainty. Explicitly kept chunks and the
latest result are protected. Released chunks retain their path/range so they
can be read again; the original activity log and conversation are not deleted.
Selection is request-local, not permanent memory. Context and action limits
still apply; incomplete inspection must not be presented as a full audit.

## Storage and configuration

Launch location no longer changes the default storage root. Existing `backend/data/companion.db` is preserved when present; otherwise data lives in `data/`. If both exist, neither is merged or removed. Set `COMPANION_DATA_DIR` to an explicit absolute directory to choose one. `COMPANION_MODELS_DIR` and `COMPANION_FRONTEND_DIR` override models and compiled UI locations.

Back up your active data directory before migrations or moving an installation. Model header validation detects structural problems; it is not a checksum verification of all weights.

## Checks

```powershell
cd backend
cargo test
cd ../frontend
npm test       # Node.js 24+ (native TypeScript test imports)
npm run build
```

The complete v5.1 design is a product roadmap, not a checklist of already-shipped features. See the review for remaining durable tasks, sandboxing, vector retrieval, automation and multi-model orchestration work.
