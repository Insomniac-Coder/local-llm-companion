# Engineering audit report (DRAFT, being filled in overnight)

Draft of the final report requested in the owner's brief. Sources: `docs/validation/2026-09-16-audit-benchmarks.md`
(every measurement), `docs/research/*` (research and first-hand investigation), `docs/design/2026-09-16-harness-patterns-plan.md`,
`docs/2026-09-16-night-decision-log.md` (decisions, the ≤5% trade-off ledger, blockers). Sections marked **TODO** wait
for measurements still running. The finished report is published as an artifact.

## Executive summary

**TODO: written last.** It answers: what was actually wrong; why performance was worse than expected; what Mica did
better; what DeepSeek Harness taught us; whether to embed llama.cpp; what was fixed; how much performance improved;
what remains.

---

## A. Gemma 2B tool-output root cause

**Symptom.** In a plain chat, asking a 2B Gemma 2 model to "write a simple rust program" produced fenced ` ```tool `
blocks with invented tool names (`rust_program`), code stuffed into a tool argument and JSON that never closed. The
app then asked the model to "re-emit exactly one tool envelope", the model repeated the block, both rounds were saved
into the reply, and the UI rendered it as a failed agent step.

**Root cause (proven by a controlled experiment, 10 runs per arm, the app's own prompt text and sampling):**

| Prompt | Tool block | Rust code block |
| --- | --- | --- |
| As shipped: chat identity + a complete `create_document` tool example | 10/10 | 0/10 |
| Document example removed | 0/10 | 5/10 |
| Truthful chat identity, no tool text | 0/10 | 10/10 |

Four causes stacked:
1. The chat system prompt always carried a full fenced tool example, whatever the request.
2. The chat identity claimed project access and tool use that chat does not have.
3. A correction fired for any unreadable tool-shaped text, even with no tools offered.
4. The UI's transcript parser ran on chat replies.

The model's template has no tool support at all.

**Fix, verified through the app** (same request ×10 after the fix: tool blocks 0/10, code fences 10/10, no
corrections, no errors):
- The chat identity says only what chat can do. Tool text is added only to a request that asks for a file.
- Only offered tools can run, end a reply or earn a correction.
- History replays prose only.
- The UI never parses chat replies as agent transcripts.

Follow-ups built from the DeepSeek Harness research (section F):
- The runtime's template capabilities are read at load, and a "Tools" tag is shown in the model list.
- Models without tool support get plain-text documents only (txt/md/csv/html/json), per the owner's decision.
- A stream splitter keeps action text out of what the user sees.
- Rejected rounds are cut from the saved reply.

## B. Gemma 4 E2B loading failure

**The download was not faulty.** The downloader fetched Ollama's registry blob byte-for-byte (SHA-256 matches the
manifest), and Ollama itself answers correctly with that exact file.

**Root cause.** Ollama's llama.cpp runs a compatibility layer that repairs its own registry files in memory. Stock
llama.cpp does not.

| File | What differs from an upstream GGUF | Stock llama.cpp says |
| --- | --- | --- |
| `gemma4:e2b` | Vision and audio towers inside the model file (2,012 tensors, 601 of them the language model); no chat template; a 4.7 GB BF16 embedding | `wrong number of tensors; expected 2012, got 601` |
| `gemma3:4b` | Missing norm/rope keys; tokenizer one entry longer than the embedding; no chat template | `key not found in model: gemma3.attention.layer_norm_rms_epsilon`, then a shape error |

Surgery on scratch copies (removing the towers, adding the keys, truncating the tokenizer) made both load. That
proves the cause, but it is model-specific and fragile, so the product does not convert files.

**Fix:**
- `models/modeldownloader.py` downloads upstream GGUFs from Hugging Face (or Ollama's registry when asked) into a
  folder per model, with resume and size + SHA-256 verification.
- It inspects the header and probes loadability with the runtime (`llama-fit-params`, ~0.5 s, no weights loaded).
  An unloadable file is kept as `.incompatible` with a plain explanation, so the app never lists it.
- The app reads the same structural facts (towers in a model file, tokenizer/embedding mismatch, missing chat
  template), shows them on the model card, and translates loader errors into one plain line.
- The upstream `unsloth/gemma-4-E2B-it-GGUF` Q4_K_M was downloaded and verified. **TODO:** live load and chat
  (live tests).

## C. Performance investigation

All numbers are tokens per second (**higher is faster**), llama-bench or llama-server timings, median of repeated
runs with the first discarded, interleaved or alternating orders where a difference was small. Full tables:
`docs/validation/2026-09-16-audit-benchmarks.md`. Machine: 24-core laptop CPU (8 performance cores), 63 GB
DDR5-5600, 12 GB laptop GPU.

### C1. The engine is not the problem

- Ollama 0.34.1 runs llama.cpp's own server. With the same weights and threads the speed is identical (CPU, 8
  threads: 18.5 vs 18.4 generation; GPU: 148 vs 146).
- At each tool's defaults on the CPU ours is faster: +23% generation, +48% prompt reading. Ollama's Windows build
  uses about half the cores.

### C2. What actually decides speed: where the model sits

| Situation | Generation |
| --- | --- |
| 8B, every layer on the GPU | 88.7 |
| 8B, two layers on the CPU | 64.2 (−28%) |
| 8B, CPU only | 14.7 |
| 14B at 16K, the fit's default (7 layers on the CPU) | 13.1 |
| 14B at 16K, q8_0 cache with every layer on the GPU | 32.7 (2.5×) |
| 27B at 32K, the fit's default (57 of 65 layers on the GPU) | 18.8 |
| 27B at 32K, q8_0 cache, 512 MiB margin, every layer on the GPU | 36.2 (1.9×) |

- **The partial-offload cliff.** Every layer on the CPU costs far more than any cache or margin setting.
- **The app's old memory planner misjudged the owner's 27B.** Its header formula counted every layer of a
  hybrid-attention model as full attention, cut 32K to 16K and called the model hybrid. llama.cpp's own fit keeps
  every layer on the GPU at short context. Fixed: loads now ask the runtime's fit (`runtime_fit.rs`) and search
  cache precision × margin for the placement that keeps the most on the GPU.

### C3. Mixture-of-experts models

On a 30B-A3B model (experts partly in RAM), placement and loading choices mattered more than anything else:

| Change | Result |
| --- | --- |
| Experts in RAM instead of whole layers on the CPU | 1.36× at empty context, 3.4× at 16K |
| Load without mmap (experts in pinned memory) | prompt reading +48%, generation unchanged, start +0.1 s |
| Micro-batch 1,024 instead of 512 (balance rule) | prompt reading +46%, generation −2.1% |
| N-gram draft length 24 instead of 48 | rewrites +42%, prose +2% |

At 16K tokens in context the choice of cache precision matters more than how many experts fit on the GPU
(higher is faster):

| Placement at 16K | Expert blocks in RAM | Generation | Prompt reading |
| --- | --- | --- | --- |
| llama.cpp default (f16 cache, 1,024 MiB margin) | 29 | 49.3 | 1,211 |
| **f16 cache, 256 MiB margin (now chosen)** | 26 | **52.3** | 1,173 |
| q8_0 cache, 1,024 MiB margin | 26 | 44.5 | 1,254 |
| q8_0 cache, 256 MiB margin (the app's old choice) | 24 | 41.1 | 1,009 |

The 8-bit cache frees room for more experts but costs this model 10% of generation. The app's search had picked the
slowest arm; it now keeps whole layers first, then the f16 cache, then the fewest expert blocks in RAM (ledger T4:
prompt reading −3.1% for generation +6.1%).

### C4. Drafting

- N-gram drafting (the app's default) nearly doubles rewrite speed on models fully on the GPU (8B 87.5 → 167;
  12B 54.4 → 100.7) and costs nothing on prose.
- On models with weights in RAM, the default draft length wastes most of the gain; the app now uses 24 there.
- The 27B's built-in draft head, on the project runtime at 8K: prose 37.2 → 52.8 (+42%), rewrites 72.2 → 101.6
  (+41% over n-gram alone), prompt reading −7.1% on short prompts and −4.3% on 4,096-token prompts, +0.9 GB VRAM.
  Adopted as the owner's approved exception to the 5% limit.
- At the owner's 32K the head's VRAM pushes 6 layers to the CPU and everything gets slower (prose 37.2 → 32.2,
  rewrites 66.9 → 50.4, deep generation 33.9 → 22.0, prompt reading 780 → 455); with every layer on the GPU and
  the head, the server ran out of memory. The app now turns the head off for a load when its reserve would cost
  layers, keeps n-gram drafting, and says why.

### C5. The runtime build

- `runtime/bin` is built from pinned source by the project, the way the official Windows release is built: clang
  for the CPU modules and tools, MSVC for CUDA/Vulkan. It measures equal to the official release on GPU and CPU.
- An all-MSVC build read prompts 7% slower on the CPU.
- A PC without an NVIDIA GPU never builds or ships CUDA.

### C6. Things tested and rejected

- **Fork patches** (expert prefetch, expert cache, async CPU splits, pinned mmap), each measured inside the fork's
  own build:
  - prefetch cost 9.6% prompt reading on typical prompts at the app's micro-batch;
  - the expert cache lost 13% code generation and 43% prompt reading;
  - async CPU splits lost 10% generation;
  - the pinned-mmap patch does nothing on Windows.
- **Other placements:** feed-forward-only offload, KV cache in RAM (3.6 tok/s at 16K) and pinning threads to
  performance cores (slower) were all rejected.

### C7. Threads and CPU

- All 24 cores are fastest for both prompt reading and generation.
- The loader picks the Alder Lake CPU variant (AVX2 + AVX-VNNI), the best this processor supports (no AVX-512).
- From 16 to 24 threads, prompt reading gains 18% and generation 1–5%, which is the basis of the Balanced/Light
  calibration profiles.
- **TODO:** 8B CPU track results (threads/poll/priority, flash attention and cache, load mode, drafting).

### C8. The app pipeline

**TODO:**
- app-pipeline overhead (first-token wait vs engine prompt time, visible vs engine speed; the classification call no longer exists);
- prompt-order before/after (C6);
- live-test timings per model.

## D. Mica analysis

**How Mica integrates llama.cpp.**
- It builds llama.cpp from source as static libraries linked into one JNI library, with no server, `common` or
  tools, and ships one APK per ARM instruction set (no runtime CPU dispatch).
- It patches only two files: a Vulkan low-priority queue for phone UI smoothness, and a Hexagon NPU build rework.
  Neither applies to a desktop CUDA/Vulkan build.

**What Mica did better** (and what we took):
- **Stable prompt prefixes.** Mica moves request-time context to the newest message, so the server's prompt cache
  keeps the history. Ours put changing blocks (project listing, memory, the reasoning instruction) before the
  history, forcing re-reads. Adopted: stable order, with a before/after measurement queued (C6).
- **Measured token ratios** instead of a fixed 4 characters per token. Adopted (H7).
- **Image handling bugs it avoided:** EXIF rotation, and sending the latest images rather than the first four.
  Adopted (M11).
- **RAM prompt cache sizing.** llama-server silently allows 8 GiB of prompt cache while a hybrid placement already
  fills RAM. Adopted (M7), now also accounting for the pinned copy of weights (night decisions).

**What not to copy:**
- phone-specific patches and constants;
- static JNI linking;
- its own reimplementations of features llama-server already has (prefix reuse, slot save/restore, reasoning
  budget);
- warm-up and disk cache: deferred by the owner, since a PC app stays running.

## E. llama.cpp integration decision

**TODO (needs the app-pipeline overhead measurement):**
- The recommendation so far is to keep the separate llama-server process, for crash isolation, the CPU-fallback
  retry, and server features (prompt cache, slot save, template capabilities, drafting modes).
- Build it from pinned source inside the project. Done: `scripts/build-runtime.ps1` / `.sh`. The clang CPU modules
  match the official release, and a PC without an NVIDIA GPU never builds CUDA.

## F. DeepSeek Harness analysis

Patterns adopted into this app, each checked for relevance to a single-user desktop app:
- **Capability gating ("claim less"):** never advertise tools to a model whose template has no tool support.
  Read from the runtime's `/props`.
- **Separate what is shown and saved from what was attempted:** a stream splitter, rejected rounds cut from the
  saved reply, and raw attempts kept only in the request records.
- **Typed failures and bounded recovery:** context overflow, unavailable, timeout, truncated, bad request, server.
  Only transient failures are retried, in place and cancellably. Overflow triggers compaction and one retry.
  Failure text is never written into the transcript.
- **Head and tail of long command output,** with stderr first on failure.
- **A durable record of every model request** (capped table, on by default, included in exports; owner decision).
- **Offline replay fixtures** of recorded streams, so streaming fixes are tested without a model.
- **Compaction fixes:** a note cut off at its limit is trimmed, and old tool results are released before
  summarising.

Deferred:
- one schema per tool (large refactor);
- a permission audit event (small value for one user).

Dropped:
- event-sourced sessions, plugins and parallel tools (sized for a multi-session product).

## G. Prioritized improvement plan

Status per item: **Done** (implemented, compiled, tested), **Done, live check pending**, **Open**, **Deferred**.
Every performance claim cites the validation doc or the night decision log.

### P0 — Fix immediately

**P0-1 Memory planner misjudged hybrid-attention models (27B cut to 16K, called hybrid).** Done, live check pending.
- Problem: the owner's main 27B loaded at half its context with layers wrongly planned on the CPU.
- Evidence: llama.cpp's fit keeps all 65 layers on the GPU at short context and 63 at 16K; the old plan said
  "~26% on the CPU".
- Root cause: a header formula that counts every layer as full attention.
- Solution: size context and placement with the runtime's own fit (`runtime_fit.rs`), searching cache precision
  × margin.
- Impact: 27B at 32K 18.8 → 36.2 tok/s; 14B at 16K 13.1 → 32.7 tok/s.
- Risk: probe cost at load (seconds); fits a pinned runtime.
- Complexity: M.
- Verify: live load of the 27B at 32K; placement notes; generation speed.

**P0-2 Chat taught tools to models that cannot use them (2B tool blocks).** Done.
- Evidence: 10/10 tool blocks → 0/10 with a truthful prompt.
- Solution: capability gating, a stream splitter, rejected rounds cut from the saved reply.
- Verify: replay fixtures; the live chat suite on the 2B.

**P0-3 8-bit cache without flash attention could be saved and failed to load.** Done.
- Solution: the UI blocks the pair (frontend first) and the backend rejects it.

**P0-4 Registry model files that stock llama.cpp cannot load.** Done, live check pending.
- Solution: an ingestion probe with a plain explanation, and upstream GGUFs.

**P0-5 The runtime search order let any system llama.cpp replace the pinned build; no from-source runtime.** Done.
- Solution: override → `runtime/bin` → PATH, and a pinned from-source build (clang CPU, MSVC GPU) measured equal to
  the official release.

**P0-6 Mixture-of-experts drafting and loading defaults wasted most of the speed.** Done.
- Evidence: n-gram length 24 vs 48 gave rewrites +42%; pinned load gave prompt reading +48%.
- Solution: placement-aware draft length, and loading without mmap when RAM has room.
- Risk: pinned RAM use (logged).

**P0-7 (found tonight) The combination search made a MoE model 14% slower at 16K.** Done.
- Evidence: night decisions 23 and 25; validation doc "30B MoE at 16K".
- Root cause: the search preferred the fewest expert blocks in RAM, which it reached with the 8-bit cache; that
  cache costs this model 10% of generation.
- Solution: keep whole layers first, then the f16 cache among combinations keeping them, then the fewest blocks.
- Impact: generation 41.1 → 52.3 tok/s at 16K (+27% over the old choice, +6% over llama.cpp's default).

**P0-8 (found by the live tests) Small models failed code tasks for app-side reasons.** Done, live rerun running.
- Evidence: night decisions 36–40 (the 4B's code tasks).
- Root causes:
  - code chat refused reads;
  - the early stop cut a write before its file text;
  - the structured format never said where file text goes;
  - "not found" errors didn't name the file that does exist;
  - a loose structured schema.
- Solution: fixed each; a per-tool `anyOf` schema; "Found with that name: project/orders.py" hints.

**P0-9 (owner request) Code-session routing like Claude Code, and the review of it.** Done, live rerun running.
- Problem: a classifier guessed chat vs agent per message and misrouted (night decisions 40–41).
- Solution:
  - no classifier: every code-session message goes to the agent in a permission mode (ask, accept edits, plan,
    auto) that Shift+Tab cycles;
  - plan runs end with `present_plan`, and "Yes" switches the mode and carries the plan out.
- Review (3 reviewers + 1 verifier) confirmed 13 of 20 findings, all fixed (night decision 42). The serious ones:
  - Accept edits could write `.git/config` or hooks, which the app's own `git status`/`git diff` would then run;
    those edits now ask, and the app's git reads switch the hooks off.
  - Cycling through Auto released waiting actions in other sessions.
  - A session grant released a web search.
- Live check (8B, browser pane): a question in Plan mode first got an invented answer about a file the model
  never opened. Two prompt phrases ("answer it directly") caused it; after rewording, the model read the file and
  answered correctly. A plan request ended with `present_plan`; approval implemented it with no prompts, and the
  tests pass.

**P0-10 Models without a chat template ran away; loads had to re-probe the fit.** Done.
- Template: an architecture-based fallback to llama.cpp's built-in formats (`--no-jinja --chat-template NAME`);
  the 4B then answered normally at 130–134 tok/s.
- Fit: the last fitting setup is remembered per model and hardware. A background search prepares fits ahead of
  first use with 4 probes in parallel (measured: identical answers with 4 or 6 at once).

### P1 — High-value improvements

- **Balance speed rule in code:**
  - micro-batch 1,024 when it costs at most 5% of generation (MoE: prompt reading +46% for −2.1%);
  - calibration measures micro-batch sizes in two passes with a pause.
  - models fully on the GPU take 1,024 when the fit still keeps every layer (prompt reading +0.5–3.3%, no
    generation change).
  - Done.
- **Built-in draft head** (owner's exception): prose +42%, rewrites +41% at 8K; turned off for a load when its VRAM
  would cost layers (at 32K it made the 27B 13–35% slower). Done.
- **Stable prompt prefix (H6/M4):** implemented. Open: the before/after C6 measurement, then remove the legacy
  switch.
- **Typed failures, bounded retries, overflow recovery; request records; replay fixtures; head+tail command output;
  compaction fixes; EXIF and latest-image fixes; measured chars-per-token; RAM prompt-cache sizing.** Done.
- **Performance modes and calibration** (Auto/Fastest/Balanced/Light/Manual, regression detection). Done, live check
  pending.
- **App-pipeline overhead measurement** (feeds the embed decision). Open.

### P2 — Future improvements

- **Measure-first items:** M8 checkpoints on hybrid/sliding-window models, M10 reasoning cap, M12 render profile,
  M18 repeat penalty, M21 history trimming. Open (queued after the live tests).
- **Deferred by the owner:** C2 CPU weight formats (needs ~9 GB of temporary files), warm-up (M5), the disk cache
  (M6).
- **Deferred as low value for one user:** one schema per tool (H8), a permission audit event (H12).
- **Deferred by the owner:** projector/vision CPU-vs-GPU test (§14.9); not needed for the current demo (chat, code reading and creation). It also needs a vision model with its projector file.
- **Revisit later:** the fork's expert-prediction branch (unfinished upstream), and TurboQuant KV types (a quality
  trade-off).

## §14.15 Why isn't the app significantly faster than CPU-only Ollama on this PC?

**TODO:** the measured answer (model choice and placement, the planner's misjudgement of the 27B, the default fit
margin, drafting settings), with the bottleneck table.
