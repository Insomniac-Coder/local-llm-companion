# Runtime, harness and ingestion research — 2026-09-16

Durable record of the read-only research done for the September 2026 engineering audit
(performance, correctness, architecture). Three independent readers studied Mica's llama.cpp
integration, DeepSeek Harness, and Ollama's runtime; a fourth agent checked their reports for
gaps and unsupported claims. Nothing here was measured by the readers: every performance claim
is a hypothesis until the benchmark record confirms it. Measurements live in
`docs/validation/2026-09-16-audit-benchmarks.md`; first-hand reproductions, file facts, API
endpoints and tool flags live in `2026-09-16-direct-investigation.md` next to this file.

Rendered from the agents' structured output without editing. Paths: `<research>/` is the
scratch area the readers cloned sources into (not kept); other paths are relative to the
folder that contains this repository. Line numbers in `backend/src/api.rs` citations may be
off by 25–100 lines: that file changed during the audit (the critic notes this).

## How to use this document

- Treat each finding as a lead with its evidence, not a decision.
- The critic section lists claims found to be wrong or unsupported: read it before acting on a finding.
- Where the audit measured something, the validation record supersedes the finding.

## Headline conclusions (from the research alone)

- **Ollama 0.34.1 runs every GGUF through an upstream llama-server subprocess** (llama.cpp b10864).
  Its Go engine was removed in May 2026. Speed differences against this app therefore come from
  launch settings, llama.cpp build and model file, not a different engine.
- **Ollama registry models load in Ollama but not in stock llama.cpp** because Ollama ships a
  compatibility layer that repairs its own packaging in memory at load (missing hyperparameter
  keys, tokenizer longer than the embedding, vision/audio towers packed into the model file).
- **Mica builds llama.cpp from source as static libraries** with two local patches, both
  Android-specific; neither applies to a CUDA desktop. Mica hand-wrote features that
  llama-server already provides (prefix reuse, slot save/restore, reasoning budget).
- **DeepSeek Harness separates committed output from attempts**, classifies errors, retries only
  transient ones, keeps prompt prefixes stable, and records what every model request saw.
  It has no tool-capability flag; its "claim less" rule is the transferable idea.

## Mica: llama.cpp integration and optimisations

Reader brief: how Mica integrates llama.cpp, every local modification, what applies to desktop.

### Summary

Mica does not launch llama.cpp programs. It builds llama.cpp b10189 from source into static libraries (llama, ggml with the CPU, Vulkan and Hexagon backends, and mtmd) and links them into one JNI library. It turns off common, the server, the tools and OpenMP. It picks the ARM instruction set at build time, which is why it ships two APKs. Only two llama.cpp files are changed, and a git diff of the fetched copy confirms it: a Vulkan low-priority device option for phone screen smoothness, and a rewrite of the Hexagon NPU build. Neither applies to a CUDA desktop.

Because Mica skipped `common` and the server, it wrote its own versions of things llama-server b10809 already has: prompt prefix reuse, a reasoning-token cap, saving the model's memory to disk, and handling for models that cannot rewind. So the lessons for us are about using those server features and keeping prompts stable, not about copying code.

The biggest problems found in our code:
1. Each request moves attachment text, images, web results, repo hints and the reasoning instruction to the newest message. The next request therefore differs early and has to re-read a lot. This is from reading the code; cache hits were not measured.
2. Nothing warms or saves the model's memory across model loads.
3. The memory-size estimate counts every layer, so it overestimates hybrid and sliding-window models.
4. Photos are never rotated to match their EXIF orientation.
5. Any image a request sends is always one of the first four in the conversation, not the latest.
6. The UI re-parses the whole markdown reply on every token.
7. The server's 8 GiB RAM prompt cache is on by default, and our memory plan does not count it.

Nothing was built, loaded or benchmarked. Paths are relative to the folder that contains this repository. LLAMA_B10189 means qwen-local-android/app/.cxx/Release/3o5p6r2m/arm64-v8a/_deps/llama.cpp-src. Server behaviour was checked in that b10189 source. For our b10809 build it was checked only through the fetched common.h and strings inside the shipped DLLs.

### 1. How Mica links llama.cpp (answer to Q1)

**What they do.** FetchContent of llama.cpp tag b10189, patched at checkout. Built as static libraries (BUILD_SHARED_LIBS OFF) and linked into one JNI library: llama, ggml (backends ggml-cpu, ggml-vulkan, ggml-hexagon) and mtmd. Switched off: LLAMA_BUILD_COMMON, server, tools, examples, tests, curl, MTMD_VIDEO and GGML_OPENMP (Android packaging does not ship libomp.so). GGML_NATIVE is off; GGML_CPU_ARM_ARCH is set per APK flavour (modern armv8.2-a+dotprod+fp16+i8mm, compat without i8mm). KleidiAI OFF, LTO OFF, CPU_REPACK ON, LLAMAFILE ON, BACKEND_DL OFF. Vulkan on by default, Hexagon opt-in (it was ON in the newest build dir). Effective compile flags from compile_commands.json are -O3 -DNDEBUG -g plus the -march value. Because there is no runtime CPU dispatch, the i8mm APK would crash on older CPUs, so the app checks /proc/cpuinfo and blocks such phones.

**What we do.** We start the prebuilt llama-server.exe (b10809, CUDA 13) as a child process. CPU variant DLLs (ggml-cpu-haswell/alderlake/zen4...) are chosen at runtime. PERFORMANCE.md already decided against forking.

**Recommendation.** Keep the sidecar. Mica's reasons for static linking (JNI, no practical child process on Android) do not apply. Its costs (two APKs, a crash guard, six Vulkan build blockers) are exactly what the runtime-loaded backends in our binaries avoid. Main takeaway: Mica built no `common` or server, so it hand-wrote features llama-server already offers. Use the server features instead of porting Mica's native code.

**Applies to desktop.** ignore: linking from source brings a build toolchain and no speed gain; our prebuilt server already exposes everything Mica rebuilt.

**Category.** architecture

**Confidence.** high

**Evidence.**

- qwen-local-android/app/src/main/jni/CMakeLists.txt:9-36
- qwen-local-android/app/src/main/jni/CMakeLists.txt:38-54
- qwen-local-android/app/src/main/jni/CMakeLists.txt:79-81
- qwen-local-android/app/src/main/jni/CMakeLists.txt:195-203
- qwen-local-android/app/build.gradle.kts:16,26,49-58,64-104
- qwen-local-android/app/.cxx/Release/3o5p6r2m/arm64-v8a/CMakeCache.txt:27,397,430,436,442,523,529,559,577,728,796,802,1051
- qwen-local-android/app/.cxx/Release/3o5p6r2m/arm64-v8a/compile_commands.json (ggml-cpu.c, quants.c, repack.cpp: -O3 -DNDEBUG -march=armv8.2-a+dotprod+fp16+i8mm)
- qwen-local-android/HANDOFF.md:150-170
- local-llm-companion/models/bin (ggml-cpu-*.dll variants, llama-server.exe)
- local-llm-companion/docs/PERFORMANCE.md:174-186

### 2. Local llama.cpp patch 1: Vulkan low global queue priority

**What they do.** patches/vulkan-low-priority.cmake inserts VK_KHR_global_priority = LOW into the queue-create pNext chain of ggml_vk_get_device. It only acts when the GGML_VK_LOW_PRIORITY env var is set, and wraps createDevice in try/catch so the device is created normally if the driver refuses. Stock ggml hardcodes queue priorities {1.0f,1.0f}. Why: the phone GPU that encodes images also draws the screen. Applied only to the image projector on Adreno, together with the upstream env var GGML_VK_MAX_NODES_PER_SUBMIT=8 (set in nativeLoadClipModel). Measured on a OnePlus 15: 24.6-26.1 s with the phone unusable, 33.8 s usable with priority, 42.0 s on CPU. The script checks it has not already run, uses single-line anchors, and fails the build if llama.cpp moves the anchors. The patch comment still says the env var is set in JNI_OnLoad, which is stale. git status/diff on the fetched source shows only ggml-vulkan.cpp and ggml-hexagon/CMakeLists.txt changed.

**What we do.** We use the CUDA backend. No Vulkan DLL ships in models/bin.

**Recommendation.** Do not copy. CUDA has no equivalent knob, and Windows GPU scheduling handles the compositor. Revisit only if a Vulkan iGPU path that drives the display is ever shipped, and measure stutter first. The general pattern is worth keeping for any future build script: a patch that is safe to re-run, anchored, and fails loudly.

**Applies to desktop.** ignore: solves Android compositor contention on Adreno Vulkan, not CUDA.

**Category.** gpu-backend

**Confidence.** high

**Evidence.**

- qwen-local-android/app/src/main/jni/patches/vulkan-low-priority.cmake:1-24,40-59,61-137
- LLAMA_B10189/ggml/src/ggml-vulkan/ggml-vulkan.cpp:6184-6186 (MAX_NODES_PER_SUBMIT env, upstream)
- LLAMA_B10189/ggml/src/ggml-vulkan/ggml-vulkan.cpp:6213 (priorities {1.0f,1.0f})
- LLAMA_B10189/ggml/src/ggml-vulkan/ggml-vulkan.cpp:6405-6442,6722-6742 (patched region)
- qwen-local-android/app/src/main/jni/vision_wrapper.cpp:191-206
- qwen-local-android/HANDOFF.md:829-902

### 3. Local llama.cpp patch 2 and build-only workarounds (Hexagon NPU, host toolchain, headers)

**What they do.** hexagon-prebuilt.cmake replaces ggml/src/ggml-hexagon/CMakeLists.txt so the ARM side builds against a vendored qaic stub (htp_iface_stub.c/.h), SDK headers and prebuilt DSP libraries (libggml-htp-v73..v81.so). Reason: the IDL compiler and hexagon-clang are Linux-only. hexagon-prebuilt-if-wanted.cmake applies it only when MICA_HEXAGON is set. At runtime: ADSP_LIBRARY_PATH is set before llama_backend_init, useLegacyPackaging keeps the DSP libs as real files, a canary matmul compares NPU output to the CPU's, free NPU memory is read from the device, and MTMD_BACKEND_DEVICE keeps clip off the NPU. Measured Llama-3.2-1B Q4_0: prefill 2469 vs 315 tok/s, generation 36 vs 66 tok/s. Hexagon and KleidiAI both refuse K-quants. Other build-only changes: host-msvc.cmake for vulkan-shaders-gen, fetched Vulkan-Headers/SPIRV-Headers plus a hand-written config package, CMAKE_MAKE_PROGRAM forwarding, glslc from the NDK, git core.longpaths.

**What we do.** No NPU path. daio.rs marks the NPU unavailable without evidence.

