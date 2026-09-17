# Audit benchmarks — 2026-09-16

Measurement record for the September 2026 performance audit. Research leads are in
`docs/research/2026-09-16-runtime-harness-research.md`; this file holds what was measured.
Updated as phases complete; the llama-bench/llama-server batch finished on 2026-09-16.

## Machine and conditions

- Intel Core Ultra 9 275HX: 8 performance + 16 efficiency cores, 24 logical processors, no SMT.
  Performance cores are logical processors 0, 1, 10–13, 22 and 23 (interleaved).
- 63.4 GB DDR5-5600. NVIDIA GeForce RTX 5070 Ti Laptop GPU, 12,227 MiB, compute capability 12.0,
  driver 591.91; the desktop holds about 1.7 GB of VRAM before any model loads.
- Windows power plan "Turbo", on AC power.
- Runtime: llama.cpp b10809 (commit 5266f24da), official Windows build (clang), CUDA 13,
  runtime-loaded backends. CPU module selected at startup: `ggml-cpu-alderlake` (AVX2 + AVX-VNNI),
  the best this processor supports (it has no AVX-512).
- Ollama 0.34.1 (which runs llama.cpp b10864 internally).
- CPU temperature could not be measured: the only sensor readable without administrator rights is an
  ACPI thermal zone that read a constant 27.9 °C throughout, so it is not a CPU sensor. GPU
  temperature and power come from `nvidia-smi`.

## Method

- One model resident at a time: every test starts with no llama-server, llama-bench or Ollama model
  loaded and stops what it started.
