# Code review: third-party llama.cpp fork with MoE-offload patches (2026-09-16)

Source: https://github.com/thecodacus/llama.cpp, the 13 branches named `fable5/*` (the owner asked for
those). Read at these tips: `cpu-tensor-parallel` 10f8aff, `expert-prediction` 41630e7,
`prefetch-experts` 5e7f627, `host-register` 20f5994, `moe-expert-cache` cca05a3, `moe-cache-overlap`
1fcb831, `turboquant` 12a0202, `cuda-q2_0` b4125d7. Code only; nothing below is measured on our
hardware yet.

## How the branches fit together

They form one stack on the fork's integration line, last synced with upstream on **2026-07-24**. Our
pinned runtime (b10809, 5266f24) is from 2026-09-04, about six weeks newer, so porting means
re-applying the patches, not merging.

1. `host-register`: pin the mapped model file (1 commit).
2. `prefetch-experts`: upload offloaded expert tensors ahead of compute (2 commits and docs).
3. `turboquant`, `cuda-q2_0`, `turbo-*`: new KV-cache quantisation types and a Q2_0 weight format.
4. `moe-expert-cache`, `moe-cache-laguna`, `moe-cache-diagnostics`, `moe-cache-readme`: copies of the
   most-used experts kept in VRAM.
5. `moe-cache-overlap`: run CPU parts of the graph on a worker thread so the GPU works at the same time.
6. `cpu-tensor-parallel`: split every weight matrix by rows between GPU and CPU. This tip contains 1–5.
7. `expert-prediction`: predict next experts and swap them into VRAM at runtime. Built on 4–5, not on 6.

## Findings per patch

### 1. Pinned mapped weights (`GGML_CUDA_REGISTER_HOST=1`)

- After load it calls `cudaHostRegister` on the mapped file range that holds weights kept in RAM, so
  RAM→VRAM copies skip the driver's bounce buffer.
- **It does nothing on Windows.** `llama_mmap::register_host` is inside `#ifdef _POSIX_MAPPED_FILES`
  (it uses `sysconf` for the page size), and every other build returns 0.
