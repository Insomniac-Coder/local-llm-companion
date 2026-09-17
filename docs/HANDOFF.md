# Handoff — start here

Last updated 2026-09-16 night, mid-audit. Everything below the "Uncommitted work" heading is **not
committed**. The speed-rule change (night) compiled and passed 416 backend tests at ~22:55; later edits
are listed in the night decision log with their compile status. Read this file, then the three audit documents it points to, before touching
code.

**Night of 2026-09-16: unattended work in progress. Read `docs/2026-09-16-night-decision-log.md` right after this file** (owner instructions, every decision, the ≤5% trade-off ledger, blockers, full checklist).

## Owner rules (non-negotiable)

- No hard-coded model names, hardware names or machine paths anywhere in the repo (code, scripts,
  docs). Examples must be neutral. Toolchains are found via PATH, environment variables, `vswhere`,
  `pkg-config`.
- Commit and push only when the owner says so ("commit and push this").
- Never load two models at once on the owner's machine. Stop llama-server / llama-bench / Ollama
  models before starting another.
- Ask before spawning agents or workflows, with the worst-case agent count in the question. Confirm
  benchmark parameters before running batches.
- Explain plainly; measure before claiming; say what a change switches off.
- Keep research and findings in the repo so they survive session clears.
- Frontend is the first line of defence, backend the second: an invalid setting or combination is
  blocked in the UI (disabled with the reason, Save blocked) and the backend still validates.
- Hybrid (GPU+CPU split) tests only on models that overflow the GPU, or come within 15% of it, at the
  context under test; never force a model that fits to split. Models above 8B are not tested on CPU.
- **Speed rule (updated 2026-09-16 night; replaces "highest output speed").** Aim for a really good
  balance of generation and prompt reading. Start from the option with the fastest generation. Switch
  to the option that reads prompts fastest among those that give up **at most 5% of generation** and
  gain **at least 5× as much prompt reading (in %) as the generation they lose**. Example (30B MoE,
  4,096-token prompt): micro-batch 1,024 is chosen over 512 (−2% generation, +46% prompt reading);
  2,048 is not (−11% generation). Options that gain generation (the built-in draft head: +54%
  generation, −5.7% prompt reading) win at the first step anyway. Applied by calibration (measured) and
  by a safe default without calibration.

## Where the audit's knowledge lives

| Document | Contents |
| --- | --- |
| `docs/research/2026-09-16-runtime-harness-research.md` | All research-agent findings (Mica llama.cpp integration, DeepSeek Harness patterns, Ollama runtime), each with evidence; critic's list of wrong/unsupported claims |
| `docs/research/2026-09-16-direct-investigation.md` | First-hand reproductions, raw outputs, file facts, API endpoints, tool flags, installer URLs and checksums |
| `docs/validation/2026-09-16-audit-benchmarks.md` | Every measurement taken, with method |
| `docs/design/2026-09-16-harness-patterns-plan.md` (owner decisions recorded at its end: request records on by default and included in log exports; safety nets first; no-tool models create plain-text documents only; warm-up and disk cache deferred) | How to incorporate the DeepSeek Harness (12) and Mica (24) findings: state per item against current code, designs, order (phases A–D), tests, open owner decisions |

## The owner's audit brief (what was asked)

1. Gemma 2 2B tool-shaped output in chat: root cause and real fix. **Done.**
2. `models/modeldownloader.py` + Gemma 4 E2B loading failure: root cause, robust architecture-aware
   ingestion, folder per model. **Done (downloader); verify live load of the Unsloth E2B.**
3. Performance investigation vs Ollama, CPU and GPU matrices, offload sweeps, thread scaling.
   **Benchmarks mostly run** (see validation doc); app-pipeline measurements still to run.
4. Mica and DeepSeek Harness comparisons. **Research done** (research doc); patterns to adopt are
   listed there.
5. Embed llama.cpp vs separate process: recommendation. **Decision so far: keep the separate
   llama-server process** (crash isolation, CPU fallback retry, server features like prompt cache and
   slot save), but **build it from pinned source inside the project**. IPC overhead still to be
   measured by `app_bench` (see below) before finalising.
6. llama.cpp CPU, Vulkan and CUDA **built from source as part of the project**; then **delete
   `models/bin`**.
7. Settings > Performance: **Auto / Fastest / Balanced / Light / Manual**, profiles from per-model
   calibration shown in the frontend.