- `llama-bench`, 512-token prompt and 128 generated tokens unless stated. Each test runs 6 repeats
  (after llama-bench's own warm-up); the **first repeat is discarded** by rule, and the table reports
  the **median** of the remaining 5 (p10, p90 and mean are in the raw results).
- Telemetry sampled throughout: GPU utilisation, VRAM, temperature and power every 500 ms; CPU
  utilisation and available RAM every second.
- Noise: two runs of the identical configuration (24 threads, "auto" vs explicit) differed by about
  7% in generation speed. Differences under ~10% need interleaved A-B-B-A runs before they count.

## CPU threads (CPU only, `--device none`)

Prompt / generation tokens per second, median of 5.

| Threads | 2B (Gemma 2 2B Q4_K_M) | 4B (Gemma 3 4B Q4_K_M) |
| --- | --- | --- |
| 1 | 43 / 6.5 | 27 / 4.4 |
| 2 | 88 / 11.8 | 55 / 7.8 |
| 4 | 166 / 20.2 | 103 / 13.3 |
| 8 (scheduled by Windows) | 273 / 28.1 | 157 / 19.3 |
| 8, pinned to performance cores | 204 / 28.0 | 155 / 15.4 |
| 12 | 312 / 32.0 | 190 / 19.8 |
| 16 | 371 / 34.1 | 240 / 25.0 |
| 24 (llama.cpp default here) | 441 / 35.9 | 283 / 25.2 |

- All 24 physical cores is fastest for both prompt and generation on this processor.
- Pinning to the performance cores is slower (4B generation 15.4 vs 19.3 unpinned at 8 threads).
- From 16 to 24 threads, prompt speed rises 18–19% but generation only 1–5%, while CPU use rises
  from about 68% to 95%. A setting that keeps generation threads lower and prompt threads high loses
  little speed and leaves the machine responsive; this motivated the calibration profiles.

## CPU micro-batch (4B, 1,024-token prompt)

| `--ubatch-size` | 128 | 256 | 512 | 1024 |
| --- | --- | --- | --- | --- |
| Prompt tok/s | 198 | 246 | 268 | 263 |

The runtime default (512) is best on CPU.

## Ollama vs this app's runtime

Same Gemma 3 4B weights in both (Ollama's registry file; llama.cpp given a copy with only the
language-model tensors and the metadata Ollama supplies in memory). Raw prompt of about 475 tokens,
128 generated tokens, temperature 0, prompt cache defeated per repeat.

| Setup | Ollama gen / prompt | llama.cpp (ours) gen / prompt |
| --- | --- | --- |
| CPU, each runtime's defaults | 19.8 / 185 (CPU 52%) | 24.3 / 275 (CPU 100%) |
| CPU, both 8 threads | 18.5 / 143 | 18.4 / 144 |
| GPU, defaults | 148 / 5,535 | 146 / 5,502 |

- At matched settings the engines are identical: Ollama runs llama.cpp's server.
- With defaults our runtime is 23% faster at generation and 48% faster at prompt processing on CPU,
  because Ollama's Windows build uses about half the cores (CPU 52%).
- On the GPU they are equal; Ollama held about 0.9 GB more VRAM (its file includes the vision tower).
- Conclusion: the inference engine is not why the app felt no faster than CPU-only Ollama. See the
  GPU matrix and app-pipeline sections.

## GPU matrix

`llama-bench`, flash attention on, f16 cache. GPU layers from `llama-fit-params` at the tested context
(−1 = every layer). Depth = tokens already in the cache when measuring.

| Model | File | Layers on GPU (empty / 16K) | Generation tok/s (empty / 16K) | Prompt tok/s (empty) | VRAM peak (empty) |
| --- | --- | --- | --- | --- | --- |
| 2B | gemma-2-2b-it Q4_K_M | all / all | 204 / 149 | 11,900 | 3.5 GB |
| 4B | gemma-3-4b Q4_K_M | all / all | 148 / 121 | 7,879 | 4.7 GB |
| 8B | Qwen3-8B Q4_K_M | all / all | 85 / 58 | 3,937 | 6.6 GB |
| 12B | gemma-4-12b-it Q4_K_M | all / all | 60 / 51 | 2,326 | 9.3 GB |
| 14B | Qwen coder 14B | all / **41 of 48** | 51 / **13** | 2,185 | 10.3 GB |
| 25B | Magistral Small UD-IQ2_M | all / 38 of 40 | 43 / 20 | 1,101 | 9.6 GB |
| 27B | Qwen3.8-27B IQ2_S (hybrid attention) | **all 65** / 63 of 65 | **36.6** / 26.0 | 927 | 11.1 GB |

- **The hybrid cliff.** 14B at 16K: seven layers on the CPU cut generation from 51 to 13 tok/s.
- **The app's memory planner was wrong for the 27B.** It classed this model as hybrid ("about 26%
  of weights on the CPU") and cut a 32K request to 16K, from a header formula that counts every layer
  as full attention. llama.cpp's own fit places all 65 layers on the GPU at short context and 63 of
  65 at 16K with an unquantised cache. Fixed: loads now size the context with the runtime's fit
  (`runtime_fit.rs`).

## GPU offload sweep (empty context)

Generation / prompt tok/s, median of 5. "GPU present" = 0 layers offloaded but the GPU device left
enabled; "pure CPU" = `--device none`. CPU use is the mean during the test.

| Layers on GPU | 8B (36 layers) | 12B (48 layers) | 14B (48 layers) |
| --- | --- | --- | --- |
| Pure CPU | 14.7 / 123 (CPU 96%) | 8.5 / 76 | 7.4 / 66 |
| 0, GPU present | 11.1 / 1,052 | 8.2 / 583 | 7.0 / 543 |
| 25% | 16.5 / 1,303 (9) | 11.5 / 771 (12) | 10.6 / 701 (12) |
| 50% | 21.6 / 1,597 (18) | 14.6 / 1,019 (24) | 13.4 / 887 (24) |
| 75% | 34.6 / 2,293 (27) | 21.3 / 1,435 (36) | 19.9 / 1,213 (36) |
| All but 2 | 64.2 / 3,180 (34) | 43.2 / 2,023 (46) | 36.5 / 1,849 (46) |
| All | **88.7 / 3,830** (CPU 8%) | **58.4 / 2,422** | **50.1 / 2,151** |

- Generation speed is set by the slowest part: the last two layers on the CPU cost 25–28% on every
  model, half the layers on the CPU cost about 75%.
- With the GPU present but no layers offloaded, prompts get 7–9× faster but generation is slower
  than pure CPU (every token pays CPU↔GPU transfers). The app's CPU fallback disables this path.
- CPU use stays at 74–91% whenever any layer runs on the CPU, even when only two do: the worker
  threads spin-wait (`--poll 50`) for the GPU half of each token. Relevant to the Light profile.
- Partial offload is a last resort. A smaller context or an 8-bit cache that keeps every layer on
  the GPU is almost always faster.

## Flash attention and cache precision (8B, all layers on the GPU, 16K tokens already in the cache)

512-token prompt and 128 generated tokens measured after a 16,384-token fill (`-d 16384`).

| Flash attention | Cache | Prompt tok/s (p10–p90) | Generation tok/s (p10–p90) | VRAM peak |
| --- | --- | --- | --- | --- |
| off | f16 | 748 (701–782) | 50.7 (47.5–51.4) | 10.3 GB |
| off | q8_0 | **fails: "failed to create context"** | — | — |
| on | f16 | 2,131 (2,109–2,183) | 55.4 (51.8–58.8) | 8.9 GB |
| on | q8_0 | 2,083 (2,003–2,094) | 57.8 (55.3–58.0) | 7.8 GB |

- Flash attention on a long context: prompt processing 2.85× faster, generation +9%, 1.4 GB less VRAM.
  It should stay on wherever the backend supports it.
- The 8-bit cache saved another 1.1 GB at 16K for no measurable speed cost (prompt −2%, generation
  +4%, both inside the noise). That is why the runtime fit falls back to q8_0 before letting a layer
  spill to the CPU: one layer on the CPU costs far more than the cache precision.
- **An 8-bit cache without flash attention cannot create a context** in this llama.cpp build (the
  quantised value cache needs the flash-attention kernel). Manual mode currently allows that pairing
  (flash attention switched off plus an 8-bit cache), which would fail to load with an unhelpful
  message. Listed as a settings fix.

## GPU micro-batch (8B, 2,048-token prompt, `-b 2048`)

| `--ubatch-size` | 256 | 512 | 1024 | 2048 |
| --- | --- | --- | --- | --- |
| Prompt tok/s | 3,592 | 3,843 | 3,916 | 3,855 |
| VRAM peak | 6.6 GB | 6.8 GB | 7.1 GB | 7.7 GB |

The spread above 512 is 2%, inside the noise, while every doubling adds 0.3–0.6 GB of compute buffer
that would otherwise hold context. The default 512 stays.

*Re-measured 2026-09-17 01:18–01:55 with a longer prompt (4,096 tokens + 128 generated, runs 512 / 1,024 /
1,024 / 512, 2-minute breaks). Higher is faster.* Prompt reading at 512 → 1,024: 4B 7,445 / 7,489 → 7,705 /
7,714 (+3.3%); 8B 3,680 / 3,679 → 3,717 / 3,724 (+1.1%); 12B 2,346 / 2,328 → 2,368 / 2,329 (+0.5%). Generation
unchanged (4B ~150, 8B ~92, 12B ~58.5). VRAM +0.31 to +0.70 GB. **Updated decision (balance rule, night decision
24):** a load takes 1,024 when its fit probe at 1,024 still keeps every layer on the GPU with the chosen margin,
so the gain comes without eating into the margin.

## Speculative decoding (`--spec-type ngram-simple`, the app's default)

llama-server, raw prose prompt of about 475 tokens (a repeated paragraph, so the text has n-grams to
copy), 200 generated tokens with end-of-sequence ignored, temperature 0, prompt cache off, 6 repeats
with the first discarded. "Drafted" is the server's own `draft_n` / `draft_n_accepted` per repeat.

| Model and placement | Off: generation tok/s (p10–p90) | ngram-simple: generation tok/s (p10–p90) | Repeats that drafted anything |
| --- | --- | --- | --- |
| 8B, all layers on the GPU | 88.7 (88.0–90.9) | 92.0 (87.2–112.7) | 3 of 5 (5/5, 60/60, 25/25 accepted) |
| 4B, CPU only | 20.1 (19.8–20.5) | 23.8 (23.0–34.7) | 1 of 5 (96 of 127 accepted) |

- When the output repeats text already in the context, drafting pays: the repeats that drafted are
  the fast tail (p90 112.7 on the GPU, 34.7 on the CPU).
- When nothing matches it costs nothing measurable: the non-drafting 8B repeats sit at the "off"
  speed.
- The 4B CPU medians differ by 18% although four of the five ngram repeats drafted nothing, so that
  gap is drift between runs (the "off" run followed straight after a GPU test), not a speed-up.
  Needs interleaved runs before any claim.
- Not measured: a rewrite or code-edit task, where the output copies large spans of the input and
  the gain should be largest. The default stays on.
- The CPU server figure (20 tok/s) is below `llama-bench`'s 25 tok/s for the same model because this
  test generates 200 tokens after a 475-token prompt (the cache is deeper) and goes through the HTTP
  server; the 8B GPU server figure matches `llama-bench` (88.7 both).

## GPU/hybrid optimisation track (2026-09-16 evening, complete 20:07)

Harness: `bench_gpu.py` (session scratch). One model at a time. Server tests run every configuration
twice (forward, then reversed order), each start with a discarded warm-up, 3 measured requests per
task per pass (6 in total). Every test waits for AC power and is redone if the power source changed:
a power cut at 18:58 invalidated the first attempt, which was discarded whole. Power during the runs
below: GPU mean 123 W, peak 140.7 W (full limit 140 W).

### The 27B's built-in draft head (multi-token prediction)

The 27B GGUF carries one next-token prediction layer (`qwen35.nextn_predict_layers = 1`, block 64).
llama-server loads it only with `--spec-type draft-mtp` (log: "creating MTP draft context against the
target model"); otherwise it reports the tensors as unused. All layers on the GPU, 8K context, flash
attention on, temperature 0. Prose: 200 tokens continuing a ~475-token prompt. Rewrite: a 60-line
Python class rewritten with type hints and docstrings (763 tokens).

| Drafting | Prose tok/s (p10–p90) | Rewrite tok/s (p10–p90) | Accepted / drafted (prose; rewrite) | Same output as no drafting (prose; rewrite) | VRAM peak |
| --- | --- | --- | --- | --- | --- |
| Off | 34.9 (33.5–36.0) | 35.3 (33.9–35.7) | — | — | 10.7 GB |
| n-gram (`ngram-simple`, app default) | 36.0 (35.3–37.9) | 62.3 (61.2–62.8) | 67/397; 2,442/4,032 | 5/6; 6/6 | 10.7 GB |
| Draft head, 1 token | 48.6 (48.0–50.9) | 52.0 (51.2–52.1) | 550/638; 2,250/2,328 | 3/6; 6/6 | 11.3 GB |
| Draft head, 2 tokens | 52.4 (50.7–58.2) | 62.2 (61.9–62.3) | 708/962; 2,982/3,192 | 3/6; 6/6 | 11.5 GB |
| Draft head, 3 tokens (server default) | **53.7** (51.3–61.4) | 69.1 (69.0–69.3) | 785/1,213; 3,348/3,708 | 3/6; 6/6 | 11.6 GB |
| Draft head, 3 tokens + n-gram | 52.9 (50.6–55.2) | **88.3** (87.4–88.7) | 788/1,539; 3,804/5,940 | 2/6; 6/6 | 11.6 GB |

- The draft head makes prose **1.54×** faster; the app's n-gram drafting gives prose almost nothing.
  Draft head + n-gram makes rewrites **2.5×** faster.
- Cost: ~0.9 GB more VRAM (the draft layer and its cache) and prompt reading 6–8% slower
  (795 → 750 tok/s prose prompt). On a model that already overflows at the user's context, the VRAM
  cost has to be weighed against layers spilling to the CPU: not yet measured.
- **Output is not always byte-identical at temperature 0.** Rewrites matched 6/6 everywhere; prose
  diverged in 3 of 6 with the draft head and 1 of 6 with n-gram alone. Verification runs the model on
  batches of different size than one-token decoding, and tiny floating-point differences flip
  near-tied tokens in low-confidence text. The app's Settings copy ("Output is identical") overstates
  it and must be corrected; answer quality is not expected to change, but that is not measured.

### Dense model spilling over: 14B at 16K

Owner rule: hybrid placements are tested only on models that overflow the GPU (or come within 15%)
at the context under test. Check before the phase: every layer on the GPU at 17,152 tokens needs
11,671 MiB of 11,026 MiB free (106%). `llama-bench`, 512-token prompt and 128 generated tokens after
16,384 tokens in context, flash attention on, 6 repeats (first dropped). Placements from
`llama-fit-params` probes.

| Placement | Layers on GPU | Generation tok/s (p10–p90) | Prompt tok/s | VRAM peak | CPU use |
| --- | --- | --- | --- | --- | --- |
| llama.cpp fit, f16 cache, default 1,024 MiB margin (what loads get today) | 41 of 48 | 13.1 (12.9–13.9) | 790 | 10,980 MiB | 70% |
| Fit with 512 MiB margin | 44 | 17.5 (16.9–18.6) | 789 | 11,519 MiB | 66% |
| Fit with 256 MiB margin | 45 | 19.9 (19.9–20.1) | 792 | 11,587 MiB | 63% |
| q8_0 cache, fit's own choice | 48 (output layer on CPU) | 24.8 (23.3–25.9) | 960 | 10,949 MiB | 62% |
| **q8_0 cache, every layer and the output on GPU** | all | **32.7 (32.1–32.9)** | **965** | 11,198 MiB | 9% |
| Feed-forward weights of 16 layers on CPU (`-ot`), rest on GPU | all attention | 19.4 (18.9–19.4) | 769 | 10,678 MiB | 63% |
| KV cache in RAM (`-nkvo`), all weights on GPU | all | 3.6 (3.6–3.7) | 663 | 9,518 MiB | 9% |

- The best placement is **2.5×** what a load gets today: 8-bit cache with everything on the GPU. It
  fits with 863 MiB of headroom by the tool's projection (inside the 1,024 MiB target, which is why
  the fit tool does not choose it), and the measured peak stayed 1 GB under the card's total.
- The 1,024 MiB fit target is costly: the fit tool's own q8_0 choice left the **output layer** on the
  CPU and lost 24% (24.8 vs 32.7). llama-server applies the same target at load (`--fit` default).
- Feed-forward-only offload (keep attention and KV on GPU) is not better than whole layers with a
  smaller margin: not worth adopting.
- KV in RAM is never worth it: attention over a 16K cache then runs on the CPU.
- No run showed Windows spilling into shared GPU memory (all peaks under 11.7 GB of 12.2 GB).

### The 27B at the owner's 32K context (hybrid attention, 65 blocks incl. the draft head)

Check before the phase: every layer on the GPU at 32,768 tokens needs 11,134 MiB of 10,876 MiB free
(102%). llama-server at `--ctx-size 32768`, flash attention on, drafting off. "Start" = 128 tokens
after a ~475-token prompt (1 warm-up + 5); "12.7K in" = 128 tokens after a 12,699-token prompt that
stays cached (only 4 tokens re-read per request).

| Placement | Layers on GPU | Start gen tok/s (p10–p90) | 12.7K-in gen tok/s | 12.7K fill tok/s | VRAM peak | CPU use |
| --- | --- | --- | --- | --- | --- | --- |
| llama.cpp fit, f16, 1,024 MiB margin (what loads get today) | 57 | 18.8 (18.4–19.6) | 16.1 | 734 | 10,837 MiB | 92% |
| Fit, f16, 512 MiB margin | 59 | 21.9 (21.3–22.2) | 20.0 | 756 | 11,159 MiB | 90% |
| Fit, f16, 256 MiB margin | 61 | 25.5 (25.3–26.0) | 22.2 | 802 | 11,327 MiB | 88% |
| q8_0 cache, 1,024 MiB margin | 62 | 27.1 (26.3–27.3) | 22.2 | 797 | 10,630 MiB | 87% |
| **q8_0 cache, 512 MiB margin: every layer** | all | **36.2 (36.0–36.3)** | **33.1** | 877 | 11,066 MiB | 7% |
| Feed-forward of 24 layers on CPU | all attention | 16.9 (16.7–17.1) | 16.3 | 708 | 10,250 MiB | 83% |
| KV cache in RAM | all | 15.0 (14.8–15.1) | 8.2 | 723 | 9,810 MiB | 100% |

- Same conclusion as the 14B: **q8_0 with every layer on the GPU is fastest (1.9× at the start, 2.1×
  deep)** and matches the model's speed with no context (36.6). Here the 512 MiB margin was needed
  for the last layers; with the default margin q8_0 alone reached 62 layers (27.1).
- Every layer on the CPU costs a lot: 57 → 61 layers moved generation 18.8 → 25.5 (4 layers, +36%).
- Feed-forward-only offload and KV-in-RAM are the slowest; neither is adopted.
- **Owner rule adopted from these results:** the automatic plan takes whatever gives the highest
  output speed: every layer on the GPU through the cache type and fit margin (1,024 → 512 → 256 MiB)
  if any combination achieves it, otherwise the most layers on the GPU. *Superseded later that night
  by the balance rule (see HANDOFF "Speed rule"): fastest generation first, then the fastest prompt
  reader that gives up at most 5% of generation for at least 5× the gain. For these placements the
  choice is unchanged, because every layer on the GPU was faster at both.*

### Mixture-of-experts: Qwen3-30B-A3B Q4_K_M (48 blocks, 128 experts, 8 used per token)

18.56 GB file. Check before the phase: every layer on the GPU needs 18,209 MiB of 11,026 MiB free at
4K (165%) and 19,433 MiB at 16K (176%). `llama-bench`, 512-token prompt, 128 generated tokens,
flash attention on, 6 repeats (first dropped). "Experts of N blocks on CPU" = `-ngl 99 -ncmoe N`
(llama.cpp's expert switch covers the combined `gate_up` tensor too: its pattern is
`ffn_(up|down|gate|gate_up)_(ch|)exps`). "Fit's own" = the `-ot` rules `llama-fit-params` printed.

| Placement | Empty context: gen tok/s (p10–p90) | prompt | VRAM | 16K in context: gen tok/s | prompt | VRAM |
| --- | --- | --- | --- | --- | --- | --- |
| CPU only | 25.5 (24.6–27.1) | 195 | — | not run (16K fill per repeat on CPU too slow) | | |
| Whole layers split (26 / 24 on GPU) | 48.5 (47.4–49.3) | 775 | 10.6 GB | 14.0 (13.2–14.4) | 607 | 10.7 GB |
| All experts on CPU, every layer on GPU | 41.0 (40.3–41.5) | 471 | **2.2 GB** | 33.4 (31.0–33.9) | 494 | **3.7 GB** |
| Experts of 24 / 28 blocks on CPU (fewest that fit) | 65.6 (64.6–67.5) | 775 | 10.5 GB | 47.7 (46.6–48.6) | 668 | 10.7 GB |
| Experts of 32 / 36 blocks on CPU | 50.3 (48.4–51.0) | 639 | 7.8 GB | 40.3 (39.5–40.3) | 573 | 7.9 GB |
| Experts of 40 blocks on CPU | 44.4 (43.6–44.9) | 542 | 5.1 GB | — | | |
| **llama.cpp fit's own (experts of ~25 / ~28 blocks on CPU)** | **66.0 (61.4–66.5)** | **945** | 10.6 GB | **48.2 (47.3–48.4)** | **819** | 10.8 GB |

Threads at the fit's own placement (empty context): 8 → 56.7, 16 → 59.3, 24 → 66.5 tok/s generation
(prompt 896 / 900 / 927). All cores stay fastest.

- **Keeping experts in RAM beats splitting whole layers: 1.36× at an empty context, 3.4× at 16K.**
  Only the 8 experts a token picks run on the CPU, and attention with its growing cache stays on the
  GPU; a whole-layer split puts attention over the 16K cache on the CPU.
- llama.cpp's own fit already chooses this and was the fastest; fewer expert blocks on the CPU is
  monotonically faster (24: 65.6, 32: 50.3, 40: 44.4).
- All experts on the CPU runs a 30B model in 2.2–3.7 GB of VRAM at 33–41 tok/s: relevant for small
  GPUs. Even CPU-only (25.5) beats the dense 8B on CPU (14.7): only ~3B parameters are read per token.
- The owner's "swap hot experts into VRAM" idea was answered by this: moving weights across the bus
  per token was not tested (llama.cpp has no such mode), but the reverse placement it motivated is the
  fastest option measured.
- App: `runtime_fit` now recognises the fit's expert rules (`Fit::ExpertsOnCpu(blocks)`) and, per the
  owner's output-speed rule, picks the cache/margin combination with the fewest expert blocks on the
  CPU (written 2026-09-16, not yet compiled).

### Micro-batch under partial offload (14B, 4,096-token prompt after 13,056 tokens)

| Placement | `-ub` 512 | 1,024 | 2,048 | 4,096 |
| --- | --- | --- | --- | --- |
| 41 layers on GPU | 827 | 948 | 973 | 977 |
| 41 layers, weight copy for big batches off (`--no-op-offload`) | 116 | — | 125 | — |
| Feed-forward of 16 layers on CPU | 809 | — | 1,013 | — |

- Under partial offload a larger micro-batch reads prompts **15–18% faster** (512 → 2,048) at no
  measured VRAM cost in these runs; fully on the GPU the gain was 2% (earlier table).
- The default "copy weights to the GPU for big batches" (op offload) is what makes hybrid prompt
  reading usable: turning it off is 7× slower. It stays on.

## Project-built runtime vs the official release (2026-09-16, 20:29)

`runtime/bin` built by `scripts/build-runtime.ps1` from the same commit (b10809, 5266f24) as the
official release previously in `models/bin`: MSVC 19.44 for all modules, CUDA Toolkit 13.4 (default
architecture list), Vulkan SDK 1.4.357. `llama-bench`, 6 repeats (first dropped), runs interleaved
A-B-B-A.

| Test | Project build (runs 1, 4) | Official release (runs 2, 3) |
| --- | --- | --- |
| 8B on GPU (CUDA0): generation tok/s | 91.3, 89.9 | 89.3, 89.2 |
| 8B on GPU: prompt tok/s | 4,025, 3,958 | 3,726, 3,944 |
| 4B CPU only: generation tok/s | 25.9, 26.3 | 25.9, 25.6 |
| **4B CPU only: prompt tok/s** | **256, 256** | **272, 279** |

Vulkan vs CUDA on the same GPU, project build: generation 88.5 / 89.0 (Vulkan) vs 94.7 / 92.4 (CUDA),
prompt 3,582 / 3,592 vs 4,100 / 4,028. CUDA is 5% faster generating and 13% faster reading prompts.
With no device named the build uses CUDA0 only (95.4 tok/s generation; the fit's memory breakdown
lists only CUDA0): the same GPU is not used twice through Vulkan, and the integrated GPU the Vulkan
backend also sees is not used.

- GPU: parity (project build equal or marginally faster).
- CPU generation: parity. **CPU prompt processing: the MSVC-built CPU modules are 7% slower** than the
  official clang-built ones, consistently in both runs. Owner decision: install Visual Studio's clang
  tools and rebuild before removing `models/bin` (the build script prefers clang-cl when present and
  keeps `cl.exe` as CUDA's host compiler).

## 30B MoE: n-gram draft length (2026-09-16, 22:57–23:33)

llama-server, the placement from the fit at 8K, flash attention on, mmap. Full rewrite (a 62-line module,
~670 tokens out) and prose (200 tokens), two passes forward and reversed, warm-up discarded, 3 requests
per task per pass, 2-minute breaks between server starts. N-gram drafting drafts nothing when the length
(`--spec-ngram-simple-size-m`) is shorter than the lookup (`--spec-ngram-simple-size-n`, default 12), so
short lengths were tested with an equally short lookup. **Higher is faster.**

| Lookup / length | Full rewrite tok/s (p10–p90) | Prose tok/s | Rewrite accepted / drafted | Same output as off (rewrite; prose) |
| --- | --- | --- | --- | --- |
| off | 60.2 (58.1–60.9) | 53.7 | — | — |
| 4 / 4 | 77.8 (77.1–78.8) | 57.2 | 2,214 / 2,832 | 0/6; 1/6 |
| 8 / 8 | 88.3 (87.3–88.9) | 57.5 | 2,196 / 2,640 | 0/6; 1/6 |
| 12 / 12 | 88.4 (86.1–88.8) | 58.9 | 2,040 / 2,448 | 0/6; 6/6 |
| **12 / 24** | **90.6 (89.1–91.1)** | **58.9** | 2,118 / 3,024 | 0/6; 6/6 |
| 12 / 48 (llama.cpp default, the app today) | 63.6 (63.2–64.1) | 57.6 | 2,160 / 4,032 | 0/6; 5/6 |

- On this model with experts in RAM, the default length gives rewrites only +6%. Length 24 gives +50%,
  42% faster than the default, and prose is even: long rejected drafts cost big verification batches
  whose experts run partly on the CPU.
- An earlier run that set `--spec-draft-n-max` (ignored by n-gram drafting) and used a prompt the model
  answered with stubs is superseded by this table.

## 30B MoE at 16K: which cache and margin the placement search should take (2026-09-17 00:57–02:20)

30B MoE, project runtime, load without mmap, 16,384 tokens already in context (`-d 16384`), 512-token prompt and
128 generated tokens, 6 repeats (first dropped), placements from `llama-fit-params` at each setting, runs in
alternating orders with 2-minute breaks (GPU 76–78 °C). Two runs per arm. **Higher is faster.**

| Arm | Expert blocks in RAM | Generation | Prompt reading | VRAM peak |
| --- | --- | --- | --- | --- |
| llama.cpp default: f16 cache, 1,024 MiB margin | 29 | 49.3 / 46.6; 49.3 / 49.4 | 1,190 / 1,188; 1,209 / 1,213 | 10.9–11.1 GB |
| f16, 256 MiB margin | 26 | **52.7 / 51.8** | 1,230 / 1,116 | 11.7 GB |
| q8_0, 1,024 MiB margin | 26 | 44.5 / 44.5 | 1,254 / 1,254 | 11.0 GB |
| q8_0, 256 MiB margin (the app's search before the fix) | 24 | 40.8 / 41.4 | 1,049 / 970 | 11.6–11.7 GB |
| same + micro-batch 1,024 | 25 | 43.1 / 42.3 | 1,178 / 1,174 | 11.6 GB |
| f16, 1,024 MiB margin, micro-batch 1,024 | 29 | 48.2 / 48.9 | 1,191 / 1,192 | 11.0 GB |

- **The 8-bit cache costs this model ~10% of generation** at 16K, although it frees room for three more expert
  blocks on the GPU. On the dense 8B, whose layers were already all on the GPU, q8_0 was neutral.
- **A smaller margin with f16 gains ~6%** (three fewer expert blocks in RAM). Prompt reading was noisy (+1.8% and
  −7.7% on the two passes).
- The app's search had taken q8_0 at 256 MiB (fewest expert blocks), which was **14% slower** than the default.
  Fixed (night decision 25): expert placements keep whole layers first, then f16 among combinations keeping them,
  then the fewest expert blocks.
- A larger micro-batch cannot help a 512-token prompt, which fits one micro-batch; its gain shows on long
  prompts (+46% at 4,096 tokens, earlier table).

## The 27B's draft head on the project runtime, at 8K and at the owner's 32K (2026-09-17 02:25–03:32)

llama-server, project runtime, two passes forward/reversed, warm-up discarded, 3 requests per task per pass,
2-minute breaks. Prose: 200 tokens after a ~475-token prompt. Rewrite: full 62-line module. Long prompt: ~4,096
tokens, 16 generated. **Higher is faster.**

**8K, every layer on the GPU (f16):**

| Drafting | Prose | Rewrite | Prompt reading, short | Prompt reading, 4,096 tokens |
| --- | --- | --- | --- | --- |
| n-gram (default length 48) | 37.2 | 72.2 | 841 | 930 |
| n-gram length 24 | 37.5 | 68.5 | 851 | 933 |
| **draft head + n-gram** | **52.8** | **101.6** | 781 | 890 |
| draft head + n-gram length 24 | 53.7 | 96.0 | 787 | 896 |

**32K (q8_0 cache; every layer fits only without the head):**

| Setup | Prose | Rewrite | Prompt reading | Generation 12.7K deep | Fill 12.7K |
| --- | --- | --- | --- | --- | --- |
| **A n-gram, every layer on the GPU (512 MiB fit)** | **37.2** | **66.9** | **780** | **33.9** | 901 / 898 |
| B draft head, 59 of 65 layers (fit with 1,024 MiB reserved for the head) | 32.2 | 50.4 | 455 | 22.0 | 735 / 503 |
| C draft head, every layer on the GPU | server ran out of memory (connection reset, then time-out) | | | | |

- With room for it (8K) the draft head is +42% on prose and +41% on rewrites, for prompt reading −7.1% on short
  prompts and −4.3% on long ones. Adopted (the owner's exception to the 5% limit).
- At 32K it costs six layers and makes everything slower (generation −13% to −35%, prompt reading −42%).
  **App decision (night decision 26):** the head is used only when every layer still fits on the GPU with its
  reserve at the requested context; otherwise the load keeps n-gram drafting and says why.
- N-gram length 24 does not help a model fully on the GPU, with or without the head (consistent with the
  8B/12B table).

## N-gram draft length on models fully on the GPU (2026-09-16 23:41 – 09-17 00:30)

Same method as the 30B table above (llama-server, 8K, every layer on the GPU, flash attention on, full rewrite
and prose, 2 passes, 2-minute breaks between starts, 5 minutes between models). **Higher is faster.**

| Model | Drafting | Full rewrite tok/s (p10–p90) | Prose tok/s | Rewrite accepted / drafted |
| --- | --- | --- | --- | --- |
| 8B | off | 87.5 (86.3–87.9) | 88.4 | — |
| 8B | 12 / 12 | 157.6 (156.3–160.0) | 91.1 | 1,968 / 2,304 |
| 8B | 12 / 24 | 163.3 (158.1–164.4) | 91.0 | 2,040 / 2,880 |
| 8B | 12 / 48 default | **167.0** (166.0–168.3) | 91.7 | 2,082 / 3,744 |
| 12B | off | 54.4 (54.0–54.6) | 55.3 | — |
| 12B | 12 / 12 | 94.4 (93.6–95.5) | 55.1 | 2,412 / 3,096 |
| 12B | 12 / 24 | 99.0 (93.0–99.9) | 55.6 | 2,520 / 3,456 |
| 12B | 12 / 48 default | **100.7** (100.3–101.2) | 55.4 | 2,574 / 4,608 |

- Fully on the GPU a rejected long draft is cheap, so the default length is best (by 1.7–2.2% on rewrites).
  N-gram drafting itself nearly doubles rewrite speed (8B 87.5 → 167; 12B 54.4 → 100.7).
- **App decision (night decision 20):** length 24 when the placement keeps weights in RAM, where it was
  +42% on the 30B MoE; the default when every layer is on the GPU.

## Clang-built runtime vs the official release (2026-09-16, 21:25–21:35)

`runtime/bin` rebuilt the way the official Windows release is built: clang 19.1.5 (GNU-style driver,
llama.cpp's `x64-windows-llvm` toolchain file) for the CPU modules and tools, MSVC 19.44 for the CUDA
and Vulkan modules only. Same commit (b10809). `llama-bench -p 512 -n 128`, 6 repeats (first dropped),
interleaved A-B-B-A. **Higher is faster.**

| Test | Clang runtime (runs 1, 4) | Official release (runs 2, 3) |
| --- | --- | --- |
| 8B on GPU: prompt tok/s | 4,043, 4,019 | 3,888, 4,017 |
| 8B on GPU: generation tok/s | 91.5, 91.5 | 89.3, 90.1 |
| 4B CPU only: prompt tok/s | **275, 273** | 270, 278 |
| 4B CPU only: generation tok/s | 25.1, 24.7 | 25.3, 24.9 |

Vulkan vs CUDA on the clang runtime: prompt 3,449 / 3,362 vs 3,991 / 4,214; generation 89.5 / 88.3
vs 90.0 / 93.0.

- **CPU prompt reading is back to parity** (the MSVC build was 256, 7% slower). GPU unchanged. Per the
  owner's decision, `models/bin` and the MSVC runtime went to the Recycle Bin.

## 30B mixture-of-experts on the stock runtime: load mode and micro-batch (2026-09-16, 21:35–21:43)

Qwen3-30B-A3B Q4_K_M, flash attention on, placement from `llama-fit-params` at the batch sizes each
test uses (default 1,024 MiB margin). `llama-bench`, 6 repeats (first dropped). The buffer type was
checked in every run's log. **Higher is faster.**

**Load mode** (2,048-token prompt at `-b/-ub 2048`, then 128 generated tokens; order mmap, none, none,
mmap):

| Load mode | Where the CPU-side experts live | Prompt tok/s | Generation tok/s |
| --- | --- | --- | --- |
| mmap (default) | `CPU_Mapped`, pageable file mapping | 2,079, 2,160 | 55.2, 59.3 |
| **none** | `CUDA_Host`, pinned copy (9.3 GB of RAM) | **3,126, 3,179** | 60.9, 59.3 |

- Loading without mmap reads prompts **~48% faster** at the same generation speed: RAM→GPU copies of
  the experts for large batches run from pinned memory. This is the stock equivalent of the fork's
  "pinned mmap" patch, which does nothing on Windows (see the fork review).
- Cost: the CPU-side weights are copied into RAM that cannot be paged out. Start time is measured
  separately (below, when done).

**Micro-batch** (4,096-token prompt). "Own fit" = each value with the placement that fits at that
micro-batch, plus 128 generated tokens. "Fixed" = one placement that fits at 4,096 for every value
(isolates the micro-batch), two passes.

| Micro-batch | Own fit: prompt tok/s | Own fit: generation tok/s | Expert blocks on CPU | Fixed: prompt tok/s (pass 1, 2) |
| --- | --- | --- | --- | --- |
| 512 (app today) | 1,617 | **67.5** | 25 | 1,398, 1,399 |
| **1,024** | **2,369** | 66.1 | 26 | 2,138, 2,139 |
| 2,048 | 3,091 | 59.8 | 28 | 2,923, 2,928 |
| 4,096 | 3,453 | 53.5 | 31 | 3,477, 3,426 |

- A bigger micro-batch reads prompts much faster but needs more VRAM for its compute buffer, which
  pushes experts to the CPU and slows generation.
- **Owner decision (balance rule, see HANDOFF):** 1,024 is the balance, with −2% generation for +46%
  prompt reading. 2,048 costs 11% of generation.

**Drafting (first attempt invalid).** The first run set `--spec-draft-n-max`, which does not apply to
n-gram drafting (its length is `--spec-ngram-simple-size-m`, default 48 tokens), so all four
"lengths" were the default. The 30B also answered the rewrite prompt with method stubs, leaving little
to copy (6 of 96 drafted tokens accepted). Even so, the default n-gram drafting made that rewrite
**20% slower** (58.1 → ~46.5 tok/s) and prose ~3% slower on this offloaded MoE: every rejected draft
costs a multi-token verification batch whose experts run on the CPU. Rerun with the correct flag and a
full-rewrite prompt below.

**Start time, mmap vs none** (llama-server start to ready, same placement, 4 starts each alternating,
file already in the Windows file cache): mmap 5.55 / 5.52 / 5.45 / 5.47 s; none 5.61 / 5.56 / 5.55 /
5.64 s. Loading without mmap costs about 0.1 s here. A true cold start (after a reboot) was not
measured.

**Drafting, second attempt (22:13).** The prompt now asks for every method body in full (outputs of
~670 tokens). `--spec-ngram-simple-size-m` set the length. **Higher is faster.**

| Drafting | Full rewrite tok/s | Prose tok/s | Accepted / drafted (rewrite; prose) | Same output as off (rewrite; prose) |
| --- | --- | --- | --- | --- |
| Off | 57.8 | 53.4 | — | — |
| Length 2 / 3 / 4 / 8 (lookup 12) | 56.4 / 56.3 / 55.0 / 55.6 | 53.6 / 52.8 / 52.9 / 52.2 | **0 / 0** | 6/6; 6/6 |
| **Default: length 48, lookup 12** | **61.5** | 52.4 | 2,160 / 4,032; 162 / 419 | **0/6**; 5/6 |

- The default n-gram drafting makes full rewrites **6% faster** on this offloaded MoE, far less than on
  the 27B fully on the GPU (+76%). A rejected 48-token draft is verified in one batch whose experts
  partly run on the CPU. Prose: about even.
- **Lengths below 12 never draft.** `common_ngram_simple_draft` returns nothing when the draft length
  (`size-m`) is shorter than the lookup (`size-n`, default 12). Those rows measure only overhead.
  Owner-approved third run: lookup = length 4/4, 8/8, 12/12, plus 12/24 and the default 12/48.
- Drafted rewrites were never byte-identical to undrafted ones (0/6), consistent with the 27B.

## Third-party fork patches (2026-09-16, 22:13–22:52)

The owner asked for the patches in a llama.cpp fork (review: `docs/research/2026-09-16-moe-offload-fork-review.md`,
with its verification section) to be tested and kept if they help. They were tested in a separate build of the
fork (`build/fork`, tip 10f8aff; clang CPU modules, MSVC CUDA for this GPU only, plus a one-line change wiring
the expert cache for the qwen3moe architecture). Every comparison stays inside that build: with its features
off the fork read prompts ~8% slower than the stock runtime (2,895 vs ~3,150 tok/s, same placement), so it is
not a stand-in for stock. 30B MoE model, async CPU splits off unless stated. **Higher is faster.**

**Expert prefetch** (`GGML_SCHED_PREFETCH_EXPERTS=1`), `llama-bench`, runs interleaved:

| Setting | Prefetch off | Prefetch on |
| --- | --- | --- |
| 2,048-token prompt at micro-batch 2,048, pinned load: prompt | 2,912 / 2,878 | 3,708 / 3,729 (+28%) |
| same: generation | 60.2 / 55.7 | 54.8 / 53.4 (not repeated in the run below) |
| 2,048-token prompt at micro-batch 2,048, mmap: prompt | 1,997 / 1,983 | 2,225 / 2,180 (+11%) |
| 512-token prompt at micro-batch 64: prompt | 383 / 383 | 201 / 201 (−48%) |
| 512-token prompt at micro-batch 512: prompt | 1,468 / 1,476 | 1,361 / 1,359 (−7.6%) |
| **micro-batch 1,024 (the app's), 512-token prompt** | 1,434 / 1,428 | **1,295 / 1,294 (−9.6%)** |
| **micro-batch 1,024, 4,096-token prompt** | 2,199 / 2,193 | **2,270 / 2,270 (+3.3%)** |
| micro-batch 1,024, generation | 59.7 / 56.7 | 60.8 / 61.2 |

Perplexity with prefetch on and off: identical (40.0171 over 16 chunks). The micro-batch 1,024 rows ran with
2-minute breaks between runs (GPU peak 75–76 °C); the earlier rows ran without breaks (up to 80 °C).

- The upload of whole expert tensors pays off only for large micro-batches; at the app's micro-batch (1,024,
  chosen by the balance rule) it costs 9.6% on a typical turn's prompt for 3.3% on long prompts. **Dropped.**

**Expert cache** (hot experts copied to VRAM; `llama-server` with real prompts, because `llama-bench` feeds random
tokens that break expert routing; routing profile traced from two different prompts; pinned load):

| Setup | Prose generation | Code generation | 4,096-token prompt |
| --- | --- | --- | --- |
| Fork's normal placement (~25 expert blocks in RAM) | 58.2 | 58.6 | 1,777 |
| All experts in RAM, no cache | 37.5 | 38.1 | 1,216 |
| + 16 cached experts per layer (2.1 GB) | 40.9 | 39.7 | 838 |
| + 32 (4.2 GB) | 50.5 | 42.8 | 883 |
| + 64 (8.4 GB) | 63.3 | 50.9 | 1,017 |

- The cache engaged (graph nodes 3,030 → 3,510). Its best setting beats the normal placement only on prose
  (+9%) and loses on code (−13%) and prompt reading (−43%: prompt batches run through the slower dual-chain
  path). **Rejected.**

**Async CPU splits** (the fork's default): generation −10% on the normal placement (58.2 → 52.3) and lower with
the cache; dense 4B and 12B fully on the GPU: prompt −2 to −3%. **Rejected.**

**Pinned mmap patch** (`GGML_CUDA_REGISTER_HOST=1`): a no-op on Windows (POSIX-only code). **Rejected**; loading
without mmap in the stock runtime gives the pinned-memory gain (+48% prompt reading, above).

**Controls** (4B and 12B fully on the GPU, prefetch off/on/on/off): no difference, as expected (prefetch cannot
engage without experts in RAM). 4B: prompt ~7,030–7,330, generation ~140–143; 12B: ~2,200–2,290 and ~56–59.

Outcome: no fork patch is kept; the stock runtime stays.

## Load time (llama-server start to `/health` ready)

8K context, `--n-gpu-layers auto`, flash attention auto. Four starts per model; "first" is the first
start in this test, the rest are reported as the median of three.

| Model | First start | Later starts (median, p10–p90) |
| --- | --- | --- |
| 2B | 2.1 s | 1.5 s (1.5–1.5) |
| 8B | 2.7 s | 2.8 s (2.8–2.8) |
| 14B | 4.4 s | 4.3 s (4.3–4.3) |
| 27B | 4.9 s | 5.0 s (5.0–5.0) |

- Every file had already been read earlier in the batch, so "first" is not a cold disk read; it
  matches the later starts. A true cold start (after a reboot) was not measured.
- Starting the runtime costs 1.5–5 s. The app's own load path adds device detection and, with the
  runtime fit, several `llama-fit-params` probes; their cost is measured in the live verification,
  not here.

## Batch status

All phases finished 2026-09-16 18:04 (one expected failure: 8-bit cache without flash attention).
Still to measure: the app pipeline (chat time-to-first-token vs the engine, multi-turn cache reuse,
classification cost), the runtime built from source vs the official build, and the fit probe cost.
