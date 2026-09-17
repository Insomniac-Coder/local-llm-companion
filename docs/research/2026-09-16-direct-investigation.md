# Direct investigation — 2026-09-16

Reproductions, experiments, file facts and tool knowledge gathered first-hand during the September
2026 audit. Companion to `2026-09-16-runtime-harness-research.md` (what the research agents read) and
`../validation/2026-09-16-audit-benchmarks.md` (what was measured). Written so none of this has to be
searched for again. Paths are relative to the repository root unless marked otherwise.

---

## 1. Gemma 2 2B: tool-shaped output in chat

### Reproduction

- Model: `gemma-2-2b-it-Q4_K_M.gguf` (upstream conversion; template present; `tool_calling=false`).
- Chat mode (no project), message `write a simple rust program` (also `write linked list in python`).
- Backend started on a spare port with an isolated data directory:
  `COMPANION_ADDR=127.0.0.1:<port> COMPANION_DATA_DIR=<empty dir> COMPANION_MODELS_DIR=<repo>/models`.
- Raw stream captured from `POST /api/chat` and decoded (SSE `token` events concatenated).

### Raw model output (verbatim, before the fix)

```
```tool
{"name": "rust_program", "args": {"code": "fn main() {\n  println!(\"Hello world!\");\n}}"}
```

I've started with a basic Rust program that prints "Hello world!". ...
```tool
{"name": "rust_program", "args": {"code": "fn main() {\n  println!(\"Hello world!\");\n}}"}
```
```

- Faults: an invented tool name (`rust_program`; other runs `run_rust_program`, `rust`), ordinary code
  placed in a tool argument, and an args object that never closes (`}}"}`).
- The chat loop then emitted a status "The tool call was not valid JSON; asking for a corrected
  one…", sent a correction asking to "re-emit exactly one tool envelope", and the model repeated the
  block. Both rounds were concatenated into the saved reply.
- The UI rendered the reply through `MessageView.parseAgentTranscript`, producing an agent timeline
  row "The model returned an unreadable action. New runs retry this automatically." in a chat.
- Correction to an earlier claim: a first quick extraction appeared to show the fence label fused
  onto the JSON (`` ```toolname": ``). That was an artefact of the extraction (it dropped stream lines
  beginning with `{`). The model wrote a well-formed fence.

### Controlled experiment (proof of cause)

Direct `POST /v1/chat/completions` to the loaded llama-server, the app's sampling
(temperature 0.7, top_p 0.9, top_k 40, repeat_penalty 1.1), system text folded into the user turn
exactly as the app does for this template, 10 runs per arm. The prompt text was extracted from
`backend/src/api.rs` itself (Rust string rules applied), not paraphrased.

| Arm | Tool block | ```` ```rust ```` code block |
| --- | --- | --- |
| A: chat prompt as shipped (identity + `create_document` example) | 10/10 | 0/10 |
| B: document paragraph removed | 0/10 | 5/10 (2/10 in an earlier batch) |
| C: truthful chat identity, no tool text | 0/10 | 10/10 |

- Cause 1: `build_system_prompt` (chat branch) always carried a complete fenced `create_document`
  example, whatever the request.
- Cause 2: the identity line claimed project access and tool use in chat ("you can see the user's
  linked project… the app can run approved tools"); in arm B the model offered to "compile and run it
  in your project" and asked questions instead of answering.
- Cause 3: the correction fired for any unreadable tool-shaped text, even an invented tool with no
  tools offered.
- Cause 4: the UI's legacy transcript parser ran on chat replies.
- Capability: the model's template has no tool support. On a genuine document request it attempted
  `create_document` 5/5 but filled the wrong spec (spreadsheet `sheets` for a `.md`). Not forced.

### Fix (verified through the app)

- Chat identity states only what chat can do; tool protocol is added to the one request that asks for
  a file (`document_tool_note`), keeping the system prefix stable for the prompt cache.
- `ChatToolOffer`: only an offered tool runs, ends the stream early or earns a correction; a
  registered-but-withheld tool in a code session is still explained ("needs approval").
- History replays prose only (`agent_runner::without_action_envelopes`), so rejected attempts are not
  fed back as examples.
- `MessageView` never parses chat-session replies as agent transcripts.
- After: `write a simple rust program` ×10 through the app → tool blocks 0/10, code fences 10/10,
  corrections 0, errors 0.

---

## 2. Model files from Ollama's registry

### The downloader script (original)

`models/modeldownloader.py` fetched `https://registry.ollama.ai/v2/library/<model>/manifests/<tag>`,
took the layer with media type `application/vnd.ollama.image.model`, and streamed
`https://registry.ollama.ai/v2/library/<model>/blobs/<digest>` to `<model>_<tag>.gguf` in the current
folder. No size or digest check, no resume, no timeout, hard-coded model and tag, and a dependency on
`requests`.

### Is the download faulty? No

- `gemma4:e2b` blob: 7,162,394,016 bytes, SHA-256
  `4e30e2665218745ef463f722c0bf86be0cab6ee676320f1cfadf91e989107448` — identical to the manifest.
- Ollama stores pulled blobs under the same digest name (`~/.ollama/models/blobs/sha256-<digest>`), so the
  script's file is byte-for-byte what Ollama uses.
- Proof: the script's own file imported into Ollama (`FROM ./gemma4_e2b.gguf`, `ollama create`) answers
  `The capital of France is Paris.` (with `think: false`; with thinking on, a 40-token budget was spent
  on hidden reasoning and the visible reply was empty).

### Why the same file fails in this app

Ollama 0.34.1 runs llama.cpp's server with a compatibility layer (`llama/compat/llama-ollama-compat.cpp`)
that repairs its registry files in memory. Stock llama.cpp (this app) does not.

| File | Architecture / tensors | What differs from an upstream GGUF | Stock llama.cpp b10809 says |
| --- | --- | --- | --- |
| `gemma4:e2b` (7.16 GB) | gemma4, 2,012 tensors: 601 language model + 656 `v.*` + 744 `a.*` + `mm.*`; 55 keys incl. `gemma4.vision.*`, `gemma4.audio.*` | Vision and audio towers in the model file; **no `tokenizer.chat_template`**; `per_layer_token_embd` stored BF16 (~4.7 GB of the file) | `done_getting_tensors: wrong number of tensors; expected 2012, got 601` |
| `gemma3:4b` (3.34 GB, digest `aeda25e6…`) | gemma3, 883 tensors: 442 `blk` + 432 `v.blk` + `mm.*`; 35 keys | Missing `gemma3.attention.layer_norm_rms_epsilon` and all `gemma3.rope.*`; tokenizer 262,145 entries vs embedding 262,144 rows; no chat template (separate 358-byte Go-template layer) | `key not found in model: gemma3.attention.layer_norm_rms_epsilon`, then (once supplied) `token_embd.weight has wrong shape; expected 2560, 262145, got 2560, 262144` |

### Experiments (scratch copies, never shipped)

- **E2B, towers removed** (drop `v.`/`a.`/`mm.` tensors and `<arch>.vision/audio.*` keys; tensor data
  copied byte for byte): 601 tensors, 6.18 GB, loads in 2 s. Without a chat template llama.cpp falls
  back to ChatML and the model echoes the prompt. With Gemma 4's own turn markers
  (`<bos><|turn>user\n…<turn|>\n<|turn>model\n`, token ids 105/106) it answers correctly; CPU:
  242 tok/s prompt, 33.7 tok/s generation.
- **Gemma 3 4B, towers removed + keys added + tokenizer truncated to the embedding rows** (keys:
  `layer_norm_rms_epsilon` 1e-6, `rope.freq_base` 1,000,000, `rope.scaling.type` linear,
  `rope.scaling.factor` 8 — the values Ollama's engine used as defaults): loads and answers
  `The capital of France is Paris. The Seine River flows directly through the heart of the city.`
  Used as the 4B benchmark model so Ollama and llama.cpp run identical tensor bytes.
- Conclusion: registry files need model-specific surgery to run upstream. The ingestion pipeline
  therefore detects and explains, and points to upstream GGUFs, rather than converting.

### Useful facts for ingestion

- **Runtime probe without loading weights:** `llama-fit-params -m <file> -lv 4` takes ~0.5 s; exit 0 when
  loadable, exit 1 with llama.cpp's own reason (`-lv 4` is needed to see the error lines).
- **Hugging Face file facts:** `GET https://huggingface.co/api/models/<owner>/<repo>/tree/main` lists
  files with `size` and `lfs.oid` (SHA-256).
- **Canonical chat template without downloading a model:**
  `GET https://huggingface.co/api/models/<owner>/<repo>?expand[]=gguf` → `gguf.chat_template`
  (Gemma 4 E2B: 18,569 characters).
- **Download:** `https://huggingface.co/<owner>/<repo>/resolve/main/<file>` (redirects to a CDN; do not
  forward `Authorization` across the host change). `HF_TOKEN` for gated repositories.
- Upstream alternatives measured by size: `unsloth/gemma-4-E2B-it-GGUF` `gemma-4-E2B-it-Q4_K_M.gguf`
  3,106,738,272 bytes (SHA-256 `740185b21d22ceb83a11c3aa62ad5842ef32c70f6096d756bbee85a1e4ec34b8`),
  `mmproj-F16.gguf` 985,654,080 bytes; `ggml-org/gemma-3-4b-it-GGUF` `gemma-3-4b-it-Q4_K_M.gguf` 2.49 GB.

### Downloader and app changes

- `models/modeldownloader.py` (now tracked): `hf <owner/repo> <file> [--mmproj <file>]` or
  `ollama <model[:tag]>`; its own folder under `models/`; `.part` + resume; size and SHA-256
  verified; header inspection; runtime probe; an unloadable file is kept as `.incompatible` with
  `INCOMPATIBLE.txt` explaining why, so the app does not list it. Standard library only.
- App: `ModelMetadata.load_issues` from the GGUF header (tower tensors in a model file, tokenizer vs
  embedding mismatch, missing chat template), shown on the model card; loader errors translated
  (`explain_load_failure`); the machine panel shows a one-line summary instead of the log excerpt.

---

## 3. Runtime and platform facts

- The bundled runtime (llama.cpp b10809, `5266f24da75dc449bd56cbed7addb9c8e4a6a73e`) was the official
  Windows release: clang, CUDA 13 (`cublas64_13.dll`), `GGML_BACKEND_DL`, all CPU variants. On the
  audit machine the loader picks `ggml-cpu-alderlake.dll` (AVX2 + AVX-VNNI).
- `llama-bench --help`: `-t` default equals physical cores (24 here); options `-C/--cpu-mask`,
  `--cpu-strict`, `--poll <0..100>` (default 50, worker threads spin-wait), `--prio <-1..3>`;
  no batch-thread option. `llama-server` has `-tb/--threads-batch` (prompt threads) separate from
  `-t` (generation threads).
- `llama-bench -o json` returns per-repeat `samples_ts`; `-d <depth>` measures at a pre-filled context.
- `llama-fit-params -m <file> -c <ctx>` prints the fitted `-ngl` (27B IQ2_S at 17,280 tokens → 63 of 65
  layers).
- Official Windows release jobs (`.github/workflows/release.yml`): CPU built with clang
  (`cmake/x64-windows-llvm.cmake`), `-DGGML_NATIVE=OFF -DGGML_BACKEND_DL=ON -DGGML_CPU_ALL_VARIANTS=ON
  -DGGML_OPENMP=ON -DGGML_OPENMP_FETCH=ON`; CUDA job builds only target `ggml-cuda` with
  `-DGGML_CPU=OFF -DGGML_CUDA=ON` (CUDA 12.4 and 13.x); Vulkan with `-DGGML_VULKAN=ON`.
- Windows CPU topology: `GetSystemCpuSetInformation` (kernel32) gives per logical processor
  `CoreIndex` (SMT siblings share it) and `EfficiencyClass` (higher is faster). On the audit laptop the
  performance cores were logical 0, 1, 10–13, 22, 23 → llama.cpp mask `0xC03C03`.
- Build toolchain on the audit machine: Visual Studio Build Tools 2022 (MSVC 14.44) with bundled CMake
  and Ninja under `Common7/IDE/CommonExtensions/Microsoft/CMake`; no clang-cl component; Vulkan SDK
  1.4.357; CUDA Toolkit absent (installed later for the from-source build). CUDA 13.4.1 local
  installer: `https://developer.download.nvidia.com/compute/cuda/13.4.1/local_installers/cuda_13.4.1_windows_x86_64.exe`,
  3,750,931,600 bytes, MD5 `76d3e0a1a99e38a8fcb5bf8aa54c8d20` (published in `docs/sidebar/md5sum.txt`).
- `nvidia-smi --query-gpu=timestamp,utilization.gpu,memory.used,temperature.gpu,power.draw --format=csv,noheader,nounits -lms 500`
  works for sampling; `typeperf "\Processor(_Total)\% Processor Time" "\Memory\Available MBytes" -si 1` for CPU/RAM.
  CPU package temperature is not readable without administrator rights (`MSAcpi_ThermalZoneTemperature`
  is denied; the ACPI "Thermal Zone Information" counter is not a CPU sensor).

---

## 4. Ollama on Windows

- Installer `https://ollama.com/download/OllamaSetup.exe` (1,570,506,608 bytes for 0.34.1), signed
  "Ollama Inc." (DigiCert). Silent install: `/VERYSILENT /NORESTART /SUPPRESSMSGBOXES`; it launches the
  tray app afterwards, so waiting for the installer process tree never returns.
- API on `127.0.0.1:11434`; `ollama ps` shows placement (`100% CPU` / `100% GPU`) and context.
- Its llama-server is private (`%LOCALAPPDATA%\Programs\Ollama\lib\ollama\llama-server.exe`, with
  `cuda_v12` and `cuda_v13` folders); it is not placed on PATH.
- For benchmarks: `/api/generate` with `raw: true`, `options.num_gpu: 0` for CPU, `num_thread`,
  `num_ctx`, `num_predict`, `temperature`, `seed`; `prompt_eval_count/prompt_eval_duration` and
  `eval_count/eval_duration` in the response. `keep_alive` keeps the model loaded between requests;
  `ollama stop <model>` unloads.

---

## 5. Things found in passing (fixed in the audit)

- The model list never dropped entries whose files were deleted by hand, and did nothing at all
  when the models folder was emptied; a placeholder "Demo 8B (stub)" with no file was registered at
  startup. Now reconciled on every list/scan (the loaded model is kept until unloaded).
- `SidecarBinary::detect` searched PATH before the project's own runtime, so any system-wide
  llama.cpp silently replaced the pinned build. Now: override → `runtime/bin` → PATH.
- The machine panel printed the whole load error including the runtime's log excerpt, where it
  stayed until the next successful load.
- `scripts/bench/*.mjs` defaulted to a hard-coded model file and `models/bin`.
- Manual-mode defaults hard-code `cpu_threads: 8` and `batch_size: 512`.