8. Context size larger than the model supports → notify the user.
9. Model list must drop models whose folders were deleted.
10. Load error must not fill the bottom-left machine panel.
11. Final report: executive summary + detailed findings + P0/P1/P2 plan (format in the brief).

## Key conclusions so far (measured)

- **Engine parity with Ollama.** Ollama 0.34.1 runs llama.cpp's server (b10864). Same weights, matched
  8 threads: identical speed. At defaults on CPU ours is +23% generation / +48% prompt (24 vs ~12
  threads). On GPU: parity. The engine is not why the app felt slow.
- **Why the app felt no faster than CPU-only Ollama:** model choice and placement. The 27B hybrid
  cliff: a 14B at 16K with 7 layers on CPU drops 51 → 13 tok/s; 8B with two layers on CPU loses 28%.
  **And the app's memory planner misjudged the owner's 27B** (hybrid-attention `qwen35`): it cut 32K
  to 16K and labelled it hybrid, while llama.cpp's own fit keeps all 65 layers on the GPU at short
  context (36.6 tok/s) and 63/65 at 16K. Fixed in code (runtime fit), unverified.
- **Threads on the 275HX:** all 24 physical cores fastest; pinning to P-cores hurts (4B gen 25.2 →
  15.4). 16 → 24 threads: prompt +18%, generation +1–5% → basis for Balanced/Light profiles.
- **Ollama registry files** load in Ollama but not upstream llama.cpp because Ollama repairs them in
  memory (compat layer). The script's download is byte-perfect (SHA-256 matches).
- **Gemma 2 tool output:** chat prompt always carried a `create_document` example: tool blocks 10/10
  → 0/10 without it, correct code 0/10 → 10/10 with a truthful prompt.

## Uncommitted work (written; compile/test status noted)

Compiled and tested (backend 333 tests, frontend 64 tests at that point):
- `api.rs`: truthful chat identity; `document_tool_note` only on document requests; `ChatToolOffer`
  (only offered tools run / stop / get corrected; withheld registered tools in code sessions still
  explained).
- `MessageView.tsx` + `App.tsx`: chat replies never parsed as agent transcripts (`sessionMode`).

**Written later — now compiled and tested (backend 358, frontend 77 on 2026-09-16 evening):**
- Backend
  - `agent_runner.rs`: `without_action_envelopes` (history replays prose only) + test; applied in
    `api.rs::build_turns_budgeted`.
  - `models.rs`: `GgufLayout` + `load_issues` (tower tensors, vocab/embedding mismatch, missing chat
    template) on `ModelMetadata.load_issues`; `ModelManager::reconcile` (drops models whose files are
    gone, keeps the loaded one) used by `list_models` and `scan_models`; tests.
  - `main.rs`: phantom "Demo 8B (stub)" registration removed; modules `calibration`,
    `cpu_topology`, `runtime_fit` registered.
  - `llamaserver.rs`: `explain_load_failure` (plain cause for loader errors) + test;
    `SidecarBinary::detect` order is override → `runtime/bin` → PATH; `runtime_dir`; server args
    `--threads-batch`, `--poll`, `--prio`.
  - `cpu_topology.rs` (new): Windows CPU sets / Linux sysfs topology (SMT siblings, efficiency class,
    performance-core mask) + tests.
  - `calibration.rs` (new): plan from topology, llama-bench runner (`-t a,b,c` one model load),
    profile selection (Balanced = fewest gen threads within 95% of fastest, prompt threads = fastest
    prompt; Light = within 75%, poll 0 if ≤3% cost, priority −1), regression check (>10% slower under
    comparable environment), environment (device fingerprint, runtime build, AC/battery, driver) +
    tests.
  - `runtime_fit.rs` (new): binary search with `llama-fit-params -c N -ctk/-ctv` for the largest
    context keeping all layers on the GPU + tests. Wired in `api.rs::fit_context_with_runtime`
    (automatic mode, GPU available).
  - `api.rs`: `GET /api/models/:id/calibration`, `POST /api/models/:id/calibrate` (SSE; unloads the
    current model first); `apply_calibrated_profile` at load; `context_limit_notice` + `notices` in
    load responses; `load_model_by_id` returns notices.
  - `storage.rs`: `runtime_calibrations` table, `save_calibration`, `calibrations_for` + test.
  - `settings.rs`: `RuntimeSettings.mode` (auto|fastest|balanced|light|manual) with migration from
    `runtime_auto` in `normalized()` + test; `HardwareSettings.threads_batch/poll/priority`;
    `cpu_threads` default = detected physical cores (was hard-coded 8).
  - `inference.rs`: `InferenceConfig.n_threads_batch/poll/priority`; default `n_threads` 0; manual-only
    values cleared in automatic policy.
