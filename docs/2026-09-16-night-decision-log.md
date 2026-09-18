# Decision log: autonomous work, night of 2026-09-16

Updated continuously so it survives context compaction. Read this after `docs/HANDOFF.md`.

## Owner instructions for this stretch (2026-09-16 ~22:30)

- Work solo: **no agents or workflows** from now on. (The one workflow already approved finished at
  ~22:45; its review findings are handled below.)
- Note down every decision made. If something needs the owner (an install, or a problem the known
  rules cannot settle), skip it and record it under "Blocked / needs the owner".
- Write things down periodically so they survive auto-compaction.
- Finish every item: all tests, and every change decided from research findings and owner approvals.
- **General rule:** prefer overall performance improvements. When one measure gets worse while another
  improves, the worse one may lose **at most 5%**, and every such trade goes in the trade-off ledger
  below so the losses can be added up at the end.
  - What counts: **speed** (generation and prompt reading).
  - Start time matters less. Try not to make it worse, but a small start-time cost is fine if speed
    gains.
- **Fork patches:** apply the same logic; if a patch benefits us, keep it. It is kept as **patch files
  on our pinned llama.cpp**, applied by the build scripts and recorded in BUILD_INFO. It is rebuilt and
  re-measured, and dropped if the port does not keep the gain.
- **Expert cache, if it wins:** set up **during per-model calibration**: routing trace, slot sizing,
  and a 4,096-token crash check before it is enabled for that model.
- **Owner answers ~22:55:**
  - **Confirmation benchmarks a decision needs may run without asking.** Same method: one model at a
    time, 6 repeats with the first dropped, alternating order. Parameters and results go in this log.
  - **Calibration measures micro-batch sizes in alternating passes with a 2-minute break between
    passes**, because a constant workload can make the GPU throttle.
  - **Model/hardware names stay in measurement records** (needed to interpret numbers); code, tests,
    UI and instructions stay neutral.
  - **Write a final report at the end** for the owner to review.
  - Start time: try not to make it worse; a small cost is fine for a speed gain.
- **Owner rule ~23:00:** when unsure what to do, **research on the internet first** before marking something
  unresolvable and moving on. Everything is solo; the owner is asleep; do as much as possible.
- **Owner rule ~23:05: breaks.** Within a batch of tests, a **2-minute break between sub-tests**; after
  each batch, a **5-minute break**. Implemented in the shared bench module: every llama-bench run and
  server start waits until 2 minutes have passed since the previous test ended; scripts call a
  5-minute batch break between phases and between scripts. (The fork queue already running at that
  moment finished its last phase, the 4B/12B controls, without the breaks; noted with its results.)
- Standing rules still apply (HANDOFF "Owner rules"): no hard-coded model/hardware names or paths;
  commit only when told; one model loaded at a time; measure before claiming; frontend first; hybrid
  tests only on overflowing models; no CPU tests above 8B; no compiling during benchmarks.

## Trade-off ledger (a measure lost up to 5% for a gain elsewhere)

| # | When | Decision | Lost | Gained | Evidence | Where applied |
| --- | --- | --- | --- | --- | --- | --- |
| T1 | 22:05 | Micro-batch 1,024 instead of 512 for GPU placements where 1,024 keeps every layer on the GPU or moves at most one expert block to the CPU (owner-approved) | Generation −2.1% (30B MoE: 67.5 → 66.1) | Prompt reading +46.5% (1,617 → 2,369) | validation doc, 30B micro-batch table | `runtime_fit::default_micro_batch`, calibration |
| T2 | 22:05 | Load without mmap for GPU+RAM (hybrid) loads when RAM has room | Start time +0.1 s (5.5 → 5.6 s, +1.8%); 9.3 GB of RAM held pinned (logged, not limited) | Prompt reading +48% (2,079–2,160 → 3,126–3,179); generation unchanged | validation doc, load mode + start time | `runtime_fit::load_without_mmap` |
| T3 | 22:50 | Built-in draft head for models that have one (owner-approved exception to the 5% limit) | Prompt reading −7.1% short prompts (841 → 781), −4.3% on 4,096-token prompts (930 → 890), project runtime at 8K; +0.9 GB VRAM. Only where every layer still fits (decision 26) | Generation +42% prose (37.2 → 52.8), +41% rewrites over n-gram alone (72.2 → 101.6) | validation doc, draft-head table; confirmation run pending | `inference::speculative_for`, fit reserve |
| T4 | 02:22 | MoE expert placements: f16 cache with the smallest margin that frees the most experts (256 MiB on the 30B at 16K) instead of q8_0 or the default margin | Prompt reading −3.1% average vs the default margin (1,211 → 1,173; passes +1.8% and −7.7%); ~0.6 GB less VRAM headroom | Generation +6.1% vs default (49.3 → 52.3); +27% vs the old q8_0 choice (41.1 → 52.3) | night decisions 23, 25 | `runtime_fit::fastest_combination` |

## Decisions (newest last)

1. **22:00 Clang runtime accepted.** CPU prompt reading is at parity with the official build (275/273 vs
   270/278 tok/s); GPU is equal. `models/bin` and the MSVC runtime went to the Recycle Bin (owner
   decision). Docs still pointing at `models/bin` are to be updated (see checklist).
2. **22:05 Speed rule changed by the owner** (HANDOFF "Speed rule"): ≤5% generation loss for ≥5× prompt
   gain. It replaces "highest output speed".
3. **22:05 Safe default, refined from the approval text:** 1,024 may move at most one extra *expert
   block* to the CPU, but **no** extra *dense layer*. Measured: a whole layer costs ~8% of generation,
   which would break the 5% limit.
4. **22:20 Draft-length test redesigned** (owner-approved): n-gram drafting cannot draft fewer tokens
   than its lookup, so the sweep runs lookup = length 4/4, 8/8, 12/12, plus 12/24 and 12/48 (default).
   It runs after the fork queue.
5. **22:45 Workflow result accepted for review.** The speed-rule code was written by the agent (not
   compiled); reviewer findings are handled in the checklist below before compiling.
6. **22:55 Cooldown between alternating passes in my own confirmation benchmarks too** (same throttling
   reasoning as the owner gave for calibration): 2 minutes between passes. GPU temperature and power
   are in the telemetry of every run to check for throttling afterwards.
7. **22:55 Review findings to fix before compiling (speed-rule code), all accepted:**
   - (a) **Blocker:** the fit reading dropped `-ngl` when expert rules were printed, so lost whole
     layers looked like fewer expert blocks. `Fit::ExpertsOnCpu` will carry both `gpu_layers` and
     `blocks`; only `=CPU` rules count; any GPU-layer drop is treated as losing a layer; ranking is
     GPU layers first, then fewest blocks.
   - (b) **Blocker:** a calibrated micro-batch is accepted at load only if GPU layers are equal, extra
     expert blocks are at most what calibration measured, and it stays every-layer-on-GPU when 512
     is; otherwise the default rule decides. Calibration stores nothing unless 512 was measured.
   - (c) Split GGUF sets: RAM checks use the size of all shards.
   - (d) Calibration benchmarks with `-lm none` when the load would load without mmap.
   - (e) Prompt-cache RAM sizing uses an estimate of the weights that stay in RAM, not the whole file.
   - (f) A GPU load that fails with the new micro-batch or load mode retries once on the GPU without
     them before the CPU fallback.
   - (g) Calibration: two alternating passes with a 2-minute break (owner answer); choice on the mean.
   - (h) The one-extra-expert-block allowance scales with the model's block count (measured on a
     48-block model: one block cost 2.1%; later blocks 2.9–4.8% each). Fewer, larger blocks get no
     free block.
   - (i) Load without mmap: only when the fit's rules target the CPU (not a second GPU), only when
     the runtime's `--help` lists `--load-mode`, with the note worded as a request; after load, the
     server log's `CUDA_Host` line confirms it.
   - (j) Notes/doc fixes: no empty placement notes, reworded rule comments, draft-head figure −5.7%
     (795 → 750), HANDOFF header, calibration card wording and time estimate.
   - Full review text: session scratch `balance_reviews.txt`; summary here.