**Recommendation.** Do not copy any of it: Snapdragon-only hardware and Android/Windows cross-compile plumbing. One idea is transferable in principle: check a new backend against the CPU with a small canary before offering it, because a backend that loads but computes wrongly cannot be detected downstream (Mica's Adreno LM path produced fluent nonsense). Worth doing only if a non-CUDA GPU backend is ever enabled.

**Applies to desktop.** ignore: phone NPU plus build plumbing; the canary idea matters only for an untrusted backend.

**Category.** mobile-specific

**Confidence.** high

**Evidence.**

- qwen-local-android/app/src/main/jni/patches/hexagon-prebuilt.cmake:1-37,69-109
- qwen-local-android/app/src/main/jni/patches/hexagon-prebuilt-if-wanted.cmake:1-13
- qwen-local-android/app/src/main/jni/host-msvc.cmake:1-16
- qwen-local-android/app/src/main/jni/CMakeLists.txt:84-102,113-193
- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:416-443,467-578
- qwen-local-android/app/build.gradle.kts:145-154
- qwen-local-android/HANDOFF.md:803-821,1260-1262
- local-llm-companion/backend/src/daio.rs:128-141

### 4. Prompt prefix stability: request-time context moves to the newest message

**What they do.** Mica keeps the reusable prefix byte-identical on purpose. The prefill and the send build their turn lists through the same function. A document's text is replayed at its own message's position. The image note is recorded with the context so it is not delivered again. Search results go in the system message and force one deliberate full re-read. HANDOFF records that a router pass which overwrote the cached token list defeated reuse. The reuse itself is a longest-common-token-prefix match followed by llama_memory_seq_rm(ctx,0,common,-1), always keeping at least one token to decode.

**What we do.** We send cache_prompt true and --cache-reuse 256, and chat, routing and agent already share one prefix builder. But the request builder moves content every turn. (1) All attachment excerpts (up to 20,000 chars) are appended to whichever user turn is latest. (2) Up to 4 images are attached to the latest user turn. (3) Web results, repo-index hints and local-knowledge blocks go on the latest user turn, yet the stored user message is only req.message, so the next request differs at that turn. (4) The reasoning preface is a second system turn inserted right after the identity prompt, and only when Reasoning is on. (5) In code mode the system prompt contains a live directory tree, which changes whenever a file is created.

**Limitation for us.** --cache-reuse can shift-reuse matching chunks of 256+ tokens, which hides part of the cost on plain transformers but not on SWA or hybrid models. llama-server may cache images by content hash, so image re-encode cost is unverified.

**Recommendation.** Redesign request assembly so per-turn context stays attached to the turn it was first sent with. Store it with the message, or re-derive it deterministically, and never move it to the newest turn. Put the reasoning instruction after history, not between identity and history. Freeze the workspace tree per conversation, or put it in a message instead of the system prompt. Measure before building, since that is the owner's rule.

**Applies to desktop.** port: model-independent, and multiplied by prompt length; biggest on CPU and for long documents.

**Expected impact.** Not measured; rough estimate. Re-reading a ~5-7K-token excerpt each turn costs about 2 s on the GPU (3,150-3,750 prompt tok/s) and about a minute on CPU (~100-109 tok/s). A change to the tree or the reasoning toggle re-reads the whole history.

**How to measure.** Log EngineTimings.cached_tokens (cache_n) and prompt_tokens per request. Run five turns of: plain chat; chat with one attached document; one turn with web search; Reasoning flipped on turn 3; a code session where the agent creates a file. Compare cache_n with the previous request's prompt length.

**Category.** performance

**Confidence.** medium

**Evidence.**

- local-llm-companion/backend/src/api.rs:73-75
- local-llm-companion/backend/src/api.rs:1200-1206
- local-llm-companion/backend/src/api.rs:1337-1346,1352-1371
- local-llm-companion/backend/src/api.rs:1551-1581
- local-llm-companion/backend/src/api.rs:2169-2175
- local-llm-companion/backend/src/api.rs:2412-2435
- local-llm-companion/backend/src/api.rs:2450-2503
- local-llm-companion/backend/src/api.rs:3142-3175,3196-3209
- local-llm-companion/backend/src/llamaserver.rs:633-652
- local-llm-companion/docs/PERFORMANCE.md:141-148
- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:271-306
- qwen-local-android/HANDOFF.md:492-517,682-695,1010-1012

### 5. Eager prefill after model load or when the user starts typing

**What they do.** When a model finishes loading with a chat open, or on the first keystroke in another chat, Mica decodes the conversation in the background. It calls generate with maxTokens=0, built by the same turn builder, and shows a PREFILLING status. Opening a chat does not trigger it, because most opened chats are only read. A send in the same chat cancels the job's UI updates but lets the native decode finish, since the mutex queues the two and the send reuses every decoded cell. Leaving the chat stops the engine. Afterwards a disk snapshot is written.

**What we do.** Every model load is a fresh process with an empty cache (cache_rebuild 'fresh_process'). The first message after a load or restart pays for the whole history.

**Limitation for us.** Burns power for chats that are only read, which is why Mica ties it to typing. It must use the exact same prefix bytes (see the prefix-stability finding) or the work is wasted.

**Recommendation.** Redesign on top of llama-server. When a model load completes with a conversation open, and on first keystroke, send one background request built by assemble_request_context with the history and max_tokens 1, and throw the output away. In b10189 the server always generates at least one token even with n_predict 0. With --parallel 1 the real send queues behind it and reuses the prefix. Drop the connection on conversation switch. Skip it while an agent run is active. Decide whether it is worth doing from estimated history tokens and the loaded model's measured prompt tok/s, not a fixed constant.

**Applies to desktop.** port: same problem (cold cache after load); the implementation is a request, not native code.

**Expected impact.** Estimate: an 8K history is about 80 s on CPU and a 32K history about 10 s on GPU, moved from after Send to before it.

**How to measure.** Time to first token of the first message after a model load, with and without the warm request, at 8K and 32K history on GPU and on CPU.

**Category.** performance

**Confidence.** medium

**Evidence.**

- qwen-local-android/app/src/main/java/com/mica/app/ui/ChatViewModel.kt:2699-2705
- qwen-local-android/app/src/main/java/com/mica/app/ui/ChatViewModel.kt:2738-2793
- qwen-local-android/app/src/main/java/com/mica/app/ui/ChatViewModel.kt:1935-1940,3000-3010
- qwen-local-android/HANDOFF.md:27-37,674-681
- local-llm-companion/backend/src/inference.rs:600,672
- local-llm-companion/backend/src/llamaserver.rs:144-145
- LLAMA_B10189/tools/server/server-context.cpp:1868-1874

### 6. Saving model memory (KV/state) to disk per conversation

**What they do.** After each settled turn, Mica writes the context state with llama_state_seq_save_file to <chatId>.bin.part, renames it into place, and writes a JSON stamp: format version, chat id, model id, projector id, context size, last message id, sealed/image flag, noted images, bytes. When a chat opens, every stamp field is checked before loading, because the sequence file stores no model identity (magic, version, tokens, raw cells only). It keeps 1 GiB of free disk space and records measured bytes per cell for Settings. Measured: ~136 KB/cell on Gemma 3 4B; the hybrid Qwen3.5 state round-trip is ~116 MB and reloads in under a second. The feature was removed once for disk cost and later reinstated behind the per-chat Reuse switch.

**What we do.** None. llama-server b10809 contains --slot-save-path and the /slots save/restore actions (strings present in the DLLs), but we never pass or call them.

**Limitation for us.** Verified in b10189 source; for b10809 only the option names were seen. KV files can be hundreds of MB to GBs at 32K, so the owner has to decide the storage policy, as Mica's user did.

**Recommendation.** Port the stamp design onto llama-server's slot save and restore. Save when a turn completes and the slot is idle; restore when a conversation opens after a model load. The stamp must record: model file identity (path, size and modified time, or a hash), n_ctx, K/V cache types, flash-attention mode, a hash of the system prompt and template, the last message id, and the runtime build. Never save a slot holding image chunks: the server saves only get_text_tokens(), so the token list and the cells would disagree. Write to a temporary file and rename. Decide disk headroom from measured free space.

**Applies to desktop.** port: restart and model-switch cost is the same problem; the server API does the native part.

**Expected impact.** Estimate: turns a 32K re-read (~10 s GPU, minutes on CPU) into a file read. Real ratio not measured.

**How to measure.** For 8K and 32K histories: restore ms from the /slots response t_ms vs cold prompt ms from timings; file bytes per token; do it on GPU and on CPU.

**Category.** performance

**Confidence.** medium

**Evidence.**

- qwen-local-android/app/src/main/java/com/mica/app/llm/ContextCache.kt:10-43,46-76,120-179
- qwen-local-android/app/src/main/java/com/mica/app/ui/ChatViewModel.kt:2801-2869,2938-2991
- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:1092-1184
- qwen-local-android/HANDOFF.md:27-33,51,755-781
- LLAMA_B10189/src/llama-context.cpp:3108-3126,3131-3137
- LLAMA_B10189/tools/server/server-context.cpp:2529-2596
- local-llm-companion/models/bin/llama-common.dll and llama-server-impl.dll (strings --slot-save-path, /slots)

### 7. RAM prompt cache that llama-server turns on by default

**What they do.** Mica has no RAM tier besides the single resident context, and one per-chat disk file.

**What we do.** We never pass --cache-ram. In b10809, common.h defaults cache_ram_mib = 8192 and cache_idle_slots = true, so on each conversation switch the idle slot's state is copied into host RAM (a std::vector copy, up to the limit). fit_to_memory's hybrid RAM check and cpu_context_cap do not count this RAM or the per-slot context checkpoints.

**Limitation for us.** Default confirmed in b10189 source and in the b10809 common.h fetched through a summarising tool, not by running the binary.

**Recommendation.** Size --cache-ram explicitly from the memory plan: RAM left after the CPU-side weights, KV and compute. Show it in the plan notes like the other decisions, instead of silently allowing 8 GiB. The feature itself is valuable (it is the RAM tier Mica never had), so keep it on where RAM allows.

**Applies to desktop.** port: a default we inherit without accounting for it; matters most for hybrid placement and 16 GB laptops.

**How to measure.** llama-server private bytes after switching between 4-5 long conversations. Compare against the plan's RAM budget, and watch for paging during decode in hybrid placement.

**Category.** memory

**Confidence.** medium

**Evidence.**

- https://raw.githubusercontent.com/ggml-org/llama.cpp/5266f24da/common/common.h (cache_ram_mib = 8192, cache_idle_slots = true, n_ctx_checkpoints = 32, checkpoint_min_step = 8192)
- LLAMA_B10189/common/common.h:618-623
- LLAMA_B10189/tools/server/server-context.cpp:1340-1359
- LLAMA_B10189/tools/server/server-task.cpp:1670-1734
- local-llm-companion/backend/src/llamaserver.rs:138-202
- local-llm-companion/backend/src/inference.rs:437-464,470-509

### 8. Models that cannot rewind (hybrid attention/SSM, recurrent) and sliding-window models

**What they do.** Mica detects llama_model_is_recurrent/is_hybrid at load and refuses prefix reuse for those models. It also has an experimental 'hybrid append mode' that only ever continues forward on the resident state. Condense and web search force a full re-read, the describe-and-rewind image note is skipped, and it verified a ~116 MB state disk round-trip.

**What we do.** We rely on llama-server, which since b10189 takes context checkpoints for models that cannot roll back or that use SWA: up to 32 per slot, at user-message starts, at least 8192 tokens apart. When a prompt diverges it restores the nearest checkpoint.

**Limitation for us.** Checkpoint RAM for hybrid models (up to 32 × recurrent state) is not measured.

**Recommendation.** Do not copy append mode: the server's checkpoints are the general solution. Make sure our prompts only diverge at or after a user-message boundary (see the prefix-stability finding) so checkpoints actually hit. Measure whether the 8192-token minimum spacing means short chats on hybrid models re-read from the start each turn. If so, --checkpoint-min-step is the knob, and it should be decided from the measurement.

**Applies to desktop.** redesign: same problem, already solved server-side; our job is prompt shape and one knob.

**How to measure.** Load a GGUF with <arch>.full_attention_interval (hybrid) and one with attention.sliding_window. Run 6 short turns and log cache_n per turn plus the server's checkpoint log lines. Record host RAM used by checkpoints.

**Category.** inference

**Confidence.** medium

**Evidence.**

- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:706-716,976-1040
- qwen-local-android/HANDOFF.md:43-54,1077-1082
- LLAMA_B10189/tools/server/server-context.cpp:3390-3403,3468-3476
- LLAMA_B10189/common/common.h:620-623

### 9. KV size: measured bytes per token vs our header formula

**What they do.** Every real snapshot write gives Mica a measured bytes-per-cell, stored per model and used for Settings estimates ('measured bytes-per-cell beats any formula across architectures'). HANDOFF notes Qwen3.5's KV is ~128 MiB at 4k because only every 4th layer is attention.

**What we do.** models.rs computes f16 KV as block_count × kv_heads × (key+value length) × 2 and takes the largest per-layer head_count_kv. That counts every layer as full attention. fit_to_memory uses this to pick context and q8_0. For hybrid models (full_attention_interval) and SWA models (the server's swa_full default is false) it overestimates the cache. Separately, PERFORMANCE.md lines 87-92 describe context halving and a 5% margin, while the code uses 1024-token steps and total/48: the doc has drifted from the code.

**Limitation for us.** Log line format and default log level confirmed only in b10189 source.

**Recommendation.** After each successful load, parse the worker's 'KV buffer size', 'RS buffer size' and 'compute buffer size' lines as they stream, because drain_worker_log keeps only an 8 KB tail. Store measured bytes per token per (model file, cache type, n_ctx) and use them for later fits. Keep the header formula only for a model's first load, and teach it full_attention_interval and sliding_window/sliding_window_pattern. Update PERFORMANCE.md to match the code.

**Applies to desktop.** port: directly affects how much context we give a model and whether we fall back to q8_0.

**Expected impact.** Estimate: several times larger fitted context for hybrid and SWA models on the same GPU, if the overestimate is confirmed.

**How to measure.** Load one hybrid, one SWA and one dense model. Compare kv_bytes_per_token × n_ctx with the logged KV and RS MiB.

**Category.** memory

**Confidence.** medium

**Evidence.**

- qwen-local-android/app/src/main/java/com/mica/app/ui/ChatViewModel.kt:2857-2864
- qwen-local-android/HANDOFF.md:32-33,1081-1082
- local-llm-companion/backend/src/models.rs:297-311,567-585,601-609
- local-llm-companion/backend/src/inference.rs:329-381
- local-llm-companion/backend/src/llamaserver.rs:359-377
- local-llm-companion/docs/PERFORMANCE.md:81-92
- LLAMA_B10189/src/llama-kv-cache.cpp:305
- LLAMA_B10189/src/llama-memory-recurrent.cpp:115
- LLAMA_B10189/src/llama-context.cpp:673
- LLAMA_B10189/common/common.h:570

### 10. Hard cap on reasoning tokens

**What they do.** Each reasoning level sends a hint and also sets a hard budget. stream_generation counts tokens inside <think>. At the budget it flushes partial UTF-8, sends '\n</think>\n\n' to the UI and decodes the same tag into the context, so the model believes it has finished thinking and continues into the answer. The budget must be under one output round, and applies only to models whose vocabulary has <think> as a single token. HANDOFF says the hint alone is followed 'perhaps half the time'; the cap is what actually holds.

**What we do.** Hint only: reasoning_preface says 'Think briefly' and similar. The budget setting only changes max_tokens (8K/12K/16K caps).

**Limitation for us.** Field handling verified in b10189 source; in b10809 only the field-name strings were seen in llama-server-impl.dll.

**Recommendation.** Port the behaviour with no native code. llama-server accepts per-request reasoning_budget_tokens (alias thinking_budget_tokens) and reasoning_budget_message, applied when the template defines thinking end tags. Map low/medium/high to a budget derived from the request's output allowance, add a short closing message, and keep the hint. Test the effect before making it the default.

**Applies to desktop.** port: same failure (hidden tokens running away); the server does the enforcement.

**How to measure.** On a fixed reasoning prompt set: completion_tokens_details.reasoning_tokens (already parsed), time to first visible token, and answer correctness, with the budget off vs on.

**Category.** inference

**Confidence.** medium

**Evidence.**

- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:813-890
- qwen-local-android/HANDOFF.md:237-252
- local-llm-companion/backend/src/api.rs:1945-1958,2058-2066
- local-llm-companion/backend/src/llamaserver.rs:1052-1055
- LLAMA_B10189/tools/server/server-common.cpp:1126-1141
- local-llm-companion/models/bin/llama-server-impl.dll (strings reasoning_budget_tokens, thinking_budget_tokens, reasoning_budget_message)

### 11. Images: EXIF rotation, which images are sent, resolution per projector

**What they do.** Mica bakes EXIF rotation into the pixels before re-encoding: 'Re-encoding drops EXIF, so rotation must be baked into the pixels or portrait photos reach the model on their side'. Fixed-resolution towers (gemma3, 896) get their native size from clip.vision.image_size; dynamic towers are capped at 768. Measured: encode time tracks pixel area (233 s native, 10.4 s at 1024, 5.1 s at 768) and image tokens go from 264 at 768 to 480 at 1024. A quantised projector can pass pairing checks and still invent text. The latest picture stays resident; older ones become notes plus a re-read offer.

**What we do.** prepare_image decodes with the image crate 0.25.10 and resizes to a fixed MAX_SIDE_PX of 1568. It never reads or applies EXIF orientation, and the crate's load_from_memory does not apply it by itself. Each request attaches images from attachments_for (ORDER BY rowid).take(4) to the latest user turn: the first four in the conversation, not the most recent.

**Limitation for us.** Orientation behaviour confirmed from docs.rs and code reading, not with a real photo. Mica's '17 t/s image prefill vs 75 t/s text' is an unexplained phone result; do not assume it applies to CUDA.

**Recommendation.** (1) Bug fix: read the decoder's orientation and call apply_orientation before resizing. (2) Select the most recent images, and keep each image on the turn it was sent (see the prefix-stability finding). (3) Measure image tokens (prompt_n delta) at 768/1024/1568 for the projectors we ship, and derive the size cap from the projector's metadata rather than one constant.

**Applies to desktop.** port: EXIF and image selection are platform-independent bugs; resolution cost matters less on a GPU but still costs prompt tokens and context.

**How to measure.** A portrait phone JPEG with Orientation=6: ask 'which way is the text facing'. Then send two images in separate turns and ask about the second.

**Category.** correctness

**Confidence.** medium

**Evidence.**

- qwen-local-android/HANDOFF.md:418-431,1125-1161,1221-1232,289-300
- local-llm-companion/backend/src/vision.rs:11-12,29-70
- local-llm-companion/backend/Cargo.lock:1321-1322
- https://docs.rs/image/0.25.10/image/enum.DynamicImage.html (apply_orientation must be called explicitly)
- local-llm-companion/backend/src/api.rs:1352-1371
- local-llm-companion/backend/src/storage.rs:773-791

### 12. Streaming to the UI: markdown re-parsed on every token

**What they do.** Tokens go into a StringBuilder and reach UI state at most every 80 ms. Per-token appends were 'quadratic twice over' (string concatenation plus a full markdown re-parse). The live rate is computed over a trailing 1.5 s window on the same throttle. Native code buffers incomplete UTF-8 before crossing into Java.

**What we do.** The backend keeps SSE frames and UTF-8 intact, which is equivalent. In the frontend, every token event calls setMsgs with the whole accumulated text, and MessageView renders ReactMarkdown over the full text. Only the tok/s readout is throttled (250 ms).

**Recommendation.** Port the throttle: accumulate in a ref and push to state once per animation frame or on a short timer, flushing on done/stop. This matters more for us because ngram speculation delivers 1,000+ tok/s on file rewrites.

**Applies to desktop.** port: the same quadratic render cost exists in React.

**How to measure.** Browser performance profile during a ~1,400-token rewrite and a long prose reply: long tasks >50 ms, dropped frames, and total main-thread scripting time, before and after.

**Category.** performance

**Confidence.** medium

**Evidence.**

- qwen-local-android/app/src/main/java/com/mica/app/ui/ChatViewModel.kt:552-557,724-730,1968-2021
- qwen-local-android/HANDOFF.md:1091-1092
- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:153-204,828-836
- local-llm-companion/frontend/src/App.tsx:668-680
- local-llm-companion/frontend/src/components/MessageView.tsx:2,260
- local-llm-companion/backend/src/llamaserver.rs:1203-1207
- local-llm-companion/docs/PERFORMANCE.md:35

### 13. Token counting: model tokenizer and measured ratios vs fixed characters-per-token

**What they do.** Each message is counted once by the model's tokenizer and tagged with the model that counted it; switching model forces a recount. The gauge prefers the engine's own cell count. Measured on Gemma 3: English 5.25 chars/token, Chinese 1.82. A fixed ratio 'told the other it was half empty at 80% full'. Documents are budgeted with the file's own measured tokens per character.

**What we do.** history_char_budget uses bytes/3. AgentContextUsage::for_turns uses chars/4 + 8 per turn, and that estimate drives chat_output_budget and the inspection compaction checks.

**Limitation for us.** Mica's ratios are from Gemma 3's tokenizer; ours will differ per model, which is the point of calibrating.

**Recommendation.** Calibrate a ratio per conversation and model from the last response's usage.prompt_tokens divided by the characters sent (both already available). Use the constant only before the first reply. Keep bytes/3 as a conservative floor if wanted, but stop using chars/4 for room decisions on CJK text.

**Applies to desktop.** port: language-dependent error, independent of hardware.

**How to measure.** Log estimated_tokens vs prompt_tokens for English prose, source code and Chinese conversations.

**Category.** correctness

**Confidence.** medium

**Evidence.**

- qwen-local-android/HANDOFF.md:783-801,505-507
- local-llm-companion/backend/src/api.rs:1231-1241,1347-1349
- local-llm-companion/backend/src/agent.rs:112-124
- local-llm-companion/backend/src/api.rs:2541-2559

### 14. Batch and micro-batch size

**What they do.** n_batch = n_ubatch = 512 below 12 GB RAM, 1024 at 12 GB and above, clamped 256-2048. One value feeds the context, the eval chunking and mtmd's batch so they cannot disagree. Rationale: prefill reads every weight once per micro-batch, so fewer micro-batches is faster, paid for in compute-buffer RAM. No measurement of 512 vs 1024 is recorded.

**What we do.** Automatic mode passes no --batch-size (runtime default 2048 logical / 512 physical). We measured only the logical size; --ubatch-size has never been measured.

**Recommendation.** Run a measurement, do not change it yet: --ubatch-size 512/1024/2048 on the 8K prefill prompt, GPU and CPU. If a larger micro-batch wins, derive fit_to_memory's compute reserve (weights/12, min 768 MiB) from the logged compute buffer size instead of that guess.

**Applies to desktop.** port: measure first; the principle holds on CUDA, the size has to come from our hardware.

**How to measure.** Existing harness: prompt tok/s from timings, VRAM compute buffer MiB from the worker log, and whether the fitted context shrinks.

**Category.** performance

**Confidence.** medium

**Evidence.**

- qwen-local-android/app/src/main/jni/llama_shared.h:20-29
- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:601-606,682-685
- qwen-local-android/app/src/main/java/com/mica/app/llm/DeviceCapabilities.kt:58-70
- local-llm-companion/backend/src/llamaserver.rs:170-175
- local-llm-companion/backend/src/inference.rs:238-241,343-354
- local-llm-companion/docs/PERFORMANCE.md:28-31,43-52

### 15. Thread count

**What they do.** usable_thread_count keeps every core whose sysfs cpu_capacity is at least 0.5 of the best core (fallback: cpuinfo_max_freq at least 0.65). The same count is used for n_threads, n_threads_batch and mtmd. Reasoning: ggml waits for the slowest thread at the end of every node, so only genuinely slow cores hurt. Measured 8 threads 76.4 vs 6: 50.7 vs 4: 45.8 prefill tok/s, both orderings and power states. Poll 50 and priority 0 did not make the phone unresponsive; no threadpool change was needed.

**What we do.** Automatic mode omits --threads (runtime default: all physical cores). Measured on the Core Ultra 9 275HX: all 24 physical cores beat 8 P-cores by 40% on prompt processing.

**Recommendation.** Nothing to port. Our measurement agrees with Mica's principle. The sysfs capacity logic is Linux/Android-only. If a CPU with low-power E-cores is ever measured slow, derive the count from the OS's per-core efficiency class rather than a fixed number, and only after the measurement.

**Applies to desktop.** ignore: already equivalent and measured on our hardware.

**Category.** scheduling

**Confidence.** high

**Evidence.**

- qwen-local-android/app/src/main/jni/llama_shared.h:39-68
- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:33-95,686-696
- qwen-local-android/HANDOFF.md:920-924,1247-1255
- local-llm-companion/docs/PERFORMANCE.md:43-66
- local-llm-companion/backend/src/inference.rs:653-657
- local-llm-companion/backend/src/runtime_selection.rs:47

### 16. Stopping during prefill and image encoding

**What they do.** llama abort_callback on every llama_decode (return true to stop) and the clip graph's cb_eval, observed every 32 nodes (return false to stop), so Stop works mid-prefill and mid-encode. The opposite polarity of the two hooks is documented as the easy mistake.

**What we do.** Stop sets a flag and aborts the tokio task, which drops the HTTP stream (chat and agent). llama-server polls for a closed connection every 1 s and sets no decode abort hook, so the current micro-batch still finishes.

**Recommendation.** Nothing to port on GPU: one 512-token micro-batch is well under a second. On CPU, just measure stop latency during an 8K prefill. Do not shrink the micro-batch for this, since that costs prefill speed.

**Applies to desktop.** ignore: our task abort already closes the request; the remaining delay is small on GPU.

**How to measure.** CPU-only load, 8K prompt: ms from Stop to the server log showing the slot released.

**Category.** api

**Confidence.** medium

**Evidence.**

- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:29-31,97-104,691-695
- qwen-local-android/app/src/main/jni/vision_wrapper.cpp:280-285
- qwen-local-android/HANDOFF.md:438-451
- local-llm-companion/backend/src/generation.rs:75-88
- local-llm-companion/backend/src/api.rs:4558-4561
- local-llm-companion/backend/src/llamaserver.rs:1196-1200,1277-1279
- LLAMA_B10189/tools/server/server-context.cpp:40

### 17. Weight loading: memory-mapped, locked, or read into memory

**What they do.** On CPU Mica memory-maps the weights without locking them ('weights stay pageable, which matters on a phone'). On the NPU it reads without mmap, because the weights are repacked into DSP buffers and a mapping would only pay for a copy that is thrown away.

**What we do.** No flag, so the runtime default (mmap).

**Recommendation.** Measure before changing anything. In hybrid placement, the CPU-side layers are file-backed pages Windows can trim under memory pressure, which may cause decode stalls; --mlock or --no-mmap could help. In full-GPU placement the choice mainly affects load time. If adopted, use b10809's --load-mode spelling: the binary says --direct-io is deprecated in favour of '--load-mode dio'.

**Applies to desktop.** redesign: the right answer depends on placement and must be measured on our machine.

**How to measure.** Load time to /health ready, and decode tok/s while another process allocates most of free RAM, for default vs --no-mmap vs --mlock, on one full-GPU and one hybrid model.

**Category.** memory

**Confidence.** low

**Evidence.**

- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:666-672
- LLAMA_B10189/include/llama.h:205-213
- LLAMA_B10189/src/llama-model.cpp:2376-2393
- local-llm-companion/backend/src/llamaserver.rs:138-202
- local-llm-companion/models/bin/llama-common.dll (string 'DEPRECATED: --direct-io and --no-direct-io are deprecated. use --load-mode dio instead')

### 18. Sampling chain

**What they do.** top_k 40 → top_p → temperature → dist, or greedy when temperature ≤ 0. No repetition penalty. Low temperature (0.3) for summaries and notes, 0 for the search router.

**What we do.** temperature 0.7, top_p 0.9, top_k 40 and repeat_penalty 1.1 on every request. Decision requests use temperature 0 and a JSON schema.

**Recommendation.** Measure only. repeat_penalty 1.1 penalises recently seen tokens, which exact file rewrites and repeated tool-call JSON need to repeat. Compare 1.0 vs 1.1 on the existing edit and tooljson prompts (draft acceptance, correctness of the rewritten file) before any change.

**Applies to desktop.** redesign: possible interaction with our speculative decoding and agent workloads; unproven.

**How to measure.** PERFORMANCE.md harness with draft_n / draft_n_accepted and a diff of the output file against the expected rename.

**Category.** inference

**Confidence.** low

**Evidence.**

- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:333-353
- qwen-local-android/app/src/main/java/com/mica/app/ui/ChatViewModel.kt:3785-3787,3463-3465
- local-llm-companion/backend/src/inference.rs:150-153
- local-llm-companion/backend/src/llamaserver.rs:633-664
- local-llm-companion/docs/PERFORMANCE.md:35-39

### 19. Chat templates, choosing the device, model lifecycle

**What they do.** llama_chat_apply_template, which matches a built-in list rather than running Jinja, with a ChatML fallback. <think> support is detected by checking it tokenises to a single token. One model and one context behind a mutex. The projector loads second and is freed before the model. The CPU device is named explicitly because n_gpu_layers=0 still let the scheduler put graph nodes on Vulkan. A native crash on the NPU path is avoided by checking before load that the file's quant type and size fit the NPU.

**What we do.** llama-server runs the GGUF's Jinja template. We read the template's shape and retry once with folded system text if the template refuses. Thinking is switched with chat_template_kwargs. CPU mode passes --device none --no-op-offload --no-kv-offload --no-mmproj-offload. Devices come from --list-devices, with one CPU retry after a GPU startup failure.

**Recommendation.** Nothing to port: our template handling is more general and we already name the device explicitly. Keep Mica's rule that every prompt path goes through the model's own template (ours already does via the server).

**Applies to desktop.** ignore: already covered, and more completely.

**Category.** api

**Confidence.** high

**Evidence.**

- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:1313-1381,610-665
- qwen-local-android/HANDOFF.md:1096-1123,818-821
- local-llm-companion/backend/src/llamaserver.rs:506-608,192-200
- local-llm-companion/backend/src/runtime_selection.rs:42-80,143-173

### 20. Speculative decoding, flash attention, KV cache precision

**What they do.** No speculative decoding. Flash attention and K/V types left at llama.cpp defaults (flash attention AUTO, f16/f16); no measurement recorded.

**What we do.** ngram-simple speculation (measured 15x on rewrites, no loss on prose). Flash attention auto on GPU and off on CPU. f16 KV by default, q8_0 only to fit. All measured on our hardware.

**Recommendation.** Nothing to learn from Mica here; our settings are measured and theirs are defaults.

**Applies to desktop.** ignore: Mica has no evidence on these.

**Category.** not-applicable

**Confidence.** high

**Evidence.**

- LLAMA_B10189/src/llama-context.cpp:3467-3505
- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:682-698
- local-llm-companion/docs/PERFORMANCE.md:23-66
- local-llm-companion/backend/src/inference.rs:165-172

### 21. Trimming history when it outgrows the budget

**What they do.** Compaction (default 65%, per chat) summarises older turns before the window fills. HANDOFF notes that in a chat right at the limit the prefill and the next prompt 'diverge at the top, and the saving is lost'.

**What we do.** build_turns_budgeted drops the oldest turns one by one until history fits. Once history is over budget, each new message can remove another turn right after the system prompt, changing the prefix for the whole history, unless compaction has already run.

**Limitation for us.** --cache-reuse chunk shifting may recover much of this on plain transformers; not on SWA or hybrid models.

**Recommendation.** Measure how often requests are sent in this trimmed state. If it happens, trim in larger steps so the prefix stays the same for several turns, or make sure compaction always fires before trimming starts.

**Applies to desktop.** redesign: the prompt-cache cost is the same for us; frequency unknown.

**How to measure.** A long chat pushed past HISTORY_CHARS or the context budget: log turns dropped and cache_n per request.

**Category.** performance

**Confidence.** low

**Evidence.**

- local-llm-companion/backend/src/api.rs:3124-3158
- local-llm-companion/backend/src/api.rs:2313-2355
- qwen-local-android/HANDOFF.md:686-695,462-472

### 22. Performance instrumentation and benchmarking practice

**What they do.** llama.cpp and mtmd logs are forwarded under their own log tag. MicaTiming logs router/fetch/first-token milliseconds. mtmd print_timings is on. Encode times are stored per mode and shown to the user. Per-message metrics run from first token to last token. Benchmark rules: never trust the first test in a run; sweep in both orders and believe only a winner that also wins from first position; record the frequency cap beside every number.

**What we do.** Engine timings incl. cache_n and draft acceptance, and a visible-output clock. A harness outside the repo that runs cold then warm.

**Recommendation.** Port two things. (1) Persist cache_n and prompt_n per request in the metrics store: the prefix, warm-up and slot-restore work above cannot be verified or kept from regressing without it. (2) Adopt the both-orders rule in the harness, since PERFORMANCE.md already notes thermal noise on the laptop.

**Applies to desktop.** port: cheap, and needed to verify the other findings.

**Category.** harness

**Confidence.** high

**Evidence.**

- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:355-377
- qwen-local-android/app/src/main/jni/vision_wrapper.cpp:260-265
- qwen-local-android/HANDOFF.md:474-490,1014-1025,1163-1205
- local-llm-companion/backend/src/inference.rs:178-219
- local-llm-companion/docs/PERFORMANCE.md:3-7,32,57-58

### 23. Defects found in Mica during this audit (do not copy)

**What they do.** (a) nativeContinueStream sets n_past = g_cached_tokens.size(). nativeAppendStream never adds the appended turn's tokens to that list, and image turns add none, so after an appended turn a continuation starts below the real end of the cache. b10189 rejects positions that do not follow on from the cache, so such a continuation probably stops after one token (code reading only, not run on a device). (b) All prompt inputs come through GetStringUTFChars, which returns Java's modified UTF-8, so emoji reach the tokenizer as 6-byte surrogate pairs; only the output direction was fixed. (c) The image generation loop reports 'length' when the window is full instead of 'context'. (d) Stale text: HANDOFF says 'KV cache cleared per turn' and 'Nothing has been benchmarked'; the Vulkan patch comment says JNI_OnLoad.

**What we do.** Not affected: positions and UTF-8 are handled inside llama-server and our Rust HTTP client.

**Recommendation.** Nothing to port. The lesson for any future native work: keep one number for where the cache ends. Mica's split between g_n_past and g_cached_tokens is what produced (a).

**Applies to desktop.** ignore: JNI-specific or Mica-internal; recorded so nobody copies these paths.

**Category.** bug-fix

**Confidence.** medium

**Evidence.**

- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:1029-1038,845-846,1229-1239
- qwen-local-android/app/src/main/java/com/mica/app/ui/ChatViewModel.kt:2310-2325
- LLAMA_B10189/src/llama-batch.cpp:255-319
- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:744,927,1018,1333,1388
- qwen-local-android/app/src/main/jni/vision_wrapper.cpp:455
- qwen-local-android/HANDOFF.md:224,1269-1270
- qwen-local-android/app/src/main/jni/patches/vulkan-low-priority.cmake:17-18

### 24. What NOT to copy, and why

**What they do.** Mica-only designs: (1) static JNI linking and two CPU-flavour APKs with an i8mm crash guard; (2) the Vulkan low-priority patch and 8-node submits; (3) the Hexagon NPU backend with vendored DSP libraries; (4) hand-written prefix matching with llama_memory_seq_rm and explicit-position batches; (5) hybrid append-only mode; (6) the built-in template matcher with ChatML fallback; (7) sysfs cpu_capacity thread selection; (8) RAM-tier batch sizes (512/1024) and context sizes (2048-32768); (9) the one-image-per-context reset with describe-then-rewind notes; (10) running the image encode on a worker thread while text prefills; (11) the fixed 80 ms number itself.

**What we do.** llama-server provides the equivalents: prompt cache, cache reuse, checkpoints, slots, Jinja, reasoning budget, internal image handling. Threads, batch, KV precision and placement come from measured policy.

**Recommendation.** Do not port these. Reasons: (1)-(3) platform and hardware specific; (4)-(6) and (10) are implemented inside llama-server, and copying them would mean forking it; (7)-(8) are phone constants, and the owner's rule is no hardcoded hardware values; (9) was a phone trade-off with a 25-40 s encode; on our GPU, re-evaluate only after measuring image cost with the prefix fix in place; (11) use animation-frame batching rather than a magic number. Carry over the ideas instead: stable prefixes, stamped snapshots, measured sizes, hard reasoning cap, throttled rendering.

**Applies to desktop.** ignore: each item is tied to Android, the phone's hardware, or a layer llama-server already owns.

**Category.** not-applicable

**Confidence.** high

**Evidence.**

- qwen-local-android/app/src/main/jni/CMakeLists.txt:79-185
- qwen-local-android/app/build.gradle.kts:61-104
- qwen-local-android/app/src/main/jni/llama_wrapper.cpp:61-95,112-149,271-306,976-1040
- qwen-local-android/app/src/main/jni/vision_wrapper.cpp:43-162,414
- qwen-local-android/app/src/main/java/com/mica/app/llm/DeviceCapabilities.kt:50-70
- qwen-local-android/HANDOFF.md:545-579
- LLAMA_B10189/tools/server/server-context.cpp:1340-1359,3390-3476
- LLAMA_B10189/tools/server/server-common.cpp:1126-1141

### Open questions

- Is llama-server b10809 the same as b10189 on the server features used above: per-request reasoning_budget_tokens, /slots save/restore keeping only text tokens, --cache-ram 8192 and cache_idle_slots defaults, and checkpoint defaults 32 / 8192? Checked in b10189 source, in b10809's common.h fetched through a summarising web tool, and by option-name strings in the shipped DLLs, never by running the binary (not allowed during this audit).
- What is the real per-turn cache_n in our app for the prefix-breaking cases (document attached, web search turn, Reasoning toggled, agent creating a file)? Found by reading code; not measured, because the machine is benchmarking.
- Does llama-server reuse an image's encoding (by content hash) when the same image moves to a later user turn, or re-encode and re-read it every request?
- Does llama-server b10809 print 'KV buffer size', 'RS buffer size' and 'compute buffer size' at its default log level, so we can parse them at load?
- How much host RAM do context checkpoints take for hybrid models (up to 32 × recurrent state per slot), and does the 8192-token minimum spacing leave short hybrid chats re-reading from the start each turn?
- Does a larger --ubatch-size (1024/2048) speed up prefill on the RTX 5070 Ti and on the CPU fallback, and how much VRAM does its compute buffer take?
- Do EXIF-rotated phone photos actually reach the vision model sideways? Needs one test image with Orientation=6.
- Does repeat_penalty 1.1 lower draft acceptance or correctness on file rewrites and tool JSON?
- On a hybrid-placement model, does default mmap vs --mlock or --no-mmap change decode stability under memory pressure on Windows?
- Is Mica's continuation-after-append defect real on a device (the reply stops after one token)? Found by reading code only.
- Mica's unexplained 17 tok/s image-token prefill vs 75 tok/s text prefill: phone-specific, or also present on CUDA? Measure image-turn vs text-turn prompt tok/s before assuming either way.
- Does compaction always fire before build_turns_budgeted starts dropping the oldest turns, or do requests regularly run in the one-turn-trimmed state that breaks the prefix?

## DeepSeek Harness: patterns for our harness

Reader brief: study the actual source; findings in the form "they do X, we do Y, limitation Z, change A".

### Summary

I shallow-cloned DeepSeek Harness (commit 0d1f500) to <research>/deepseek-harness and read its source. I did not install, build or run anything, and no model was loaded.

The harness keeps three things apart. What the model produced is saved as typed pieces: text, reasoning and tool calls, and tool calls come back from the provider on a separate channel. Failed or retried attempts are saved as log-only records that never reach the model's history or the screen. Every request is rebuilt from an append-only session log, and a runtime check compares the two. Errors carry stable codes. Retries only happen for transient codes, wait with backoff and never add text to history. A context overflow triggers pruning and compaction, then one retry.

The harness has no tool-support flag. It assumes every model accepts native tools, so it has no direct answer for a model whose chat template lacks tool support. What carries over is its rule for images: when unsure, claim less (text only), and turn history into something the model can take.

Our app uses a text tool format. That is a deliberate choice: raw file-body blocks avoid JSON escaping. The weak points are around that format, not the format itself:
- Chat teaches tool calls to every model, whatever its template supports.
- Every token, tool blocks included, goes straight to the screen.
- The saved reply joins all rounds, including rejected ones, and is fed back as history.
- Model-call errors are written into the agent's working transcript, and an overflow makes the context bigger.
- Command output is cut from the head only.
- Changing parts of the prompt sit before the history, so the model server's prompt cache is lost.

The fixes below fit one user and one model server: a capability record, a stream splitter, separate visible and raw text, error classes, head+tail output, and one table recording each model request. None of them needs a plugin framework.

### 1. Capability gating: advertising tools to a model whose template has no tool support (the 2B malformed-block case)

**What they do.** DeepSeek Harness resolves exact per-model facts before a request is admitted (prepareCall, packages/core/agent-loop/src/agent.ts:501-551; LlmResolvedModelInfo, packages/llm/llm/src/types.ts:383-393). Its default is to claim less: an undeclared model is text-only, because 'over-claiming admits one the provider then rejects mid-turn, after the message is durable' (packages/llm/llm-pi-ai/src/config.ts:63-79; types.ts:325 'absent means unknown, an explicit omission is negative'). It then changes the request to fit the model: images become text placeholders for a text-only model (packages/llm/llm/src/index.ts:1051-1055; content.ts:327-360). Caveat: it has NO tool-support capability. Tool schemas go to every route (agent.ts:554-567, 611-615), and a search for supportsTools/toolChoice in llm types found nothing. So for a no-tool template it gives the pattern, not the answer.

**What we do.** Our project computes tool_calling with a substring test on the GGUF template (backend/src/models.rs:350-356). That flag is only read by the compatibility-warning endpoint (backend/src/api.rs:5622-5634). The chat system prompt always teaches the ```tool create_document envelope in chat mode (api.rs:1541-1545) and read tools in code mode (api.rs:1507-1537), whatever the model is. run_agent never checks the flag (api.rs:4248-4300).

**Limitation for us.** A small model that was never trained on tool use copies the envelope badly. The malformed block streams to the user and is saved, and the next turn feeds it back. The one capability signal we already have is ignored exactly where it matters.

**Recommendation.** Resolve one ModelCapabilities record when a model loads. Give tools three states: supported, unsupported, unknown. Take the first source that answers: (1) the user's metadata.json override; (2) llama-server GET /props chat_template_caps (supports_tool_calls, supports_system_role, etc.; confirm the pinned build exposes it); (3) the existing template heuristic, marked unknown rather than supported. In Chat, include tool instructions only when tools are supported. Otherwise leave all tool text out, and fulfil document requests on the host side: documents::requested_document_kind detects the request, then a separate request is constrained by a JSON-schema response_format, as classify_request already does (llamaserver.rs:857-875). llama.cpp enforces that grammar without relying on the template's tool support. For agent runs on an unsupported model, start in the structured envelope mode (agent_runner.rs:295, 1881-1888) instead of the fenced mode, or refuse with a plain message. Keep this as one global rule with no model names.

**Applies to desktop.** redesign — the harness has no tool capability, so borrow its 'claim less, then fit the request to the model' pattern rather than any code.

**Expected impact.** Stops malformed tool text from reaching chat on models without tool support, and document requests still work there.

**How to measure.** Without loading a model: unit-test that build_system_prompt output has no ```tool text when capabilities.tools is unsupported. When the machine is free: send the same document request to a small no-tool-template model before and after, and count saved assistant messages that contain envelope markers.

**Category.** correctness

**Confidence.** high

**Evidence.**

- local-llm-companion/backend/src/models.rs:350-356
- local-llm-companion/backend/src/api.rs:1541-1545
- local-llm-companion/backend/src/api.rs:1507-1537
- local-llm-companion/backend/src/api.rs:5622-5634
- local-llm-companion/backend/src/api.rs:4248-4300
- local-llm-companion/backend/src/llamaserver.rs:857-875
- deepseek-harness/packages/llm/llm-pi-ai/src/config.ts:63-79
- deepseek-harness/packages/llm/llm/src/types.ts:315-326
- deepseek-harness/packages/llm/llm/src/types.ts:383-393
- deepseek-harness/packages/llm/llm/src/index.ts:1051-1055
- deepseek-harness/packages/llm/llm/src/content.ts:327-360
- deepseek-harness/packages/core/agent-loop/src/agent.ts:554-567
- https://raw.githubusercontent.com/ggml-org/llama.cpp/master/tools/server/README.md
- https://raw.githubusercontent.com/ggml-org/llama.cpp/master/common/jinja/caps.h

### 2. Streaming: keeping tool-call text out of what the user sees

**What they do.** In DeepSeek Harness the provider returns tool calls on a separate wire field. The adapter turns them into typed tool-call-delta chunks, separate from text-delta and reasoning-delta (packages/llm/llm-deepseek/src/protocols/chat-completions/translate.ts:152-195). The web chat shows a live chunk only if it is non-empty text or reasoning, and never shows tool-call blocks (packages/client/ui-chat/src/client/conversation-nodes/turn-process.ts:47-72). The headless JSON output is built only from committed messages, 'never from a live attempt that may still be retried or discarded' (packages/bundle/headless/src/json-stream.ts:1-6).

**What we do.** Our chat on_token sends every content delta as an SSE 'token' event and appends it to the saved partial text (backend/src/api.rs:2576-2580). Early stop only looks for ```tool (api.rs:2593-2597), but the parser also accepts <tool_call>, bare JSON and the Gemma envelope (agent_runner.rs:739-762). The frontend strips only ```tool fences (frontend/src/components/MessageView.tsx:56-58, 192-194; AgentChatProgress.tsx:207). Plain chat-mode conversations write no activity journal (api.rs:2292, 2621), so the raw text is what renders. The agent streams raw deltas as thought_delta (agent_runner.rs:1914-1922).

**Limitation for us.** Envelopes, <<<CONTENT file bodies and malformed attempts appear in the chat bubble while it streams. The backend parser and the UI regex disagree about what counts as an action.

**Recommendation.** Add one server-side splitter between SidecarClient::stream and the SSE channel, shared by chat and agent. It is a small state machine: prose tokens go out at once. Text that could open an action is held back: a line-start ```, <tool_call>, <|tool_call>, or a leading '{' in the reply. The splitter holds it until the existing locate_action/fence_opener logic can say whether it is an action or an ordinary code block. Ordinary code is released. Actions become a separate 'action' event (tool name and path, never the body). Only prose is sent as 'token'. The frontend regexes then become a fallback for old messages.

**Applies to desktop.** redesign — the harness gets this separation from native tool calls; we need the same typed split in our own stream layer because llama-server gives us everything as content.

**Expected impact.** No action text in the visible stream for any envelope shape. A plain code block in an answer shows up a few tokens late, never dropped.

**How to measure.** Cargo tests that feed recorded SSE byte streams through the splitter (fenced, tagged, Gemma, bare JSON, plain ```json example code, and a truncated envelope) and assert the token/action events. No model is needed.

**Category.** harness

**Confidence.** high

**Evidence.**

- local-llm-companion/backend/src/api.rs:2576-2597
- local-llm-companion/backend/src/api.rs:2292
- local-llm-companion/backend/src/api.rs:2621
- local-llm-companion/backend/src/agent_runner.rs:739-762
- local-llm-companion/backend/src/agent_runner.rs:1914-1922
- local-llm-companion/frontend/src/components/MessageView.tsx:56-58
- local-llm-companion/frontend/src/components/MessageView.tsx:192-194
- local-llm-companion/frontend/src/components/AgentChatProgress.tsx:207
- deepseek-harness/packages/llm/llm-deepseek/src/protocols/chat-completions/translate.ts:152-195
- deepseek-harness/packages/client/ui-chat/src/client/conversation-nodes/turn-process.ts:47-72
- deepseek-harness/packages/bundle/headless/src/json-stream.ts:1-6

### 3. Saved reply and history include raw round text, rejected attempts and partial envelopes

**What they do.** DeepSeek Harness saves a failed, retried or stream-error attempt as a log-only assistant/attempt, and it never enters model history (packages/core/agent-loop/src/agent.ts:400-465; docs/architecture.md:117-119). A cancelled attempt keeps only non-empty text and reasoning. Tool calls are dropped because they never ran, and the message is marked interrupted (packages/llm/llm/src/assembler.ts:162-179; agent.ts:402-420). Unstarted tool calls get synthetic 'aborted' results so history stays valid (agent-loop/src/tool-calls.ts:389-400). The UI hides attempts (turn-process.ts:64). A runtime check fails if a request differs from what the log derives, so 'model-visible means logged' is enforced (agent-loop/src/invariant.ts:19-57).

**What we do.** Our saved assistant message is every round's raw text joined together (backend/src/api.rs:2736-2757). That includes tool envelopes and a round that was rejected and corrected: the correction path at api.rs:2678-2691 changes only the request-local turns, not the saved text. History replays assistant content word for word (api.rs:3176-3187). Stop saves the raw partial text, including a half-written envelope (backend/src/generation.rs:80-110). visible_progress removes only complete, located actions (agent_runner.rs:907-917).

**Limitation for us.** Broken envelopes come back as the model's own earlier output. That works like a few-shot example and makes a small model more likely to repeat the mistake. Copy copies envelopes too. A cancelled half-action pollutes the next request.

**Recommendation.** Save two things per assistant message. The message content becomes the visible answer: prose from the committed rounds with action spans removed, including an unterminated envelope at the end. The raw text of each round goes into message_activities as a journal event, which already exists (storage.rs:569-579). Build history from the visible content only. When the loaded model's tool support is unsupported or unknown, turn legacy envelopes in old history into one-line notes such as '[read_file src/main.rs]', the way the harness turns images into text for a text-only model. On Stop, save only the filtered prose and add an 'interrupted' activity.

**Applies to desktop.** port — the separation between committed content and attempt records maps directly onto our messages and message_activities tables.

**Expected impact.** Corrections and failed rounds no longer show up in the saved reply or the next request's history.

**How to measure.** Without a model: a count over companion.db (on a copy) of assistant messages containing ```tool, <tool_call> or <<<CONTENT, before and after. Plus a cargo test that runs a correction round and asserts the saved content has no envelope.

**Category.** correctness

**Confidence.** high

**Evidence.**

- local-llm-companion/backend/src/api.rs:2678-2691
- local-llm-companion/backend/src/api.rs:2736-2757
- local-llm-companion/backend/src/api.rs:3176-3187
- local-llm-companion/backend/src/generation.rs:80-110
- local-llm-companion/backend/src/agent_runner.rs:907-917
- local-llm-companion/backend/src/storage.rs:569-579
- deepseek-harness/packages/core/agent-loop/src/agent.ts:400-465
- deepseek-harness/packages/llm/llm/src/assembler.ts:162-179
- deepseek-harness/packages/core/agent-loop/src/tool-calls.ts:389-400
- deepseek-harness/packages/client/ui-chat/src/client/conversation-nodes/turn-process.ts:62-72
- deepseek-harness/packages/core/agent-loop/src/invariant.ts:19-57
- deepseek-harness/docs/architecture.md:117-123

### 4. Retries, error classes and context-overflow recovery

**What they do.** DeepSeek Harness gives every failure a stable code: CONTEXT_WINDOW_EXCEEDED, QUOTA, EMPTY_RESPONSE, AUTH, RATE_LIMIT, SERVER, TRANSPORT, TIMEOUT (packages/llm/llm/src/error.ts:24-48, 80-86; llm-deepseek chat-completions/adapter.ts:97-109). A completed response with no content counts as EMPTY_RESPONSE (translate.ts:133-141). Failures go through an agent/request-error waterfall. llm-retry retries only retryable codes (retry-policy.ts:222-232), at most 5 times by default, with capped exponential backoff, jitter and the provider's retry-after. Each retry is logged, and the wait is cancellable (llm-retry/src/index.ts:231-236, 321-413). On overflow, compaction-basic prunes tool results and compacts, then retries at most once by default (compaction-basic/src/index.ts:176-220; config.ts:93). The failed attempt never adds text to history (agent.ts:444-464).

**What we do.** Our project turns every sidecar failure into InferenceError::Generation(String) (backend/src/llamaserver.rs:1024-1033, 1045, 1201-1202); the only retries are one connect retry and one template-refusal reshape (llamaserver.rs:975-1034). When a request fails, the agent appends 'The model call failed ({e})...' as a user turn and asks again at once (backend/src/agent_runner.rs:1949-1963). Its status says 'Attempt N of 4' while the guard allows 6 (agent_runner.rs:1738 vs 1954). Chat stops and saves 'Inference interrupted' (api.rs:2708-2711, 2804-2822).

**Limitation for us.** When llama-server rejects a request for exceeding the context (type exceed_context_size_error, HTTP 400, in llama.cpp master), the agent adds a turn and makes the context bigger. A transient failure, such as the server restarting, is retried with zero delay and uses up the 6-attempt budget in seconds. The model sees transport errors it cannot act on. The status text is wrong.

**Recommendation.** In llamaserver.rs, map failures to an enum using HTTP status plus the JSON error.type: ContextExceeded, TemplateRefusal, Unavailable/Transport, Cancelled, EmptyResponse, BadRequest, ServerError. In the agent: ContextExceeded forces prune/compaction and one retry, and never appends text. Transient errors get a small bounded retry with backoff that watches the cancel token; the transcript is untouched, and the retry counts against the progress guard only when it is exhausted. Anything else stops with a plain message. Chat uses the same classes. Fix the 'of 4' text so it reads from the guard's limit.

**Applies to desktop.** port — the code-based retry decision and the rule that errors never enter history fit one local server; the provider-policy configuration does not.

**Expected impact.** An overflow recovers instead of failing the run after 6 attempts, and transport errors no longer reach the model.

**How to measure.** Cargo tests with the existing mock sidecar returning 400 {error:{type:'exceed_context_size_error'}}, 503 and a connection reset; assert the transcript length, the retry count and that compaction ran.

**Category.** bug-fix

**Confidence.** high

**Evidence.**

- local-llm-companion/backend/src/llamaserver.rs:975-1034
- local-llm-companion/backend/src/llamaserver.rs:1201-1202
- local-llm-companion/backend/src/agent_runner.rs:1738
- local-llm-companion/backend/src/agent_runner.rs:1949-1963
- local-llm-companion/backend/src/api.rs:2708-2711
- local-llm-companion/backend/src/api.rs:2804-2822
- deepseek-harness/packages/llm/llm/src/error.ts:24-48
- deepseek-harness/packages/llm/llm/src/error.ts:80-86
- deepseek-harness/packages/llm/llm-deepseek/src/protocols/chat-completions/adapter.ts:97-109
- deepseek-harness/packages/llm/llm-deepseek/src/protocols/chat-completions/translate.ts:133-141
- deepseek-harness/packages/llm/llm/src/retry-policy.ts:222-232
- deepseek-harness/packages/llm/llm-retry/src/index.ts:321-413
- deepseek-harness/packages/compaction/compaction-basic/src/index.ts:176-220
- deepseek-harness/packages/core/agent-loop/src/agent.ts:444-464
- https://raw.githubusercontent.com/ggml-org/llama.cpp/master/tools/server/server-common.cpp

### 5. Tool output truncation drops the tail (stderr, build errors)

**What they do.** In DeepSeek Harness, a plain-text tool result over maxInlineBytes is saved whole to a session spill file. The model gets a head+tail preview plus the file location and how to read it (packages/spill/spill-policy/src/index.ts:96-103, 125-204). The model-free pruner keeps head 4096 + tail 1024 characters once a result passes 8192 (compaction-tool-result-pruner/src/config.ts:195-203; index.ts:83-122).

**What we do.** Our project caps stdout and stderr each at their first 50,000 characters (backend/src/terminal.rs:120-124), then formats stdout before stderr (terminal.rs:128-150). The agent keeps the first min(16,000, room/5) characters of the combined text (agent_runner.rs:34-37, 2449). Chat keeps the first 16,000 (api.rs:1707-1714).

**Limitation for us.** When a build prints a lot to stdout, the stderr section is dropped entirely, and so are the final error lines at the end of stdout. The model is told it can 'read the file again', which does not apply to command output.

**Recommendation.** For execute_command, and any tool whose output is not a file chunk, keep the exit code, the head, and a larger tail (for example a third head, two thirds tail), and show a short stderr tail before stdout when the exit code is non-zero. Write the full output to a per-run log under the app data directory and put its path in the result, so read_file-style access can fetch the middle. file_read chunks stay as they are, since they already carry continuation positions.

**Applies to desktop.** port — the head+tail preview with a saved full copy is small and fits one user directly.

**Expected impact.** The agent sees compiler and test errors on verbose builds instead of guessing, so fewer failed steps.

**How to measure.** A unit test with synthetic output (60K characters of stdout plus 2K of stderr) asserting the stderr tail survives at the smallest room size. No model is needed.

**Category.** correctness

**Confidence.** high

**Evidence.**

- local-llm-companion/backend/src/terminal.rs:120-124
- local-llm-companion/backend/src/terminal.rs:128-150
- local-llm-companion/backend/src/agent_runner.rs:34-37
- local-llm-companion/backend/src/agent_runner.rs:2449
- local-llm-companion/backend/src/api.rs:1707-1714
- deepseek-harness/packages/spill/spill-policy/src/index.ts:96-103
- deepseek-harness/packages/spill/spill-policy/src/index.ts:125-204
- deepseek-harness/packages/compaction/compaction-tool-result-pruner/src/config.ts:195-203

### 6. Prompt order and the llama-server prompt cache

**What they do.** DeepSeek Harness sends dynamic context (time, approval policy and similar) as a user-role snapshot placed after the saved history, so the system-prompt prefix stays byte-stable (packages/core/agent-loop/src/runtime-context.ts:108-158). The approval plugin comment says 'The complete current value travels after retained history, so switching policy does not rewrite the stable system-prompt cache prefix' (packages/interaction/user-approval/src/index.ts:153). On models that read a later system message, a changed system prompt is appended after the cached history (runtime-context.ts:52-98; docs/architecture.md:111).

**What we do.** Our chat inserts the reasoning preface as a system turn at index 1, before history (backend/src/api.rs:2412-2430). Saved memory is appended to the system prompt (api.rs:1341-1346), and so is a live directory tree snapshot (api.rs:1337-1341). The agent builds a different system prompt (agent_runner.rs:1674-1685, 1696-1698), yet api.rs:1200-1206 says chat, routing and the agent share one prefix.

**Limitation for us.** Turning reasoning on or off, adding a memory, or the agent creating a top-level file changes the bytes in front of the whole history. That forces a full re-prefill, which api.rs:1204-1206 itself says costs minutes on a CPU-only machine. Handing a chat over to the agent re-prefills everything.

**Recommendation.** Order every request as: stable identity and rules; saved history; one dynamic block (workspace snapshot, memory, reasoning preface, retrieval hints); the latest user turn. Let chat and agent share the stable identity part, and put the agent's rules in the dynamic block, or accept and document that the agent re-prefills. Read engine timings to check.

**Applies to desktop.** port — prefix stability matters more for us than for a cloud API because prefill runs on the user's hardware.

**Expected impact.** Much less re-prefill after a reasoning toggle, a memory change or a file creation; largest on CPU placement.

**How to measure.** Once benchmarking is finished and the owner allows it: replay a fixed conversation, toggle reasoning or add a file between turns, and record the cached versus processed prompt tokens that EngineTimings already parses, before and after. Report per-request numbers, not averages.

**Category.** performance

**Confidence.** medium

**Evidence.**

- local-llm-companion/backend/src/api.rs:1200-1206
- local-llm-companion/backend/src/api.rs:1337-1346
- local-llm-companion/backend/src/api.rs:2412-2430
- local-llm-companion/backend/src/agent_runner.rs:1674-1698
- local-llm-companion/backend/src/llamaserver.rs:1269-1272
- local-llm-companion/backend/src/llamaserver.rs:1318-1330
- deepseek-harness/packages/core/agent-loop/src/runtime-context.ts:52-158
- deepseek-harness/packages/interaction/user-approval/src/index.ts:153
- deepseek-harness/docs/architecture.md:111

### 7. Token accounting for compaction and pruning

**What they do.** The DeepSeek Harness token meter uses the provider's reported usage from the latest successful call as the baseline, and adds heuristic estimates only for what changed since (packages/llm/token-meter/src/index.ts:145-190, 277-302). The fixed 4 characters per token is only a fallback (token-meter/src/estimate.ts:12-19). Automatic compaction starts at 80% of the model's context window (compaction-basic/src/config.ts:20).

**What we do.** Our project estimates characters/4 plus 8 per turn (backend/src/agent.rs:118-124). Compaction and pruning decide on that estimate (agent_runner.rs:945-947, 970-985, 1185-1191), even though every request reports the real prompt_tokens (agent_runner.rs:1939-1946; llamaserver.rs:1261-1268, 1318-1330). The code comment already records overshooting to 112% (agent_runner.rs:22-30).

**Limitation for us.** Code and non-Latin text take more tokens per character, so the estimate runs low. Compaction starts late, and the run hits the overflow path.

**Recommendation.** Store the last reported prompt_tokens and the transcript length at that request. Estimate the next request as reported + estimate(turns added since) - estimate(turns removed). Use /tokenize (llamaserver.rs:1364-1384) for the added turns when they are large. Fall back to the current estimate when no usage has been reported yet.

**Applies to desktop.** port — cheap, and uses numbers we already receive.

**Expected impact.** Compaction starts on time and the usage gauge stops overshooting 100%.

**How to measure.** Without a model: read the context events already in the message_activities journal (on a copy of companion.db), which carry both estimated_tokens and prompt_tokens, and plot estimate error per request. After the change, the same query should show the error bounded by the size of the change since the last request.

**Category.** inference

**Confidence.** high

**Evidence.**

- local-llm-companion/backend/src/agent.rs:118-124
- local-llm-companion/backend/src/agent.rs:150-157
- local-llm-companion/backend/src/agent_runner.rs:22-30
- local-llm-companion/backend/src/agent_runner.rs:970-985
- local-llm-companion/backend/src/agent_runner.rs:1185-1191
- local-llm-companion/backend/src/agent_runner.rs:1939-1946
- local-llm-companion/backend/src/llamaserver.rs:1364-1384
- deepseek-harness/packages/llm/token-meter/src/index.ts:145-190
- deepseek-harness/packages/llm/token-meter/src/estimate.ts:12-19
- deepseek-harness/packages/compaction/compaction-basic/src/config.ts:20

### 8. Tool schemas: one source for prompt text, validation and constrained decoding

**What they do.** DeepSeek Harness defines each tool once with a typed parameter schema compiled to JSON Schema. Arguments are validated before execute, and violations come back to the model as an 'Error: invalid arguments: ...' tool result (packages/core/tools/src/schema.ts:461-466, 478-480, 545, 587; core/tools/src/index.ts:1879-1887). Invalid JSON arguments are passed through as a raw string, so validation reports them (agent-loop/src/tool-calls.ts:244-251). An unknown or hidden tool gets a result that names the correct way to call it (core/tools/src/index.ts:1436-1448).

**What we do.** Our ToolDescriptor holds only name, description and risk (backend/src/tools.rs:11-16). Argument examples are hand-written in the agent prompt (agent_runner.rs:1314-1331) and again in the chat prompt (api.rs:1519-1520, 1536-1537). Each tool validates its own arguments by hand (tools.rs:230-236, 384-386). The structured fallback schema accepts any args object (llamaserver.rs:877-892), so the agent has to fall back when constrained output has invalid arguments (agent_runner.rs:2465-2474).

**Limitation for us.** Three copies of the argument rules drift apart. Constrained decoding cannot enforce required arguments, the very case where small models fail.

**Recommendation.** Add an args JSON Schema and one example to ToolDescriptor. Generate the agent and chat tool docs from it. Validate generically before execute with one error format that lists what is missing. Build the structured-envelope response_format as a oneOf over the tools allowed in this run, each with its required fields, so llama.cpp's grammar enforces them. Keep raw CONTENT/OLD/NEW blocks outside the schema.

**Applies to desktop.** port — a static Rust registry with schemas is enough; no dynamic plugin registry is needed.

**Expected impact.** Fewer invalid-argument steps in structured mode, and the chat and agent tool docs can no longer disagree.

**How to measure.** Unit tests: the generated docs match the registry, the validator rejects missing required arguments, and a snapshot of the generated oneOf schema. Constrained-decoding effect: count 'Invalid tool arguments' events per run in journals before and after, from runs the owner makes.

**Category.** api

**Confidence.** medium

**Evidence.**

- local-llm-companion/backend/src/tools.rs:11-16
- local-llm-companion/backend/src/tools.rs:230-236
- local-llm-companion/backend/src/agent_runner.rs:1314-1331
- local-llm-companion/backend/src/api.rs:1519-1520
- local-llm-companion/backend/src/llamaserver.rs:877-892
- local-llm-companion/backend/src/agent_runner.rs:2465-2474
- deepseek-harness/packages/core/tools/src/schema.ts:461-466
- deepseek-harness/packages/core/tools/src/schema.ts:545-587
- deepseek-harness/packages/core/tools/src/index.ts:1879-1887
- deepseek-harness/packages/core/agent-loop/src/tool-calls.ts:244-251

### 9. A durable record of what each model request saw (proportionate substitute for the session event log)

**What they do.** In DeepSeek Harness the append-only session log is where the model's context comes from. Every request is rebuilt from it and checked (docs/architecture.md:117-125; agent-loop/src/invariant.ts:19-57). Turns, steps, attempts, retries and compaction are all events. On resume, an interrupted tail is repaired with synthetic closing events (packages/core/session/src/repair.ts:29-89). JSONL storage uses a kernel lock that allows one writer across processes (session-persistence-jsonl/src/lease.ts:1-30).

**What we do.** Our project keeps messages plus message_activities, a UI activity journal of human-readable events (backend/src/storage.rs:257-312, 569-579). The agent's working transcript lives only in memory (agent_runner.rs:1685-1727). When the database reopens, unfinished journals are marked interrupted (storage.rs:659-712). An unreadable reply is journaled only as a 900-character head and a 400-character tail inside a status message (agent_runner.rs:2050-2063).

**Limitation for us.** We cannot rebuild the exact request that produced a malformed output, such as the 2B case, or check what compaction removed. Agent runs cannot resume after a restart.

**Recommendation.** Do not port JSONL event sourcing or Cordis. Add one SQLite table, model_requests, with these columns: owner id (message or run), seq, kind (chat / agent / compaction / review / classify), request (the turns plus options, or a hash plus the turns appended since the previous request), raw output, finish_reason, outcome (committed / rejected / retried / cancelled / failed), prompt and cached token counts, created_at. Write it behind the request so it never slows a response, with a retention cap. Leave agent resume out of scope.

**Applies to desktop.** redesign — keep the one idea (the model's input can be rebuilt from storage) and drop the plugin-wide event system.

**Expected impact.** Malformed outputs can be diagnosed from data instead of guessed at, and the recorded requests become replay fixtures.

**How to measure.** Check that a stored request, sent through the same sampling_body, produces an identical request body (a unit test, no model). Track database growth per 100 messages against the retention cap.

**Category.** architecture

**Confidence.** medium

**Evidence.**

- local-llm-companion/backend/src/storage.rs:257-312
- local-llm-companion/backend/src/storage.rs:569-579
- local-llm-companion/backend/src/storage.rs:659-712
- local-llm-companion/backend/src/agent_runner.rs:1685-1727
- local-llm-companion/backend/src/agent_runner.rs:2050-2063
- deepseek-harness/docs/architecture.md:117-125
- deepseek-harness/packages/core/agent-loop/src/invariant.ts:19-57
- deepseek-harness/packages/core/session/src/repair.ts:29-89
- deepseek-harness/packages/session/session-persistence-jsonl/src/lease.ts:1-30

### 10. Compaction: truncated summaries and prune-before-summarize

**What they do.** DeepSeek Harness rejects a summary cut off by the token cap ('summarization truncated at the token cap (incomplete checkpoint)') and returns an error on error or aborted finishes (compaction-basic/src/summarizer.ts:194-209). Before choosing a range to summarize, it runs the model-free tool-result pruner and measures again (compaction-basic/src/index.ts:275-309). The summary call reuses the conversation's own prefix to keep the cache warm (summarizer.ts:24-30, 143-161), and range selection never splits a tool call from its result (compaction/src/tool-pairing.ts; compaction-basic/src/region.ts:118-156).

**What we do.** Our agent compaction also reuses the cached transcript and adds a host-written record of completed actions, a strong fit for small models (backend/src/agent_runner.rs:1066-1170). It uses the note text whatever its finish reason: the metrics are ignored (agent_runner.rs:1110-1121; the finish reason is available in Metrics, llamaserver.rs:1099). Releasing tool-result bodies (prune_transcript) runs after compaction, not before it (agent_runner.rs:1771-1856).

**Limitation for us.** A note cut off mid-sentence can replace earlier turns. Compaction may call the model when simply releasing old tool results would have been enough.

**Recommendation.** Keep our design. Port two small things. (1) If the note's finish_reason is 'length', trim it to the last complete line, or drop it and rely on the host record. (2) Before calling the model to compact, release old tool-result bodies (the prune_transcript first loop) and measure again; call the model only if usage is still over the threshold.

**Applies to desktop.** port — two local changes to existing functions.

**Expected impact.** No half-written notes, and fewer compaction model calls on small context windows.

**How to measure.** Unit tests with a mock sidecar returning finish_reason 'length' for the note. From journals: the count of 'Context compacted' events per run, before and after.

**Category.** harness

**Confidence.** high

**Evidence.**

- local-llm-companion/backend/src/agent_runner.rs:1066-1170
- local-llm-companion/backend/src/agent_runner.rs:1110-1121
- local-llm-companion/backend/src/agent_runner.rs:1771-1856
- local-llm-companion/backend/src/llamaserver.rs:1099
- deepseek-harness/packages/compaction/compaction-basic/src/summarizer.ts:194-209
- deepseek-harness/packages/compaction/compaction-basic/src/index.ts:275-309
- deepseek-harness/packages/compaction/compaction-basic/src/region.ts:118-156

### 11. Offline replay tests from recorded model streams

**What they do.** DeepSeek Harness runs keyless snapshot tests that replay recorded sessions through the shipped profiles without calling a model (AGENTS.md:90-91; packages/test-support/llm-replay/src/index.ts:1-8).

**What we do.** Our project has mock-SSE unit tests with hand-written frames (backend/src/llamaserver.rs:2066, 2147) and parser tests in agent_runner.rs. Real failure streams, such as the 2B malformed block, are not kept as fixtures.

**Limitation for us.** Regressions in parsing, splitting and saving are caught only by hand-built cases, and checking a fix needs a model, which cannot be loaded while benchmarking runs.

**Recommendation.** Store captured raw SSE bodies (or the raw_output from the proposed model_requests table) as fixture files. Add cargo tests that run them through the stream splitter, locate_action, the saving path and history building, asserting the visible text, action events and saved content.

**Applies to desktop.** port — fixture replay is simple, and it respects the rule against loading models.

**Expected impact.** Streaming, leak and correction fixes can be checked without a model.

**How to measure.** The fixture count and cargo test results; each reported malformed-output bug adds one fixture.

**Category.** harness

**Confidence.** high

**Evidence.**

- deepseek-harness/AGENTS.md:90-91
- deepseek-harness/packages/test-support/llm-replay/src/index.ts:1-8
- local-llm-companion/backend/src/llamaserver.rs:2066
- local-llm-companion/backend/src/llamaserver.rs:2147

### 12. Loop guards, parallel tools and permission tiers (mostly keep ours)

**What they do.** DeepSeek Harness has no step limit (no maxSteps in the source). Its advisory repeat-call reminder fires at 3, 5 and 8 identical calls and resets when the user sends a message (guard/repeat-tool-reminder/src/index.ts:46, 213-232). Up to 10 parallel-safe tool calls run at once, with exclusive tools as barriers (agent-loop/src/constants.ts:6; tool-calls.ts:200-242). Tools get cooperative per-tool timeouts (guard/timeout-policy/src/index.ts:334-359). Permissions combine sandbox read-only / workspace-write / danger-full-access with approval ask/never (permission-presets/src/index.ts:113-115). Approval fails closed, logs an asked/decided audit pair, and tells the model when the user changes policy (user-approval/src/index.ts:177-212, 208-302).

**What we do.** Our project stops after 6 attempts without progress, warns after 2 identical repeats and stops after 5, stops when the same call fails 3 times, and caps a run at 30 iterations (backend/src/agent_progress.rs:14-17; agent_runner.rs:1738, 2518-2523; agent.rs:225-233). It allows one action per turn (agent_runner.rs:1371) on a single server slot (llamaserver.rs:144-145). Permissions are RiskLevel x AutonomyLevel with per-workspace session grants for Moderate tools (permissions.rs:13-33, 97-140). An approval request is journaled (agent_runner.rs:2992-3018), but how it was approved (once, session grant, or released by Auto) is not its own event (agent_runner.rs:2330-2372).

**Limitation for us.** Only the gap in approval auditing: from the journal alone you cannot tell whether an action ran by one-time approval, a session grant or an Auto release.

**Recommendation.** Keep our stricter guards and one action per turn: they suit small local models and one slot. Do not port parallel tool scheduling or presets. Add one 'permission_decided' activity event holding once / session / auto / denied / cancelled, and treat a missing answer as denied, which is already the case (agent_runner.rs:2342).

**Applies to desktop.** ignore — except the one audit event; the harness's concurrency and preset machinery is sized for a multi-session product.

**Expected impact.** Small: approval decisions can be audited from the journal.

**How to measure.** A unit test asserting the event after each decision path.

**Category.** not-applicable

**Confidence.** high

**Evidence.**

- local-llm-companion/backend/src/agent_progress.rs:14-17
- local-llm-companion/backend/src/agent_runner.rs:1371
- local-llm-companion/backend/src/agent_runner.rs:2330-2372
- local-llm-companion/backend/src/agent_runner.rs:2992-3018
- local-llm-companion/backend/src/permissions.rs:13-33
- local-llm-companion/backend/src/llamaserver.rs:144-145
- deepseek-harness/packages/guard/repeat-tool-reminder/src/index.ts:46
- deepseek-harness/packages/guard/repeat-tool-reminder/src/index.ts:213-232
- deepseek-harness/packages/core/agent-loop/src/constants.ts:6
- deepseek-harness/packages/interaction/user-approval/src/index.ts:208-302
- deepseek-harness/packages/interaction/permission-presets/src/index.ts:113-115

### Open questions

- Does the pinned llama-server build 10809 (commit 5266f24da) return chat_template_caps (supports_tools, supports_tool_calls, supports_system_role) from GET /props? Confirmed only on llama.cpp master (tools/server/README.md and common/jinja/caps.h).
- Does build 10809 use error type 'exceed_context_size_error' for context overflow? Confirmed only in master server-common.cpp.
- How does llama-server report an error that happens mid-stream in SSE? Not found in the README or server-common.cpp. Our parser silently skips any frame without a delta (llamaserver.rs:1213-1215), and a stream that ends with no finish_reason counts as success.
- Would sending native 'tools' to llama-server (jinja parser, tool_calls deltas) cut malformed actions on models that support tools, compared with our text envelope? Needs a measured A/B when the machine is free. Our raw CONTENT/OLD/NEW blocks exist to avoid JSON escaping failures (agent_runner.rs:419-440), and native tool calls would bring those back.
- Can chat_sse run while an agent run is active? chat_sse does not call guard_agent_running, but classify_request does (api.rs:1401). On --parallel 1 that would swap out the agent's KV prefix. The frontend flow that prevents it was not checked.
- How pi-ai, the harness's external library dependency (not in the repo), handles routes without tool support was not inspected. The claim that the harness assumes tool support on every route rests only on its own llm types and agent-loop code.
- Whether the harness's runtime invariant checks (agent-loop/src/invariant.ts) are switched on in production profiles, or only in diagnostics builds, was not checked.

## Ollama: runtime behaviour and registry-file compatibility

Reader brief: Ollama's runtime defaults from source, engine selection, timing semantics, why registry GGUFs fail upstream.

### Summary

Main finding: Ollama no longer has its own Go engine for GGUF models. Starting with v0.30.0 (commit 9db4bdbad6, 29 May 2026), every GGUF model, including gemma3, gemma3n and gemma4, runs in a llama-server child process. That process is built from a pinned upstream llama.cpp plus Ollama's own load-time patch for older files. The installer downloaded for the benchmark is v0.34.1 (read from the file's version info, not run). Its Go code is the same as main at a43fad1, and it pins llama.cpp b10864. Ours is b10809. So the comparison is the same engine with different launch flags, a different prompt path and a different model file.

Differences that will move benchmark numbers:
- **Threads:** Ollama leaves the thread count to llama-server. Its Windows build uses a MinGW compiler, which probably means logical cores / 2 (inferred). Ours probably counts all physical cores.
- **Batch:** Ollama sets both batch sizes to num_batch: 512, rising to 1024 or 2048 when the context is larger.
- **Context:** 4096 by default when total VRAM is under 23 GiB, which includes CPU-only machines.
- **Model loading:** memory mapping is off on CPU-only machines and on Windows with CUDA.
- **Thinking:** on by default for gemma4.
- **Sampling:** the gemma4 model settings are temp 1, top_k 64, top_p 0.95; Ollama also sends min_p 0. The /v1 endpoint forces temperature and top_p to 1.0 when the request leaves them out.
- **Extra work in our app only:** Ollama uses no n-gram speculation and no cache reuse.
- **Timing fields:** prompt_eval_count includes cached tokens.

GGUF compatibility: Ollama's old gemma3 converter never wrote gemma3.attention.layer_norm_rms_epsilon for vision models, and it put the vision tower inside the same file. Upstream requires that key. I read only the header of the gemma4 e2b blob. It does have the key, but it declares tokenizer "llama" and carries 1,411 vision, audio and projector tensors. Upstream rejects that on tensor count, so adding keys alone cannot fix either blob.

Method: I fetched raw source files with curl into the scratch folder to get exact line numbers, and read only the GGUF header of the partly downloaded blob. Nothing was loaded, run or built.

### 1. Which engine Ollama runs (Go engine vs llama.cpp), how it picks, and which version is being benchmarked

**What they do.** Current Ollama (main a43fad1; v0.34.1 has identical Go code) serves ALL GGUF models through an upstream llama-server subprocess: llm/server.go says 'All GGUF models are served via the upstream llama-server subprocess'. runner/runner.go only dispatches '--mlx-engine'. Engine choice is by manifest config model_format: 'safetensors' -> MLX runner (x/mlxrunner, Metal or CUDA 13 incl. Windows), anything else -> llama-server. llama-server is built from LLAMA_CPP_VERSION (b10864 in v0.34.1, b10969 on main) plus llama/compat patches. The Go engine (runner/ollamarunner, model/models/*) was deleted in commit 9db4bdbad6 (2026-05-29); v0.24.0 still had it and forced gemma3, gemma3n, gemma4, qwen3, gpt-oss, etc. onto the Go engine via OllamaEngineRequired(); v0.30.0 is the first tag without it. gemma4:e2b's registry config says model_format gguf, so it runs on llama-server.

**What we do.** Launch prebuilt llama-server b10809 (commit 5266f24da) as a child and call /v1/chat/completions with SSE.

**Limitation for us.** The llama.cpp builds differ (b10864 vs b10809), so part of any gap can come from upstream changes neither app controls.

**Recommendation.** Treat Ollama v0.34.1 vs our app as the same engine with a ~55-build llama.cpp gap. Record the Ollama version and its LLAMA_CPP_VERSION next to every benchmark row. Do not attribute gaps to 'Go engine vs llama.cpp' unless the installed Ollama is <= v0.24.x.

**Applies to desktop.** ignore - nothing to port; it changes how results must be read, since both sides are llama-server.

**Expected impact.** Frames every other finding: gaps come from flags, llama.cpp build, prompt rendering and model file, not from a different inference engine.

**How to measure.** Ollama server log lines 'using llama-server for model' and 'starting llama-server cmd=...' show the exact argv; compare it with our 'spawning ...' log line.

**Category.** architecture

**Confidence.** high

**Evidence.**

- llm/server.go:97-111 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/server.go#L97-L111
- runner/runner.go:9-21 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/runner/runner.go#L9-L21
- server/sched.go:528-594 (llama-server vs mlxrunner.NewClient) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/sched.go#L528-L594
- server/images.go:92-94 IsMLX = model_format safetensors - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/images.go#L92-L94
- Commit removing CGO engines: https://github.com/ollama/ollama/commit/9db4bdbad6 ; GitHub contents API shows runner/ollamarunner present at v0.24.0 and absent at v0.30.0
- v0.24.0 fs/ggml/ggml.go:277-303 OllamaEngineRequired list - https://github.com/ollama/ollama/blob/v0.24.0/fs/ggml/ggml.go#L277-L303
- v0.24.0 llm/server.go:148-164 engine selection - https://github.com/ollama/ollama/blob/v0.24.0/llm/server.go#L148-L164
- compare v0.34.1...a43fad1: only LLAMA_CPP_VERSION, llama/compat/llama-ollama-compat.cpp/.h and llama/server/CMakeLists.txt differ; v0.34.1 LLAMA_CPP_VERSION = b10864 - https://github.com/ollama/ollama/compare/v0.34.1...a43fad18b088095de20fbd7a8f0de50824cf5d27
- Downloaded installer version info (read, not executed): scratchpad/dl/OllamaSetup.exe FileVersion 0.34.1
- Registry config for gemma4:e2b (model_format gguf, renderer gemma4, parser gemma4): https://registry.ollama.ai/v2/library/gemma4/blobs/sha256:c6bc3775a3fa9935ce4a3ccd7abc59e936c3de9308d2cc090516012f43ed9c07
- Our launcher: local-llm-companion\backend\src\llamaserver.rs:138-202

### 2. num_thread default on Windows x86 (physical cores? P-cores only on hybrid Intel?)

**What they do.** v0.34.1 does not compute threads: it passes '-t' only when num_thread > 0, otherwise llama-server picks. llama.cpp's default (common_cpu_get_num_math) filters E-cores and hyperthreads only on Linux x86_64. On Windows the MSVC-ABI path counts physical cores (P+E) with GetLogicalProcessorInformationEx, but that path is excluded under __MINGW64__, which falls back to hardware_concurrency()/2. Ollama builds the Windows x64 CPU llama-server with llvm-mingw (build_windows.ps1 findWindowsCPUCompiler, and release.yaml installs llvm-mingw), so its default is most likely logical/2 (inferred). The batch thread count copies the generation thread count. Old Ollama (<= v0.24.0) did compute a default: the sum over sockets of CoreCount - EfficiencyCoreCount, i.e. P-cores only, falling back to runtime.NumCPU().

**What we do.** In automatic mode threads = 0, so no --threads flag and llama-server picks. Our binaries appear to be the official llama.cpp Windows release, built with clang targeting the MSVC ABI (release.yml windows-cpu job with x64-windows-llvm.cmake), so the default is all physical cores including E-cores. inference.rs notes 'all physical cores are used'.

**Limitation for us.** I could not run Ollama's llama-server.exe to confirm the MinGW default. The logical/2 figure is inferred from build scripts plus source.

**Recommendation.** For any CPU or partial-offload benchmark, set the same explicit thread count on both sides: Ollama 'num_thread' in options (a runner option, so it triggers a reload) and our manual n_threads. Before trusting numbers, read the actual count from each server's 'system_info: n_threads = X (n_threads_batch = Y) / Z' line. On a hybrid Intel CPU (e.g. 6P+8E, 20 logical), defaults could be 10 (Ollama) vs 14 (ours).

**Applies to desktop.** ignore - our current default is the one we measured; this is a benchmark-parity control, not something to copy.

**Expected impact.** On hybrid Intel CPUs, CPU decode and prefill can differ by tens of percent purely from thread count. On non-hybrid SMT CPUs both defaults coincide.

**How to measure.** Grep both servers' logs for 'system_info: n_threads'. Then run both apps at the same explicit thread count and compare eval rates.

**Category.** performance

**Confidence.** medium

**Evidence.**

- llm/llama_server.go:413-417 (-t only if NumThread > 0) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L413-L417
- api/types.go:1159 NumThread: 0 'let the runtime decide' - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/api/types.go#L1159
- discover/gpu.go:20-38 (current GetSystemInfo has no thread count) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/discover/gpu.go#L20-L38
- llama.cpp common/common.cpp:116-147 (Windows physical cores, excluded for __MINGW64__, fallback logical/2) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/common/common.cpp#L116-L147
- llama.cpp common/common.cpp:202-228 (E-core/HT filtering only on x86_64 Linux) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/common/common.cpp#L202-L228
- llama.cpp common/common.cpp:288-298 and common/arg.cpp:892 (n_threads -1 -> num_math; batch threads copy) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/common/common.cpp#L288-L298
- same MinGW guard at Ollama's pin: https://github.com/ggml-org/llama.cpp/blob/391fac16460f15233a7740550d858ac96df3419d/common/common.cpp#L116
- scripts/build_windows.ps1:88-110 and 369-416 (llvm-mingw chosen for the CPU llama-server build) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/scripts/build_windows.ps1#L88-L110
- llama/server/CMakeLists.txt:49-53 (WIN32 AND MINGW branch) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llama/server/CMakeLists.txt#L49-L53
- .github/workflows/release.yaml:205-207 and 343-349 (llvm-mingw installed) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/.github/workflows/release.yaml#L343-L349
- llama.cpp .github/workflows/release.yml:649-704 and cmake/x64-windows-llvm.cmake (clang in a vcvars MSVC environment) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/.github/workflows/release.yml#L649-L704
- v0.24.0 discover/gpu.go:32-41 (threads = CoreCount - EfficiencyCoreCount) - https://github.com/ollama/ollama/blob/v0.24.0/discover/gpu.go#L32-L41
- v0.24.0 llm/server.go:177-182 - https://github.com/ollama/ollama/blob/v0.24.0/llm/server.go#L177-L182
- Ours: backend/src/llamaserver.rs:184-187; backend/src/inference.rs:623 and 653-657

### 3. num_batch / ubatch defaults

**What they do.** Ollama passes '-b N -ub N' with both equal to num_batch whenever num_batch > 0. Defaults: api.DefaultOptions NumBatch = 512, then an automatic tier for generation models when num_batch is not set in the request or Modelfile. Effective context (num_ctx x parallel) <= 4096 gives 512; > 4096 gives 1024; > 32768 gives 2048. It steps down while predicted VRAM + surcharge (768 MiB for 1024, 2 GiB for 2048) exceeds 80% of available memory, or above the 75%/60% headroom rules. CUDA with flash attention disabled uses 512, or 256 when ctx > 4096 and GPU memory <= 8 GiB. Embedding models get min(2048, ctx). With no GPU, available memory is 0 and the fit check returns true, so the context tier alone decides.

**What we do.** Automatic mode sends no --batch-size, so llama-server defaults apply: n_batch 2048, n_ubatch 512. Manual mode passes --batch-size only, never --ubatch-size, so ubatch stays 512.

**Limitation for us.** Larger ubatch raises compute-buffer VRAM, which our fit_to_memory would need to account for.

**Recommendation.** For prefill comparisons pin both sides to one value: Ollama options.num_batch = N, and our manual n_batch = N plus the same ubatch. At Ollama's 4k default both run ubatch 512. On a GPU box with ctx > 4096 Ollama may run ubatch 1024/2048. Measure our prefill at ubatch 512 vs 1024/2048 before considering any change to our defaults.

**Applies to desktop.** redesign - only after measuring whether a larger ubatch helps our GPU prefill; the fix would be a global setting, not a copy of Ollama's tiers.

**Expected impact.** Prefill tokens/s on GPU can differ noticeably between ubatch 512 and 1024/2048. Decode speed is unaffected.

**How to measure.** Ollama log line 'starting llama-server cmd=' shows -b/-ub. Run the same long prompt through both apps at matched ubatch and compare prompt_n/prompt_ms.

**Category.** performance

**Confidence.** high

**Evidence.**

- llm/llama_server.go:582-595 appendBatchArgs (-b and -ub both = NumBatch) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L582-L595
- api/types.go:1157 NumBatch: 512 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/api/types.go#L1157
- server/routes.go:192-202 usesAutomaticNumBatch - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/routes.go#L192-L202
- server/sched.go:540-547 and 802-920 automaticGenerationBatch - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/sched.go#L802-L920
- llm/llama_server.go:75-104 embedding batch default - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L75-L104
- llama.cpp common/common.h:451-452 (n_batch 2048, n_ubatch 512) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/common/common.h#L451-L452
- Ours: backend/src/llamaserver.rs:170-175; backend/src/inference.rs:663-667

### 4. num_ctx default, OLLAMA_NUM_PARALLEL, and KV allocation

**What they do.** num_ctx comes from, in priority order: request option, Modelfile, OLLAMA_CONTEXT_LENGTH, then a server default chosen at startup from total GPU VRAM (minus OLLAMA_GPU_OVERHEAD). >= 47 GiB gives 262144; >= 23 GiB gives 32768; otherwise 4096. CPU-only machines list no GPUs, so they get 4096. The value is capped at the model's trained context, raised to at least 2048 for vision-capable models, and at least 4. OLLAMA_NUM_PARALLEL defaults to 1, forced to 1 for embedding models and some architectures. llama-server gets '-c num_ctx*parallel -np parallel', so KV is allocated for every slot. If a load hits OOM with an automatic ctx, Ollama retries once at the next lower tier (32768 or 4096).

**What we do.** Request 32768 by default. fit_to_memory lowers it on GPU; CPU runs are capped at 8192 (lower if RAM is short); the model's trained context caps it. '--parallel 1' and '--ctx-size n_ctx'.

**Limitation for us.** The VRAM tier boundary (23 GiB) means the test machine's GPU size changes Ollama's default. It must be recorded per run.

**Recommendation.** Set the same context on both sides for every benchmark (Ollama options.num_ctx or OLLAMA_CONTEXT_LENGTH; our manual n_ctx). Otherwise Ollama runs at 4096 on a <23 GiB machine while we run at 8192 (CPU) or up to 32768 (GPU): different KV size and different GPU layer placement. Keep OLLAMA_NUM_PARALLEL=1 so KV per request matches.

**Applies to desktop.** ignore - our fitted context is a deliberate product choice; this is a parity control.

**Expected impact.** Different context gives different KV memory and, on GPU, possibly different offloaded layer counts, which can swing decode speed a lot on partial offload.

**How to measure.** 'ollama ps' CONTEXT column or the '-c' in the Ollama log, vs our resolved policy effective_context; compare the 'KV buffer size' log lines from both servers.

**Category.** memory

**Confidence.** high

**Evidence.**

- server/routes.go:2036-2051 VRAM-tier default - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/routes.go#L2036-L2051
- server/routes.go:126-160 option layering and 180-190 usesAutomaticNumCtx - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/routes.go#L126-L190
- envconfig/config.go:230 ContextLength default 0; 277 NumParallel default 1 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/envconfig/config.go#L275-L277
- server/sched.go:174-182 (min 4, vision >= 2048) and 501-515 (parallel forced to 1) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/sched.go#L501-L515
- server/sched.go:762-800 and 922-931 (OOM retry, effective ctx x parallel) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/sched.go#L762-L800
- llm/server.go:102-107 (cap to n_ctx_train) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/server.go#L102-L107
- llm/llama_server.go:370-378 (-c NumCtx*numParallel, -np) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L370-L378
- discover/types.go:47-63 (CPU-only lists no devices) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/discover/types.go#L47-L63
- docs/context-length.mdx:7-12 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/docs/context-length.mdx#L7-L12
- Ours: backend/src/inference.rs:137, 470-509, 529-581; backend/src/llamaserver.rs:144-149

### 5. Flash attention default

**What they do.** OLLAMA_FLASH_ATTENTION unset: pass '--flash-attn auto' if every selected device supports it, else 'off'. Unsupported means CUDA compute < 6.0, compute 7.2, or driver < 7; CPU, Metal, ROCm and Vulkan count as supported; an empty device list (CPU-only) counts as supported. When set, 'on' or 'off' is forced. v0.24.0 instead enabled it per architecture (gemma3/gemma4 in the list) and disabled it for gemma4 on pre-Turing CUDA.

**What we do.** Automatic mode passes '--flash-attn auto'; manual passes on or off.

**Limitation for us.** None.

**Recommendation.** Nothing to change; both sides use llama.cpp's auto resolution on supported hardware. Confirm the resolved value in both logs (llama.cpp prints whether flash attention was enabled) so a pre-Pascal GPU or forced env var doesn't skew one side.

**Applies to desktop.** ignore - same behaviour already.

**Expected impact.** None expected on supported hardware.

**How to measure.** Look for the flash attention enabled/disabled line in each server's startup log.

**Category.** gpu-backend

**Confidence.** high

**Evidence.**

- llm/llama_server.go:597-623 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L597-L623
- ml/device.go:301-330 FlashAttentionSupported - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/ml/device.go#L301-L330
- envconfig/config.go:216 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/envconfig/config.go#L216
- llama.cpp common/common.h:499 flash_attn_type AUTO - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/common/common.h#L499
- v0.24.0 llm/server.go:193-223 and fs/ggml/ggml.go:890-908 - https://github.com/ollama/ollama/blob/v0.24.0/llm/server.go#L193-L223
- Ours: backend/src/llamaserver.rs:160-168; backend/src/inference.rs:637-644

### 6. KV cache type default (OLLAMA_KV_CACHE_TYPE)

**What they do.** The env var defaults to empty, so no --cache-type flags and llama-server's f16 default applies. When set (e.g. q8_0), Ollama lower-cases it and passes the same type for K and V. The docs say default f16. PredictServerVRAM (used only for scheduling and batch choice) always assumes f16 KV.

**What we do.** f16 K/V by default; q8_0 for both when tuning.kv_cache is q8_0 or fit_to_memory selects it.

**Limitation for us.** None.

**Recommendation.** If our fitter picked q8_0 on the test machine, either set OLLAMA_KV_CACHE_TYPE=q8_0 for Ollama or force f16 on ours. Quantized KV changes attention cost and memory.

**Applies to desktop.** ignore - parity control only.

**Expected impact.** Small decode-speed and quality differences when one side quantizes KV and the other does not.

**How to measure.** Compare the 'KV buffer size' and cache type lines in both logs.

**Category.** memory

**Confidence.** high

**Evidence.**

- envconfig/config.go:222 and 319 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/envconfig/config.go#L222
- llm/server.go:109-110 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/server.go#L109-L110
- llm/llama_server.go:394-397 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L394-L397
- llm/llama_server.go:2701-2717 PredictServerVRAM f16 assumption - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L2701-L2717
- docs/faq.mdx:354 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/docs/faq.mdx#L354
- Ours: backend/src/inference.rs:533-537, 575-577; backend/src/llamaserver.rs:156-159

### 7. mmap / mlock / load mode

**What they do.** use_mmap defaults to unset, and the scheduler then decides. It disables mmap ('--load-mode none') when running CPU-only (NumGPU==0, no GPUs, or all devices 'cpu'), on Windows with any CUDA device ('windows_cuda'), on Metal partial offload, and on Linux under host memory pressure. It uses '--load-mode dio' for Linux integrated CUDA/ROCm GPUs. There is no mlock option anymore; the Runner struct has no UseMLock. The compat layer can also turn mmap off for transformed tensors (gemma4 MoE gate/up fusion only).

**What we do.** No load-mode flag, so llama.cpp's default 'auto' applies (mmap unless a device lacks support).

**Limitation for us.** Effect on steady-state speed is not verified.

**Recommendation.** Expect Ollama's load_duration and peak RAM on CPU-only and Windows+CUDA to differ from ours because weights are read into anonymous memory instead of mapped. Do not compare cold-load times without noting this. Steady-state decode should be unaffected (inferred). If load time matters, measure ours with '--load-mode none' too.

**Applies to desktop.** ignore - no evidence it helps; it mainly avoids Windows CUDA pinned/mapped memory issues Ollama hit. Measure before adopting.

**Expected impact.** Load time and resident RAM differ, especially for large files. Token rates are unaffected (inferred).

**How to measure.** Ollama log 'disabling mmap for llama-server load by default reason=...'; compare 'CPU_Mapped' vs 'CPU' model buffer lines and load_duration on a cold request.

**Category.** memory

**Confidence.** high

**Evidence.**

- server/sched.go:1143-1182 applyLlamaServerMmapDefaults and disableMmapDefaultReason - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/sched.go#L1143-L1182
- server/sched.go:1205-1246 host pressure rule - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/sched.go#L1205-L1246
- llm/llama_server.go:625-640 appendLoadModeArgs - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L625-L640
- api/types.go:589-597 Runner options (no mlock) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/api/types.go#L589-L597
- llama/compat/llama-ollama-compat.cpp:1100-1102 and 001-llama-cpp-hooks.patch:17-19 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llama/compat/001-llama-cpp-hooks.patch#L17-L19
- llama.cpp common/arg.cpp:2718-2735 --load-mode (default auto) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/common/arg.cpp#L2718-L2735
- Ours: backend/src/llamaserver.rs:138-202 (no load-mode flag)

### 8. GPU layer count and memory fitting

**What they do.** Ollama does not compute layers. It omits -ngl unless num_gpu is set ('-ngl 0' for explicit 0) and lets llama-server's --fit (default on, n_gpu_layers -1, 1024 MiB target margin per device) place layers. Its own estimate, PredictServerVRAM = file size + f16 KV for ctx x parallel, is used only to pick a single GPU (predicted <= 80% of free VRAM, unless OLLAMA_SCHED_SPREAD), decide eviction, and choose auto batch size. For monolithic multimodal blobs it passes the same file as --mmproj. It adds the projector size + 1 GiB to LLAMA_ARG_FIT_TARGET, or disables projector offload when CPU-only, on partial offload, or when free VRAM < projector + 1 GiB. On OOM it retries once with the projector on CPU; gemma3n keeps the projector on GPU. The GPU list is refreshed per load; each GPU reserves 457 MiB minimum plus OLLAMA_GPU_OVERHEAD.

**What we do.** '--n-gpu-layers auto' (llama.cpp fit) in automatic mode after our own fit_to_memory picks context and cache type. --mmproj only when a separate projector path is configured. On CPU: '--device none --no-op-offload' (+ '--no-mmproj-offload').

**Limitation for us.** The effect of Ollama's projector padding on layer count is inferred, not measured.

**Recommendation.** On the GPU box, compare 'offloaded X/Y layers' and per-device buffer sizes from both logs before comparing speeds. For gemma4:e2b, Ollama also loads the embedded vision and audio encoders onto the GPU and pads the fit target, so it may offload fewer text layers than we do with the same free VRAM (inferred). Use a text-only run with the same projector setting on both sides, or none.

**Applies to desktop.** ignore - both defer to llama.cpp fit; the projector padding is specific to Ollama's monolithic blobs.

**Expected impact.** On VRAM-tight GPUs a few layers of difference can dominate decode speed.

**How to measure.** Compare 'offloaded N/M layers to GPU' and 'CUDA0 model/KV/compute buffer size' lines from both servers for the same model file and context.

**Category.** gpu-backend

**Confidence.** high

**Evidence.**

- llm/llama_server.go:403-411 (-ngl omitted by default) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L403-L411
- llm/llama_server.go:650-743 (mmproj offload rules, LLAMA_ARG_FIT_TARGET padding) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L650-L743
- llm/llama_server.go:865-897 compatClipArches (gemma3, gemma4 -> --mmproj = model file) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L865-L897
- llm/llama_server.go:1093-1125 projector CPU retry - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L1093-L1125
- llm/llama_server.go:2701-2717 PredictServerVRAM - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L2701-L2717
- server/sched.go:977-1081 single-GPU placement at 80% - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/sched.go#L977-L1081
- server/sched.go:620-630 and ml/device.go:111-116 (457 MiB minimum) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/ml/device.go#L111-L116
- llama.cpp common/common.h:473-481 (n_gpu_layers -1, fit_params true, 1 GiB target) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/common/common.h#L473-L481
- Ours: backend/src/llamaserver.rs:150-155, 188-200; backend/src/inference.rs:545-581

### 9. keep_alive and load_duration

**What they do.** OLLAMA_KEEP_ALIVE defaults to 5m; a negative value means forever, 0 means unload immediately; a request keep_alive overrides it. load_duration = time from handler start until the scheduler returns a runner. It includes a full model load on a cold request and a small scheduling overhead on warm ones. Changing any Runner option (num_ctx, num_batch, num_gpu, num_thread, use_mmap) between requests triggers a reload.

**What we do.** The model process stays up until model or config change; every model load is a fresh process.

**Limitation for us.** None.

**Recommendation.** Discard the first request per Ollama configuration, or report it separately as load. Set keep_alive to -1 (or long) during the run so a slow iteration doesn't unload the model. Keep options identical across requests to avoid silent reloads that show up as large load_duration.

**Applies to desktop.** ignore - harness guidance only.

**Expected impact.** A cold request inflates total_duration by seconds and can be mistaken for slow prefill.

**How to measure.** Check load_duration per response; it should be a few ms on warm requests.

**Category.** harness

**Confidence.** high

**Evidence.**

- envconfig/config.go:126-144 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/envconfig/config.go#L126-L144
- server/sched.go:517-520 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/sched.go#L517-L520
- server/routes.go:2436, 2638, 2817-2818 (checkpointStart/Loaded, LoadDuration) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/routes.go#L2817-L2818
- server/sched.go:1400-1420 needsReload option comparison - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/sched.go#L1400-L1420
- docs/faq.mdx:297-318 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/docs/faq.mdx#L297-L318

### 10. Sampling defaults and where the sampler runs

**What they do.** The sampler runs inside llama-server (C++). Ollama sends explicit sampling fields on every /completion or /v1/chat/completions call to its llama-server: temperature, top_k, top_p, min_p (no omitempty, so 0 is sent), repeat_penalty, repeat_last_n, frequency/presence penalty, typical_p, seed, cache_prompt:true. Values are layered: api.DefaultOptions (temp 0.8, top_k 40, top_p 0.9, min_p 0, repeat_penalty 1.0, repeat_last_n 64, num_keep 4, seed -1, num_predict -1 -> bounded to 10x num_ctx), then GGUF general.sampling.* keys, then Modelfile params, then request options. The gemma4:e2b params blob is {temperature:1, top_k:64, top_p:0.95}. Ollama's OpenAI-compatible /v1/chat/completions forces temperature=1.0 and top_p=1.0 when absent, and has no top_k/repeat_penalty fields. v0.24.0 defaulted repeat_penalty 1.1, and its Go engine sampler took only temperature/top_k/top_p/min_p/seed, with no repeat penalty.

**What we do.** We send temperature 0.7, top_p 0.9, top_k 40, repeat_penalty 1.1, cache_prompt true. min_p and repeat_last_n are not sent, so llama.cpp defaults apply (min_p 0.05, penalty_last_n 64). temperature 0 is sent for deterministic calls.

**Limitation for us.** Ollama rejects typical_p on new requests (errTypicalPUnsupported).

**Recommendation.** Use Ollama's native /api/chat (not /v1) and send identical options on both sides: temperature, top_k, top_p, min_p, repeat_penalty, repeat_last_n, seed. For speed benchmarks use temperature 0 or a fixed seed with identical num_predict/max_tokens so output lengths match. Otherwise decode timings compare different token sequences and lengths.

**Applies to desktop.** ignore - our sampling defaults are a product choice; this is parity.

**Expected impact.** Sampler cost itself is small. Different sampling produces different output lengths, which changes total_duration and eval averages.

**How to measure.** Compare eval_count vs our predicted_n per prompt; at temperature 0 with equal max tokens they should match or be very close.

**Category.** inference

**Confidence.** high

**Evidence.**

- api/types.go:1137-1164 DefaultOptions - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/api/types.go#L1137-L1164
- server/routes.go:130-160 option layering (GenerationDefaults, Modelfile, request) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/routes.go#L130-L160
- types/model/generation.go:32-42 general.sampling.* mapping - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/types/model/generation.go#L32-L42
- llm/llama_server.go:1403-1426, 1573-1592 (/completion body) and 2172-2189 (/v1/chat body) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L1573-L1592
- llm/llama_server.go:106-118 boundedNumPredict - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L106-L118
- openai/openai.go:672-696 (temperature/top_p forced to 1.0 when absent) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/openai/openai.go#L672-L696
- gemma4:e2b params blob: https://registry.ollama.ai/v2/library/gemma4/blobs/sha256:56380ca2ab89f1f68c283f4d50863c0bcab52ae3f1b9a88e4ab5617b176f71a3
- llama.cpp common/common.h:230-240 sampling defaults - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/common/common.h#L230-L240
- v0.24.0 api/types.go:1077-1093 (RepeatPenalty 1.1) and sample/samplers.go:130 - https://github.com/ollama/ollama/blob/v0.24.0/sample/samplers.go#L130
- Ours: backend/src/llamaserver.rs:633-664; backend/src/inference.rs:150-154

### 11. Thinking default and prompt rendering path for gemma4

**What they do.** gemma4:e2b's registry config sets renderer 'gemma4' and parser 'gemma4'. That makes usesOllamaRenderedChat true: Ollama renders the prompt in Go (renderer resolves to 'gemma4-small' for e2b/e4b) and calls llama-server /completion. llama-server is started with '--no-jinja --chat-template chatml' as a placeholder. The gemma4 parser reports thinking support, so the model has the thinking capability. /api/chat sets think=true when the request omits it, and /v1 leaves it at that default unless reasoning_effort is given ('none' turns it off). Thinking tokens count in eval_count. Models without a renderer, parser or Go template use llama-server's own jinja template via /v1/chat/completions.

**What we do.** Use the GGUF's jinja template in llama-server via /v1/chat/completions. We send chat_template_kwargs.enable_thinking only when RequestOptions.thinking is Some(..); otherwise the template default applies.

**Limitation for us.** The Go renderer and the GGUF jinja template are not guaranteed to produce identical token sequences.

**Recommendation.** For a fair speed test send "think": false to Ollama (or reasoning_effort 'none' on /v1) and enable_thinking false on ours, or both true. Compare prompt_eval_count with our prompt_n + cache_n for the same messages. If they differ, the two prompt renderings differ and prefill numbers are not like-for-like.

**Applies to desktop.** ignore - we already use the model's own template; nothing to copy.

**Expected impact.** With thinking on in Ollama and off in ours, Ollama's replies can be many times longer. total_duration comparisons become meaningless; per-token eval rate stays comparable.

**How to measure.** Check message.thinking in Ollama responses and eval_count; for rendering parity, use Ollama's debug render option vs llama-server /apply-template for the same messages.

**Category.** inference

**Confidence.** high

**Evidence.**

- server/routes.go:2605-2615 (think defaults to true for thinking-capable models) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/routes.go#L2605-L2615
- server/routes.go:2350-2369 chatModeForModel / usesOllamaRenderedChat - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/routes.go#L2350-L2369
- server/routes.go:2669-2672 and 2771-2784 (rendered path calls Completion) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/routes.go#L2771-L2784
- llm/llama_server.go:1-6 and 773-784 (--no-jinja --chat-template chatml) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L773-L784
- server/renderer_resolution.go:34-75 (gemma4-small for e2b) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/renderer_resolution.go#L34-L75
- model/parsers/parsers.go:85-88 (gemma4 parser hasThinkingSupport true) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/model/parsers/parsers.go#L85-L88
- server/images.go:437-441 capability from parser - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/images.go#L437-L441
- openai/openai.go:536-559 thinkFromReasoningEffort - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/openai/openai.go#L536-L559
- gemma4:e2b config blob (renderer/parser gemma4): https://registry.ollama.ai/v2/library/gemma4/blobs/sha256:c6bc3775a3fa9935ce4a3ccd7abc59e936c3de9308d2cc090516012f43ed9c07
- Ours: backend/src/llamaserver.rs:621-664

### 12. Speculative decoding, prompt caching and context shift

**What they do.** Speculative decoding is used only for MTP or draft models: '--spec-type draft-mtp|draft-dflash --spec-draft-n-max N'. It applies when the manifest has a draft layer, or the GGUF has nextn_predict_layers or qwen35 mtp.* tensors. Even then draft_num_predict is zeroed unless the manifest has a draft path or the user set it. No n-gram speculation, so none for gemma4:e2b. Prompt caching: every request sets cache_prompt:true with 1 slot. No --cache-reuse (llama.cpp default 0). llama.cpp defaults remain: --cache-ram 8192 MiB host prompt cache and 32 context checkpoints per slot. Ollama adds '--context-shift' (except deepseek2) and '--keep num_keep' (default 4); long prompts are truncated in Go keeping num_keep tokens.

**What we do.** '--spec-type ngram-simple' and '--cache-reuse 256' by default; cache_prompt true; no --context-shift.

**Limitation for us.** None.

**Recommendation.** Run the benchmark twice on our side: defaults, and with speculative 'off' plus cache_reuse off. The second run matches Ollama's engine work for gemma4. When ngram-simple is active, report draft_n/draft_n_accepted. Use fresh, non-repetitive prompts for decode speed, or the n-gram drafter will favour us on repetitive outputs.

**Applies to desktop.** ignore - our defaults were measured as beneficial; Ollama has no equivalent to port.

**Expected impact.** On repetitive outputs (code edits, rewrites) our ngram-simple can show much higher decode rates than Ollama; on novel prose little difference (per our docs).

**How to measure.** Our EngineTimings draft_tokens/draft_accepted; repeat with speculative off.

**Category.** performance

**Confidence.** high

**Evidence.**

- llm/llama_server.go:799-848 appendDraftArgs / hasMTPDraft - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L799-L848
- llm/llama_server.go:898-911 (MTP only if metadata says so) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L898-L911
- server/routes.go:139-160 (DraftNumPredict zeroed without a draft path) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/routes.go#L139-L160
- llm/llama_server.go:1576 and 2175 cache_prompt true - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L1576
- llm/llama_server.go:280-332 and 786-797 (truncation, --context-shift, --keep) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L786-L797
- server/sched.go:134-152 supportsContextShift - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/sched.go#L134-L152
- llama.cpp common/common.h:626-632 (n_cache_reuse 0, n_ctx_checkpoints 32, cache_ram_mib 8192) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/common/common.h#L626-L632
- Ours: backend/src/llamaserver.rs:176-183; backend/src/inference.rs:165-172, 583-592

### 13. CPU backend variants and runtime selection

**What they do.** The Windows x64 llama-server is built with GGML_BACKEND_DL=ON, GGML_NATIVE=OFF, GGML_CPU_ALL_VARIANTS=ON and GGML_OPENMP=ON (preset cpu_windows), using a MinGW compiler. ggml then builds x64, sse42, sandybridge, ivybridge*, piledriver*, haswell, skylakex, cannonlake, cascadelake, icelake, cooperlake*, zen4*, alderlake and sapphirerapids* (* = only when not MSVC). At startup ggml_backend_load_best scans for ggml-cpu-*.dll and loads the highest ggml_backend_score for the running CPU. For GPUs, Ollama sets GGML_BACKEND_PATH to the chosen GPU backend DLL and prepends its lib dirs to PATH.

**What we do.** Our models/bin ships the same 14 ggml-cpu-*.dll variants (alderlake, cannonlake, cascadelake, cooperlake, haswell, icelake, ivybridge, piledriver, sandybridge, sapphirerapids, skylakex, sse42, x64, zen4), loaded dynamically by the same mechanism.

**Limitation for us.** OpenMP runtimes differ (llvm-mingw libomp vs upstream's fetched OpenMP); thread-pool behaviour is not verified identical.

**Recommendation.** No action. Variant choice should be identical on the same CPU. Confirm by the 'load_backend: loaded CPU backend from ...ggml-cpu-<variant>.dll' line in both logs.

**Applies to desktop.** ignore - already equivalent.

**Expected impact.** None expected.

**How to measure.** Compare the load_backend lines in both startup logs.

**Category.** gpu-backend

**Confidence.** high

**Evidence.**

- llama/server/CMakePresets.json:4-31 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llama/server/CMakePresets.json#L4-L31
- llama.cpp ggml/src/CMakeLists.txt:487-518 variant list (b10969) - https://github.com/ggml-org/llama.cpp/blob/391fac16460f15233a7740550d858ac96df3419d/ggml/src/CMakeLists.txt#L487-L518
- llama.cpp ggml/src/ggml-backend-reg.cpp:480-546 ggml_backend_load_best scoring - https://github.com/ggml-org/llama.cpp/blob/391fac16460f15233a7740550d858ac96df3419d/ggml/src/ggml-backend-reg.cpp#L480-L546
- llm/llama_server.go:443-560 library paths / GGML_BACKEND_PATH - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L489-L523
- Local listing: local-llm-companion\models\bin\ggml-cpu-*.dll (14 files)

### 14. How Ollama reports timings and how to compare them with llama-server timings

**What they do.** Ollama copies llama-server's final timings object. prompt_eval_count = cache_n + prompt_n, so it INCLUDES prompt tokens served from cache. prompt_eval_cached_count = cache_n. prompt_eval_duration = prompt_ms, which covers only uncached tokens (the docs say 'time spent evaluating uncached prompt tokens'). eval_count = predicted_n (includes the first token and any thinking tokens). eval_duration = predicted_ms, measured from end of prompt to last token. In llama-server's own predicted_per_second, the first token is free: rate = (n_gen-1)/t_gen. Ollama's documented rate eval_count/eval_duration uses n_gen, a small upward bias (~1% at 100 tokens). load_duration = scheduling plus any model load. total_duration = handler start to final chunk. For format+thinking double passes, the second prefill is folded into eval_duration.

**What we do.** EngineTimings: prompt_tps = prompt_n/prompt_ms (excludes cache), predicted_tps = predicted_n/predicted_ms (same n vs n-1 bias as Ollama's documented formula), cached_tokens = cache_n, draft stats.

**Limitation for us.** prompt_ms also includes llama-server's per-request prompt setup time on both sides, so very short prompts give noisy prefill rates.

**Recommendation.** Compute Ollama prefill rate as (prompt_eval_count - prompt_eval_cached_count) / prompt_eval_duration, never prompt_eval_count/prompt_eval_duration. Compute decode rate as eval_count/eval_duration, which matches our predicted_tps formula exactly. Compare load_duration only on cold runs, and total_duration only when output lengths match.

**Applies to desktop.** ignore - our formulas already exclude cache; this is interpretation guidance.

**Expected impact.** On multi-turn benchmarks with cache hits, naive Ollama prefill rates can be overstated many times over.

**How to measure.** Log prompt_eval_cached_count; it should be 0 on the first turn and large on later turns.

**Category.** harness

**Confidence.** high

**Evidence.**

- llm/llama_server.go:1514-1527 promptEvalCount = CacheN + PromptN - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L1514-L1527
- llm/llama_server.go:1737-1746 and 2039-2045 final metrics mapping - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L1737-L1746
- api/types.go:557-565 Metrics - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/api/types.go#L557-L565
- server/routes.go:2785-2819 metrics assembly incl. structured-output fold-in - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/server/routes.go#L2785-L2819
- docs/api.md:100-110 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/docs/api.md#L100-L110
- llama.cpp tools/server/server-common.h:390-428 (t_prompt_ms, t_gen_ms, n_gen_steps = n_gen-1) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/tools/server/server-common.h#L390-L428
- llama.cpp tools/server/server-common.cpp:67-88 timings JSON - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/tools/server/server-common.cpp#L67-L88
- same code at Ollama's pin: https://github.com/ggml-org/llama.cpp/blob/391fac16460f15233a7740550d858ac96df3419d/tools/server/server-common.cpp#L86-L96
- Ours: backend/src/inference.rs:174-219

### 15. Why Ollama registry gemma3 blobs fail upstream with 'key not found in model: gemma3.attention.layer_norm_rms_epsilon'

**What they do.** Verified from source. Ollama's gemma3 converter (v0.6.0, when gemma3 shipped) writes gemma3.attention.layer_norm_rms_epsilon only for Gemma3ForCausalLM (1B). The multimodal branch (4B/12B/27B) omits it, writes no rope.freq_base / freq_base_swa at all, and embeds the SigLIP vision tower and projector in the same file under Ollama names (v.patch_embedding, v.position_embedding, v.post_layernorm, *.layer_norm1/2, *.mlp.fc1/fc2, mm.mm_input_projection, mm.mm_soft_emb_norm). The 1B branch uses non-standard nested keys gemma3.rope.local.freq_base / gemma3.rope.global.freq_base. Text tensor names match upstream (post_attention_norm, post_ffw_norm, attn_output), and the RMSNorm +1 shift is already baked in. Ollama's old Go engine read these keys with defaults (eps 1e-6, local base 10000, global base 1e6), so the files worked in Ollama. Upstream llama.cpp gemma3 reads LLM_KV_ATTENTION_LAYERNORM_RMS_EPS as REQUIRED and throws 'key not found in model'. hparams load before vocab and tensors, so this is the first error. Ollama's own llama-server now carries a compat hook that detects these files and fixes them in memory: it copies nested rope keys, injects eps 1e-6, freq_base 1e6, freq_base_swa 1e4, a chat template and linear x8 rope scaling for 131072-ctx files, truncates tokenizer arrays to token_embd rows, and hides v./mm. tensors from the text loader. It then passes the same file as --mmproj, where a clip translator adds clip.* keys, renames 10 tensor patterns and promotes the patch/position embeddings to F32.

**What we do.** Upstream llama-server b10809 with no compat layer, so these blobs fail at hparams load.

**Limitation for us.** I did not read a gemma3 registry blob header (rule: no model downloads). The exact key set of a specific gemma3 tag is inferred from converter source plus Ollama's detection comment ('all of them omit layer_norm_rms_epsilon').

**Recommendation.** Do not use Ollama registry blobs as benchmark inputs for our app. Use an upstream-format GGUF (+ separate mmproj) in both apps; Ollama's compat detection leaves such files untouched. If our app should accept Ollama blobs, detect them generically at metadata-read time and show a plain error naming the cause. Markers: v./a./mm. tensors inside the main model file, required per-arch keys missing, tokenizer.ggml.model inconsistent with tokenizer.ggml.pre.

**Applies to desktop.** redesign - a generic detector plus a clear message is cheap; in-memory translation like Ollama's would mean patching llama.cpp, which we avoid.

**Expected impact.** Explains the reported failure; the fix is choosing model files, not changing flags.

**How to measure.** Read the GGUF header (metadata only) of a gemma3 blob: look for missing gemma3.attention.layer_norm_rms_epsilon, and count v.*/mm.* tensors.

**Category.** correctness

**Confidence.** high

**Evidence.**

- v0.6.0 convert/convert_gemma3.go:77-110 (multimodal branch has no layer_norm_rms_epsilon or rope.freq_base; 1B uses rope.local/global) - https://github.com/ollama/ollama/blob/v0.6.0/convert/convert_gemma3.go#L77-L110
- v0.6.0 convert/convert_gemma3.go:113-141 tensor name replacements - https://github.com/ollama/ollama/blob/v0.6.0/convert/convert_gemma3.go#L113-L141
- later converter still omits eps in the multimodal branch: convert/convert_gemma3.go:131-146 and 202-217 at 6bba484f1a - https://github.com/ollama/ollama/blob/6bba484f1a8a68e862665604ea6396771807b4a7/convert/convert_gemma3.go#L131-L146
- v0.24.0 model/models/gemma3/model_text.go:81-83 (Go engine defaults) - https://github.com/ollama/ollama/blob/v0.24.0/model/models/gemma3/model_text.go#L81-L83
- llama.cpp src/models/gemma3.cpp:18 (required eps) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/src/models/gemma3.cpp#L18
- llama.cpp src/llama-model-loader.cpp:420-433 (required key throws) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/src/llama-model-loader.cpp#L420-L433
- llama.cpp src/llama.cpp:348-369 (hparams -> vocab -> tensors order) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/src/llama.cpp#L348-L369
- llama/compat/llama-ollama-compat.cpp:205-270 handle_gemma3 (main; +13 lines in v0.34.1) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llama/compat/llama-ollama-compat.cpp#L205-L270
- llama/compat/llama-ollama-compat.cpp:1857-1902 handle_gemma3_clip - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llama/compat/llama-ollama-compat.cpp#L1857-L1902
- llama/compat/README.md:1-11, 43-74, 81-83 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llama/compat/README.md
- llama/compat/001-llama-cpp-hooks.patch:13-42, 75-91 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llama/compat/001-llama-cpp-hooks.patch
- llama.cpp tools/mtmd/clip-impl.h:105-150 upstream mmproj names (v.patch_embd, position_embd, attn_out, ffn_down, ln1, mm.input_projection, mm.soft_emb_norm) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/tools/mtmd/clip-impl.h#L105-L150

### 16. Registry gemma4 blob: what differs from upstream (verified from the actual e2b blob header)

**What they do.** I read only the header of the gemma4:e2b blob sha256:4e30e266... (7,162,394,016 bytes). It has 55 KV and 2012 tensors. It DOES contain gemma4.attention.layer_norm_rms_epsilon = 1e-6 and every other key upstream gemma4 requires: sliding_window, sliding_window_pattern, shared_kv_layers, key/value_length_swa, embedding_length_per_layer_input, rope.freq_base 1e6, rope.freq_base_swa 1e4. So the rms-eps error should not occur for this blob. Differences: (1) tokenizer.ggml.model = 'llama' with tokenizer.ggml.pre = 'gemma4'. Upstream then picks the SPM vocab instead of gemma4 BPE; Ollama's compat notes special tokens then come out as raw text. (2) Tensors by prefix: 601 text (595 blk.* + 6 global), 658 v.*, 749 a.*, 4 mm.*. Upstream's text loader counts all 2012 file tensors, and done_getting_tensors throws 'wrong number of tensors; expected 2012, got <text count>'. (3) Audio names differ from upstream (a.pre_encode.out -> a.input_projection, mm.a.fc -> a.pre_encode.out, per-block ln1/ln2/layer_pre_norm/linear_pos renames). (4) Older MoE gemma4 blobs use ffn_gate_inp.per_expert_scale (renamed to ffn_down_exps.scale). Text tensor names otherwise match upstream (inp_gate, proj, post_norm, layer_output_scale, rope_freqs, per_layer_*). The current converter still writes tokenizer model 'llama' and embeds a./v./mm.

**What we do.** Upstream loader, no translation.

**Limitation for us.** The exact 'got N' count (about 601) is inferred from upstream counting rules, not observed.

**Recommendation.** Expect a tensor-count or tokenizer problem, not the rms-eps error, when loading this gemma4 blob upstream. Use an upstream gemma4 GGUF + mmproj for our app, and import that same GGUF into Ollama for the benchmark.

**Applies to desktop.** ignore - explanatory; the generic detector in the gemma3 finding covers it.

**Expected impact.** Loading this blob upstream fails or tokenizes wrongly. Benchmarking Ollama's blob against an upstream GGUF is a file-level confound.

**How to measure.** Run the same read-only header dump on any other blob before using it.

**Category.** correctness

**Confidence.** high

**Evidence.**

- Header dump (read-only, first bytes of scratchpad/dl/gemma4_e2b.gguf): <research>/gguf/gemma4_e2b_header.txt lines 1 (2012 tensors), 6 (layer_norm_rms_epsilon present), 50 (tokenizer.ggml.model llama), 52 (pre gemma4), 57 (prefix counts blk 595 / a 749 / mm 4 / other 6 / v 658), 62-64 (a.pre_encode.out, mm.a.fc)
- Registry manifest (model layer digest and size): https://registry.ollama.ai/v2/library/gemma4/manifests/e2b
- 6bba484f1a convert/convert_gemma4.go:71-73 (tokenizer model 'llama') and 116 (writes eps) - https://github.com/ollama/ollama/blob/6bba484f1a8a68e862665604ea6396771807b4a7/convert/convert_gemma4.go#L71-L116
- llama/compat/llama-ollama-compat.cpp:966-1103 handle_gemma4 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llama/compat/llama-ollama-compat.cpp#L966-L1103
- llama/compat/llama-ollama-compat.cpp:2539-2633 handle_gemma4_clip (audio renames, clip keys) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llama/compat/llama-ollama-compat.cpp#L2539-L2633
- llama.cpp src/models/gemma4.cpp:3-20 required keys - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/src/models/gemma4.cpp#L3-L20
- llama.cpp src/llama-vocab.cpp:1928, 1955-1956, 2084-2085 (llama -> SPM, gemma4 -> BPE) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/src/llama-vocab.cpp#L2084-L2085
- llama.cpp src/llama-model-loader.cpp:1385-1395 and llama-model-loader.h:241 (partial=false default) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/src/llama-model-loader.cpp#L1385-L1395
- llama.cpp src/llama-arch.cpp:429, 466, 549-556 per-layer tensor names - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/src/llama-arch.cpp#L549-L556

### 17. Can an Ollama blob be made loadable by adding metadata keys only?

**What they do.** Ollama itself does not rewrite files; it translates KV, tensor names and tensor visibility in memory inside its patched llama.cpp, and the README calls this short-lived. Upstream allows metadata overrides at load time: '--override-kv KEY=TYPE:VALUE' is applied before the key-exists check, so a missing required key can be supplied. Types int, float, bool and str are supported, and tokenizer.ggml.model is read through the same path. There is no upstream way to hide extra tensors: done_getting_tensors(partial=false) throws when tensors are left unclaimed.

**What we do.** No overrides, no rewriting.

**Limitation for us.** Case (b) is inferred and untested (no model loads allowed here). It also cannot be a hardcoded per-model rule in our code.

**Recommendation.** Verdict. (a) Blobs with embedded v./a./mm. tensors (gemma3 4B/12B/27B, gemma4): metadata-only fixes are NOT enough. The text model needs a new GGUF with only the text tensors (data copied unchanged, no requantization) plus metadata fixes. Vision/audio needs a separate mmproj GGUF with clip.* keys and renamed tensors (gemma3: 10 rename patterns; gemma4: audio renames; the F32 patch-embedding promotion Ollama cites is for Metal). (b) Text-only gemma3 1B blobs: inferred that '--override-kv gemma3.rope.freq_base=float:1000000,gemma3.rope.freq_base_swa=float:10000' (eps is already written) could make it load and be correct. Without the rope keys it loads but silently uses rope base 10000 for global layers, because upstream treats that key as optional with default 10000. (c) If tokenizer arrays are longer than token_embd rows, token_embd fails its shape check, so the arrays must be truncated too; that is metadata-level. For our product, prefer upstream-format files over building a converter.

**Applies to desktop.** redesign - if ever supported, it would be an explicit 'convert to upstream layout' step (streamed tensor copy), priced against simply pointing users to upstream GGUFs.

**Expected impact.** Avoids spending effort on a key-injection fix that cannot work for multimodal blobs.

**How to measure.** A header-only check: tensors with v./a./mm. prefixes in a main model file mean a metadata-only fix is impossible.

**Category.** correctness

**Confidence.** medium

**Evidence.**

- llama.cpp src/llama-model-loader.cpp:255-259 (override applied before key check) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/src/llama-model-loader.cpp#L255-L259
- llama.cpp common/arg.cpp:2962-2964 --override-kv types - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/common/arg.cpp#L2962-L2964
- llama.cpp src/llama-model-loader.cpp:1385-1395 tensor count check - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/src/llama-model-loader.cpp#L1385-L1395
- llama.cpp src/llama-model.cpp:1311-1313 (rope_freq_base optional, default 10000) - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/src/llama-model.cpp#L1311-L1313
- llama.cpp src/models/gemma3.cpp:39 token_embd shape uses n_vocab - https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/src/models/gemma3.cpp#L39
- llama/compat/llama-ollama-compat.cpp:250-266 (vocab truncation, skip v./mm.) - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llama/compat/llama-ollama-compat.cpp#L250-L266
- llama/compat/README.md:3-11 and 106-113 - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llama/compat/README.md#L106-L113
- same tensor check and required eps at Ollama's pin: https://github.com/ggml-org/llama.cpp/blob/391fac16460f15233a7740550d858ac96df3419d/src/llama-model-loader.cpp#L1391

### 18. Model file itself differs: Ollama's gemma4 blob is not a typical Q4_K_M GGUF

**What they do.** The gemma4:e2b blob is file_type 15 (Q4_K_M). But per_layer_token_embd.weight [8960 x 262144] is BF16 (about 4.7 GB of the 7.16 GB file), audio tensors are BF16 and vision tensors F16, all in one file that Ollama loads as both model and --mmproj. The runtime therefore reserves memory for the full vision and audio encoders even for text-only chats.

**What we do.** Load whatever GGUF the user selected, with a projector only if one is configured.

**Limitation for us.** How an imported GGUF is rendered in Ollama (Go renderer vs llama-server jinja) was not verified; see open questions.

**Recommendation.** Benchmark with byte-identical model files. Import the same upstream GGUF into Ollama ('ollama create' from a GGUF), or point both apps at the same file. Record file size and each tensor type group in the results. Differences in embedding precision and loaded encoders change RAM/VRAM and GPU placement, which masks runtime differences.

**Applies to desktop.** ignore - benchmark hygiene.

**Expected impact.** Likely GBs of RAM/VRAM difference and different layer placement on small GPUs (inferred).

**How to measure.** Compare the model buffer size lines and 'ollama ps' SIZE with our process memory for the same prompt.

**Category.** memory

**Confidence.** high

**Evidence.**

- <research>/gguf/gemma4_e2b_header.txt lines 37 (general.file_type 15), 70 (per_layer_token_embd type 30 = BF16), 72 (token_embd type 14 = Q6_K)
- Registry manifest model layer size 7162394016: https://registry.ollama.ai/v2/library/gemma4/manifests/e2b
- llm/llama_server.go:745-771 mmprojMemoryRequirement sums v./mm./a. tensors of the same file - https://github.com/ollama/ollama/blob/a43fad18b088095de20fbd7a8f0de50824cf5d27/llm/llama_server.go#L745-L771

### Open questions

- What thread count does Ollama v0.34.1's shipped llama-server.exe actually use on the test machine? Logical cores / 2 is inferred from its MinGW build; confirm from the 'system_info: n_threads' line in the Ollama server log. Also confirm our bundled llama-server.exe (only 9,216 bytes in models/bin) is the official MSVC-ABI build that counts physical cores.
- Does the test GPU machine have >= 23 GiB total VRAM? If so, Ollama's default context becomes 32768 and its automatic batch/ubatch becomes 1024, which changes both prefill and memory comparisons.
- Which gemma3 tag produced the reported error, and does that blob also have tokenizer arrays longer than token_embd rows? I did not read a gemma3 blob header, because of the no-download rule.
- Would the 1B gemma3 blob load and give correct output upstream with only --override-kv for rope freq_base/freq_base_swa? Inferred, untested.
- When an upstream-format GGUF is imported into Ollama v0.34.1 ('ollama create' from a GGUF), does the model get a Go renderer/parser (the /completion path) or use llama-server's jinja template (/v1/chat/completions)? x/create changed on 2026-09-15 and I did not trace it.
- How much VRAM do gemma4's embedded vision and audio encoders take when Ollama passes the blob as --mmproj on the GPU machine, and does the LLAMA_ARG_FIT_TARGET padding reduce offloaded text layers vs our run? Measure from the buffer-size and offload log lines.
- Performance differences between llama.cpp b10864 (Ollama v0.34.1) and b10809 (ours) for the benchmarked architecture were not reviewed.
- Does '--load-mode none' (Ollama's default on CPU-only and Windows+CUDA) change steady-state speed or only load time and RAM, given gemma4's lazily read BF16 per-layer embedding? Not verified.
- Method note: I fetched raw source files with curl into the scratch folder, instead of WebFetch, to get exact line numbers. Beyond that I only read the GGUF header of the partly downloaded blob and version info on the installer. Nothing was loaded, executed or built.

## Critic: gaps, unsupported claims, contradictions, what must be measured

### Unsupported or wrong claims

1. Mica: 'chat, routing and agent already share one prefix builder' (citing api.rs:1200-1206). Wrong for agent runs. run_agent builds its own system prompt with system_prompt_for(...) plus READ ONLY text and a Task turn (agent_runner.rs:1673-1686, 1719-1726). Only the history turns come from build_turns_budgeted. The api.rs:1200-1206 comment overstates what the code does. PERFORMANCE.md:141-148 only says routing shares the chat prefix and the completion review rides on the agent transcript.
2. Ollama report: 'Automatic mode passes --flash-attn auto' and 'Nothing to change; both sides use llama.cpp's auto resolution on supported hardware.' False for CPU runs: cpu_configuration sets flash_attn_auto=false and flash_attn=false, and policy flash_attention='off' (runtime_selection.rs:44-46, 72).
3. Mica: 'on each conversation switch the idle slot's state is copied into host RAM', blamed on cache_idle_slots=true. In b10809 (scratch server-context.cpp:2420-2430) the idle-slot save loop runs after the new task is launched and skips slots that are processing, so with --parallel 1 it saves nothing. RAM-cache saves do happen, but during slot selection, when the new prompt keeps under 50% of the slot's context or the slot is picked by LRU (server-context.cpp:1604-1647). The RAM-use effect probably holds; the mechanism does not.
4. Mica: 'Never save a slot holding image chunks: the server saves only get_text_tokens()'. That describes b10189. In b10809 the save path serialises slot->prompt.tokens with serialize() and returns ERROR_TYPE_NOT_SUPPORTED if that throws (scratch server-context.cpp:2549-2556). The design reasoning has to be re-derived for b10809.
5. Mica: the 8192-token checkpoint spacing 'means short chats on hybrid models re-read from the start each turn'. Weakened by b10809: prompt processing always breaks for a checkpoint at the last user message position (scratch server-context.cpp:3534-3540). The eviction rule removes only checkpoints from other tasks that sit within min_step of an earlier one (2311-2323). The real risk is narrower and still unmeasured.
6. Mica (Stop): 'the current micro-batch still finishes' and 'the remaining delay is small'. The server has no abort callback and llama_decode loops over every micro-batch of the logical batch (b10189 llama-context.cpp:1835-2013). So the whole logical batch finishes, 2048 tokens by default. On CPU that is about 20 s at ~104 tok/s, not one 512-token micro-batch.
7. Mica: 'Flash attention auto on GPU and off on CPU. ... All measured on our hardware.' The CPU 'off' setting is not the documented measured choice. PERFORMANCE.md chose variant D with FA auto (lines 52, 70-73), and the auto-vs-off comparison was contaminated by a background compiler (lines 57-58).
8. Mica: 'Measured on the Core Ultra 9 275HX: all 24 physical cores beat 8 P-cores by 40%' used to show the runtime default is 24. PERFORMANCE.md never records the server's reported n_threads (lines 43, 61 assume it). The '24 cores' row C is the contaminated one, and the 16-thread row gave 99 vs 104 tok/s prefill (lines 51, 55), which is within noise. The data does not show the default is 24 or that 24 beats 16.
9. Ollama report: 'Our binaries appear to be the official llama.cpp Windows release, built with clang targeting the MSVC ABI'. Not verified. models/bin has libomp.dll plus LICENSE-LLVM-OpenMP and 9,216-byte exe stubs with -impl DLLs. That fits an LLVM-built release but does not prove the build job or the ABI. The claim that decides the thread default stays inferred.
10. Harness: 'Our chat on_token sends every content delta as an SSE token event ... (backend/src/api.rs:2576-2580). Early stop only looks for ```tool (api.rs:2593-2597)'. The behaviour is right but the lines are not: api.rs:2570-2600 is now local-knowledge retrieval code. The handler is at api.rs:2680-2683 and should_stop at 2695-2700 (the file changed at 16:38).
11. Harness: 'Plain chat-mode conversations write no activity journal (api.rs:2292, 2621)'. Could not be checked at the cited lines because api.rs shifted. It needs re-anchoring before anyone relies on it.
12. Mica (reasoning cap): 'llama-server accepts per-request reasoning_budget_tokens ... applied when the template defines thinking end tags', and its evidence for b10809 is only DLL strings. It is verifiable in b10809 source (scratch server-common.cpp:1366-1376 reads reasoning_budget_tokens/thinking_budget_tokens and sets reasoning_budget_end_tags from chat_params.thinking_end_tags), but the report did not cite that source.
13. Mica (mmap): '--mlock or --no-mmap could help ... use b10809's --load-mode spelling: ... --direct-io is deprecated'. Incomplete: b10809 also deprecates --mlock in favour of '--load-mode mlock' (scratch common/arg.cpp:2693-2695), and warns when --load-mode is combined with the old flags (arg.cpp:882-884).

### Contradictions between reports

1. Mica vs Harness on the agent prefix. Mica says chat, routing and agent already share one prefix builder. Harness says the agent builds a different system prompt, so handing a chat to the agent re-reads everything. The code sides with Harness (agent_runner.rs:1673-1686).
2. Mica vs Ollama on our CPU flash attention. Mica says off on CPU. Ollama says our automatic mode passes auto and both apps use llama.cpp's auto. The code sides with Mica (runtime_selection.rs:44-46), while PERFORMANCE.md:70-73 sides with the Ollama reading, so the doc and code disagree too.
3. Mica vs Harness on where changing context goes. Mica: keep per-turn context (attachments, web results, hints) on the turn it was first sent with, stored with the message, so earlier bytes never change. Harness: build every request as identity, history, then one dynamic block (workspace snapshot, memory, reasoning preface, retrieval hints), then the latest user turn. Under Harness's layout each request diverges at the previous dynamic block, re-reading the last user and assistant turn. On SWA or hybrid models that relies on a checkpoint sitting at that spot. Mica's layout avoids the divergence but grows the history. The two designs cannot both be adopted.
4. Mica vs Harness on fixing token estimates. Mica: calibrate a per-conversation, per-model characters-per-token ratio from the last usage.prompt_tokens divided by characters sent. Harness: use the last reported prompt_tokens as a baseline plus a heuristic estimate of turns added or removed since, with /tokenize for big additions. Both replace chars/4, but they are different mechanisms and give different errors after compaction or pruning.
5. Mica vs Ollama on how sure we can be of our thread default. Mica states it as fact with high confidence ('runtime default: all physical cores', 24 on the 275HX). Ollama calls it probable (medium confidence) and lists as an open question whether our bundled llama-server.exe is an MSVC-ABI build that counts physical cores.
6. Both reports vs the code comment on the reasoning preface. Mica and Harness both say inserting it as a system turn at index 1 changes the bytes in front of the whole history. The code comment at api.rs:2508-2510 says this placement is 'so the cached prefix survives it'. Only the identity prompt survives, so the comment overstates.

### Gaps

1. DeepSeek Harness report never uses the X -> Y -> Z -> A pattern form the brief asked for. Every finding is prose, so no finding lays out a flow like request -> attempt -> retry -> commit.
2. Harness topics from the brief that are missing or only mentioned in passing: orchestration (the turn/step/attempt structure is never laid out as a flow), cancellation (not traced through tools or subprocesses; the clone has packages/subprocess/win32-process and packages/shell/pwsh-local, which are relevant to how our terminal.rs stops commands on Windows), concurrency beyond parallel tools (packages/subagent/*), observability and logging (packages/runtime-diagnostics/invariants), performance measurement (BENCHMARK.md and an in-repo benchmarks/ folder with session-open, long-session-browser, terminal-io, while our harness lives outside the repo per docs/PERFORMANCE.md:4-7), resources and sandboxing (packages/sandbox/sandbox-windows-acl), extensibility (packages/hooks/hook-protocol, mcp, skill), and model abstraction layering (llm vs llm-pi-ai vs llm-deepseek). None of these were read or cited.
3. Harness open question 'are the invariant checks on in production profiles' could have been answered from the clone. The checks live in a separate package, packages/runtime-diagnostics/invariants, which was not read.
4. Mica open question 1 (is b10809 the same as b10189?) can largely be answered from b10809 source the Ollama agent had already fetched into <research>/llamacpp (tree.json sha 5266f24da). Examples: reasoning_budget_tokens in server-common.cpp:1366-1376, slot save in server-context.cpp:2545-2575, checkpoint rules at 2311-2330 and 3534-3540, RAM prompt cache at 1590-1660. The Mica report relied on DLL strings and b10189 instead.
5. Mica's KV-formula fix covers only full_attention_interval and sliding_window. The gemma4 header the Ollama agent dumped also has gemma4.attention.shared_kv_layers=20 and key_length_swa/value_length_swa=256 vs 512 for global layers (<research>/gguf/gemma4_e2b_header.txt lines 5-11). models.rs:297-311 ignores all of these (a grep of models.rs and inference.rs for sliding_window/full_attention_interval/swa finds nothing), so the overestimate on gemma4-style models is bigger than the report describes.
6. Ollama report never works out the thread default for the target CPU. The Core Ultra 9 275HX has 24 logical = 24 physical cores (no hyperthreading). The MinGW fallback in llama.cpp common.cpp:116-147 (scratch copy) gives 24/2 = 12 threads for Ollama, vs 24 for an MSVC-ABI build like ours. That is a 2x thread gap on CPU and hybrid runs, not the 10-vs-14 example the report gives.
7. Ollama report misses flash-attention parity on CPU. Our CPU fallback forces --flash-attn off (runtime_selection.rs:44-46, 72, called from llamaserver.rs:1538 and runtime_selection.rs:158,166). Per the report, Ollama sends auto on CPU-only machines.
8. Ollama report does not cover GPU library choice on the RTX 5070 Ti Laptop (Blackwell, compute 12.0): cuda_v12 vs cuda_v13, and the driver filter at discover/runner.go:118-121. Ours ships cublas64_13/cudart64_13 (models/bin listing). A different CUDA major version between the two apps is a possible confound.
9. Ollama report does not cover integrated-GPU and Vulkan discovery on this hybrid laptop. Source shows Vulkan is on by default on Windows (docs/gpu.mdx:141-146). Integrated non-CUDA GPUs are dropped unless OLLAMA_IGPU_ENABLE is set (discover/runner.go:382-437). That matters because the default-context VRAM tier sums every listed GPU (server/routes.go:2033-2050). It needs a log check, not an assumption.
10. Ollama timing section covers only the llama-server timings fields. It never covers client-side time to first token, or Ollama overhead outside those timings (Go prompt rendering for gemma4, Go-side truncation and tokenization, the HTTP proxy hop). Those land in total_duration and user-visible latency but not in prompt_eval_duration.
11. api.rs changed during the audit: mtime 2026-09-16 16:38:52, after the research folders were created at 16:13-16:30. api.rs citations past about line 1250 in the Mica and Harness reports are now off by 25-100 lines. Examples: the chat on_token handler is now api.rs:2680-2683 (cited 2576-2580); the reasoning-preface insert is now 2512-2523 (cited 2412-2435); the chat tool-envelope example is now at 1568 (cited 1541-1545). Findings should be re-anchored by function name before anyone acts on them.
12. No report puts a number on Stop latency on CPU. The server has no abort_callback (grep of b10189 tools/server finds none), and llama_decode runs every micro-batch of the logical batch in one call (llama-context.cpp:1835-2013). A Stop during prefill therefore waits for the whole logical batch, 2048 tokens by default. At the documented ~104 tok/s CPU prefill (PERFORMANCE.md:52) that is up to about 20 s.
13. Documentation drift nobody flagged: PERFORMANCE.md:52 and 70-73 say the chosen CPU configuration keeps --flash-attn auto (variant D), but the code forces it off (runtime_selection.rs:44-46). The CPU FA row C used for that choice was also taken with a compiler running in the background (PERFORMANCE.md:57-58).
14. Manual-mode defaults hardcode hardware values that none of the reports caught: InferenceConfig::default has n_threads 8 and n_batch 512 (inference.rs:138-139), and manual mode passes requested.n_threads (inference.rs:652-656). On the target, 8 threads is the P-core-only choice that measured about 40% slower on prefill (PERFORMANCE.md:54). This conflicts with the owner's no-hardcoded-hardware rule.
15. Harness's capability finding never notes how loose the existing heuristic is. models.rs:357 sets tools = lower.contains("tool_call") || lower.contains("tools"), which matches any template that mentions the word 'tools'. That makes the proposed 'unknown rather than supported' demotion more necessary than the report suggests.
16. Benchmark parity gap: our request body never sends a seed or min_p (llamaserver.rs:633-651; cfg.seed exists but is unused there). The Ollama report's advice to send 'identical ... min_p, seed' on both sides cannot be followed without a code change. Our side stays at llama.cpp's default min_p 0.05, while Ollama sends min_p 0.
17. Mica's architecture comparison stops at linking. It does not compare fault isolation: a native crash takes down Mica's app process, while our llama-server child can crash and be retried on CPU (runtime_selection.rs:158-166). It also does not compare concurrency models (Mica's JNI mutex vs one --parallel 1 slot shared by chat, classify and agent).

### Must be settled by measurement

1. Thread default parity: start our llama-server and Ollama v0.34.1 on the target and read each startup log's 'system_info: n_threads = X (n_threads_batch = Y)' line. Expected if the inference holds: ours 24, Ollama 12. Then run CPU-only (ours --device none; Ollama num_gpu 0) with an 8K-prompt prefill and a 300-token decode at 8, 12, 16 and 24 threads. Keep flash attention fixed, do two passes in A-B-B-A order, plugged in, same Windows power mode.
2. CPU flash attention: same model, 8K context, CPU-only, --flash-attn off (our shipped CPU fallback) vs auto (the PERFORMANCE.md choice and Ollama's default). Measure prefill tok/s on the 8K prompt and decode tok/s on chat/edit/tooljson, both orders, with no background compiler.
3. Default context and batch parity on the 12 GB GPU. Confirm Ollama's 'vram-based default context total_vram=... default_num_ctx=...' log line and the '-c/-b/-ub' values in 'starting llama-server cmd='; expected 4096 and 512/512. Also confirm the Intel iGPU is logged as dropped. Then run both apps at matched num_ctx (8192 and 32768) and matched ubatch, comparing prompt_n/prompt_ms and predicted tok/s.
4. Micro-batch sweep: --ubatch-size 512, 1024 and 2048 (with --batch-size >= ubatch) on the ~8,100-token prefill prompt, on GPU full offload and CPU-only. Record prompt tok/s, the logged CUDA0 compute buffer MiB, and whether fit_to_memory's fitted context shrinks.
5. Load mode: default mmap vs '--load-mode none' (Ollama's default on Windows+CUDA) vs '--load-mode mlock', on one full-GPU and one hybrid-placement model. Record cold time to /health ready, peak private bytes, and steady decode tok/s while another process holds most free RAM.
6. Model-file confound: load Ollama's gemma4:e2b blob in Ollama and an upstream gemma4 E2B GGUF plus mmproj in both apps. From the logs compare 'offloaded N/M layers', CUDA0 model/KV/compute buffer MiB, and text-only decode at matched ctx, with the projector on and off.
7. KV estimate vs reality: for gemma4 (shared_kv_layers 20, SWA dims 256), one hybrid model with full_attention_interval, and one dense model, compare models.rs kv_bytes_per_token x n_ctx against the logged 'KV buffer size' and 'RS buffer size'. Also check whether those lines print at llama-server's default log level in b10809.
8. Prefix breakage in our app: log cache_n and prompt_n per request over five turns each of plain chat, chat with one attached document, one web-search turn, Reasoning toggled on turn 3, memory added mid-conversation, a code session where the agent creates a top-level file, and chat -> agent run -> back to chat. The last case tests the different agent system prompt and whether the RAM prompt cache restores the chat on return.
9. RAM prompt cache: switch between 4-5 long conversations on one loaded model. Record llama-server private bytes and the 'updating prompt cache' and 'prompt cache update took' log lines, plus cache_n on returning to each conversation, against fit_to_memory's RAM plan in hybrid placement.
10. Checkpoints on SWA and hybrid models: six short turns (<8,192 tokens apart) on gemma4 and on a hybrid model. Count checkpoint create and erase log lines, cache_n per turn, and checkpoint MiB, to see whether any turn re-reads from the start.
11. Stop latency on CPU: CPU-only, 8K prompt, press Stop 1 s into prefill. Measure ms until the server log shows the slot released or a new request starts processing, at the default --batch-size (2048) and at --batch-size 512. Predicted: up to ~20 s vs ~5 s.
12. Warm-up request: time to first token of the first message after a model load, with 8K and 32K history, on GPU and CPU, with and without a background max_tokens=1 request built from the same prefix.
13. Slot save and restore on b10809: /slots save for a text-only slot and for a slot that holds an image (does it error with not-supported?). Record restore t_ms vs cold prompt_ms for 8K and 32K histories, file bytes per token, on GPU and CPU.
14. Reasoning budget: a fixed reasoning prompt set with reasoning_budget_tokens unset vs a budget derived from max_tokens. Record completion_tokens_details.reasoning_tokens, time to first visible token, and answer correctness.
15. repeat_penalty 1.0 vs 1.1 with ngram-simple on the edit and tooljson prompts. Record draft_n_accepted/draft_n, decode tok/s, and a byte diff of the output against the expected rename.
16. Speculation parity: our app with --spec-type ngram-simple and --cache-reuse 256 vs both off, against Ollama, on novel prose and on a file rewrite at temperature 0 with equal max tokens. Confirm eval_count equals predicted_n.
17. Thinking and template parity for gemma4: Ollama with think=false vs ours with enable_thinking=false on identical messages. Compare prompt_eval_count with our prompt_n + cache_n to detect a Go renderer vs Jinja token difference before comparing prefill rates.
18. End-to-end latency parity: client-side ms from request send to first streamed content byte, and to the final chunk, for both apps. Compare with prompt_eval_duration/prompt_ms to size Ollama's out-of-engine overhead (Go render, tokenize, proxy).
19. GPU library on Ollama: read the 'inference compute' log line for the RTX 5070 Ti (library, CUDA variant v12 or v13, driver). If it differs from our CUDA 13 build, run one decode and prefill comparison with both on the same CUDA major version, or record it as a confound.
20. Image token cost: send the same photo resized to 768, 1024 and 1568 px through our shipped projector. Record the prompt_n delta, prompt_ms and VRAM on the GPU, to decide whether MAX_SIDE_PX=1568 should come from projector metadata.
21. UI streaming cost: a browser performance profile on the target during a ~1,400-token ngram-accelerated rewrite and a long prose reply. Count long tasks >50 ms, dropped frames and scripting time, with the current per-token setMsgs (App.tsx:668-680) vs an animation-frame batch.