- Frontend
  - `services/calibration.ts`, `components/CalibrationCard.tsx` (Models → Details: Calibrate +
    profile cards + measurements table + regression/out-of-date notes), `tests/calibration.test.mjs`.
  - `SettingsPanel.tsx`: Performance mode radio group (Auto/Fastest/Balanced/Light/Manual) with
    `PerformanceModeNote`; Manual fields add prompt threads, wait (spin/sleep), priority; context-size
    warning (`services/contextSupport.ts` + test).
  - `Rig.tsx` + `services/loadError.ts` (+ test): one-line load error in the machine panel.
  - `ModelLibraryItem.tsx`: `load_issues` warnings on the card; CalibrationCard in details.
  - `App.tsx`: shows load `notices` as warning toasts.
  - `RuntimePage.tsx`, `SetupWizard.tsx`: point to the build scripts, not `models/bin`.
  - `pages.css`: styles for the above (uses existing tokens `--line`, `--text-2`, `--tally-line`,
    `--tally-wash`, `--caution-*`).
- Scripts and repo
  - `runtime/llama.cpp.lock.json` (tag b10809, commit 5266f24da75d…); `scripts/build-runtime.ps1`
    (rewritten: VS via vswhere, bundled CMake/Ninja, all selected backends in one build, stage then
    swap into `runtime/bin`, BUILD_INFO.json; parses in PS 5.1; not yet run) and
    `scripts/build-runtime.sh` (Linux/macOS; `bash -n` ok and `--dry-run` verified on Git Bash).
  - `run.ps1`: builds the runtime once if `runtime/bin/llama-server.exe` is missing. `run.bat` does the same from
    Command Prompt (the runtime build still runs `scripts\build-runtime.ps1`, with `-ExecutionPolicy Bypass` for that
    process only). `run.sh` does the same on
    Linux and macOS (`runtime/bin/llama-server`, `scripts/build-runtime.sh`); commit both `.sh` files with the
    executable bit (`git add --chmod=+x run.sh scripts/build-runtime.sh`).
  - `models/modeldownloader.py` (tracked via `.gitignore` exception): `hf` / `ollama` sources, folder
    per model, resume, size + SHA-256 verification, header inspection, runtime probe, `.incompatible` +
    `INCOMPATIBLE.txt`. **Verified live**: downloaded `unsloth/gemma-4-E2B-it-GGUF`
    `gemma-4-E2B-it-Q4_K_M.gguf` into `models/gemma-4-e2b-it/`, checksum ok, 601 tensors, template
    present (probe deferred).
  - **Committed models (Git LFS):** `models/gemma-4-e4b-it-qat/` (4 parts, night decisions 51, 55) and
    `models/gemma-4-e2b-it/` (3 parts, decision 57), each split with the runtime's
    `llama-gguf-split --split --split-max-size 1500M`, tracked in `.gitattributes`, with a `.gitignore`
    exception per folder. Every clone that fetches both pays 6.8 GiB of the owner's 10 GiB monthly LFS
    bandwidth; the README shows `lfs.fetchinclude` for fetching one.
  - `scripts/bench/*.mjs`: runtime/bin default, no default model.
  - `.gitignore`: `/build/`, `/runtime/*` except the lock file, downloader exception.
- Docs: the three audit documents above; this handoff.

## Written 2026-09-16 evening while the GPU track ran — NOT YET COMPILED

Owner: "do all code changes that won't skew the results"; building is deferred until the GPU
benchmark ends (a cargo build loads every core). Expect compile errors to fix. Design and rationale:
`docs/design/2026-09-16-harness-patterns-plan.md`.

- **H4 typed failures** — `inference.rs` `SidecarFailure` (ContextExceeded / Unavailable / Timeout /
  Truncated / BadRequest / Server) + `InferenceError::Sidecar`; `llamaserver.rs` `classify_failure`,
  `classify_error_object`, `transport_failure`; mid-stream `{"error":…}` frames and streams without
  `finish_reason`/`[DONE]` are errors. Agent (`agent_runner.rs`): transient → up to 3 in-place retries
  (1/2/4 s, cancellable, transcript untouched); ContextExceeded → forced compaction + wider pruning
  reserve + refunded attempt (max 2); failures never written into the transcript; `ProgressGuard::
  refund_attempt`. Chat (`api.rs`): overflow before any tool round → `drop_oldest_history` + one retry.