- **Our pinned runtime already gets pinned weights without the patch:** load with mmap off.
  `make_cpu_buft_list` puts the GPU's host (pinned) buffer type first for weights kept in RAM.
  `llama_model_loader` swaps it for plain CPU memory only when mmap is on ("avoid using a host buffer
  when using mmap"). So `--load-mode none` (`--no-mmap`) gives pinned expert weights, at the cost of
  a full RAM copy that the OS cannot page out. This is what the "no-mmap is ~21% faster prefill"
  observation measures. The planned mmap-vs-none test covers this patch on Windows.

### 2. Expert prefetch (`GGML_SCHED_PREFETCH_EXPERTS=1`)

- Plain scheduler code in `ggml-backend.cpp`, portable to Windows, about 200 lines.
- **When it acts:** only when a split starts with `MUL_MAT_ID`, its weights are in host memory, and
  `tokens × experts_used ≥ 2 × n_expert`. For a 128-expert, top-8 model that is batches of 32 tokens
  or more, so prompt processing only; generation takes the stock path. It also needs a backend with
  async and events support. ~~The Vulkan backend does not advertise events, so there it stays off.~~
  **Corrected by verification:** Vulkan advertises both (ours and the fork's `ggml-vulkan.cpp`), so
  with the variable set the patch also runs on Vulkan, untested. It needs `GGML_CUDA_NO_PEER_COPY`
  off, which is the default.
- **What it does:** stock code reads the routing ids back from the GPU (a sync) and uploads only the
  experts in use. The patch skips that sync and uploads the whole expert tensor on a second CUDA
  stream into staging slots, ordered with events, so uploads for the next tensor run during compute
  of the current one.
- **Cost:** VRAM for the slots, 3 by default (up to 8), each the size of the largest offloaded expert
  tensor: exactly 472.5 MiB (3 × 157.5 MiB) on the 30B test model. Slots are allocated during the
  first large batch, outside anything a memory fit sees. If allocation fails, the patch turns itself
  off (or continues with 2 slots) and the stock path runs. But slots that do allocate can starve later
  CUDA pool growth and abort (see Verification), so our fit margin would have to include them.
- **Correctness:** it points the staging tensor at a slot only for the length of one split launch
  and restores it afterwards. The second commit fixes a use-after-free in the first. Output is
  claimed token-identical, and the design supports that (same kernels, same data).
- **Depends on pinned sources?** An async copy from pageable memory is effectively synchronous, so
  most of the overlap should need item 1, which on Windows means mmap off. Verification: the source
  code does not settle this, so it is measured with a 2×2 test (mmap/none × prefetch off/on).
- Author's number: RTX 3060, pp2048 at ubatch 2048, `-ncmoe 26`: 1,143 → 1,880 tok/s with 1+2.

### 4. Expert cache (`--moe-cache-profile`, `--moe-cache-slots`)

- **Static, not the owner's dynamic-swap idea.** A one-time trace per model (`llama-moe-trace`)
  records which experts the router picks. At load, the S most frequent experts of each CPU-side layer
  are copied into a VRAM "pack". The RAM copies stay, so RAM use does not drop. Each MoE layer then
  runs two chains that are added together: hot experts on the GPU pack, and the rest on the original
  tensors, with ids remapped (`-1` = not mine).
- **Architectures:** only `qwen35moe`, `deepseek2` and `laguna` pass the layer to `build_moe_ffn`.
  **Our 30B test model is `qwen3moe`, so the cache would not engage on it.** Wiring it is a one-line
  change in `src/models/qwen3moe.cpp`: the graph check (separate gate/up/down, SILU, no scales)
  otherwise fits that architecture.
- **Touches CUDA kernels:** it adds support for `-1` ids to `mul_mat_id` (mmvq, mmf, mmq, fallback).
  The commit log records a large-batch mmq fault with sparse ids that "resists isolation". It was
  routed around (skip-flagged ops never use mmq/mmf at large batches), not explained. That is a
  stability risk for a desktop app that does not control prompt sizes.
- **Memory:** the pack is allocated after load, so the fit knows nothing about it. The author's own
  guidance is to leave about 900 MB free beyond the pack, or the first large prompt can crash on CUDA
  pool growth. An app integration would have to size slots inside our fit.
- **"Bit-identical":** hot experts run on GPU kernels and cold ones on CPU kernels, which is not the
  same arithmetic as all-CPU decode. Expect token-identical in practice, not guaranteed identical
  bits.
- Author's numbers (RTX 3060, `-ncmoe 99`): decode +21% on a 256-expert model, +44% on a 64-expert
  model, +5% when VRAM fits under 15% of the experts. Bigger experts gain more per slot.

### 5. Async CPU splits (`--sched-async-cpu`, **on by default in that branch**)

- A worker thread computes a CPU split while the main thread launches later splits that do not read
  its outputs. It joins before any split that reads a CPU tensor or is itself a CPU split.
- Useful only with the expert cache, where the hot GPU chain and the cold CPU chain of one layer are
  independent. Author: +4–5% with speculative decoding, about ±2% without.
- **For any A/B on a fork build, the baseline must pass `--sched-async-cpu 0`,** or "patches off"
  still differs from stock.

### 6. CPU as a tensor-parallel device (`--cpu-tp`)

- Only `common/` code, about 125 lines. It adds the CPU to the device list for `--split-mode tensor`
  and sizes the row split from free VRAM minus a margin (default 512 MiB), turning off the fitter.
  Stock `--device` rejects the CPU, which is why a patch is needed. The tensor split mode itself is
  upstream (our pin has `LLAMA_SPLIT_MODE_TENSOR`), and our dense test architectures are not on its
  unsupported list.
- Relevant to **dense** models that overflow VRAM (where we measured the layer-spill cliff), not to
  MoE. It replaces "some layers entirely on CPU" with "every matrix partly on CPU", which also means
  activation transfers in every layer. No numbers are published; it needs measuring.

### 7. Expert prediction (`LLAMA_MOE_SPEC_ROUTER`, `--moe-cache-ring`)

- The closest match to the owner's idea: a ring of VRAM slots refilled at runtime from a "speculative
  router" that runs each layer's router on the pre-attention input.
- **Not ready:** it covers `qwen35moe` only, all 13 commits are dated 2026-09-10, and there are no
  published results. The fix commits describe experts landing on the GPU "as a scramble of tensors"
  and a bug that marked arbitrary experts resident ("fast, and wrong"). Revisit when it settles.

### 3. TurboQuant KV types and Q2_0

- New KV-cache and weight quantisation types. They change output quality, so they are not the
  lossless placement work in scope here. Recorded as a future item next to C2 (weight formats).

## What this means for the app

| Patch | Windows | Needs | Candidate for us |
| --- | --- | --- | --- |
| Pinned mapped weights | no-op | — | Use stock mmap off instead (test planned) |
| Expert prefetch | works (CUDA; also switches on for Vulkan, untested) | slot VRAM in the fit; whether it needs pinned sources is to be measured | **Test**: prompt speed on the 30B MoE |
| Expert cache | works | qwen3moe wiring; per-model trace; fit integration | Test after prefetch; stability risk noted |
| Async CPU splits | works | expert cache | Only together with the cache |
| CPU tensor-parallel | works | tensor split mode on the architecture | Test on an overflowing dense model |
| Expert prediction | — | qwen35moe; unfinished | Not now |
| TurboQuant / Q2_0 | — | quality trade-off | Future, with C2 |

Test approach (proposed, not yet approved): one separate build of the `cpu-tensor-parallel` tip,
outside `runtime/bin`. Every comparison is patch off vs on **inside that build**, since the fork's
base is older than our runtime. If a patch wins, port it to our pinned commit and measure again there
before adopting it.

## Verification (2026-09-16, three independent read-only checks)

The owner approved three agents to try to disprove the claims above (pinned memory, prefetch, expert
cache with async CPU). They read our pinned source, the fork tip and the test model's GGUF header;
nothing was built or run. Results:

**Confirmed**
- Patch 1 does nothing on Windows: `_POSIX_MAPPED_FILES` can only come from `unistd.h`, which no
  Windows toolchain here has, and defining it would break the Windows mmap build.
- In our pinned runtime, with mmap off, CPU-side weights, including experts forced to the CPU by
  `-ncmoe`, `-cmoe`, `-ot …=CPU` or the fit's rules, land in pinned `CUDA_Host` memory. With mmap on,
  the loader swaps that for mapped, pageable `CPU_Mapped` memory.
- Prefetch's switch-on conditions are as described, and no llama-bench/llama-server eval callback turns
  it off. Exact slot cost for the test model: the largest offloaded expert tensor is a Q6_K
  `ffn_down_exps` of 157.5 MiB, so 3 slots are **472.5 MiB**.
- The expert cache is wired only for `qwen35moe`, `deepseek2` and `laguna`. The qwen3moe wiring is one
  line, and the test model meets every condition: separate gate/up/down tensors, no scales or biases,
  SILU, no clamp.

**Corrections and additions**
- Vulkan: see the correction in patch 2. Prefetch switches on there too.
- **The fork with every feature off is not stock.** Its CUDA `mul_mat_id` paths do extra work on every
  call (zero-row kernels, extra memsets) whether or not ids are `-1`. It also defaults to async CPU
  splits, has TurboQuant edits in the flash-attention templates, and is based on a six-week-older
  upstream. Every comparison must stay inside the fork build, and a winner must be measured again
  after porting.
- **Async CPU splits affect every model**, not only the cache: even a model fully on the GPU has a CPU
  split (the token embedding lookup), so each batch pays a thread handoff. It is on by default for
  any program built against the fork's library. Baselines need `--sched-async-cpu 0` /
  `--no-sched-async-cpu`.
- **Expert cache:**
  - Packs are built, and the success line logged, even on an architecture whose graph never uses
    them. Proof that the cache is really in the graph is the `graph nodes` count, not the load line.
  - llama-bench has no cache flags, only the `GGML_MOE_CACHE_PROFILE`/`GGML_MOE_CACHE_SLOTS` env
    variables, and it feeds **random tokens**, so routing does not match a real-text profile. The
    cache must be judged on real prompts through llama-server.
  - The dual chains also run during prompt processing, using the slower general gather path.
    Contrary to the commit message, this is not "decode only", so prompt speed must be measured too.
  - Per-slot cost: 130.78 MiB × (packed layers ÷ 48). For example, 32 slots with all 48 layers'
    experts on the CPU is about 4.1 GiB.
  - The fork tip lacks the diagnostics commit its README describes: no "max slots" hint, just "pack
    allocation failed".
  - `-1` ids exist only in the CPU and CUDA kernels, yet the cache accepts any GPU device (a Vulkan
    hazard). LoRA adapters are ignored for packed layers.
  - With hot experts on GPU kernels, output is not bit-identical: judge by greedy-token agreement.
- **Prefetch at small batches:** it always uploads whole tensors, while stock uploads only the experts
  in use. At 32–64-token batches it could be slower, so a micro-batch sweep is needed, not just
  2,048. It logs nothing when it engages; evidence is VRAM rising by ~472.5 MiB after the first large
  batch. A slot allocation that succeeds can still starve later CUDA pool growth (abort), and on
  Windows the driver may spill into system RAM instead of failing.
- **Porting prefetch to our runtime is not a clean apply:** our upstream now lets CUDA graphs capture
  these splits and skips the data-pointer check on graph reuse, so a rotating slot pointer could be
  read stale. A port needs fixed slots per split, or graphs disabled for them, plus identity checks.
- **Test traps found:**
  - Never pass `--no-host`: Q4_K experts then go to `CPU_REPACK`, which is not a host buffer, and
    that silently disables both op offload and prefetch.
  - `-lm mlock` means mmap+mlock in the fork but no-mmap in our runtime, and the fork has no `auto`.
    Use only `none`/`mmap`.
  - llama-bench separates `-ot` rules with `;` (commas make sweeps).
  - llama-bench `-fitt` silently discards `-ncmoe`/`-ot`.
  - llama-server with `-ot` and the default `--fit on` runs a dry fit first: use `-fit off`.
  - A failed pinned allocation falls back silently, so every pinned run must show
    `CUDA_Host model buffer size` in its log (llama-bench `-v`, llama-server `-lv 4`).
  - The fork's llama-bench output adds a `sched_async_cpu` column: parse by name.