8. **22:45 Fork test results (fork build, 30B MoE; every comparison inside the fork build).
   Higher is faster.**
   - **Expert prefetch** (2,048-token prompt at micro-batch 2,048; two runs each):
     - Pinned (mmap off): prompt 2,912 / 2,878 → **3,708 / 3,729 (+28%)**; generation 60.2 / 55.7 → 54.8 /
       53.4 (−6.6% on the means, noisy).
     - mmap on: prompt 1,997 / 1,983 → 2,225 / 2,180 (+11%); generation 57.7 / 57.3 → 57.1 / 55.5 (−2%).
     - **Small batches** (512-token prompt, pinned): micro-batch 64: 383 → **201 (−48%)**; micro-batch 512:
       1,472 → **1,360 (−7.6%)**.
     - Perplexity with prefetch on vs off: **identical** (40.0171, all 16 chunks). VRAM +~470 MiB.
     - Verdict so far: **not kept at these settings.** It breaks the 5% limit on generation and on prompt
       reading at micro-batch ≤512. Typical chat turns (a few hundred new tokens) are exactly those small
       batches. Next: a confirmation test at the app's micro-batch (1,024): 512- and 4,096-token prompts
       plus generation, with the 2-minute breaks. If it still fails, prefetch is dropped. If it wins only
       for large batches, a threshold-limited port is judged on that data.
   - **Expert cache** (llama-server, real prompts, 128 tokens generated; pinned load; async off):

     | Setup | Prose gen | Code gen | 4,096-token prompt |
     | --- | --- | --- | --- |
     | Fork's stock placement (~25 expert blocks in RAM) | 58.2 | 58.6 | 1,777 |
     | All experts in RAM, no cache | 37.5 | 38.1 | 1,216 |
     | + cache 16 slots | 40.9 | 39.7 | 838 |
     | + cache 32 slots | 50.5 | 42.8 | 883 |
     | + cache 64 slots | **63.3 (+9%)** | **50.9 (−13%)** | **1,017 (−43%)** |

     **Rejected:** the best cache setting still loses 13% of code generation and 43% of prompt reading
     against the normal placement. The cache engaged (graph nodes 3,030 → 3,510; 16/32/64 slots uploaded
     2.1/4.2/8.4 GB). Its prompt path through the dual expert chains is slow, as the review predicted.
   - **Async CPU splits** (the fork's default): generation −10% on the stock placement (58.2 → 52.3), −8 to
     −26% with the cache; dense 4B/12B controls: prompt −2 to −3%. **Rejected.**
   - **Pinned mmap patch:** no-op on Windows (verified). **Rejected**; the stock "load without mmap"
     (trade T2) gives the pinned-memory gain.
   - **4B/12B controls:** prefetch on vs off made no difference (4B prompt ~7,030–7,330 and generation
     ~140–143 either way; 12B ~2,200–2,290 / ~56–59), as expected: it never engages on models fully on the
     GPU. These runs had no cooldown breaks (the rule came mid-run); GPU peaked at 82–83 °C.
   - The fork build itself (features off) read prompts ~8% slower than our stock runtime (2,895 vs
     ~3,150), consistent with the review's "fork off ≠ stock".

9. **22:55 Review fixes (decision 7) written; first compile passed.** `cargo test`: 416 passed, 0 failed,
   0 warnings, covering (a) the fit carrying layers and blocks, (b) the calibrated-size check, (c) split
   sizes, (d) calibration measured without mmap, (e) the prompt-cache estimate, (g) two passes with the
   pause, (h) block-scaled allowance, (i) the `--load-mode` support check. Written after that compile,
   and compiled after the running benchmark batch:
   - (f) GPU retry without the micro-batch/load mode, plus "unable to allocate CUDA_Host" treated as a
     GPU start failure;
   - (j) the calibration card (every size vs 512 with its gain and loss, expert placement label, time
     estimate), with a new frontend test;
   - doc fixes (draft head −5.7%, HANDOFF header).
10. **23:00 Not done from the review, with reasons:**
    - Checking the server log for the `CUDA_Host` line after load. The app does not keep the sidecar's
      load log in a readable place today, so the note says "asks the runtime" and that ordinary memory
      is used if pinned memory fails.
    - Probing 1,024 across every combination before settling on 512's combination: not worse than
      today, and it adds probes to every load.
    - Draft-head VRAM in the probes: handled when the draft head is wired in (checklist).

11. **22:48 Draft head (owner-approved) written, not yet compiled.**
    - The model scan reads `<arch>.nextn_predict_layers`. When automatic drafting is on and a model has
      a draft head, the load uses `--spec-type draft-mtp,ngram-simple --spec-draft-n-max 3`; otherwise
      n-gram drafting as before.
    - The fit probes leave 1,024 MiB more VRAM free for it (measured +0.9 GB), while the server keeps
      the plain margin (it counts the draft context itself).
    - A CPU fallback drops the draft head and keeps n-gram drafting (only measured on the GPU).
    - **Conflict with the 5% rule:** the earlier measurement showed prompt reading 795 → 750 tok/s
      (−5.7%, "6–8%" in the validation doc) with the draft head. That is just over the owner's 5% limit,
      although generation gained +54% (prose) and +150% (rewrites with n-gram).
    - Decision: run a confirmation test on the project runtime (27B, draft head on/off, prompt reading
      and generation, two passes with breaks). If the prompt cost is ≤5%, it ships as approved.
      Otherwise the code stays, with the automatic choice turned off, and the item goes under "Blocked /
      needs the owner" (their approval vs their later 5% rule).

12. **22:50 Owner: the draft head is the one exception to the 5% limit** ("since I already approved it").
    It ships with the automatic choice on. The confirmation test still runs, to record its real cost in
    the trade-off ledger (T3) for the final report, not to decide. The owner then went to sleep; everything
    after this is unattended.
13. **22:50 Stale `models/bin` references updated:** ARCHITECTURE (binary search order and setup),
    PERFORMANCE ("build recipe not executed" replaced by the executed result), the `detect` doc comment in
    `llamaserver.rs`, and the workspace launch config's fallback to `models/bin` removed.

14. **22:53 Expert prefetch dropped; no fork patch is kept.** Confirmation at the app's micro-batch (1,024),
    30B, pinned load, async off, runs off/on/on/off with 2-minute breaks (GPU peak 75–76 °C). Higher is
    faster.

    | Test | Prefetch off | Prefetch on |
    | --- | --- | --- |
    | 512-token prompt (one micro-batch; a typical chat turn) | 1,434 / 1,428 | 1,295 / 1,294 (**−9.6%**) |
    | 4,096-token prompt (four micro-batches) | 2,199 / 2,193 | 2,270 / 2,270 (+3.3%) |
    | Generation after them | 59.7 / 56.7 | 60.8 / 61.2 (no loss) |

    - The −6.6% generation seen in the first prefetch run did not repeat here, so it was noise or heat
      from runs without breaks.
    - Prompt reading still loses 9.6% on the prompt size most turns have, for +3.3% on long prompts, so
      it breaks the 5% limit.
    - The +28% it measured at micro-batch 2,048 does not apply: 2,048 itself was rejected by the balance
      rule (−11% generation).
    - Outcome of the whole fork evaluation: expert cache, async CPU splits and the pinned-mmap patch were
      rejected (decision 8), and prefetch is now dropped too. **No patch files are needed**; the stock
      runtime stays. The fork build and source in `build/fork` can be deleted (regenerable, ignored by
      git).

15. **22:56 Owner-rule sweep: hard-coded names neutralised in code, tests, scripts and UI** (not compiled yet):
    - **Changed:**
      - a workspace error hint and two tests used the owner's project names and a machine path
        (`e.g. RageV`, `D:/Dev/RageV`, `ember`);
      - tests used specific model names (`Qwen 14B`, `qwen-14b`, `gemma-4-12b`, `Qwen3-8B`) and the owner's
        GPU (`RTX 5070 Ti`, full `nvidia-smi` name);
      - the model download dialog's placeholder was `qwen-14b`;
      - the e2e script defaulted to `qwen3-8b`;
      - the benchmark system prompt said "model: Qwen3 8B";
      - a build-script comment said "RTX 50 series".

      All now use neutral names (`SampleEngine`, `example-14b`, `Example GPU`, `my-model`, …); the e2e script
      requires a model id.
    - **Kept on purpose:** names that describe real model-family formats the code must handle (Gemma's
      native tool envelope, the `<tool_call>` tag Qwen/Hermes-family models use, Gemma 2's template
      restrictions, Ollama's gemma3/4 blob repair), GGUF architecture identifiers in fixtures (`qwen2`,
      `gemma4`, …), and generic device-name formats in the device-list parser test. Measurement records
      keep names (owner, decision 5 answers).
16. **22:56 Live tests: how they run** (owner plan in HANDOFF; no agents):
    - **Driver.** A script drives the real backend over its HTTP API, the same calls the UI makes: code
      sessions classify first, then either stream a chat reply or start an agent run, and approvals are
      granted as the owner allowed for these test folders. Everything runs against a debug build, an
      isolated data directory (the owner's history is untouched) and the project runtime, one model at a
      time, with the break rules.
    - **Why an API driver rather than clicking through the UI:** 8 models × 9 chat messages and 5 models
      × 4 code tasks, with exact timings, final texts and failure causes, are only practical and
      reproducible this way. A UI spot check in the browser pane covers each model's session, and the
      generated apps are opened and clicked in the browser pane.
    - **Where files go.** Everything the code tests create is under `C:\Users\ism19\Code\companion-live-tests\`
      (a copy of a small test project per model, plus each model's `calculator/` and `clock/` apps).
      Screenshots are taken with headless Edge into `docs/validation/live-tests/`. The folder goes to the
      Recycle Bin after analysis.

17. **23:00 Final report format** (from the owner's original brief, saved in session scratch `owner_brief.md`):
    - **Executive summary** answering: what was actually wrong; why performance was worse than expected;
      what Mica did better; what DeepSeek Harness taught us; whether to embed llama.cpp; what was fixed;
      how much performance improved; what remains.
    - **Sections:** A Gemma 2B tool output, B Gemma 4 E2B loading, C performance investigation, D Mica,
      E llama.cpp integration decision, F DeepSeek Harness, G P0/P1/P2 plan (each item: problem, evidence,
      root cause, solution, impact, risk, complexity, verification).
    - **Plus:** the answer to §14.15 ("why isn't the app significantly faster than CPU-only Ollama"), with
      the bottleneck proven by measurement.

18. **23:35 N-gram draft length: the default (48) wastes most of the gain on the 30B MoE.** Sweep with lookup
    and length set together (llama-server, full-rewrite and prose tasks, 2 passes, 2-minute breaks).
    Higher is faster:

    | Lookup / length | Full rewrite | Prose | Rewrite drafts accepted |
    | --- | --- | --- | --- |
    | off | 60.2 | 53.7 | — |
    | 4 / 4 | 77.8 | 57.2 | 2,214 / 2,832 |
    | 8 / 8 | 88.3 | 57.5 | 2,196 / 2,640 |
    | 12 / 12 | 88.4 | 58.9 | 2,040 / 2,448 |
    | **12 / 24** | **90.6** | **58.9** | 2,118 / 3,024 |
    | 12 / 48 (default) | 63.6 | 57.6 | 2,160 / 4,032 |

    - Length 24 is best on both tasks: +42% on rewrites against the default, with prose even.
    - A rejected long draft costs a big verification batch whose experts partly run on the CPU.
    - Prose output was identical to no drafting at 12/12 and 12/24 (6/6); rewrites were not (0/6 for
      every drafting setting, as before).
    - Next: before changing the app's default, confirm on models fully on the GPU (8B, 12B: off, 12/12,
      12/24, 12/48), and add a draft-head + length-24 arm to the 27B test. The default changes only where
      nothing loses more than 5%.

19. **23:36 Everything written tonight compiles and passes.** Backend `cargo test`: 420 passed, 0 failed, 0
    warnings. `companion-backend` debug build: ok. Frontend: 81 tests pass, `tsc` clean, `vite build` ok.
    This covers the review fixes, the GPU retry, the calibration card, the draft head and the name sweep.
    The queue continues: n-gram length on the 8B/12B fully on the GPU, the 27B draft head (with length 24),
    then the 16K MoE combination test, each with 5-minute breaks between them.

20. **00:31 N-gram draft length: 24 for split (GPU + RAM) loads, the default when the model is fully on the GPU.**
    Confirmation on models fully on the GPU (llama-server, 8K, full-rewrite and prose, 2 passes, 2-minute
    breaks). Higher is faster:

    | Model | Off (rewrite / prose) | 12/12 | 12/24 | 12/48 default |
    | --- | --- | --- | --- | --- |
    | 8B | 87.5 / 88.4 | 157.6 / 91.1 | 163.3 / 91.0 | **167.0 / 91.7** |
    | 12B | 54.4 / 55.3 | 94.4 / 55.1 | 99.0 / 55.6 | **100.7 / 55.4** |
    | 30B MoE, experts in RAM (decision 18) | 60.2 / 53.7 | 88.4 / 58.9 | **90.6 / 58.9** | 63.6 / 57.6 |

    - Fully on the GPU, the default is marginally faster (rewrite −2.2% and −1.7% at length 24).
    - With weights in RAM, length 24 is +42% on rewrites.
    - Choosing by placement loses nothing anywhere, so it is better than a single global value, which
      would have cost up to 2.2% on GPU models. **No trade-off ledger entry.**
    - CPU-only loads keep the default until the CPU drafting test (C5, which now includes lengths 12 and
      24) measures them.
    - Implemented in the load path next to the micro-batch choice (automatic modes only): `--spec-ngram-simple-size-m 24`
      when the placement keeps weights in RAM and n-gram drafting is on. The CPU fallback resets it.

21. **00:33 Micro-batch default for models fully on the GPU: measure before keeping it.**
    - The balance-rule code picks 1,024 when every layer stays on the GPU at both sizes, since generation
      cannot lose. But the only measurement (8B, 2,048-token prompt) showed +1.9%, inside the noise, and
      the validation doc had concluded "512 stays".
    - Measure-before-claiming applies, so a short test (4B, 8B, 12B, 4,096-token prompt + 128 generated
      tokens, micro-batch 512 vs 1,024, alternating runs with breaks) runs after the combo test and a
      compile. If 1,024 is not measurably faster, the fully-on-GPU case goes back to 512.
    - The live tests run after that decision, so they test the final configuration.

22. **00:34 Draft head at the owner's 32K context needs its own test.**
    - The draft-head measurements (and tonight's cost run) use the 27B at 8K. At the owner's 32K,
      every layer only just fits with q8_0 at a 512 MiB margin (peak 11,066 MiB). The draft head's
      ~0.9 GB (reserved as 1,024 MiB in the fit) will likely move layers to the CPU, at about 8%
      generation each.
    - Test queued after the micro-batch check (`bench_mtp32k.py`): A = no draft head at the 512 MiB
      fit; B = draft head at the fit with the reserve (what the app would load); C = draft head with
      A's layers. Short and 12.7K-deep generation plus prompt reading.
    - If B is slower than A, the app turns the draft head on only when its reserve does not cost layers
      (probe with and without it), and the owner's 32K 27B keeps n-gram drafting.

23. **01:14 The app's combination search makes a mixture-of-experts model slower at 16K.** 30B, 16K in context,
    pinned load, runs A B C C B A with 2-minute breaks. Higher is faster:

    | Arm | Generation | Prompt reading | VRAM peak |
    | --- | --- | --- | --- |
    | A llama.cpp default: f16 cache, 1,024 MiB margin, 29 expert blocks in RAM | **49.3 / 46.6** | **1,190 / 1,188** | 11,055 / 10,853 MiB |
    | B app's choice (`fastest_combination`: most layers, then fewest blocks): q8_0, 256 MiB margin, 24 blocks | 40.8 / 41.4 | 1,049 / 970 | 11,679 / 11,601 MiB |
    | C = B + micro-batch 1,024 (25 blocks) | 43.1 / 42.3 | 1,178 / 1,174 | 11,647 MiB |

    - The rule "fewest expert blocks in RAM" came from a sweep at one fixed cache and margin (24: 65.6,
      32: 50.3, 40: 44.4 tok/s). Reaching fewer blocks by shrinking the cache precision and the margin is
      a different trade, and it loses 14% here.
    - Cache type and margin changed together, so a follow-up (`bench_moe_combo2.py`) separates them:
      f16 at 256 MiB, q8_0 at 1,024 MiB, and the default with micro-batch 1,024.
    - The code change waits for that data. Likely outcome: expert placements keep the default cache and
      margin.
    - **Harness bug:** the 27B draft-head run failed. Its prompt patch called itself (recursion), so all
      its requests errored. Fixed and requeued. The failed files were renamed `*_failed_recursion.*`.

24. **01:56 Micro-batch 1,024 stays for models fully on the GPU** (decision 21's test). 4,096-token prompt +
    128 generated, every layer on the GPU, runs 512 / 1,024 / 1,024 / 512 with 2-minute breaks (GPU 69–75 °C).
    Higher is faster:

    | Model | Prompt at 512 | Prompt at 1,024 | Generation 512 → 1,024 | VRAM |
    | --- | --- | --- | --- | --- |
    | 4B | 7,445 / 7,489 | 7,705 / 7,714 (+3.3%) | 148.1 / 150.6 → 152.2 / 149.6 | +0.58 GB |
    | 8B | 3,680 / 3,679 | 3,717 / 3,724 (+1.1%) | 91.4 / 92.5 → 92.1 / 91.3 | +0.31 GB |
    | 12B | 2,346 / 2,328 | 2,368 / 2,329 (+0.5%) | 58.8 / 58.5 → 58.4 / 58.3 | +0.70 GB |

    - A small prompt-reading gain, consistent across both passes for the 4B and 8B, with no generation
      change. The extra compute buffer is inside the fit: the load probes 1,024 at the chosen cache and
      margin and takes it only if every layer still fits with that margin. So VRAM headroom is not
      reduced below the margin.
    - Code unchanged; no trade-off ledger entry, since no speed measure lost.

25. **02:22 Expert placements keep the f16 cache; only the margin shrinks.** `bench_moe_combo2.py`: 30B, 16K
    in context, pinned load, runs A D E F F E D A with 2-minute breaks (GPU 76–77 °C). Higher is faster:

    | Arm | Expert blocks in RAM | Generation | Prompt reading | VRAM peak |
    | --- | --- | --- | --- | --- |
    | A f16, 1,024 MiB margin | 29 | 49.3 / 49.4 | 1,209 / 1,213 | 11,095 / 10,976 MiB |
    | D f16, 256 MiB margin | 26 | **52.7 / 51.8** | 1,230 / 1,116 | 11,706 / 11,651 MiB |
    | E q8_0, 1,024 MiB margin | 26 | 44.5 / 44.5 | 1,254 / 1,254 | 11,039 MiB |
    | F f16, 1,024 MiB, micro-batch 1,024 | 29 | 48.2 / 48.9 | 1,191 / 1,192 | 11,043 MiB |

    - **The q8_0 cache is what hurts this model:** −10% generation, although it frees three expert
      blocks. On the 8B dense model q8_0 was neutral, because every layer was already on the GPU.
    - **A smaller margin with f16 helps:** +6% generation. Prompt reading averaged −3.1%, noisy: +1.8%
      and −7.7% on the two passes.
    - F matches A: with a 512-token prompt a larger micro-batch cannot help. The earlier +46% was on
      4,096-token prompts.
    - **Code:** `fastest_combination` still keeps whole layers on the GPU first. Among combinations
      keeping the most layers, it takes an f16 one when there is one, then the fewest expert blocks in
      RAM. On a small GPU, q8_0 still wins if it keeps more whole layers. A saved q8_0 preference is
      honoured. Tests encode the measured table. Not compiled yet (a benchmark is running).
    - **Trade-off ledger T4:** prompt reading −3.1% (average, noisy) for generation +6.1%.

26. **03:33 Draft head: measured cost, and off when its VRAM would cost layers.** 27B, project runtime, 2 passes,
    2-minute breaks. Higher is faster.

    At 8K (every layer on the GPU with room to spare):

    | Drafting | Prose | Rewrite | Prompt reading (short / 4,096) |
    | --- | --- | --- | --- |
    | n-gram (default length) | 37.2 | 72.2 | 841 / 930 |
    | draft head + n-gram | **52.8** | **101.6** | 781 / 890 |
    | draft head + n-gram length 24 | 53.7 | 96.0 | 787 / 896 |
    | n-gram length 24 | 37.5 | 68.5 | 851 / 933 |

    At the owner's 32K (q8_0; every layer fits only without the head):

    | Setup | Prose | Rewrite | Prompt reading | Deep generation (12.7K in) |
    | --- | --- | --- | --- | --- |
    | A n-gram, every layer on the GPU | **37.2** | **66.9** | **780** | **33.9** |
    | B draft head, 59 of 65 layers (fit with the head's reserve) | 32.2 | 50.4 | 455 | 22.0 |
    | C draft head, every layer | server ran out of memory (connection reset, then time-out) | | | |

    - **At 8K** the head gives prose +42% and rewrites +41%, for prompt reading −7.1% (short) and −4.3%
      (long). This is the owner's approved exception, and ledger T3 is updated with these numbers. Length
      24 does not help with the head on the GPU (rewrites 96.0 vs 101.6), consistent with decision 20.
    - **At 32K** the head costs 6 layers and everything gets slower (generation −13 to −35%, prompt
      reading −42%). The owner approved the head for its speed, and here it has none, so it is not used.
    - **Code:** the load path probes with the head's reserve first. If no cache/margin combination keeps
      every layer on the GPU at the requested context, it turns the head off for that load (n-gram
      drafting stays), probes again without the reserve, and says why in a note. The context is never
      shrunk to make room for the head. Not compiled yet (compiles next).

27. **03:50 Live-test harness fix and measure-first scripts prepared (nothing run yet).**
    - The app's model id is a slug of the folder name (dots become dashes: `qwen-3.8-27b-2bit` is listed as
      `qwen-3-8-27b-2bit`). The chain passed folder names, so the 27B's chat and code runs would have
      failed to load for a harness reason. `live_tests.py` now maps folder names to the app's id before the
      chat suite starts. The verify "model-list reconcile" row reads `ok: false` for the same reason; the
      list itself was right (8 models, the deleted one gone).
    - Scripts written for after the live tests, each with 2-minute breaks and alternating arms:
      - `bench_repeat.py` (M18): penalty 1.1 vs 1.0 at the app's sampling, fixed seeds; rewrite and edit
        exactness by syntax tree, prose repetition, draft acceptance; 2B, 8B, 27B.
      - `bench_checkpoints.py` (M8): six-turn chats, `--checkpoint-min-step` default vs 0; tokens re-read
        per turn and server RAM; 27B (hybrid) and 12B (sliding window).
      - `bench_reasoning.py` (M10): ten word problems with thinking on; uncapped vs cap 2,048 vs cap 1,024;
        correctness, wait before the first visible word; 8B and 27B.
      - `app_trim.py` (M21): 24-turn chat through the app at 4K context, compaction automatic vs off;
        dropped messages and cached tokens per turn.

28. **03:48 Live verify suite results, and an app defect it found (fixed in source; compiles after the live tests).**
    - Model list reconcile: correct (8 models listed, the deleted one gone). The row reads `ok: false` only
      because of the harness's id comparison (decision 27).
    - Context above the model's limit: the load notice appears ("supports at most 8,192 tokens ... reduced
      to 8,192 for this model; the setting itself is unchanged"). Pass.
    - Balanced mode without a calibration: explained in the load notes. Pass.
    - Unloadable model file: the model card lists both load issues in plain words (packed vision tensors,
      no chat template). Pass.
    - **Defect:** loading that model answered "The llama.cpp runtime is not built yet", with the hint
      "Check the id and try again", although `runtime/bin` is built. The runtime was looked up beside the
      models folder, so any `COMPANION_MODELS_DIR` elsewhere (documented in the README) made every load
      fail. The plugins folder was found the same way.
    - **Fix:** `AppConfig.root` (the installation root, already resolved for the data folder) is passed to
      the state. The runtime (`runtime/bin`) and plugins are found there, wherever the models are. A
      missing runtime now answers 503 with a hint that the model is fine and the runtime is needed. A new
      test covers finding the runtime at the root.
    - Next: compile after the live tests, then rerun this verify check to see the unloadable model's own
      error.

29. **04:30 Owner awake: chat-format fallback for files without a template (owner-approved), and two live findings.**
    - **Found live:** the 4B test file carries no chat template. The runtime wrapped every conversation in
      ChatML, the model never produced its own end-of-turn marker, and two replies ran to the 8,192-token
      limit, inventing conversations. That is a file problem, not the model's ability, so the 4B's first
      chat run does not count and is rerun after the fix.
    - **Owner decision (04:20):** "add the architecture-based template fallback".
    - **Design, checked in the pinned source:**
      - llama.cpp's built-in formats (`llm_chat_template`) apply only outside the Jinja engine
        (`--no-jinja --chat-template NAME`). With Jinja on, the name would be rendered as literal text.
      - Without Jinja, the only request feature refused is native `tools`/`tool_choice`, which the app
        never sends. `response_format` still works.
      - `models::builtin_chat_format` maps a GGUF architecture to a format only where every
        instruction-tuned release uses one format: gemma/gemma2/gemma3/gemma3n → gemma, qwen2/qwen2moe/
        qwen3/qwen3moe → chatml (the runtime's own fallback, so no argument), llama4, command-r, gpt-oss,
        hunyuan-moe, hunyuan-dense, seed_oss.
      - Architectures shared by releases with different formats (llama, phi3, deepseek2, chatglm, glm4,
        granite, minicpm, cohere2, grok) keep the load-issue warning instead of a guess.
      - The load path uses the format only if the installed runtime's `--help` lists it. The help text is
        now cached once per binary and shared with `supports_argument`. The person loading is told in a
        notice.
      - A file with its own template is untouched. The card's "no chat template" issue disappears when a
        format applies.
      - Tests cover the table, the load issues, the launch arguments and parsing the help list.
      - Not compiled yet: the chat suite is running. The compile follows it.
    - **Found live:** generation in the app is slower than the benchmarks: 8B 60–68 tok/s in chat vs 85–92
      in llama-bench and plain server runs; 2B 113–150 vs 204. It is not vocabulary-specific. Candidate
      causes: GPU idle between messages, per-token timings, sampler chain, drafting on prose.
      `bench_overhead.py` (written) isolates them on the 8B and 2B.
    - **Plan:** when the chat suite ends, stop the chain before its code tests, compile (runtime-root fix
      plus this fallback), then run `chain_after.sh`: load-error recheck → 4B chat rerun → code suite (5
      models) → fence → overhead → CPU track → C6 → app_bench → M18 → M8 → M10 → M21.

30. **04:40 Owner: demo priority, and B1 deferred.**
    - "For the current demo I just need to show chats, code reading and creation", which is what the live
      tests cover. B1 (projector/vision test) goes on the back burner.
    - Consequence for the queue: the live path comes first and is not to be delayed by measurement work.
      That path is the compile of the two fixes, the load-error recheck, the 4B chat rerun, and the code
      suite on 5 models (reading a project, changing it, creating the calculator and clock apps). Any
      app-side failure found there is fixed and rerun before the measure-first items (fence, overhead,
      CPU track, C6, app_bench, M18/M8/M10/M21), which keep their order after it.

31. **04:45 Speed gap narrowed to conversation switching (hypothesis, measurement queued).**
    - Per-reply engine speeds from the live chat run, higher is faster:
      - The first reply after a load runs at benchmark speed: 8B 89.2 tok/s, 12B 56.7.
      - Every later reply in a *new* conversation, which starts from the shared system prompt restored
        through the server's RAM prompt cache, is a third slower: 8B 60–67, 12B 39–40.
      - A continued 12B conversation was fast again (55.2); the 8B's continuation stayed slow (62.4).
    - So idle time and sampling are unlikely to be the main cause.
    - `bench_overhead.py` was rewritten to isolate it on the 8B and 12B with the app's launch: switching
      vs continuing a conversation, RAM prompt cache on vs off, 32K vs 8K context, drafting on vs off,
      per-token timings, greedy sampling, and a 10-second idle. It runs right after the code suite (demo
      path first, decision 30).

32. **05:00 Owner: remember the fit, search ahead of the first load, run fit probes in parallel** (written, not
    compiled yet).
    - **Why:** the 14B's load took 30 s in the live test. 24 s of that was the fit search (about 20 probes of
      ~1.2 s each), and the 27B's load took about the same.
    - **Remembered decision:** stored in `runtime_fit_decisions` (SQLite), keyed by
      `runtime_fit::fit_memory_key`:
      - the model file's name, bytes of every shard and modification time;
      - the fit tool's path and modification time (a rebuilt runtime searches again);
      - the requested context, cache preference, fit or requested-size mode, calibrated micro-batch and
        draft head;
      - the GPU's total VRAM, rounded to 256 MiB.
    - **At load:** a remembered decision is confirmed with one probe of its setting; if the fit differs,
      the full search runs.
      - A decision that cannot improve (the full request on the GPU with the preferred cache and margin) is
        always confirmed.
      - A compromise (smaller context, weights in RAM, or less headroom) is searched again when more than
        256 MiB more VRAM is free than when it was found, or when free VRAM is unknown.
      - The load notes say when a placement was reused, and how long its check took.
    - **Ahead-of-time search (`FitPreparation`):**
      - Runs while no model is loaded: 20 s after startup, and 3–5 s after models are added, after
        settings that decide the fit change, after an unload and after a calibration.
      - The default model goes first. Models with load issues and models already remembered are skipped.
      - A load or calibration cancels it at once, dropping its probe processes, and then waits for them
        to exit (at most ~2 s).
    - **Parallel probes (`FIT_PROBE_CONCURRENCY` = 4, pending measurement):**
      - The preferred combination is probed alone first, since most models fit with it. After that, the
        other combinations, the context search (k-ary, at most 3 rounds for 4K–32K) and the two micro-batch
        sizes run in batches.
      - Risk: each probe measures free VRAM with its own GPU context, so probes running together can only
        see too little. A batched result is therefore checked with probes run alone:
        - the chosen setting;
        - the next context step, when the context was reduced;
        - the draft head, when it was turned off.
      - If a check disagrees, the search repeats one probe at a time.
      - The overlap is measured before the concurrency is kept: the same probes on the 14B, alone and
        4 or 6 at once. If they disagree, the value becomes 1 and the owner is told.
    - Tests:
      - the batched context search gives the same answer as the binary search for 10 limits × 4 widths,
        in at most 3 rounds at width ≥ 4;
      - reuse rules, key inputs, serialisation and storage round trip.
33. **04:58 Owner: "update the project license to unrestricted free usage".**
    - It already is: `LICENSE` has been The Unlicense (public domain) since the initial commit.
    - Made explicit:
      - a README "License" section (clone, fork, copy, modify, publish, sell, any purpose, no conditions,
        no attribution);
      - `license = "Unlicense"` in `backend/Cargo.toml`, `"license": "Unlicense"` in
        `frontend/package.json`.
    - Third-party parts keep their licenses and are not stored in the repository: the llama.cpp runtime
      (MIT, built by the script), dependencies, and models.

34. **05:12 Compiled; parallel fit probes measured safe; one harness incident.**
    - **Chat suite finished at 05:06** (8 models). The 4B's run is invalid (decision 29) and is rerun.
    - **Incident:** stopping the chain's background task left its bash script running. Clearing its
      `sleep` let it start the code suite with the old build at 05:07:17. The 4B loaded and answered one
      "question" task before its processes were stopped by PID at ~05:09, so that row in `results.jsonl` is
      invalid. The test folder it created was sent to the Recycle Bin.
      - The first probe-overlap run overlapped with that load starting, so it was repeated on an idle GPU
        (798 MiB used).
      - Lesson: stop a chain by killing its bash and child processes by PID, not only the task wrapper.
    - **Probe overlap (`probe_overlap.py`):**
      - Setup: the 14B, q8_0 cache, 256 MiB margin, contexts 20,480–25,600, each alone, then 4 and 6 at
        once, two rounds.
      - Identical answers in every round: 22,528 fits every layer, 23,552 gives 48 layers.
      - One probe takes 0.63 s and peaks at +153 MiB of VRAM, yet probes running together did not see
        each other.
      - **`FIT_PROBE_CONCURRENCY` stays at 4.** The check with probes run alone stays as the safety net for
        other drivers.
      - With 0.63 s per probe, about 13 s of the 14B's 24 s pre-spawn time was probing. The rest was the
        device check, calibration-environment lookup and help reads: to look at next.
    - **Compile:** backend `cargo test` 431 passed, 0 failed, 0 warnings (10 new tests). Frontend 81 pass.
      Covers decisions 28 (runtime root), 29 (chat-format fallback) and 32 (fit memory).
    - **05:12 `chain_after.sh` started:**
      - 5-minute cooldown, then the load-error recheck;
      - 4B chat rerun, then the code suite (5 models);
      - fence, overhead (8B/12B), CPU track, C6, app_bench, M18, M8, M10, M21.

35. **05:18 Load-error recheck passed; the message made plain** (written, compiles after the code suite).
    - With the runtime-root fix, the unloadable file now reaches llama.cpp. The app names the real cause:
      vision tensors packed into the model file; download the upstream GGUF.
    - The message was not plain yet:
      - "Generation failed:" appeared twice;
      - the cause came after "llama-server exited before becoming ready (exit code: 1)";
      - about 20 raw runtime log lines followed.
    - **Change:**
      - the load failure is one sentence (the recognised cause, or the exit plus the runtime's last error
        line);
      - the runtime log follows a marker, so the GPU-to-CPU fallback check still reads it;
      - the API shows only the sentence and writes the full text to the app log (exported sessions include
        it), with a hint that says so;
      - no doubled "failed to load" prefix in session loads.
    - Tests: parsing the last runtime error line, and splitting off the diagnostic.

36. **05:23 Chat-format fallback verified live on the template-less 4B file.**
    - The load notice says llama.cpp's built-in "gemma" format is used for the gemma3 architecture.
    - Replies end normally: 63 / 129 / 112 tokens, where the first run gave 681 and ran to the 8,192
      limit twice.
    - No `<|im_end|>` text.
    - Engine generation is 130–134 tok/s (86–107 before).
    - The fit search for this load took 1.6 s (every layer fits on the first probe).
    - The answers' remaining faults are the model's own: an arithmetic slip (135 minutes called "1 hour
      15") and an email in a code fence (the fence check covers the second).
    - **Rerun complete (05:26):** 9 replies, no errors, 24–217 tokens each, engine 119–134 tok/s.
    - **A clue for decision 31:**
      - Here every new conversation had 0 cached tokens (the slot was cleared, not restored from the RAM
        prompt cache), and the speed held.
      - With the 8B and 12B, a new conversation restored the shared system prompt (73–77 cached tokens),
        and generation fell by a third.
      - `bench_overhead.py`'s no-prompt-cache server tests exactly that.

37. **05:33 App defect found by the 4B's code question: reads were withheld from models without native tool
    support** (fix written, compiles after the code suite, then the 4B question is rerun).
    - **What happened:**
      - The 4B was asked "What does the project do, and which function computes the order total including
        tax?"
      - It emitted a correct `read_file` call for `project/orders.py`.
      - The app skipped it with "needs approval — use the Agent panel or /plan".
      - The model then guessed functions that do not exist (`calculate_order_total`, `apply_tax`). The real
        one is `order_total`.
    - **Cause:** harness item H1 gates tools on the template capabilities the runtime reports. With no tool
      support, `ChatToolOffer` offered nothing, even in a code session, whose system prompt always teaches
      the read tools ("no approval needed for reads"). The prompt and the offer contradicted each other.
      Agent runs already serve such models with a schema-constrained action format.
    - **Fix:** a code session offers its read-only inspection tools whatever the template reports. The
      gate stays for plain chat (the measured 2B case) and for document creation.
    - The "needs approval" message now only applies to what it describes: writes and commands.
    - Only models whose runtime reports no tool support are affected. In the live set that is the 4B
      (its file has no template); the 8B/12B/27B/30B templates support tools.

38. **05:38 Two more app defects found by the 4B's change task; code suite restarted after the fixes.**
    - **What happened:** "add low_stock and a test, run the tests" failed in 4 s. Three `write_file` calls
      each arrived with `{"path":"project/orders.py"}` and no text. The request records show why.
    - **Defect 1: early stop cut writes before their text.**
      - The agent stops the stream once an action reads as complete. A write's text follows the object in a
        `<<<CONTENT` block.
      - The check treated an object whose block had not started yet as finished, so the stream stopped at
        `}}`. The raw output ends exactly there, recorded as `early_stopped`.
      - The existing test only covered a block that had opened and not closed.
      - Whether it hits depends on how the tokens fall, so any model could lose a write this way.
      - **Fix:** `action_complete` treats write_file and append_file without `content`, and edit_file
        without `old`+`new` or `patch`, as unfinished. A model that never sends the text still ends its
        reply and gets the missing-argument correction. Test added.
    - **Defect 2: the schema-constrained action format hid where file text goes.**
      - Templates without tool support start in the JSON envelope. Its instruction says "every required
        argument", while the tool docs say the text comes in a `<<<CONTENT` block, which JSON cannot carry.
        The 4B followed the docs.
      - **Fix:** the instruction now says raw blocks do not exist in that format; the text goes in
        `args.content`, or in `args.old`/`args.new`.
    - **Also compiled:** decision 37 (code-session reads offered whatever the template reports) and
      decision 35 (plain load errors). `cargo test`: 432 passed, 0 failed, 0 warnings.
    - **Results thrown away:** the chain was stopped by PID. The 4B code rows from 05:31–05:34 in
      `results.jsonl` are invalid, and the test folder went to the Recycle Bin.
    - `chain_after2.sh` restarts at the code suite (5 models), then the measurement queue unchanged.

39. **05:46 The 4B's code question again: reads now run, but the model dropped the folder from the path.**
    - With decision 37 the read tools were offered. The 4B still called `read_file` with `orders.py`
      three times, although the listing and the repo-index hint both say `project/orders.py`.
    - The app answered "Tool I/O error: The system cannot find the file specified", so the model had
      nothing to correct from. It asked the user to check the file.
    - `search_text` found `project\orders.py:1`, but the model did not take the path from it.
    - **App-side improvement:** a missing path (read_file, list_directory, edit_file, delete_file) now
      answers "no such path 'orders.py' (paths are relative to the project root). Found with that name:
      project/orders.py". The walk is bounded: depth 8, 20,000 entries, 5 matches; hidden folders,
      node_modules, target, __pycache__, dist and build are skipped. Test added.
    - Compiled with `cargo test --no-run` during the 4B's 2-minute break (05:44–05:45), so no task was
      running. The running build does not have it yet.
    - **Plan:** let the 4B finish change, calculator and clock on this build to see every failure mode.
      Then stop the chain before the 8B, fix what is app-side, rebuild, and rerun the code suite.

40. **05:53 The 4B's remaining code tasks, two more app-side fixes, and the owner's routing question.**
    - **4B results on the 05:43 build** (the 4B code rows from 05:43 to 05:50 are pre-fix and do not
      count):
      - change: FAILED. It used the constrained envelope with an empty tool name, a `<<<CONTENT` block
        closed with `<<<CONTENT` instead of `CONTENT>>>`, and a `search_text` without a query. The block
        would also have overwritten all of `orders.py`. Mostly model limits.
      - calculator: the 4B routed it as **plan**; the run ended with a list of steps and no files.
      - clock: routed as **ask**; answered in chat, no files.
    - **Fix 1, the constrained envelope is enforced per tool.** `structured_action_schema` is now an
      `anyOf`: one alternative per registered tool with its name as `const`, its required arguments
      required (`tools::required_args`) and `answer` as `""`, plus the final answer (`name` `""`, empty
      args, non-empty answer).
      - Blank names and calls missing an argument can no longer be generated.
      - A test checks that every listed argument is really refused when missing.
      - **Checked against the runtime** (`schema_check.py`): llama-server accepted the schema, and the
        4B (template fallback) produced `{"kind":"tool","name":"write_file","args":{"path":"hello.py",
        "content":"print('hello')"},"answer":""}` in 3 of 3 seeds.
    - **Fix 2, routing checked against the wording.** `request_router::reconcile_with_wording`: when the
      model says ask or plan but the wording rules call the message work, it runs as agent.
      - Questions about work ("can we add caching here?", "what would it take to add dark mode?", the
        live project question) stay questions, as do follow-ups with no work wording.
      - Tested with the classification instruction's own examples. The response source stays "model".
    - `cargo test` 436 passed, 0 failed, 0 warnings; build ok.
    - **Owner question (05:51): "how are requests classified by other harnesses?"** Researched and
      answered.
      - Cursor, GitHub Copilot, Claude Code, Cline and Aider let the user choose the mode (Ask / Plan /
        Agent or equivalents); inside Agent the model decides whether tools are needed.
      - Roo Code is the exception: the model calls a `switch_mode` tool with the user's permission, and an
        Orchestrator mode delegates by each mode's "When to Use" text.
      - Codex CLI has no request classes, only approval modes and a sandbox. Mica's model router only
        decides web search.
      - Recommendation given: keep the wording guard, and add an Auto/Ask/Plan/Agent picker in the code
        composer. Waiting for the owner's answer.
    - Code suite restarted (`chain_after2.sh`) on this build.

41. **06:15 Owner: "Add more claude code like routing" → full Claude Code model** (owner answers ~06:00: full
    model, pause the code tests and build now, solo build plus a review workflow of at most 4 agents; ~06:08:
    "make the edit auto too so that you can see them work autonomously").
    - **Routing:**
      - No classification request. Every code-session message, except slash commands, starts an agent run,
        which answers or acts.
      - Removed: `/api/chat/classify`, `request_router.rs` (sent to the Recycle Bin),
        `SidecarClient::classify_request` and the `classified` flag on chat and agent requests. The frontend's
        `classifyRequest` and the `e2e.mjs` / harness / app_bench uses went with them.
    - **Permission modes** (a global setting cycled with Shift+Tab in the code composer; four-button picker;
      Settings select; Permissions summary):
      - **ask:** reads and searches free; edits, commands and deletion ask (`WorkspaceAgent`, previously
        `Assisted`, which asked even for reads).
      - **accept_edits:** new `AcceptEdits` level. Reads and file edits (write/append/edit_file,
        create_document) in the project run without asking; commands, delete_file, git_commit, open_path and
        web_search ask.
      - **plan:** runs read-only (`AgentMode::Plan`).
      - **auto:** `Autonomous`.
      - Settings gain `agent.permission_mode`: derived from `autonomous_enabled` for older files, kept in sync
        with it, and a client that only flips the switch changes the mode.
      - Switching mode releases waiting actions the new mode allows.
    - **Plan approval:**
      - A completed plan run that is the session's latest run shows "The plan is ready" with Yes, accept edits
        / Yes, ask before edits / No, keep planning.
      - Yes switches the mode and sends "Implement the plan", which resumes the planned task.
      - `/plan <task>` now starts a real plan run.
    - **Plan-run quality:** a tool-free reply before anything was read gets one push to inspect first
      (measured in the UI check: an 8B plan run ended on "Let me first locate orders.py").
    - **Checked live in the browser pane** (isolated verify data dir, `ui-check` project):
      - the picker shows the four modes; Shift+Tab cycled Plan → Auto → Ask → Accept edits → Plan with focus
        kept in the composer;
      - a Plan-mode request ran as a plan run (`mode: plan`), and the approval bar appeared (scroll-into-view
        added after);
      - "Yes, accept edits" switched the mode and started the implementation. The model's wrong path
        `orders.py` got the new "Found with that name: project/orders.py" hint, and it read, wrote and
        appended without a prompt.
      - Result: `low_stock` added correctly. The 8B put its test in orders.py instead of test_orders.py
        (model).
    - **Tests:** backend 433 passed, 0 failed, 0 warnings (new: accept-edits and ask-mode permission tests;
      three tests updated from "reads ask"). Frontend 83 pass (routing, mode cycling, plan approval, four-mode
      summary), tsc clean, vite build ok.
    - Live harness: code tasks go straight to `/api/agent/run` with the backend in Auto mode (owner).
    - Review workflow running (3 reviewers + 1 verifier, read-only).

42. **06:45 Routing review: 13 of 20 findings confirmed, all fixed before the code suite reruns.** Workflow
    `wf_141cfea8-878` (3 reviewers, 1 verifier, read-only). Refuted: #4, #10, #11, #16, #18, #19.
    - **Safety:**
      - **Mode cycling released waiting actions (#0, #7).** Shift+Tab passed through Auto, and every step saved
        the mode, which releases waiting actions in other sessions; a press during a save was dropped. Now
        Shift+Tab cycles Ask → Accept edits → Plan only (Auto only from the picker), shows the change at
        once and saves where it stops (450 ms). Saves go one at a time, always the latest choice
        (`PermissionModeSaver`), and sending a message first settles a pending save.
      - **Accept edits could reach command execution through `.git` (#1).** An edit to `.git/config` or a hook
        is a command the app's own `git status`/`git diff` would run. Edits inside any `.git` folder now ask in
        Accept edits (`tools::touches_git_internals`, `PermissionManager::decide_call`, used by the agent gate,
        the direct tool endpoint and the resume path). The app's git reads also switch off what a repository
        can configure: `-c core.fsmonitor=false`, `--no-ext-diff`, `--no-textconv`.
      - **A session grant released a waiting web search (#3).** Search consent is its own prompt; only Auto
        releases it on resume.
    - **Questions to the agent (#2, #12, #13):** with every code-session message going to the agent, questions
      got the "no action taken, start the work" push and a CSV/PDF mention forced a document. A question
      (`asks_for_information`: starts with what/why/how/which/where/when/who/is/are/does/do/did/explain/
      describe/summarize, or ends with "?"; "can/could/would/will/please …" are requests) skips both. The push
      itself now says a question gets its answer.
    - **Plan approval (#5, #6, #8, #17), following Claude Code's ExitPlanMode:**
      - New `present_plan` tool, offered only in plan runs. A plan run ends with it, a question gets a plain
        answer, and only a presented plan shows the approval card (`plan_ready` on the run summary; journal
        and continuation status `planned` only then).
      - Small-model fallbacks: the prose written before the call counts as the plan when the argument is
        shorter; a plan written as a final reply gets one request to present it, and `present_plan` with an
        empty argument then presents that reply (no need to write it twice).
      - The card counts as answered only once the work started (or Plan mode is in effect); its buttons are off
        while that happens, and a missing model or a busy session says so.
      - "No, keep planning" switches to Plan mode.
    - **Inspector (#9):** starting work no longer opens the inspector or shows a "Work started" toast; progress
      shows in the conversation.
    - **"yes"/"go ahead" after an offer (#14):** a reply ending in a question makes "yes"/"ok" a task for the
      model; "go ahead"/"do it" first continue a saved task, and only when none exists accept the offer.
      "Implement the plan" still resolves to the planned task.
    - **Images in agent runs (#15):** code-session questions used to go through chat, which attached images.
      Agent runs now attach them on the message they were sent with, and an image sent with the task goes
      on the task turn (with a projector loaded). Shared helper `prepare_conversation_images`.
    - **Tests:** backend 438 passed, 0 failed, 0 warnings (new: `.git` edits, Accept edits on `.git`,
      question detection with document kind, a reply that asks, a session grant and web search). Frontend 84
      pass (keyboard cycle without Auto, serialized/debounced saves, `plan_ready`), tsc clean, vite build ok.
    - **Rechecked live in the browser pane (07:00, 8B, isolated verify data dir, `ui-check` project):**
      - Four quick Shift+Tab presses from Accept edits went Plan → Ask → Accept edits → Plan: Auto was never
        passed, and only one save was sent.
      - **A question in Plan mode, first try:** the question skipped the read-first push, and the 8B answered
        "What does orders.py compute, and what is the tax rate?" without opening the file, inventing
        discounts and a region-dependent tax. So questions keep the push, reworded towards reading
        (`READ_BEFORE_ANSWERING`: read the files, answer from them, change nothing).
      - **Second try:** its "if it needs no files, answer again" way out was taken, and the 8B repeated the
        invented answer word for word. The way out was removed. The real cause was the prompts' own
        "answer it directly" / "otherwise answer directly", now "read the files it concerns and answer from
        them" / "Answer a question about the project from the files it concerns, read first".
      - **Third try:** the 8B listed the folder, read `project/orders.py`, and answered correctly (order
        totals, `TAX_RATE = 0.08`). No approval card, no toast.
      - **A change request in Plan mode:** the 8B searched and read both files, then called `present_plan`
        itself (`plan_ready: true`), and the card appeared. "Yes, accept edits" switched the mode (one toast),
        the continuation started the planned task as an agent run, the card went away, and the inspector
        stayed closed. The 8B wrote `total_quantity` and its test with no prompts; the project's 3 tests pass.
      - Not exercised live: "No, keep planning" (it calls the same mode change that was exercised).
      - Also fixed: after a backend restart the inspector toasted "agent stream failed: 404" for a run the
        server no longer holds (runs live in memory). A 404 is now dropped quietly.
      - Also: `create_document` is no longer offered to a question that mentions a document type.
      - Tests after these: backend 438 passed, frontend 84 pass, tsc clean.
    - `ui-check` project sent to the Recycle Bin. Code suite rerun in Auto mode on the five models next
      (`chain_after2.sh`).

43. **07:15 Owner: start scripts for other systems. Also: the code suite stopped for a question defect and
    restarted.**
    - **Owner request:** the offline notice named only `.\run.ps1`; add variants for other systems and have the
      message name them.
      - New `run.sh` (Linux, macOS), with the same steps as `run.ps1`:
        - builds the runtime once through `scripts/build-runtime.sh` unless `COMPANION_LLAMA_SERVER_BIN` is set;
        - runs `npm ci` when `node_modules` is missing, then `npm run build`;
        - runs `cargo run` with `COMPANION_ADDR=127.0.0.1:5173`.
      - Also in `run.sh`: plain errors when npm or cargo is missing; Windows shells are pointed at `run.ps1`;
        sourcing is refused.
      - The notice, the sidebar and the welcome screen name both scripts (`StartScripts`), this computer's first
        (`startScriptsFor`: macOS/Linux/ChromeOS browsers get `./run.sh` first; Windows or unknown get
        `.\run.ps1` first).
      - The backend's "frontend build not found" warning names its own platform's script. README (tree,
        prerequisites, runtime section, Run, `COMPANION_ADDR` row) and HANDOFF updated. At commit:
        `git add --chmod=+x run.sh scripts/build-runtime.sh` (commits from Windows lose the bit; the README
        also gives `chmod +x` / `bash run.sh`).
      - Found in passing: `scripts/build-runtime.sh` expanded possibly-empty arrays with `"${a[@]}"` under
        `set -u`. macOS's bash 3.2 stops on that when ninja is missing. Now `${a[@]+"${a[@]}"}`.
      - **Checked:**
        - `bash -n`;
        - 15 path checks with fake npm/cargo/uname in Git Bash: first run, later run, override, runtime build
          failure, build without llama-server, UI build failure, backend error, Windows shell, missing cargo,
          sourcing.
        - The sourcing check found a defect, now fixed: the guard came after `set -euo pipefail`, which switched
          on exit-on-error in the user's shell and closed it.
        - Browser pane, backend stopped: the notice reads "…with its start script: `.\run.ps1` on Windows or
          `./run.sh` on macOS and Linux."
        - Not run on a real Mac or Linux machine.
      - Review workflow (owner approved, at most 4 agents: macOS bash 3.2/BSD, Linux and parity with
        `run.ps1`, UI/docs, plus 1 verifier) running.
    - **Code suite defect (the owner chose "stop, fix, rerun"):**
      - The 4B's question ("What does the project … do, and which function computes the order total including
        tax?") first answered correctly (`order_total`).
      - The completion check then asked for the tests to be run. The 4B's commands used wrong paths, and the
        run failed after 6 unsuccessful steps.
      - Cause: since decision 41 every code-session message is an Agent run, and the check compares an
        answer with a "request". Read-only runs skipped the check for the same reason.
      - Fix: a question (`asks_for_information`) with no pending evidence requirement finishes on its answer.
      - The same conversation's change task then failed too. The 4B only announced actions ("I will now
        implement…") after the failure narrative in its history. Watch in the rerun.
      - Stopped by PID (chain bash, harness, backend, model server); the rows are marked invalid in
        `results.jsonl`, and the 4B's folder went to the Recycle Bin.
      - Rebuilt: backend 438 passed; frontend 85 pass (new `startScripts` test); tsc clean.
      - Code suite restarted at 07:16 (`chain_after2.sh`).

44. **07:40 Start-script review applied and the Linux build run for real; the suite stopped again for a 4B
    format loop; the owner asked for a 4B Gemma with coding and tool-use skills.**
    - **Review** (workflow `wf_5546fe2e-0de`: 3 reviewers + 1 verifier): 17 findings.
      - Refuted #12: the sourcing guard was already before `set -euo pipefail` when the verifier read it.
      - Plausible #3: the executable bit; covered by the commit instruction in HANDOFF and README.
      - Duplicates: #6, #7, #10.
      - Confirmed and fixed:
        - **#0 (high, existing script):** `scripts/build-runtime.sh`'s module list ran `ls libggml-* ggml-*`
          under pipefail. Off Windows `ggml-*` matches nothing, so every real Linux/macOS build stopped after
          staging without a message, and `./run.sh` could never finish its first run. Only `--dry-run` had
          ever been run. Now `{ ls … || true; }`, with version suffixes stripped from the recorded names.
        - **#1:** `runtime/bin` was not self-contained. The binaries kept CMake's absolute build-folder library
          path, and the versioned links (`libllama.so.0`) were not copied. Now
          `CMAKE_INSTALL_RPATH=$ORIGIN` (macOS `@loader_path`) with `CMAKE_BUILD_WITH_INSTALL_RPATH=ON`, as
          upstream's release build does, and links copied with `cp -P`.
        - **#2:** without ninja (a stock Mac), the build ran on one core; `--jobs` now defaults to the core count.
        - **#4:** `run.sh` started through a symlink looked for the project in the link's folder; the link is
          now followed (portable, no `readlink -f`).
        - **#5:** the backend's missing-runtime error named Vulkan and CUDA on a Mac; it now says "the backends
          this machine supports".
        - **#8:** no `.gitattributes`, so a Windows checkout gave the `.sh` files CRLF and Linux bash could not
          run them; added `*.sh text eol=lf`.
        - **#9:** a folder shared between Windows and WSL. The Linux build would have moved the Windows
          runtime aside. `build-runtime.sh` now refuses when `runtime/bin/llama-server.exe` exists, and the
          README says to use one clone per system.
        - **#11:** an empty `COMPANION_LLAMA_SERVER_BIN` made the backend refuse the runtime `run.sh` had just
          built; empty now counts as unset.
        - **#13:** Chrome OS browsers report "Chrome OS", which the pattern did not match; fixed.
        - **#14:** the test did not render the component; it now renders `StartScripts` with Windows,
          Chrome OS and Safari-style navigators and checks the full order of both lists.
          `startScriptsText` (used only by the test) was removed.
        - **#15, #16:** README fixes: the Windows first-run condition, and bash forms of the runtime check,
          downloader, checks and troubleshooting.
    - **Checked:**
      - `run.sh` path tests: 15 pass in Git Bash, 16 in WSL Ubuntu (real Linux bash, including the symlink
        case).
      - **Real CPU build in WSL Ubuntu, in an isolated copy under `~` (not this folder):**
        - the guard refuses a Windows runtime;
        - the build finished in 57 s with exit 0; the old module line exits 2 on the same files;
        - RUNPATH is `$ORIGIN`, and every project library resolves from `runtime/bin`;
        - after deleting the build folder and moving the project folder, `llama-server --version` runs, and
          `llama-bench` loaded `libggml-cpu-alderlake.so` from `runtime/bin` and ran a small model: prompt
          reading 194.6 tok/s, generation 26.7 tok/s (8 threads, WSL; higher is faster).
        - The copy is left at `~/companion-buildtest-moved` in WSL for the owner to remove or keep.
      - Backend 439 passed, frontend 86 pass, tsc clean.
      - Not run on a Mac.
    - **Code suite, second stop (07:24):**
      - With the question fix, the 4B's question passed (read `project/README.md` and `project/orders.py`,
        answered correctly in 6.1 s).
      - Its change task failed again with no failure narrative in history, so history was not the cause.
      - The model requests show why. The 4B's template has no tool support, so its runs start in the
        schema-constrained JSON format. There the 4B wrote `{"kind":"final","answer":"I will now add the
        function…"}` eight times, and never `kind: tool`; in the question run it even put a text tool call
        inside a final answer. The existing recovery leaves the constrained format only after invalid
        arguments.
      - Fix: `ActionResponsePolicy::final_sent_back`. A constrained final answer the run sends back (the
        no-action push, plan pushes, or the completion check finding work left) returns the next step to the
        native text format. The native format worked for the 4B in the question run. Two unreadable native
        replies still return to the constrained format. New unit test.
      - Rows marked invalid; the folder went to the Recycle Bin; code suite restarted at 07:35 with the
        rebuilt binary.
    - **Owner request (07:30): a 4B Gemma with good coding and tool use; download it, instructions later.**
      - First read as 12B: the widely shared community fine-tunes are 12B (yuxinlu1
        `gemma-4-12B-agentic-fable5-composer2.5-v2`, self-reported tau2 ~55% vs ~15% base on 20 tasks).
        The owner corrected it to 4B.
      - For 4B, Google's model card has the numbers: Gemma 4 E4B, 4.5B effective / 8B total, native tool
        calling, tau2 42.2% (E2B 24.5%), LiveCodeBench v6 52.0%. The 4B community fine-tunes publish no
        benchmarks and have few users.
      - The Hugging Face API shows the files use the `gemma4` architecture, which the pinned runtime loads.
      - **Owner chose:** `gemma-4-E4B-it-qat-UD-Q4_K_XL.gguf`, 4.22 GB, from `unsloth/gemma-4-E4B-it-qat-GGUF`
        (a 4-bit quant of Google's quantization-aware-trained checkpoint).
      - Downloaded with `models/modeldownloader.py` (SHA-256 verified) to `build/model-downloads/gemma-4-e4b-it-qat/`
        (git-ignored, not scanned by the app). The load probe was skipped so nothing touches the GPU during
        the suite. It moves into `models/` between runs or when the owner says. No projector or MTP file.

45. **07:45 Code suite stopped a third time; the owner replaced the Gemma 3 4B with the Gemma 4 E4B.**
    - **What stopped it:** the 4B's change task, still in the constrained JSON format, failed in 4.2 s.
      - `list_directory` came three times with empty args (identical result, host note given).
      - Then `read_file` `orders.py` three times. Each error said "Found with that name:
        project/orders.py".
      - The run stopped on the repeated failure.
      - In the native format the same model fixed that path at the first hint (its question run). In
        decision 39, again constrained, it repeated `orders.py` too.
    - **Where the constrained default came from:** it goes back to commit `ac77a1b`, "template without tool
      support starts constrained". No measurement is recorded, and tonight's evidence for this model points
      the other way.
    - **Open, not fixed:** a constrained step that makes no progress (a failed or identical call) could
      return to the native format, as a final answer sent back now does (decision 44). The model that shows
      it is gone, so the change could not be measured; it is left open.
    - **Owner (07:45):** "remove the 4B file that keeps failing and replace it with the one that you just
      downloaded, it has tool support".
      - Recycle Bin limit checked first: C: 49,360 MB, permanent delete off. `models/gemma3-4b` (2.4 GB) went
        to the Recycle Bin.
      - The download (SHA-256 `df0fd4ee…`, `gemma4`, chat template present) moved to
        `models/gemma-4-e4b-it-qat/`.
      - Runtime probe (`llama-fit-params`, nothing else on the GPU): loads, 5,258 MiB projected with all
        layers on the GPU at the default context.
    - **Rows:** the 07:35 session's 4B change row is marked invalid; its question row stands as a Gemma 3 4B
      record.
    - **Plan (`chain_after3.sh`, started 07:47):**
      - 5-minute cooldown;
      - the chat suite for the E4B (a new model in the library of eight);
      - 5-minute break;
      - the code suite: E4B, 8B, 12B, 27B, 30B;
      - the measurement queue unchanged.

46. **07:50 Owner: "for the coding run please only test the 4B model for now, park the rest for later, I need
    this project to be at least ready for showcase today with the 4B model".**
    - **Parked:** the code suite for the 8B/12B/27B/30B, and the whole measurement queue (fence, overhead, CPU
      track, C6, app_bench, M18, M8, M10, M21). The queue is parked too because it runs for hours and no
      compiling is allowed during it, which would block any fix the showcase needs today.
    - `chain_after3.sh` was stopped during its cooldown (nothing had run). `chain_e4b.sh` started 07:50: a
      2-minute gap, the E4B code suite (question, change, calculator, clock), a 5-minute break, then the E4B
      chat suite. Code comes first as the riskier half.

47. **07:55 Owner: "delete WSL stuff and we will commit everything after today's showcase section is done".**
    - The WSL build-test copy (`~/companion-buildtest-moved`, 252 MB) was moved out of WSL into the scratchpad and
      sent to the Recycle Bin (WSL has none). The WSL instance started for the test was stopped again
      (`wsl -t Ubuntu`; state Stopped, as before).
    - **Commit:** everything goes in one commit once the showcase section is done, with
      `git add --chmod=+x run.sh scripts/build-runtime.sh` for the two scripts.

48. **08:00 E4B change task failed on Gemma's native tool-call syntax; owner rule for models without tool
    support.**
    - **E4B code suite so far:**
      - Question passed (6.1 s, correct).
      - Calculator passed (38.8 s, screenshot saved).
      - Change failed after 18.3 s. The E4B wrote its calls in the Gemma 4 native syntax
        (`<|tool_call>call:NAME{key:<|"|>text<|"|>}<tool_call|>`), sometimes with a progress note before
        the call, quoted keys, an `args:` wrapper, no closing marker, or the file text in a `<<<CONTENT` block
        closed with a bare `>>>`.
      - The app's Gemma parser read only a whole reply of the form `call:NAME{args:{JSON}}<tool_call|>`, so
        six calls were rejected as unreadable. The recovery then fell back to the constrained JSON format,
        where the model named `list_directory` with a `text` argument, and the run stopped.
    - **Fix written, compiled after the suite** (no compiling during a run). `gemma_action` reads the syntax
      the way llama.cpp's own Gemma 4 parser does (`common/chat.cpp`, pinned commit): `<|"|>` strings,
      unquoted keys, bare numbers/booleans/null, nested objects and arrays. It also accepts the JSON forms
      the model mixed in, one `args:` wrapper, a note before the call, a missing closing marker (only once the
      turn has ended), and raw blocks before or after the closing marker.
      - A bare `>>>` closes a raw block only as the reply's final line.
      - Still refused: two calls, a call inside a code block, text after the call, an invalid name,
        non-object arguments.
      - Two old test expectations changed on purpose: a note before the call, and a call left unclosed at the
        end of the turn, are now accepted (the fenced envelope already accepts both). The recorded replies
        are the new test fixtures.
    - **Owner rule (08:00): "model with no tool support will be able to do limited things, hence the support
      of tools from the harness side will be limited".** Owner chose: chat plus read-only code.
      - A model whose template reports no tool support chats normally. In code sessions it may list, read and
        search to answer questions.
      - Accept edits, Auto and plan approval are unavailable for it, with the reason shown before choosing
        (frontend first, backend enforces).
      - The constrained JSON format stays, for reads only.
      - Gated on the template's reported capability, never a model name.
      - Not needed for today's showcase (the E4B has tool support); built after the showcase work.

49. **08:10 E4B first code session reviewed; page link check added; E4B code suite rerun on one build.**
    - **First session (07:47–07:57):**
      - Question passed, correct.
      - Change failed (decision 48).
      - Calculator "completed" in 38.8 s but does not work:
        - asked for one `index.html`, it wrote three files;
        - the page inside `calculator/` links `calculator/style.css` and `calculator/script.js`, so neither
          loads, and the screenshot is unstyled;
        - the script ends with a stray `</script>`, and the buttons call functions that are not global.
        - The run's completion check accepted it.
      - Clock completed in 60.7 s and works: styled, ticking live clock, Stopwatch and Countdown tabs.
      - Screenshots and events kept as `*-run1`.
    - **App-side addition (generic, deterministic):**
      - Pages a run writes (`.html`/`.htm`) have their local `src`/`href` references checked on disk before a
        completion claim (`page_reference_problems`).
      - A missing target keeps the run going with "the browser resolves src and href from the page's own
        folder", naming each broken link, plus "use \"style.css\"" when that file sits next to the page.
      - Links with a scheme, `//`, `#`, root-relative `/` paths, `data-src` and template placeholders are
        ignored.
      - Test from the live page's shape. The model's JavaScript quality is its own and is not checked.
    - Backend 441 passed. The chain was stopped in its break before the chat suite (07:58, nothing running).
    - Rebuilt; the E4B code suite reran from 08:10 (`chain_e4b.sh`: code, a 5-minute break, then chat).

50. **08:13 Second E4B code session (08:02–08:13), on the native-call parser.**
    - **Question:** passed in 2.1 s. Four native calls read without a single retry, answer correct.
    - **Change:** failed.
      - Reads and an `edit_file` in native syntax worked, and `low_stock` landed in `orders.py`.
      - Its first full-file `write_file` closed the text block with `<<<CONTENT>>>` and then started a second
        call.
      - Three `append_file` calls came as `<|tool_call>call:append_file{path:...}<tool_call|>` with no text.
        In that syntax the call ends the model's turn, so the `<<<CONTENT` block meant to follow never
        arrives. The model then pasted the test as a code block in a separate reply, and the run stopped
        on the repeated failure.
    - **Calculator:** failed.
      - Two whole pages (6.1 KB and 7.1 KB) were closed with `<<<CONTENT>>>`, so both were unreadable.
      - The recovery moved to the constrained JSON format. There the model wrote the whole page as
        `content` and named the tool `list_directory`, twice.
      - Cause: serde_json sorted the schema's keys (answer, args, kind, name), so generation reached the tool
        name only after the arguments, and the grammar allowed list_directory with extra arguments.
    - **Clock:** passed again, in 54.3 s.
    - **Fixes (compiled offline; backend 442 passed):**
      - A text block closes with `CONTENT>>>` or `<<<CONTENT>>>`. A line that is exactly the opening marker
        with `>>>` appended cannot be file text by accident.
      - A write or edit that arrives without its text gets a correction saying where the text goes, in both
        syntaxes: after the arguments in a block, or, in the `<|tool_call>` form, inside the call as
        `content:<|"|>…<|"|>`. It also says a code block in a separate reply is not written.
      - Rule 2b gains one line for the `<|tool_call>` form. To keep the fixed instructions under 50% of a
        4K window's history room (the test caught 52%), the 2b escaping sentence was shortened.
      - serde_json `preserve_order` (indexmap was already in the lock file; nothing downloaded). The
        constrained schema's properties are now kind, name, args, answer in generation order, and the
        format instruction says "in this order". Test checks every alternative's order.
      - A stray quote in an unquoted key (`path":"."`) is ignored.
    - Harness: `LIVE_CODE_TASKS=change,calculator` reruns selected tasks.
    - Run 2 screenshots and events kept as `*-run2`; test folder to the Recycle Bin.
    - `chain_e4b2.sh` started 08:20: change and calculator, a 5-minute break, then the E4B chat suite.

51. **08:20 Owner: "when you commit and push the changes I want you to also commit THIS 4B model to the
    repository".**
    - **Constraint (GitHub docs, checked today):**
      - Normal pushes reject files over 100 MB.
      - Git LFS allows at most 2 GB per file on GitHub Free and Pro (4 GB Team, 5 GB Enterprise Cloud).
      - Free and Pro include 10 GB of LFS storage and 10 GB of bandwidth a month.
      - The file is 4,021 MiB (4.22 GB); git-lfs 3.7.1 is installed.
    - **Owner chose:** split into parts of about 1.4 GB with llama.cpp's `llama-gguf-split` (split GGUFs load
      natively; the README already documents keeping parts together in one folder), tracked with Git LFS.
    - **Plan, after the showcase tests (no heavy disk or GPU work during them):**
      1. Split into `models/gemma-4-e4b-it-qat/`.
      2. Check the app lists one model and loads and chats with the parts.
      3. Send the single file to the Recycle Bin.
      4. `git lfs track` the parts, and add a `.gitignore` exception for that folder.
    - Clone cost: 4.2 GB of the monthly LFS bandwidth per clone.

52. **08:25 E4B code suite passes on the showcase build.**
    - **Change** (08:18–08:20, 16.3 s, no retries):
      - listed and read the project, including `sample_order.json`;
      - wrote `low_stock` (a list comprehension) into `orders.py` and three unit tests into
        `test_orders.py`;
      - ran `python -m unittest` itself. 5 tests pass (checked again after the run).
    - **Calculator** (47.8 s):
      - one `index.html` of 5,008 bytes with inline CSS and JS, as asked, and a styled keypad;
      - twice a write arrived without its text; each time the new correction (decision 50) led to the
        right form on the next step;
      - the completion check had it list the root once, then accepted.
    - **Clicked through in the browser pane:** 7+5=12, 6×7=42, 9/4=2.25, 3−8=−5, 1.5+2.25=3.75,
      2+3×4=14 (precedence right), C → 0, no console errors. The model's own flaws: a duplicate "=" button
      and "/" where the task wrote ÷.
    - **Clock** (run 2, the previous build): styled, ticking live clock, Stopwatch and Countdown tabs. That
      folder was recycled before this rerun, so only its screenshot remains.
    - **Question** (run 2): correct in 2.1 s, four native calls, no retries.
    - E4B chat suite follows after the 5-minute break.

53. **08:32 E4B chat suite: 9 answers, all sound.**
    - **Speed:** first text after 0.08–0.32 s; generation 113–129 tok/s (higher is faster); no truncation,
      no reasoning text, no leaked tool blocks, no stray actions.
    - **Answers:**
      - facts: seasons from axial tilt, 4 sentences;
      - reasoning: 14:35→17:10 = 2 h 35 min, with steps;
      - writing: a polite two-paragraph email;
      - format: 5 tips, each under 12 words;
      - code explanation: `ZeroDivisionError` on an empty list;
      - multi-turn: remembered its own three names;
      - document: a markdown checklist;
      - summary: faithful in one sentence, with one embellishment ("$2.3 million", the source names no
        currency).
    - **Showcase next:**
      1. a demo rehearsal in the app with the E4B (isolated verify data);
      2. split the model and check the app loads the parts;
      3. report, then commit and push when the owner confirms.

54. **08:35–09:05 Demo rehearsal in the app (E4B, isolated verify data, `showcase-rehearsal` copy of the test
    project), and what it fixed.**
    - **Flows passed:**
      - chat: first text 0.5 s, 100 tok/s;
      - Ask-mode question: 5 steps, correct;
      - Plan → "No, keep planning" (card closes, Plan stays, composer focused) → refinement → "Yes, accept
        edits" → implementation; the test command asked for approval; the model fixed its own missing
        import after a failing run; 3 tests pass;
      - Auto to-do app: `todo/index.html`, tested over localhost: add, tick, delete, blank ignored,
        persists;
      - model list after the Gemma 3 4B folder was deleted: 8 models, the old one gone.
    - **App fixes found by the rehearsal** (backend 444 passed, frontend 91 pass, tsc clean):
      - The answer showed twice in the activity list, and rows read "Step 15" for a 5-step run.
        `visibleTimelineEvents` now hides a progress note the final answer repeats and keeps only the latest
        "preparing the next action" row; rows show their model step. Tests added.
      - The approval card named only `execute_command`, not the command. `approvalTarget` now shows the
        command and its folder, the path, the commit message or the query. Checked live: "python -m unittest
        test_orders.py (in project)".
      - "View changes" showed every line removed and re-added. `unified_hunks` is now a real line diff with
        3 lines of context (common prefix/suffix trimmed, LCS on the middle, whole-middle fallback above 4M
        cells). Checked live: one `@@ -13,7 +13,7 @@` hunk.
      - "Allow for session" was offered for commands but only moderate-risk tools keep a grant, so it was
        silently ignored. `PendingTool.session_grantable` now makes the UI offer it only when it applies;
        commands still ask every time (unchanged security design).
      - A native call that lost its `<|tool_call>call:` opener but kept `<tool_call|>` is now read for a
        registered tool name (`present_plan{plan:<|"|>…<|"|>}<tool_call|>`, verbatim), with its raw text
        no longer shown.
      - The to-do app first went into the wrong folder: `mkdir todo`, then `cd todo`, then `index.html`
        written to the root, and the report said "todo/index.html" was done. Now a bare `cd` gets a note
        that it does not carry over, and a folder the run created that is still empty holds up completion,
        with the fix named. Rerun: `todo/index.html` correct.
      - Two more E4B habits accepted: a stray extra `}` before `<tool_call|>`, and a block closed by
        repeating `<<<CONTENT` as the reply's last line.
    - **Left as rough edges** (listed in the rehearsal PDF): jargon in recovery status rows ("constrained JSON
      format"); code sessions stay titled "New task"; the "Last used with another model" banner stays after
      a reply; the model still makes small mistakes (a docstring edit removed its `def` line).
    - **Rehearsal PDF (owner request):** `docs/showcase/2026-09-17-demo-rehearsal.pdf`, 6 pages, from the
      `.html` next to it. It has a before-the-demo checklist (owner request), the exact prompts, expected
      results, what I saw, timings and rough edges. The demo project is in `docs/showcase/demo-project/`;
      screenshots are in `docs/showcase/img/`.

55. **09:05 Split model checked in the app (owner request).**
    - `llama-gguf-split --split --split-max-size 1500M` took 5 s, giving four parts: 30 MB, 1,513 MB,
      1,431 MB and 1,049 MB (largest 1.6 GB, under GitHub LFS's 2 GB). The parts are in
      `models/gemma-4-e4b-it-qat/`; the single file is held in `build/model-hold/` (git-ignored, not scanned)
      until the commit is done.
    - **App:**
      - scan lists one model, same id and name, tool support from the template, no load issues;
      - load 5.7 s, the runtime opened `…-00001-of-00004.gguf`, 32K, fully on the GPU;
      - chat: first text 0.22 s, 111 tok/s;
      - code question: read `project/README.md`, answered correctly in 2.0 s.
    - **Commit prep still to do (at commit time):** `.gitignore` ignores `/models/*` and `*.gguf`, so the folder
      needs an exception, plus `git lfs track` for its `.gguf` parts.
    - The rehearsal app was stopped and the model unloaded (nothing holds the GPU).

56. **09:20 Owner: fix session titles and the model banner, add a batch variant of run.ps1, then commit and push
    everything (with the split model).**
    - **Session titles:**
      - Auto-titling renamed only "New chat", so code sessions ("New task") kept that name.
      - It also looked the session up in the list captured when the message was sent.
      - Now `autoTitle` (tested) treats "New chat" and "New task" as placeholders, drops a leading slash
        command, and cuts at a word boundary with an ellipsis; it reads the latest list through a ref.
      - Checked live: a code session renamed itself to "Which file holds the unit tests?".
    - **"Last used with another model" banner:**
      - After a reply the page never reloaded the conversation list, so it missed the `last_model` the
        backend had recorded.
      - Agent runs never recorded `last_model` at all.
      - Now `send` reloads the list after a reply and after work starts, and `run_agent` records the loaded
        model on the session.
      - Checked live: a chat last answered by the 2B showed the banner with the E4B loaded; after one reply
        it was gone, and the code session recorded the E4B.
    - **run.bat (owner: "because of the execution policy bypass issue"):**
      - Same steps as run.ps1 from Command Prompt: npm/cargo checks, a one-time runtime build through
        `scripts\build-runtime.ps1` with `-ExecutionPolicy Bypass` for that process only, `npm ci` when
        `node_modules` is missing, `npm run build`, and `cargo run` on `127.0.0.1:5173`.
      - `setlocal` keeps `COMPANION_ADDR` inside the script, every tool starts with `call`, and the file is
        CRLF (`.gitattributes`: `*.bat`, `*.cmd` eol=crlf).
      - 13 path checks pass in cmd.exe with fake npm/cargo/powershell: first run and order, later run,
        override, runtime build failure, build without llama-server.exe, UI build failure, backend error,
        no env leak.
      - Found on the way: `NoDefaultCurrentDirectoryInExePath=1` in this shell stops cmd.exe finding a
        script by bare name (the test calls it by full path; run.bat itself uses `%ROOT%` paths).
      - The in-app restart message, the backend warning, README, HANDOFF and the rehearsal PDF name
        `run.bat` next to `.\run.ps1`.
    - **Tests:** backend 444 passed; frontend 92 pass; tsc clean; PDF regenerated.
    - The rehearsal project went to the Recycle Bin; the app was stopped and the model unloaded.
    - **Commit:** everything, with the model's four parts through Git LFS (`.gitignore` exception for
      `models/gemma-4-e4b-it-qat/`) and the executable bit on `run.sh` and `scripts/build-runtime.sh`. After
      the push, the single-file copy in `build/model-hold/` goes to the Recycle Bin.
    - Pushed as `45b1656` (LFS 4/4, 4.2 GB); the single-file copy went to the Recycle Bin.

57. **17:30 Owner: split Gemma 4 E2B IT the way the E4B was split and push it; remove `scripts/split-model.sh`.**
    - **Pulled first:** `1d37014` (owner's later commit) added `scripts/split-model.sh`, a split-and-track helper.
    - **Defect found before using it (reproduced on a 30-byte fake GGUF in the scratchpad):**
      - The script's own help says to pass `--name` matching the repository folder, i.e. the folder the model
        already sits in. The script then stops with "move them aside or pass --force".
      - `--force` removes every `.gguf` in the output folder with `rm -f`, including the file being split,
        before the split runs: the model is gone (not in the Recycle Bin) and the split fails.
      - **Owner chose:** remove the script ("I don't want it"). It went to the Recycle Bin; history keeps it.
    - **Model:** `models/gemma-4-e2b-it/gemma-4-E2B-it-Q4_K_M.gguf` (unsloth/gemma-4-E2B-it-GGUF, checksum-verified
      at download), 3,106,738,272 bytes, SHA-256 `740185b2…34b8`.
    - **Split, same way as the E4B:**
      - The single file moved to `build/model-hold/gemma-4-e2b-it/` (git-ignored, not scanned).
      - `runtime/bin/llama-gguf-split --split --split-max-size 1500M` took 4 s: three parts of 43,316,608,
        1,614,807,232 and 1,448,614,752 bytes (largest 1.61 GB, under GitHub LFS's 2 GB). Part 2 is one
        tensor, the per-layer embedding table, which gguf-split cannot divide.
      - The parts total 320 bytes more than the source (the `split.*` keys).
    - **Lossless check:** reading source and parts with gguf-py, all 56 metadata keys match (the `split.*` keys
      aside), and all 601 tensors match in order, type, shape and bytes (3,090,917,516 bytes, one SHA-256 over
      all tensor bytes identical). The check caught a single flipped bit in a scratch copy of part 1.
    - **App check (verify data, one model at a time, on AC), before and after the split:**

      | | single file | split |
      |---|---|---|
      | listed | one model, id `gemma-4-e2b-it` | same; only `model_file` and size (+320 bytes) differ |
      | runtime opened | `…-Q4_K_M.gguf` | `…-Q4_K_M-00001-of-00003.gguf` |
      | load | 4.7 s, GPU, 32K | 4.3 s, GPU, 32K |
      | chat, one-line answer | 132.8 tok/s | 146.7 tok/s |
      | chat, 141–143 tokens | 154.6 tok/s | 174.5 tok/s |

      Output speed, higher is faster; one run each, so the split is not claimed to be faster. Both answers
      were correct.
    - **Found, not caused by the split (same before and after):** in a code session the E2B answers the
      question "what does the project do…" with the text `list_directory\n{"path":"."}` (10 tokens). That is
      not a format the parser reads; the app pushes back once ("Nothing in the project has been read yet"),
      the model repeats it, and the run completes with that text as the answer in 0.5 s. The E2B code suite
      was never run (code tests were the 4B–30B). Not fixed here.
    - **Git LFS:** `git lfs track "models/gemma-4-e2b-it/*.gguf"`, `.gitignore` exception for the folder.
      - Storage: E4B 3.93 GiB + E2B 2.89 GiB = 6.82 GiB of the 10 GiB GitHub Free/Pro include (per account;
        uploads count toward storage only, not bandwidth). LFS objects can only be removed by deleting and
        recreating the repository (GitHub docs).
      - Bandwidth: a clone that fetches both now pays 6.82 GiB of the 10 GiB a month; when it runs out, LFS
        is disabled on the account until the next month.
      - Fetching one model, tested in a throwaway repository with a local LFS remote: `git clone -c
        "lfs.fetchinclude=<folder>/*"` fetched only that folder (the other stayed a pointer, also after a later
        `git pull`); `git config lfs.fetchinclude` before `git pull` in an existing clone did the same; `git lfs
        pull --include` fetched the skipped one later. README, HANDOFF and the rehearsal checklist say so.
    - **Commits:** the script removal on its own, then the model with its docs. After the push, the single-file
      copy in `build/model-hold/` goes to the Recycle Bin.
    - Pushed as `3a8f37a` and `d6d69c2` (LFS 3/3, 3.1 GB); the single-file copy went to the Recycle Bin.
    - The code-session problem above was handed to its own session (owner started it).

58. **18:00 Owner: a code session must stay in its project ("I switched the project and went back to an old chat
    … it started operating in a different folder"); owner approved grouping sessions by project.**
    - **Cause:**
      - Choosing a project in the sidebar picker while a code session was open re-linked that session to the new
        project; the only trace was a toast, "Linked this session to …".
      - Its next task then ran in the new folder. The backend checked only that the run's folder matched the
        session's link, and the link had already been changed.
      - Seen in the verify data: "develop a website…" (created in Test Python) and "UI check: routing" (created in
        ui-check) were both linked to showcase-rehearsal.
    - **Rule:**
      - A code session keeps its project once it has a message: its history names files in that folder.
      - Before its first message, or when it has no registered project (an older session, or its project was
        removed), a project can still be set.
    - **Frontend (first line):**
      - `services/projectSessions.ts` (11 tests): `sessionProject`, `canChangeSessionProject`, `projectPick`,
        `groupSessionsByProject`, `groupActivity`, `projectGroupOpen`.
      - The picker switches project instead of moving the session. It opens the session last used in that
        project (or its newest), or the project's new-task screen. It moves the open session only if nothing
        has been asked in it yet, or if it has no project.
      - Picks from a new-task screen (the project shortcuts, "Open or create a project…") never reopen an old
        session.
      - Opening a session sets the picker to its project, or to "Choose a project" for a session without one.
      - Sidebar "Tasks by project": the current project first and expanded, the others collapsed, each with a
        task count, a lamp when a task in it is working or waiting for approval, and a + for a new task there.
        Expanded/collapsed is remembered per browser. Sessions without a project form a last group.
      - Deleting the open session opens another in the same project, or its new-task screen.
    - **Backend (second line):** `PATCH /api/conversations/:id` answers 409 "this session already works in another
      project" when the workspace would change, or be cleared, on a session with messages whose project still
      exists, including via a hop through chat mode (`Storage::message_count`). Two tests. With the check
      switched off, the first test fails.
    - **Found and fixed on the way:**
      - `send` reloaded the session list from an older render (decision 56's refresh), where the session it had
        just created was not open. The list refresh then reopened that session (a history reload) and put the
        sent text back in the new-task draft, which came back on the next new-task screen.
        - `refreshConvs` now reads the open session from the ref and opens a session only on the first load, or
          when the open one is gone.
        - Sending from the new-task screen clears its draft.
      - The group's + button tooltip extended past the list, which scrolled the sidebar sideways by 120 px
        (scrollWidth 473 vs 289). It now uses a native tooltip; scrollWidth equals the list width.
    - **Live check** (verify data, two scratch projects with distinct files, E4B only, Ask mode):
      1. A task in proj-one listed `alpha.py` and `alpha_notes.md`.
      2. Picking proj-two left that session in proj-one and opened proj-two's new-task screen; a task there
         created a second session in proj-two.
      3. Reopening the proj-one session and asking again read `alpha.py` (answer: `alpha` returns 1). The
         session is still linked to proj-one.
      4. A PATCH moving it to proj-two returned 409 with the hint.
      5. An empty task created with proj-one's + moved to proj-two when proj-two was picked.
      - The model was unloaded before step 4, and the app was stopped afterwards.
    - **Tests:** backend 446 passed, 4 ignored; frontend 103 pass; tsc clean.
    - Not changed: the verify data holds two registered projects with the same name and path, which show as two
      groups. Registering a project does not deduplicate by path.
    - **Not committed (owner commits when asked).**
    - Committed and pushed as `dc3dad3` (owner: "commit and push everything").

59. **18:40 E2B code sessions answer with the text of a tool call; owner: make tool support model-agnostic for
    every model whose template supports tools.**
    - **Symptom (decision 57):** asked a code question, the E2B replied `list_directory\n{"path":"."}` (10 tokens);
      after one "read first" pushback it repeated it, and the run completed with that text as the answer.
    - **Not stripped tokens (measured):**
      - In both Gemma 4 files `<|tool_call>`, `<tool_call|>` and `<|"|>` are user-defined tokens (rendered as
        text); `<|turn>`/`<turn|>` are control. The two chat templates are byte-identical (18,808 chars).
      - Only difference: the E2B file marks `<eos>` normal with eos id 106; the E4B marks it control, eos 1, eot 106.
      - The recorded request (no `tools`, text tool list in the system prompt) replayed to the sidecar's
        `/completion`: the E2B's greedy reply is 11 ordinary text tokens, `list` `_` `directory` `\n` `{"` `path`
        `":"` `."` `}` `\n` `<eos>`. The chat endpoint returns the same text with no tool_calls.
    - **Model behaviour, not the runtime:** first-token probabilities for that same prompt:
      - E2B: `list` 57%, `print` 24%, a control token 14%; `<|tool_call>` not in the top eight.
      - E4B: `<|tool_call>` 79%, `list` 0.45%.
      - With the app's sampling (seed 1) the E2B wrote a sentence, then `<|tool_call>call:list_directory{path:` —
        it copies the app's own tool list (`- list_directory …` / `args: {"path":"."}`) when that list is
        the only description of the tools it gets.
    - **Why it was accepted as an answer:** no parser shape matches a bare name + object, so
      `looks_like_action_attempt` was false; the reply was a tool-free answer; the no-action pushback runs once;
      the second tool-free reply to a question with no pending evidence completes the run.
    - **Native tool calling measured (scratch harness, runtime's own tool definitions, read-only tools run on a
      copy of the demo project, same system prompt without the text tool list and envelope rules):**
      - The template renders the tools natively (`<|tool>declaration:list_directory{…}<tool|>`); the runtime
        (pinned build: PEG Gemma 4 parser, template-derived auto-parser, lazy grammar for call arguments) returns
        `tool_calls`.
      - E2B greedy: list `.`, list `project`, read README.md, read orders.py, answer naming `order_total`
        (5 steps, 1.3 s). Seed 2: the same, 1.1 s. Seed 1: one list, then a 1-token empty reply.
      - E4B greedy: the same 5 steps, 1.5 s. Seed 1: one list, then a 1-token empty reply. Seed 2: listed the
        absolute workspace path the prompt named, which the harness (not the app) rejects.
      - 17 of 17 calls arrived parsed; no call text leaked into content.
      - 7 of the 8 library models report template tool support (all but the Gemma 2 2B).
    - **Owner decisions:**
      - Not a parser patch for one model's shape: tool support is model-agnostic for models that support tooling.
      - A tool check per model, at the model's **first load** (new model, or its template or the runtime
        changed), with the user told beforehand that the first load takes a few seconds longer.
      - The result is a small configuration file **next to the model**, `models/<folder>/tooling.json`: the
        method that works (native calls, the text format, or none), where file text goes, when it was checked,
        and the runtime and template it was checked against. The agent follows it; no model names in code.
      - A model with no working method gets chat + read-only code (rule 48). A "Check again" in the UI.
      - Live code suite afterwards on the E2B, E4B and Qwen3 8B, one model at a time. Solo, no agents.

60. **19:00–20:45 Model-agnostic tool calling built (decision 59) and measured live.**
    - **Built:**
      - `backend/src/tooling.rs`: the per-model check (a native listing call, then a small Python file whose
        text must arrive byte for byte; if either fails, the same two in the text format), `tooling.json` next
        to the model (method native | text | none, `can_write`, `file_text`, runtime build, template
        fingerprint, each check's detail), staleness by version, runtime and template.
      - Load (`start_sidecar`): an unchecked or stale model is checked after the runtime starts (progress stage
        `checking_tools`), and the load returns `tooling_notice`. `GET /api/models` adds `tooling` and
        `tooling_state`; `POST /api/models/:id/tooling/check` re-checks the loaded model.
      - Runtime client: `ChatTurn` carries `tool_calls` / `tool_call_id` / `name`; requests can offer `tools`
        (`tool_choice auto`, `parallel_tool_calls false`); streamed call fragments are joined by index; the
        request log shows parsed calls. Strict-alternation templates get calls and results flattened to text.
      - Agent: native method = tool definitions for exactly the tools the run may use, a prompt without the
        text tool list, the parsed call as the step's action, every outcome of a call answered as a `tool`
        turn, `repair_tool_pairs` after pruning/compaction. `can_write: false` = read-only whatever the mode.
      - UI: a notice before a load that runs the check ("takes a few seconds longer"), "Checking how it calls
        tools" in the machine panel, the badge (Tools: native / text format / Read-only code), a Tool check
        section in the model's details with each check and Check again (loaded model only), and only Ask
        offered for a read-only model (Shift+Tab skips the others).
      - Also fixed: model details sized a split model by its first part ("0 GB"); `tooling.json` is
        git-ignored.
    - **Probe text, corrected:** the first file text had "first line with" and "last line:"; the E2B dropped
      them as labels in both formats while every backslash arrived intact, so it was marked read-only. The
      probe is now a small Python file (quotes, a Windows path, a `\n` escape, a tab, braces, non-ASCII) and
      the check version is 2. Results: E2B native in 0.6 s, E4B native in 0.7 s, Qwen3 8B native in 1.5 s, all
      writing (158 characters intact).
    - **Three app defects found by the live runs, all fixed:**
      1. *A note copied as file content.* Verified native writes had their text replaced by a note inside the
         `content` argument; after four such writes the E2B wrote the note itself into `clock/script.js`
         (144 bytes). Native call text now stays verbatim and is released oldest-first only when the window
         is short (`release_call_text`), like old results. After the fix the E2B's clock works.
      2. *The repeat stop ignored corrections.* `FailedCalls` counted identical failures for the whole run, so
         a test command failing again after a fix still reached "failed three times without a correction"
         (stopped the E4B and the Qwen3 8B mid-fix). A successful file change now resets the counts.
      3. *Repeated calls after a note.* The Gemma 4 template renders an assistant turn's text after the call's
         result, so "I will create __init__.py" read as the next step and the call was sent again. Over the
         evening's native runs: the next call repeated the previous one 7 times in 15 after a call with a
         note, 0 in 91 after one without. Native call turns now keep the call only (the note is journaled).
    - **Live code suite (native; before defects 2 and 3 were fixed):**

      | model | question | change | calculator | clock |
      |---|---|---|---|---|
      | E2B | answered, 4.1 s (named `order_total`, skipped what the project does) | failed: wrote the function into the test file, ran tests from a doubled path | page built; "=" types a space, so no result | works (three files, not one) |
      | E4B | answered, 4.0 s | failed: replaced `order_total`, missing import | page built; "=" computes but never updates the display; "." unlabelled | works |
      | Qwen3 8B | answered, 6.1 s | failed: wrong expectation in its own test | 7, 8, 9 missing; 30-step limit (completion check kept finding work) | works |

      Every call in every task arrived parsed. The remaining failures are the models' own code; the two
      calculators were clicked through (7 + 8 = shows 8) and their bugs read in the source.
    - **Change task, native vs text, after all fixes (E4B, alternating):** native passed 3 of 3 (8.1, 12.1,
      16.1 s); text passed 1 of 3 (20.2 s; the failures were format errors: two actions in one reply, an
      unterminated `<<<` block, an unclosed object, then a loop). Qwen3 8B native: 1 of 2 (30.2 s; the failure
      was its own test, a stray `}` and one edit sent three times unchanged, a correct stop). Small samples;
      the native path is not worse than the text format on the demo task and avoids its format failures.
    - Tests: backend 462 pass (4 ignored), frontend 108 pass, tsc clean. Not committed.

61. **2026-09-18 04:45–06:00 Why the big-model runs got stuck: diagnosis from the owner's own runs plus one live
    reproduction (owner asked, no fixes applied yet).**
    - **Evidence read:** the owner's four runs of 2026-09-17 evening (27B build + "resume from the last
      checkpoint", E4B, 26B) from the history database, and one live reproduction of the owner's prompt on the
      E4B (isolated data directory, 32K window, Auto mode, 30-step default): FAILED at the step limit, 344.8 s,
      24 tool calls, no runnable project (no index.html, no vite config, no entry point).
    - **1. The token estimate is inflated since native tool calling landed.** `observe_prompt_size` counts only
      each message's `content`, while the runtime's `prompt_tokens` covers the whole request. A native write
      carries the file text in the call's *arguments*, and the tool definitions (~4,400 characters) travel
      outside the messages, so the learned characters-per-token ratio collapses: measured on the owner's E4B run,
      content alone gives 0.86–1.4 characters per token where the real request is 3.0–3.6. The estimate the
      thresholds use is then 1.2–2.8x the truth (owner's E4B run: 94,587 estimated vs 34,219 real at step 22;
      live reproduction: 2.42x at step 13). Releases and pruning therefore start at about a third of the real
      usage — on a 128K window the E4B began losing file text at 34K.
    - **2. Compaction never runs on a coding task.** When compaction is due the loop releases old tool results
      first (down to threshold-10%), then re-checks: the release has already brought usage under the threshold,
      so the summary never happens. `compactions: 0` in all four owner runs and in the reproduction, while
      "Released N older tool result(s)" fired at steps 23, 26, 30, 34, 36 of the resume run. The destructive
      half of context management runs; the half that preserves the plan does not.
    - **3. Nothing sees a read loop.** `ProgressGuard` compares only against the *previous* success, so a
      rotation through 14 files never matches. The owner's resume run: 43 steps, 34 reads, 6 listings, **zero
      file changes**, 20 of the 43 calls identical to an earlier one, 16 targets read two or three times. No
      warning, no stop; the owner stopped it. The released-result marker even says "run the tool again if you
      need it", and the host's work log records only changes, never reads, so after a release the model has no
      record that it has already seen a file.
    - **4. Each step reprocesses the whole window once pruning starts.** Dropping the oldest turn every step
      invalidates the prefix: cached tokens fell to 1,257 of ~15,000 from step 22 on, and the step time went
      from a median 1.8 s to 18.4 s.
    - **5. "Resume from the last checkpoint" has no checkpoint.** A follow-up run gets the conversation's
      *messages* only — the run's work log, journal and progress note are never carried — so the first request
      was 3,206 tokens: the rules plus one sentence. The model re-explored from zero.
    - **6. The app wrote its own placeholder into two files.** After a release replaced an earlier write's text
      with `[Released: N characters ...]`, the 27B copied that sentence back as `content`: Header.tsx and
      PlanetView.tsx were written as 169-byte placeholder files. The next run then spent four steps trying to
      understand the file the app had corrupted. Nothing rejects a write whose text is the marker.
    - **7. A timed-out command loses everything it printed, and its real process survives.** `terminal.rs` kills
      `cmd.exe` only; the grandchild keeps the pipe. Measured with the same shape as the runner: 0 bytes
      captured, `node.exe` still running after the kill. Every `npm run dev` in the owner's runs came back
      "exit 1, (timed out and was killed), stdout (empty)" — and in the reproduction the model concluded from
      that "the development server is running successfully".
    - **8. There is no way to look at the result.** The registry has no page/preview tool; `search::extract_page`
      exists but is not wired to anything. Rule 8 of the prompt tells the model that opening a browser shows it
      nothing, which is true today, so "review it yourself and fix obvious UI issues" cannot be done.
    - **9. Smaller things measured:** the compaction threshold in the context popup comes from the last run's
      recorded usage, so a 90 -> 80 change only appears after the next step (`contextUsage.ts` prefers
      `usage.compact_at_pct`, and nothing refetches `/context` when settings are saved); POSIX pipelines fail on
      `cmd.exe` with no corrective hint (`'tail' is not recognized`, twice in one run); `npx tailwindcss init -p`
      failed three times identically in the 26B run (Tailwind 4 has no `init`; the identical-failure stop allows
      three).

62. **2026-09-18 05:30-06:55 The seven fixes from decision 61, built (owner approved 1-6, then 7 after an
    explanation; the owner also asked that models be taught to keep HANDOFF.md and MEMORY.md, and that the
    iteration limit stop being what ends a run).**
    1. **The token estimate measures the whole request.** `llamaserver::prompt_chars` now counts a call's
       `arguments` and the tool definitions, not only message text, so the learned characters-per-token ratio
       stops collapsing when a model writes files through the tool interface.
    2. **Compaction runs before releasing.** The loop used to release old tool results first and then re-check
       whether a summary was due, which it never was. The note is written first; pruning stays as the safety
       net below. The compaction block now also carries **what the run has already inspected** (a line per
       file read, folder listed, search and preview), and the released-result note no longer invites a re-read.
    3. **Loops are caught across a run, not between two steps.** `agent_progress::LoopWatch` counts steps with
       no change to the project and how many of them repeat earlier inspections: at 8 steps with 3 repeats the
       model is told what it has already looked at and asked for the next change; at 18 with 6 (or 30 steps
       with nothing changed at all) the run stops and keeps its work. A run that may not write (a plan or a
       question) is judged on repetition alone. `max_iterations` is now a safety ceiling of 400 (settings
       accept 1-5000) rather than the thing that ends a run.
    4. **Commands keep their output and take their children with them.** `terminal::run` collects both streams
       as they arrive and kills the whole process tree (`taskkill /T /F` on Windows, the process group
       elsewhere), so a timed-out command returns what it printed. A command meant to keep running goes to the
       background instead: `execute_command {"background": true}` returns an id after watching it settle,
       `{"background_id": N}` reads it later, `{"background_id": N, "stop": true}` ends it, and every
       background command started by a run is stopped when the run ends.
    5. **A run can look at what it built.** New tool `preview_page {"url": "http://localhost:5173"}`: it waits
       for the server, renders the page in the installed browser with no window (`--headless=new --dump-dom`),
       and reports the status line, the title, the text a reader would see, and what the page logged, saying so
       plainly when the page is blank. Loopback addresses only; nothing leaves the machine. Offered on windows
       of 16K or more, where a page report fits.
    6. **A follow-up continues instead of starting over.** A run in a session that already did work seeds its
       task turn with the host's record of that work (rebuilt from `tool_executions`) and with HANDOFF.md and
       MEMORY.md if the project has them.
    7. (a) A write whose text is the host's own release note is refused with an explanation, so the note can
       never become a file again. (b) The context meter reads the compaction threshold from the setting unless
       a run is going (a run keeps the threshold it started with), and the meter refreshes when settings are
       saved. (c) A command the shell does not have is answered with the shell's own equivalent.
    - **Also asked for and done:** models are told to keep HANDOFF.md and MEMORY.md as they work (rule 8b, on
      windows with room for it), and a changed line in the agent tab's diff is coloured across the whole line
      (`width: max-content; min-width: 100%`), measured at 1,106 px of 1,106 where it was 519 px before, so
      scrolling right no longer shows unpainted background.
    - **Measured on the same prompt, the same model and the same window (32K), before and after:**

      | | before | after |
      |---|---|---|
      | steps | 30 (the ceiling) | 45, ended by the repeat guard on a failing edit |
      | what was built | no project at all: no index.html, no vite config, no entry point | a scaffolded Vite project, dependencies installed, dev server running |
      | `npm run dev` | timed out, exit 1, stdout `(empty)`; the model concluded "the development server is running successfully" | returned what it printed, and the model restarted it with `"background": true` and got an id |
      | estimate vs real prompt | 1.13x to 2.42x | 0.89x to 1.14x |
      | compactions | 0 | 2 (one summarized 39 turns, 22,694 to 7,627 tokens, note plus host record) |

      The app the model produced still does not compile: its own JSX in a 295-line component. That is the
      model, not the host - and this time the host said so and stopped instead of circling.
    - Two smaller things the owner asked about while this was built: a model is now told whether the workspace
      is itself a project or a folder of projects (one built in the root and another made a folder, and neither
      had been told which this was), and rule 4 absorbed the old "run builds and tests" rule rather than
      repeating it.
    - Tests: backend 480 pass (4 ignored), frontend 109 pass, tsc clean. Not committed.

63. **2026-09-18 06:55-07:05 Verification on the owner's other model, then two more owner decisions: notes the
    model keeps, and no step ceiling at all.**
    - **The 26B, the owner's prompt, the fixed build, a fresh workspace.** 60 steps in 10.5 minutes (a
      mixture-of-experts model, 26B total with 4B active, loaded hybrid at 32K in 50 s): a scaffolded Vite
      project and 30 files under `src/components`, and - unprompted by anything but rule 4b - MEMORY.md and
      HANDOFF.md as its fifth and sixth actions. The estimate tracked the real prompt at 1.03x. It reached the
      step ceiling the test set while still producing.
    - **Then "resume from the last checkpoint" on it**, the same words that had sent the 27B into 43 steps of
      re-reading. The run opened with "Continuing work already done in this session: 59 recorded action(s) and
      the project's own notes", worked for 58 steps (21 edits, 12 writes, 3 compactions) and was stopped by the
      repeat guard when the same edit_file failed three times. The build went from broken syntax to six tidy-up
      errors (unused imports, one missing prop). The remaining weakness is the model's edit_file arguments, not
      the host.
    - **Notes the model keeps (owner: "implement and measure it").** The summary written at each compaction is
      fresh prose, so anything the model worked out but did not repeat in it was lost. New tool `remember`
      ({"note": "...", "replace": "..."}): up to 12 short facts that live in the task turn, which nothing
      prunes, and are re-attached after every compaction. Measured on a task that reads a configuration first,
      writes ten modules in the middle (forcing compaction at 45%), and must state the configuration's values
      at the end; see the table below.
    - **The step ceiling is gone (owner).** `max_iterations` is removed from settings, from the API and from
      the UI, and the run loop is unbounded. What ends a run is evidence: six steps with nothing executed
      (`ProgressGuard`), the same call failing three times (`FailedCalls`), 18 steps with nothing changed and
      six of them repeats or 30 with nothing changed at all (`LoopWatch`), the completion check, or the user.
      An old settings file keeps its `max_iterations` key; it is ignored.
    - **One guard added with the ceiling gone:** writing the same file over and over without ever building,
      testing or opening the project is the other way to look busy while going nowhere, and nothing bounded it
      any more. `LoopWatch` now counts writes of one path with no command or preview in between: at four the
      model is told to check what it has written, at seven the run stops and says which file it was. A build
      or a preview between writes clears the count, and writing different files never counts at all.
      **Corrected the same day, on the owner's point:** counting any repeated write would have stopped a model
      writing a long file in parts, which rule 2c tells it to do. Only writes of the *same text* with nothing
      run in between end a run (three of them); different edits are warned about once and never stopped, and
      `append_file` never counts.
    - **The kept-notes measurement (owner: "implement and measure it").** Task: read `config/service.json`,
      write twelve modules (6 to 10 compactions in an 8K window), then report the configuration's values.
      Three arms of the same model, three runs each, in `docs/validation/2026-09-18-agent-context-and-loops.md`.
      The tool alone changed nothing: the E4B never called it. What changed the outcome was the compaction
      instruction: asking for the values themselves rather than where they were read turned "I have read
      config/service.json previously" into "Service Name observatory-relay, Port 8412, Region eu-west-3,
      Retry Count 7", and that run wrote its report without reading the file again (1 read where every earlier
      run needed 2). With a reminder in the block the model then used `remember` in two runs of three.
    - **The loop watch proved itself while measuring.** With no ceiling in force, a run that had already
      finished its twelve modules and its report carried on reading them back and ended itself at step 72:
      "18 steps passed without a single change to the project, and 9 of them repeated something already
      inspected." Work kept, circling stopped - which is what the owner asked for instead of a step count.

64. **2026-09-18 ~07:15 Remove a project, and its tasks with it (owner: "right now I can just add and it stays
    there").** `DELETE /api/workspaces/:id?tasks=delete|keep`: the default keeps the chats (they report a
    missing project until relinked, as before), `tasks=delete` removes them with their transcripts, tool
    history and attachments, and the answer says how many there were and how many went
    (`{deleted, chats, chats_deleted}`). A run still working in one of those chats is a 409 naming the task.
    **The folder on disk is never touched** - asserted in the test, because that is the one way this could be
    a disaster rather than a tidy-up. In the UI each project group in the code sidebar has a remove button
    beside its "+", which opens an alert dialog naming the folder that stays, a switch for "delete its N tasks
    as well" (on by default, the wording and the confirm button change with it), and the warning that
    transcripts go with them. Checked live: removing a project with two tasks left the other project and its
    task untouched, the folder on disk in place, and the toast said so.

65. **2026-09-18 ~07:30 Four tools the runs asked for, and three noted for later (owner picked 1-4 of the
    seven suggested).**
    - **`replace_lines {path, from, to, text, expect?}`** - editing by the line numbers `read_file` already
      prints. Every run that died on 2026-09-18 died on `edit_file` arguments: the E4B sent it without a path,
      the 26B's `old` text never matched, and both were stopped by the repeat guard for it. Reproducing code
      byte for byte is the hard part for a small model; two numbers are not. `expect` optionally carries the
      first line as the model read it, and a mismatch refuses rather than destroying the wrong lines. The
      answer says how the numbering moved.
    - **`outline {path}`** - a file's definitions with their line numbers, or a folder's files with their
      sizes and line counts (`outline.rs`). Reading is what filled the window: one run read fourteen whole
      files to find things, another paged the same 450-line component over and over. The line numbers feed
      `replace_lines` directly.
    - **The project's own instructions reach code sessions.** `AGENTS.md` / `CLAUDE.md` / `MUSE.md` /
      `PROJECT.md` were shown in chat since Stage 24 and never in a code run, which is where they apply. The
      first one found is appended to the system prompt, cut to the window's share, and framed as how to work
      in this project - explicitly not as something that changes the host's rules or what needs approval, so a
      file in a repository cannot talk its way past them.
    - **`project_check {path?}`** (`project_check.rs`) - finds what the project is (package.json scripts,
      Cargo.toml, go.mod, Python tests), runs its build and then its tests, and reports the problems as
      `file:line: message`, deduplicated and capped, instead of a wall of output. A failed build stops it
      before the tests, which would say nothing. A placeholder `"test": "echo no test specified"` is not a
      test. A folder that is not a project is told so, with what to do instead.
    - **A PC without the toolchain (owner question).** The app itself needs none of this: Python, npm, cargo
      and go are the *project's* tools, not the app's. `project_check` now looks for the program on PATH the
      way a shell does (`.exe`, `.cmd`, `.bat` on Windows; Python under python, python3 or py) and, when it is
      not there, says "npm is not installed on this PC (it is not on PATH)" and points at execute_command
      instead of running a command that would fail with a shell error nobody can act on.
    - **Found by the first live run of the new tools:** `python -m unittest discover -q` from the project root
      found nothing (exit code 5) on the ordinary layout - a `tests/` folder with no `__init__.py` beside the
      code it imports - and the report called that "no error line could be picked out". Discovery now runs
      inside `tests` with the project root on the import path, which is what a person would type; exit code 5
      is reported as "the runner found no tests to run"; and a build that cannot compile stops the run before
      the tests.
    - **Room:** the instructions may not take more than half of a small window's history room, and each tool
      costs its description and arguments in every request. `preview_page`, `project_check`, `git_commit`,
      `list_processes` and `system_info` are therefore offered only on windows of 16K or more, and `edit_file`
      now points at `replace_lines` in one line instead of explaining itself in three.
    - **Noted for later (the owner's "we will get back to them"):** a layout report on top of `preview_page`
      (element boxes, off-screen and overflowing elements - the 26B's dashboard rendered correctly and sat
      below the fold, which reading the page text would never catch); screenshots into the chat's artifacts
      panel so the user can see what was built even though local models cannot; and a changes view for the run
      (what it has written, with sizes and a diff against where it started) as both a tool and a panel.

66. **2026-09-18 ~08:00 The other three tools (owner: "target the last 3 too").**
    - **The browser is now driven, not just started** (`cdp.rs`). The command-line flags can dump a page and
      take a picture of it and nothing else, so `preview_page` could not say how big anything was or where it
      ended up - which is how the 26B's dashboard passed as "renders fine" while sitting entirely below the
      fold. `cdp.rs` speaks the DevTools protocol over a WebSocket written by hand (handshake, masked frames
      out, unmasked frames in, pings answered; base64 was already a dependency, nothing new was added). The
      browser is started with `--remote-debugging-port=0` and its port read from its own output, so nothing is
      assumed about what is free.
    - **`preview_page` now reports the layout**: window and page size, how many elements, how far the page
      overflows sideways and what is too wide, what sits below the first screen, what is outside the window
      altogether, and boxes with content but no size. Console messages now come from the browser's own events
      (`Runtime.consoleAPICalled`, `Runtime.exceptionThrown`, `Log.entryAdded`) rather than from parsing its
      error stream, and the build-error overlay is detected by looking for it rather than by matching text.
      When the browser cannot be driven on a PC, the old dump-the-page path still answers.
    - **Screenshots go to the conversation's Artifacts panel** (`preview_for_run` in the agent runner). A local
      model cannot look at an image; the person who asked for the work can, and that is the point. The picture
      is taken through the same session (`Page.captureScreenshot`), stored with the conversation, and named in
      the report the model reads.
    - **`changes`** answers what this task has created, changed or deleted, with each file's size now. The host
      has always kept that record for compaction; a run could not ask for it, so a model whose earlier turns
      had been summarized away could not tell what it had already built without reading the folder again.
    - Room again: `changes` joins the tools offered only on windows of 16K or more, so a 4K window's
      instructions still leave more than half its room for history.
    - **Checked against a real browser and real runs.** An ignored test (`preview_measures_a_real_page`, run
      with `cargo test -- --ignored`) drives a page built to fail the way the 26B's did - content under a
      full-height sidebar, a strip wider than the window, a thrown exception - and the report came back:
      "window 1258x702, page 1824x905, 8 elements / The page is 566 px wider than the window ... div.too-wide
      1800x40 at 24,841 / Below the first screen ... main.main 1258x184 at 0,721", with the page's own log,
      its 404 and its ReferenceError, and a screenshot on disk. Then in a live run against the 26B's own
      project the model called preview_page and reported back that the dashboard's panels sit below the first
      screen; its screenshot is in that conversation's Artifacts panel (12 KB PNG). A second live run edited a
      file - with `replace_lines`, unprompted - and `changes` answered "1 file(s) changed by this task:
      src/App.css: changed, now 23 lines (504 bytes)".
    - **Two bugs found by those checks, both fixed:** the browser's debugging port answers HTTP but never
      closes the connection, so reading to the end of the stream timed out (it is read by Content-Length
      now); and the 16-bit WebSocket length was parsed from the wrong two bytes, which mangled every frame
      over 125 bytes. Also: tools the run carries out itself (preview_page, changes, remember) were missing
      from the audit trail, which records every other tool - they are recorded now.

67. **2026-09-18 ~08:30 Two minutes of nothing, twice (owner: "I need you to watch what the app is doing
    right now it seems stuck").** Watching a live run: at 02:51:30 and again at 02:54:22 the model ran
    `npx serve .` in the foreground. That command is not meant to finish, so each one sat there for the full
    two-minute timeout and then came back as "killed after 120s" - four minutes of a run spent on nothing,
    and in between the model had already started the same server correctly in the background. Two changes:
    - **A command that never exits is now refused in the foreground before it is run** (`keeps_running` in
      `terminal.rs`, checked in `execute_command`). It recognises the usual dev and static servers, watchers
      and `--watch` flags, and answers immediately with what to do instead: send the same command with
      `"background": true`, and you get its output and the address it prints. The refusal costs one step
      rather than two minutes. Ordinary commands (`npm run build`, `npm install`, `cargo test`, `git status`)
      are untouched - both halves are in the test.
    - **The same server started twice gives back the one already running.** A run that asks for a command it
      already has up, still alive, gets that process back instead of a second one binding a second port
      (measured live: four servers for one folder). Commands are grouped per run, so this never reaches
      another conversation's processes.
    - **The diff colours now cover the whole line** (owner: "no matter how right I scroll it should have that
      colour"). The red and green stopped where the text stopped. The lines now share one box as wide as the
      longest of them and each line fills that box: sizing each line to its own text left short lines ending
      in mid-air, and sizing them to the container cut the colour off at the right edge. Measured in the
      browser pane: every line 954 px, the same as the scroll width.
    - Green after all three: 497 backend tests, 110 frontend tests, `tsc` clean, both builds rebuilt so the
      running app picks them up.

68. **2026-09-18 ~09:25 Diagnosis: the 26B run at 200K that ended in "start Companion again" (owner: "something
    broke while the agent was running not sure what it was"). Fixes proposed, not built.**
    - **What the records show** (a copy of the history, Windows' event logs, the desktop app's log): the run started
      08:37:57 and made 40 model requests. At 08:52:34 the 41st reply broke off mid-stream ("error decoding
      response body": the connection closed while the reply was arriving). The run recorded "the model request
      failed ... retry when the model is ready" at 08:52:35, the last thing ever written. No crash report for the
      model server or the backend, no GPU driver reset, no low-memory event (63 GB RAM, 36 GB free with the model
      loaded). The laptop slept at 08:53:44 and was restarted from the Start menu at 08:55:37.
    - **The request did not break the model server.** The last three recorded requests were replayed against the
      same model and the owner's settings (200K, hybrid placement, q8_0 cache, n-gram drafting), in the order
      38, 39, 40, 39, 40, 39, 40: all seven finished cleanly, the server healthy after each.
    - **The desktop app updated itself during the run**: it quit for an update at 08:45:41, and its package
      was installed at 08:46:46 and again at 08:52:28, six seconds before the reply broke off. A console window
      opened at 08:26:32 was declared hung and closed at 08:46:46. Whether the run script lived in a window the
      desktop app started is not recorded anywhere; if it did, the update's shutdown of the app's processes
      explains a model server stopped without a crash. Asked the owner.
    - **Why the cause cannot be pinned down: nothing was kept.** The backend logs to its console and to memory;
      the model server's output is an 8 KB tail in memory, used only when a load fails. A restart erased both.
    - **Every step from 31 on re-read about 34K tokens.** A fixed cap of 60 turns (`TRANSCRIPT_TURNS`) dropped
      the oldest turn after the task on every step once reached, so the prompt changed at token 2,298 each time
      and the cache never matched past it (2,298 cached on requests 31-40 in the run and in every replay).
      Measured cost: 34K tokens at ~1,184 tok/s = ~29 s of each ~44 s step; with the cache, a step reads its
      2-3K new tokens in 2-3 s. The cap's own comment says prefix caching makes a long transcript nearly free;
      the cap is what stopped it.
    - **`--cache-reuse 256` is on for every model but is a no-op here.** The pinned runtime turns it off when
      the cache cannot be shifted: a sliding-window cache can shift only when allocated at full size
      (`--swa-full`, not used), and it is also off when an image projector is loaded. It says so only in its own
      log. Moved text can only be reused by shifting, so neither cache reuse nor the planned M8 checkpoints would
      have saved these steps; not changing the start of the prompt does.
    - **Two messages misled.** The run said "retry when the model is ready" without checking whether the model
      server was still running. The UI shows "The local runtime isn't responding ... start Companion again with its
      start script" after one failed status check - which a sleep/resume blip can cause - and says the same when
      only the model server is down.
    - **Proposed (owner to choose):** (1) remove the 60-turn cap; when the window really fills, compact or prune
      in one large step so the cache survives many steps; (2) keep the backend's log and the model server's output
      on disk, rotated, under the data folder; (3) after a broken reply, check whether the model server is still
      running: retry the step once if it is, say it stopped (exit code, its last lines) if not; (4) show the
      runtime notice only after two failed checks, and tell "the model server stopped" apart from "Companion is
      not answering"; (5) on shutdown, stop running tasks first with an honest reason, and handle the window
      being closed and Windows shutting down, not only Ctrl+C.

69. **2026-09-18 ~10:00 The five fixes from decision 68, built and checked live (owner: "apply all fixes";
    and on the cap: "why is there a 60 turn cap? I literally asked you to remove it").**
    - **Why the cap was still there.** It dates from the project's first commits (2026-09-12/13). When the owner
      asked for the turn limit to go (decision 63), the step ceiling was removed; this second count - how many
      turns stay in the prompt, not how many steps a run may take - was missed. It should have gone then.
    - **(1) No turn count, and one large cut.** `TRANSCRIPT_TURNS` is gone: the transcript is bounded by size
      only. When it no longer fits, `prune_transcript` cuts back to three quarters of the budget in one go
      instead of to just under it, so the steps after a cut extend a prompt the cache still holds. The dead
      `max_iterations: 30` in `agent.rs` (only a test stub read it) is gone too. Tests: 200 steps in a large
      window drop nothing; a full window is cut once and the next three steps leave the earlier prompt as it was.
      **Measured live** (small model, 36 one-file steps, 105 turns at the end, nothing pruned): every agent
      request from 30 to 67 took 90%+ of its prompt from the cache (request 31: 4,915 of 5,010). The owner's
      run at the same point: 2,298 of 31,342.
    - **(2) Logs on disk** (`logfile.rs`): `logs/companion.log` (every record the backend logs) and
      `logs/model-server.log` (the model server's output a line at a time with the time it arrived, plus
      Companion's notes: the command line it was started with, ready, stopped by Companion, ended by itself and
      how) in the data folder. 4 MB a file, two older files kept, rotated by renaming - the rename takes the
      place of the oldest file, which is how a capped log works, not a user file being deleted.
    - **(3) A model server that ends while answering is named.** The in-place retries already repeated a
      broken reply while the model server was alive (up to 3, 1/2/4 s). What was missing: when it is not
      alive, the run said "the model request failed ... retry when the model is ready". Now: "Stopped: the
      model server stopped while answering (exit code 1: an error it reported, or it was ended from outside:
      Task Manager and taskkill end a program with code 1). ... load the model again ... Its output is in
      <data>/logs/model-server.log." Exit codes are put in plain words (`describe_exit`). The same report is
      in `InferenceStatus.stopped` and in the chat path's message. The owner's run itself shows the model
      server had already exited at 08:52:34: the reply error was of the retryable kind, yet no retry was
      recorded, and the retry only stops early when the process is gone.
    - **(4) Two notices, told apart** (`services/runtimeHealth.ts`). "Companion isn't answering" appears only
      after two failed status checks in a row; a failure is checked again after 3 s, so a real outage shows in
      seconds and a blip is forgotten. "The model server stopped" (with how, and a Load it again button)
      appears when the backend answers but its model server ended by itself.
    - **(5) Closing records running tasks first** (`AppState::shut_down`, `shutdown.rs`, `main.rs`). Ctrl+C,
      Ctrl+Break, the window closing, signing out and shutting down (Windows) or SIGTERM and SIGHUP (unix):
      running tasks are cancelled and recorded with the reason ("Stopped: Companion was closed while this task
      was running (its window was closed) ..."), then chat generation and the model server stop. For the
      window closing, signing out and shutdown the process exits straight after: Windows ends it a few seconds
      later regardless. tokio 1.53 holds its handler thread for those events, so the async side gets that time.
    - **Checked live, each on a throwaway data folder, one model at a time, 2-minute breaks:**
      A - kill the model server mid-reply: the run's message, the status's `stopped`, the log's "ended by
      itself" line and the saved message all as above; the notice showed in the browser and Load it again
      reloaded the model and cleared it. Stopping the backend showed "Companion isn't answering" within 8 s.
      One simulated failed check: rechecked at +3 s, notice never shown in 34 s. B - the cache measurement
      above. C - closing the backend's console mid-reply: exit 0 after 0.3 s, the task recorded with "(its
      window was closed)", status interrupted, the model server stopped 8 ms after the record, nothing left
      running; Ctrl+C: the same with "(Ctrl+C in its window)" and "exited cleanly".
    - **A trap in checking this:** processes started from this session's shell inherit "ignore Ctrl+C", so
      the first Ctrl+C check never reached the backend. A launcher must call
      `SetConsoleCtrlHandler(NULL, FALSE)` before starting it; a PowerShell window does not have the flag.
    - **Found while measuring, not built (owner to decide):** the completion check is sent without the tool
      list. The template writes the tool definitions at the top of the prompt, so the check re-reads the whole
      prompt, and the next step re-reads it again because the cache now holds the check's version. In the
      owner's run: requests 35 and 36, 35,087 and 33,734 tokens from scratch, about 58 s on the 26B. Proposed:
      send the check with the same tool list and `tool_choice: "none"`, measured before adopting.
    - Green: backend 504 tests, frontend 114, `tsc` clean, both builds done, no warnings.

70. **2026-09-18 ~11:05 The completion check and the compaction note carry the run's tool list (owner: "yes please
    try that").** Both are host-side requests on the run's own transcript, and both were sent without the tool
    list. The template writes the tool definitions at the top of the prompt, so neither matched any of the cached
    prompt: every check and every compaction note re-read the whole transcript, and on the 26B (RAM prompt
    cache off) so did the step after a check.
    - **Built:** `SidecarClient::chat_turns_on_run_prompt` - the run's tool list, in the same order, with
      `tool_choice: "none"`, used by `review_completion` and `compact_run_transcript`; the list's tokens are
      counted in their budgets (`tool_list_tokens`). The pinned runtime renders the tools whatever `tool_choice`
      says (`common_chat_params_init_gemma4` renders `inputs.tools`; "none" only drops the tool-call grammar and
      parsing), so the prompt stays the run's. A tool call written out as text anyway is no verdict and cannot
      pass as COMPLETE. Test: both requests carry the list switched off; without a list, none is sent.
      Backend 505 tests.
    - **Measured by replaying recorded requests** (fresh model load for each way; the step after a check rebuilt
      as the current code sends it). Three verdicts per check per way at the run's temperature:

      | model | request | today | with the list | notes |
      |---|---|---|---|---|
      | 26B (owner's NEON run, 35K) | check + next step | 35,087 + 33,862 read, ~56 s | 6,067 + 3,191, ~9 s | CONTINUE 6/6 |
      | 26B (snake game) | check | 13,583 read, 11.1 s | 13,043, 10.9 s | an early earlier-finding; COMPLETE 6/6 |
      | 27B (three checks) | check + next | 25-29K read, 35-39 s | 7-17K, 19-30 s | CONTINUE 18/18 |
      | E4B (five checks) | check | 3-7K read | 1.1-2.0K | same verdicts 30/30 |
      | 26B (three compaction notes, ~19K) | note | 15.2-16.8 s | 1.1-1.7 s | normal notes both ways |
      | E4B (two compaction notes, 21-23K) | note | 4.1-4.6 s | 1.1 s | normal notes both ways |

      Live, through the rebuilt app (small model, 12 files): the check went out with the run's 18 tools and
      `tool_choice: "none"`, 3,532 of its 4,269 tokens came from the cache, verdict COMPLETE.
    - **What still limits it: the check filters out earlier review turns** ("Completion review found remaining
      work: ..."), because a checker once repeated a finding after it was fixed. That makes the check leave the
      run's prompt at the first such turn, so after the host's first "before finishing, list the folder ..."
      request most of the saving is lost (snake game: nothing saved; 27B: 19-30 s instead of 35-39). Measured a
      third way - the check as a pure continuation of the run's prompt, earlier review turns left in, plus one
      sentence ("Earlier completion reviews above may already be resolved: judge what the latest versions and the
      tool evidence show now, not what those reviews said"): 26B NEON ~4 s (3,371 + 116 tokens), 27B 4.6-5.2 s,
      snake 2.2 s, verdicts identical in every check that had earlier review turns (6 checks, 18/18). But on a
      small-model check with NO earlier review turns the sentence made 2 of 3 replies empty (the model most
      likely tried to call a tool to check the files). Proposed, not built: leave the turns in and add the
      sentence only when there are earlier review turns; with none, the check is exactly what is built now.
      Every case that proposal covers is already measured: 30/30 identical verdicts, no empty replies. Owner to
      decide.
    - **Why a change in the middle costs everything on these models:** Gemma 4 uses a sliding-window cache, and
      the pinned runtime can only rewind it to a saved point. Points are saved at user turns (at least 8,192
      tokens apart) and just before each prompt's end; with native tool calls a run has almost no user turns, so
      a change mid-prompt rewinds to the start of the task (2,298 tokens in the owner's run - the number every
      request after step 31 reused there).
    - **Replay traps, each of which produced a wrong number first:** the model server's RAM prompt cache carries
      one way's states into the next unless the model is reloaded between them; repeating a check three times
      before the next step erased the save point that step needed ("too close to an earlier one"); and a recorded
      next step from a run that still had the 60-turn cap dropped a turn of its own. The first 26B replay started
      45 s after the previous test rather than 2 minutes: it measured token counts, which heat does not change.

71. **2026-09-18 ~11:10 The completion check keeps earlier review turns (owner: "yes go ahead").** The check is now
    the run's own transcript, turn for turn, plus its instruction; earlier "Completion review found remaining
    work" and "could not confirm" turns stay in. When any are in view the instruction adds "Earlier completion
    reviews above may already be resolved: judge what the latest versions and the tool evidence show now, not what
    those reviews said." - only then, because with none in view the sentence pointed at nothing and a small model
    answered with an empty reply 2 times in 3 (decision 70). Every case this covers was measured in decision 70:
    30 of 30 verdicts the same as before, no empty replies; 26B NEON check and next step ~56 s -> ~4 s, 27B checks
    35-39 s -> ~5 s, snake-game check 11 s -> 2 s.
    - Test rewritten (`the_completion_check_is_the_run_prompt_and_is_told_earlier_reviews_may_be_resolved`): the
      check's messages are the transcript turn for turn, the sentence is there with reviews in view and absent
      without. Backend 505 tests.
    - **Live** (small model; the task ran a command, so the host asked for a look at the folder before finishing):
      the check at step 7 had that review in view, carried the run's 17 tools with `tool_choice: "none"`, included
      the sentence, and took 2,853 of its 3,254 tokens from the cache - it read only its own 401-token instruction.
      Verdict COMPLETE.
    - Seen on the way, not caused by this work: right after a successful tool result the small model sometimes
      ends its turn at once (one token, nothing visible). Runs from 2026-09-17 already show it (3 of 30 steps, 5 of
      50, 7 of 72); in the repetitive 36-file run it was 31 of 68. The host's existing "Your previous response was
      empty" nudge recovers every time, for one short request that is read entirely from the cache.

    - *Times in entries 62-67 were corrected afterwards against the files' own timestamps
      (`agent_progress.rs` 06:55, `api.rs` 07:16, `project_check.rs` 07:38, `cdp.rs` 08:00): the clock
      readings written at the time ran several hours ahead of the machine's.*

## Blocked / needs the owner

- **B1 (23:00) Projector/vision GPU test (brief §14.9): DEFERRED by the owner (04:40, "not that important"); not part of the current demo.** No installed model has a vision projector
  (`mmproj`) file, and the owner's rule is not to download models. What unblocks it: the owner adds a vision
  model with its projector (e.g. the E2B model's projector file). The test is then projector on CPU vs GPU:
  load time, image encode time, first-token wait, VRAM and RAM.

## Checklist of everything still to do (kept current)

- [x] Fork queue: done (decision 8). Cache, async CPU splits, pinned mmap rejected.
- [x] Prefetch confirmation at micro-batch 1,024: dropped (decision 14).
- [x] Draft-length sweep (30B) and GPU confirmation (decisions 18, 20): 24 for split loads; code compiled (decision 26 build); [ ] CPU value from C5.
- [x] Fork outcome applied: nothing won, so nothing is ported (decision 14).
- [x] Speed-rule review findings handled (decisions 7, 9, 10); first compile passed.
- [x] Recompiled (decision 19): all green.
- [x] Draft head code written and compiled (decisions 11, 12, 19); cost measured and gate added (decision 26, compiled: 421 tests).
- [ ] CPU track (≤8B): C1 threads/poll/priority, C3 flash attention/cache on CPU, C4 load mode, C5
      drafting; C6 prompt order before/after, then remove `COMPANION_LEGACY_PROMPT_ORDER` if the new
      order wins.
- [x] (decision 13) Update `docs/ARCHITECTURE.md` (~345), `docs/PERFORMANCE.md` (~185), the workspace launch config
      (`C:\\Users\\ism19\\Code\\.claude\\launch.json`, companion-verify falls back to `models/bin`), and the
      test scripts that still point at `models/bin`.
- [ ] (running since 03:34, `chain_live.sh`: verify → chat → code, 5-minute breaks) Live tests (owner plan in HANDOFF): chat with all 8 models; code chat with 4B/8B/12B/27B/30B (a
      test project, a change to it, a calculator app, a stopwatch/timer/clock app); screenshots to
      `docs/validation/live-tests/`; test projects to the Recycle Bin; failures must be the model's,
      not the app's.
- [ ] Remaining live verification items (HANDOFF step 5 list) and the app-pipeline benchmark.
- [ ] After the live tests: compile (decision 28 fix), then `scratchpad/chain_after.sh` (written 04:15): verify
      load-error recheck → 2B fence check (`bench_fence.py`, the chat prompt's "Format code with fenced blocks."
      seen fencing non-code answers) → CPU track → C6 prefix GPU/CPU → app_bench 8B/27B → M18 → M8 → M10 → M21;
      5-minute breaks between, 2-minute gaps inside. Then M12 render profile in the browser pane, then the
      generated apps' functional check and screenshots review.
- [ ] Phase C measure-first items from the harness plan (after the main tests; each adopted only if the
      measurement justifies it, by the 5% rule): M18 repeat penalty 1.1 vs 1.0 (rewrite + prose, draft
      acceptance, exactness), M8 cache checkpoints on the hybrid 27B / sliding-window model, M10 hard
      reasoning cap (correctness on a fixed prompt set), M12 streamed-render profile, M21 history trimming.
- [x] MoE combination search at 16K (decisions 23, 25): fix compiled; validation doc updated.
- [x] Draft-head cost record (decision 26): validation doc updated.
- [ ] Final report (owner format) published as an artifact; ARCHITECTURE/PERFORMANCE updates.