- **H9 request records** — `storage.rs` `model_requests` table (300 rows / ~50 MB cap),
  `record_model_request`, `model_requests_for`; `SidecarClient::with_recorder` + `RecordedRequest`
  (image data stripped, reasoning text not kept); `api.rs` `request_recorder` wired into chat,
  /compact and agent runs (the code-session classifier was removed in decision 41); included in conversation export; Privacy setting
  `record_model_requests` (default on) with a Settings switch; export toast warns about file contents.
- **H11 replay fixtures** — `backend/tests/fixtures/streams/*.sse` + `.expect.json` (9 cases) and
  `llamaserver::replay_fixture_tests` (1-byte, 7-byte and whole-body chunks).
- **M11 images** — `vision.rs` applies EXIF orientation before resizing (test builds a sideways JPEG);
  requests send the latest images, not the first four.
- **H10 compaction** — `complete_note` trims a note cut off at its cap; `release_tool_results` runs
  before a model-written compaction note.
- **H5 command output** — `terminal::head_tail` (⅓ head, ⅔ tail) for captured output and for the
  agent's view of `execute_command`; stderr first when a command fails.
- **M7 RAM prompt cache** — `inference::prompt_cache_ram_mib` → `--cache-ram` from free RAM minus the
  model memory in RAM (CPU fallback recomputes); plan note.
- **H2 stream splitter** — new `stream_split.rs` (prose vs actions, same recognisers as the parser,
  tested at many chunk sizes); chat sends `token` (prose) and `action` events, keeps `raw_cb` for
  parsing and `full_cb` (visible, saved, kept on Stop) for display; agent live deltas prose-only.
- **H3 committed content** — rejected chat rounds are cut from the visible reply (`replace` event).
- **H1 capabilities + Tools tag** — `inference::TemplateCaps`/`Support` from `GET /props`
  (`chat_template_caps`) at load, stored per model file (`model_template_caps`), overlaid in the model
  list (`tool_support_source`: runtime/template); stricter template test `template_handles_tools`;
  no-tool models: no tools offered, plain-text documents (txt/md/csv/html/json) saved from the reply
  (`documents::save_plain_document`), structured documents declined with a plain notice; agent starts
  in structured mode; frontend `services/toolSupport.ts` (+ test) for the tag and document note.
- **H6 + M4 stable prompt order** — system prompt no longer carries the project listing or memory
  (`changing_context_block` goes on the latest user turn), reasoning preface after history, attachment
  excerpts and images on the message they were sent with (`build_turns_with_owners`,
  `attachment_owner`). `COMPANION_LEGACY_PROMPT_ORDER=1` restores the old layout **only for the C6
  before/after measurement; remove it afterwards.**
- **H7 token ratio** — `agent::chars_per_token` learned from reported prompt sizes
  (`observe_prompt_size`), reset per load; fixed 4 was low for code and non-Latin text.
- Settings copy: speculative decoding no longer claims identical output (measured: prose can differ).

**Owner decisions 2026-09-16 (evening):** (1) **Built-in draft head: yes** — use `--spec-type
draft-mtp` (plus ngram-simple, n-max 3 measured best) automatically when the GGUF declares
`<arch>.nextn_predict_layers > 0` and speculative decoding is on; its ~0.9 GB VRAM must be counted by
the fit (verify whether `llama-fit-params` accounts for it). (2) **Prefer the 8-bit cache when it keeps
every layer (including the output layer) on the GPU — "yes, if that's the best configuration"**:
confirmed on the 14B **and the 27B** (27B at 32K: 18.8 → 36.2 tok/s with q8_0 + every layer on the GPU,
which needed the 512 MiB margin). (3) **Rule (owner): the automatic plan chooses whatever gives the
highest output speed; when options clash, the measured fastest wins.** Applied as: every layer on the
GPU if any (cache type × fit margin) achieves it, trying f16@1024, q8_0@1024, f16@512, q8_0@512,
f16@256, q8_0@256 in that order (equal speed once everything fits, so the most headroom wins);
otherwise the combination with the most layers on the GPU. The chosen margin is passed to llama-server
(`--fit-target`) so its own load-time fit agrees. Mixture-of-experts placement: decide from the G4
results. No user setting for the margin.

**Findings that led to those decisions (GPU track):** the 27B's built-in draft head (+54% prose, 2.5×
rewrites, +0.9 GB VRAM) — adopt automatically when a GGUF has `nextn_predict_layers`? The fit safety
margin (1,024 MiB default costs a lot; 256–512 measured safe) and preferring q8_0 with every layer
including the output on the GPU (14B at 16K: 13.1 → 32.7 tok/s). `runtime_fit.rs` also needs fixing:
it treats `-ngl N` with N = block count + 1 and MoE `-ot` expert overrides as partial placement.

## Status at 2026-09-16 ~20:30

- GPU/hybrid track **complete** (results in the validation doc: draft head, 14B/27B placements, fit
  margins, micro-batch, 30B mixture-of-experts).
- Everything written this evening **compiles and passes**: backend 392 tests, frontend 79 tests, `tsc`
  clean. Fixes on the way: helpers dropped by the fit rewrite restored; splitter no longer emits a stray
  newline when a closing fence arrives without its newline; `client_surfaces_http_errors` now asserts
  the typed failure; settings layout test counts the new Privacy field group.
- **CUDA Toolkit 13.4 installed** (owner approved; silent install of nvcc, nvvm, cudart, cublas(+dev),
  thrust/CCCL, crt, nvjitlink, nvtx, nvml_dev only; this installer bundles no display driver).
  `CUDA_PATH` is set machine-wide; shells opened before the install do not see it.
- `scripts/build-runtime.ps1` fixed: native-tool stderr no longer aborts under Windows PowerShell 5.1
  (`Invoke-Native`), vswhere put on PATH for vcvars, CUDA 13 runtime DLLs found in `bind`, and
  nvJitLink copied. **Runtime build in progress** (cpu + vulkan + cuda, MSVC, llama.cpp default CUDA
  architectures). After it: A/B against `models/bin`, then the CPU track and the prompt-order C6 run
  on the project build.

## Status at 2026-09-16 ~22:30 (supersedes the ~20:30 build notes above)

- **MSVC runtime built and A/B'd** (validation doc, "Project-built runtime vs the official release"):
  GPU parity, CPU prompt 7% slower than the official clang build. Kept aside as
  `runtime/bin.msvc-reference` until the clang A/B is done.
- **Clang rebuild done, verified, not yet benchmarked.** `clang-cl` failed: ggml gives it MSVC's
  `/arch` flags, so the Alder Lake variant could not enable AVX-VNNI. `build-runtime.ps1` now follows
  the official release: GNU-style `clang` via `cmake/x64-windows-llvm.cmake` builds the CPU modules and
  tools (`build/llama.cpp/build-clang`), and MSVC builds only `ggml-cuda`/`ggml-vulkan` with
  `GGML_CPU=OFF` (`build-msvc`, `-DCMAKE_C/CXX_COMPILER=cl`, clang removed from PATH for that pass).
  Also fixed: `VULKAN_SDK`/`CUDA_PATH` are read from the machine environment when the shell predates
  the SDK install (a run silently built without CUDA before this), and the OpenMP runtime the CPU
  modules import (`libomp140.x86_64.dll`, VS clang's LLVM OpenMP) is found through `dumpbin` and
  staged. Checked: BUILD_INFO (compiler clang, GPU modules msvc), `--list-devices` (CUDA0, Vulkan0/1),
  imports of every module, and a smoke run that loaded `ggml-cpu-alderlake.dll`.
  **Next: `bench_ab.py`** (clang runtime vs `models/bin`); if CPU prompt reaches parity, tell the
  **move `models/bin` and `runtime/bin.msvc-reference` to the Recycle Bin** (owner, final word: they
  empty the bin themselves later). If parity fails, keep both and show the owner the numbers. Update
  ARCHITECTURE:345,
  PERFORMANCE:185, the workspace launch config and the README clang note with the measured number.
- **Owner decisions (2026-09-16 night):**
  - Stock MoE tests on the 30B (`bench_moe_stock.py`, session scratch): load mode mmap vs none (prompt
    speed + load time), micro-batch 512–4096, n-gram draft length 0/2/3/4/8. Approved.
  - Third-party fork review (`docs/research/2026-09-16-moe-offload-fork-review.md`). Owner chose to
    test **expert prefetch** and the **expert cache** (CPU tensor-parallel not chosen), in one separate
    fork build (`build/fork/src`, tip `fable5/cpu-tensor-parallel`) with **CPU + CUDA for this GPU's
    architecture only**. The 4B and 12B join **only the fork patch control** (prefetch off/on, async
    CPU splits off/on) to show the effect on models the patches don't target. Three read-only agents
    verified the review (owner asked for fewer than nine). Corrections are in the review doc's
    "Verification" section; the biggest: prefetch also switches on for Vulkan, the fork with features
    off is not stock, and llama-bench's random tokens make it useless for judging the cache.
  - **Owner approved the corrected fork plan** (`bench_fork.py`, session scratch; phases prefetch,
    smallbatch, perplexity, trace, cache, controls):
    - prefetch 2×2 (mmap/none × off/on) at prompt 2048 + generation 128 with the buffer type proven;
    - small-batch check at micro-batch 64/512;
    - a perplexity identity check;
    - the cache judged through llama-server on real prompts (the fork's stock placement, all experts on
      CPU, 3 slot counts sized from measured VRAM, async off/on), with engagement proven from
      `graph nodes`, token agreement, and a ~4,096-token prompt for the crash risk;
    - 4B/12B controls.

    The fork build has the one-line qwen3moe wiring (`build/fork/src/src/models/qwen3moe.cpp`, marked
    as a local test edit).

## Written 2026-09-16 night — NOT YET COMPILED (speed rule: micro-batch and load mode)

Written while benchmarks ran (no cargo/npm allowed). Expect compile errors to fix. Owner-approved
design; the speed rule is in the owner rules above.

- `speed_rule.rs` (new, registered in `main.rs`): `Speed`, `MAX_GENERATION_LOSS` 0.05,
  `MIN_GAIN_PER_LOSS` 5.0, `balanced_choice` + tests on the measured tables (micro-batch picks 1,024,
  2,048 rejected, load mode none beats mmap, draft head wins, 5.7%/+200% and 2%/+8% rejected, invalid
  inputs).
- `inference.rs`: `InferenceConfig.micro_batch` (`--ubatch-size`, None = 512) and `load_without_mmap`
  (`--load-mode none`), both serde-defaulted; `apply_to` clears both (Manual never gets them).
- `llamaserver.rs::server_args`: emits `--ubatch-size`, raises `--batch-size` to the micro-batch when
  the logical batch is smaller (2,048 when unset), emits `--load-mode none`; two tests.
- `runtime_fit.rs`: `probe` takes the micro-batch (`probe_args`, `micro_batch_args`: `-ub N`, `-b N`
  above 2,048); `default_micro_batch` (1,024 when both fits keep every layer on the GPU, or experts in
  RAM with at most one more block, or exactly the same whole layers; a dense layer costs ~8%, so the
  approval's "or layer" was not applied); `calibrated_micro_batch_fits` (a calibrated size is set aside
  when it would move whole layers off the GPU at this load's context/cache); `load_without_mmap`
  (weights in RAM while using the GPU, free RAM ≥ file + 4,096 MiB); `tensor_overrides`,
  `fitted_gpu_layers`, `parse_fit_output` now public; `FIT_COMBINATIONS` doc updated to the balance
  rule (choice unchanged). Tests for each.
- `api.rs`: `fit_context_with_runtime` probes at 512, then decides the micro-batch on the chosen
  combination (calibrated size or the default rule) and returns the fit; `start_sidecar` reads the
  calibration once (`latest_calibration`; `apply_calibrated_profile` is now sync and takes it), applies
  the calibrated micro-batch in every automatic mode (Auto included) when the calibration is
  comparable, sets `load_without_mmap` and takes the file size out of the RAM the prompt cache is sized
  from (replacing the half-the-weights estimate). The start-time cost of loading without mmap is still
  to be measured.
- `runtime_selection.rs::cpu_configuration`: CPU fallback resets both and drops their notes; test.
- `calibration.rs` + `calibrate_model`: `FittedPlacement` (`-ngl` plus the fit's `-ot` expert rules,
  which the thread sweep now passes too: before, a mixture-of-experts calibration benchmarked every
  expert on the GPU); on GPU placements measures micro-batch 512/1,024/2,048 (4,096-token prompt, own
  fit each, fastest-profile threads; up to 3 extra llama-bench runs + 2 fits) and stores
  `micro_batches` + `micro_batch` (serde defaults; old-record test). Calibration still fits at the fit
  tool's default cache and margin, not the load's combination (pre-existing).
- Frontend: `CalibrationCard.tsx` shows one line for the chosen micro-batch; types in
  `services/calibration.ts`.

## Next steps, in order

Done since the first version of this list (2026-09-16 evening):
- **Benchmark batch finished** (18:04). Every phase is in the validation doc, including flash
  attention/cache precision, GPU micro-batch, speculative decoding and load time.
- **Everything compiles and passes**: backend 358 tests, frontend 77 tests, `tsc` clean. Fixed on the
  way: a client that flips only the old `runtime_auto` switch had its change reverted by the mode
  (`AppSettings::reconciled_with`, applied in `put_settings` under the settings lock).
- **8-bit cache without Flash Attention** (owner rule: *frontend is the first line of defence, backend
  the second*): `frontend/src/services/cacheCompatibility.ts` (+ test) disables the q8_0 option while
  Manual has Flash Attention off, disables switching Flash Attention off while q8_0 is selected, and
  blocks Save with a message when the pair is reached by switching modes. Backend: `put_settings`
  rejects the pair; the policy stores values at f16 for an old save (test
  `an_old_manual_save_never_pairs_a_quantized_value_cache_with_flash_attention_off`). Manual
  threads_batch/poll/priority are validated too.
- Settings copy corrected to the measurements: q8_0 "measured no slower with Flash Attention on";
  speculative decoding no longer claims "several times faster".

Remaining:
1. **Live verification of the settings screen** — in progress. The workspace-level launch config
   `companion-verify` (in the parent folder's `.claude/launch.json`, not in this repo) builds the UI
   and runs the backend on 3877 against a **copy** of the history in `build/verify-data` (ignored),
   using `models/bin` until `runtime/bin` exists. Check: q8_0 option disabled, Flash Attention switch
   locked, Save blocked on the conflict, mode radio group, Manual fields. Never save test values into
   the owner's real settings.
2. **CUDA Toolkit 13.4.1**: installer downloaded and MD5-verified in the old session's scratch folder
   (may be gone; URL + MD5 in the direct-investigation doc). The owner approved installing it; the
   installer needs the owner to accept the administrator prompt.
3. **Build the runtime**: `scripts/build-runtime.ps1` (auto: cpu + vulkan + cuda). Expect to fix CMake
   option names on the first run. Verify `runtime/bin/BUILD_INFO.json`, `llama-server --version`,
   `--list-devices` (CUDA + Vulkan). The build uses MSVC (no clang-cl on the machine): **benchmark the
   MSVC CPU module against the official clang build** before trusting it.
4. **A/B**: our CUDA build vs the old `models/bin` on 8B GPU; Vulkan vs CUDA on one model; CPU module
   MSVC vs clang on 4B. Then switch the app to `runtime/bin` and **delete `models/bin`** (owner
   request).
5. **Live tests, owner's plan (2026-09-16 night), after the benchmark queue:**
   - **Library (2026-09-16 night):** the owner deleted the 25B Magistral folder. Eight models remain:
     2B, E2B, 4B, 8B, 12B, 14B, 27B, 30B. The app's history still knows the 25B, so the first live start
     is the real "model list reconcile after deleting a folder" check.
   - **Chat:** every model in the library, a varied set of questions (facts, reasoning, writing,
     multi-turn follow-ups, documents; same set for every model so results compare).
   - **Code chat:** 4B, 8B, 12B, 27B and 30B. The 4B was first the benchmark Gemma 3 4B
     (`models/gemma3-4b/`, no template, so the constrained JSON action format). **2026-09-17 07:45 the owner
     replaced it** with Google's Gemma 4 E4B (`models/gemma-4-e4b-it-qat/gemma-4-E4B-it-qat-UD-Q4_K_XL.gguf`,
     4.22 GB, unsloth's quant of Google's QAT checkpoint; native tool calling), which also gets the chat
     suite. The Gemma 3 4B folder is in the Recycle Bin (night decision 45). (1) A small test project created in the Code directory:
     ask questions about it and ask for a change. (2) Ask each model to build two web apps in the Code
     directory: a basic calculator, and a stopwatch/timer/clock. Screenshot each final product (kept
     under `docs/validation/live-tests/`), then move every test project to the Recycle Bin once
     analysed (owner empties it). Never touch other folders in the Code directory.
   - **A small model may do badly, but must never fail for reasons that aren't the model's ability:**
     before judging a failure, rule out context too small or truncated, reasoning eating the token
     budget, timeouts, tool-format/template mismatch, app parsing bugs, a wrong working folder, and VRAM
     spill. Fix app-side causes and rerun.
   Then the rest of the original live verification (one model at a time): 27B load with runtime fit at the owner's
   32K setting (expect far more context/all layers on GPU; also time the `llama-fit-params` probes);
   Unsloth E2B load + chat; calibration on a CPU-size and a GPU model; Performance modes apply at load;
   model list reconcile after deleting a folder; load error one-liner; context warning toast; Gemma 2
   regression check.
6. **App-pipeline benchmark** (`app_bench.py`, in the old scratch folder; recreate if missing): chat
   TTFT vs engine prompt time, visible vs engine tok/s, code-session agent start (no classifier since decision 41), multi-turn cache
   reuse with Reasoning toggled; debug vs release backend build (`run.ps1` and `run.sh` use a debug build).
7. **Mixture-of-experts placement (owner question, 2026-09-16).** The owner proposed swapping the
   experts a token needs into VRAM and cold ones out to RAM. Answer given: at one-user scale it loses —
   routing changes every token, a 30B-A3B touches ~1 GB of expert weights per token, and this laptop's
   GPU link is PCIe 5.0 x8 (~32 GB/s) while the CPU reads DDR5 at ~90 GB/s; the measured analogue is
   "GPU present, 0 layers" (prompts 7–9x faster, generation slower than pure CPU). The version that
   works is the reverse: attention, shared weights and KV on the GPU, expert tensors in RAM computed by
   the CPU (`--n-cpu-moe N` / `--cpu-moe`); llama.cpp stores a layer's experts as one merged tensor,
   so placement is per layer. **To do (needs the owner's OK for a ~17–18 GB download):** measure a MoE
   model with `--n-cpu-moe` vs `-ngl` partial offload; check whether `llama-fit-params` already
   prefers expert tensors on the CPU; if not, make the automatic plan and calibration do so.
8. **Optimisation test tracks (owner-approved 2026-09-16, GPU track first):** CPU work uses models up to
   8B only (owner: larger models on CPU are pointless); heavy models are for GPU/hybrid. GPU track: G1 27B
   built-in draft head (`--spec-type draft-mtp`; the file has `nextn_predict_layers = 1`, currently
   loaded and ignored) vs n-gram vs off; G2 14B at 16K whole layers vs FFN-only on CPU (`--n-cpu-ffn` /
   `-ot`) vs KV in RAM (`-nkvo`) vs q8_0 cache; G3 same on 27B at 32K; G4 Qwen3-30B-A3B expert placement
   (owner downloading `Qwen3-30B-A3B-Q4_K_M.gguf`); G5 fit margin 1024/512/256 MiB and Windows shared-
   memory spill; G6 micro-batch and `--no-op-offload` under partial offload. CPU track after: C1 8B
   threads/poll/priority; C3 Flash Attention and cache on CPU at depth; C4 mmap/no-mmap/mlock; C5 drafting
   methods on prose and rewrite (interleaved); C6 the app pipeline on CPU (prompt sizes, first-token wait,
   JSON-constrained agent steps, history reuse, Windows efficiency mode in the
   background). CPU ceiling for context: dual-channel DDR5-5600 (~90 GB/s) / ~4.7 GB read per 8B token
   = ~19 tok/s; measured 14.7.
   **Future task (owner deferred): C2 weight formats on CPU** — speed-only requantized copies of the 8B
   (Q4_0, IQ4_NL) in `build/`, repacking on/off; needs the owner's OK for ~9 GB of temporary files.
9. Follow-ups measured-first: CPU flash attention (CPU fallback forces it off, docs say auto); Stop
   latency on CPU with batch 2048; per-token `action_complete` re-parse cost; prompt-prefix
   stability; interleaved A-B-B-A rerun of speculative decoding on CPU (the 18% gap was drift) plus a
   rewrite task where drafting should pay most; a true cold-start load time.
10. Update `docs/ARCHITECTURE.md` and `docs/PERFORMANCE.md` (drift noted by the critic), then write the
   **final report** in the owner's format (executive summary; A–G; P0/P1/P2 with problem, evidence,
   root cause, solution, impact, risk, complexity, verification) and publish it as an artifact.
11. Commit only when asked.

## Corrections to remember

- An early message claimed Gemma 2 fused the fence label onto the JSON; that was an extraction
  artefact. The model wrote a proper fence.
- A GPU-matrix log was briefly misread (rows lined up to the wrong models); the per-row JSON is
  authoritative and consistent.
