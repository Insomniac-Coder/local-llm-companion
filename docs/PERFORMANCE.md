# Inference performance: measurements and decisions (2026-09-13)

Every launch flag Companion sets was chosen against numbers from the bundled
runtime on this machine, not from folklore. The harness lives outside the repo
(a Node script that spawns `llama-server` with one flag set, runs a fixed prompt
suite twice, and reads the engine's own `timings` object); the tables below are
its output. Re-run it whenever the runtime binary or a default changes.

Test machine: Intel Core Ultra 9 275HX (8 P + 16 E cores), 64 GB, RTX 5070 Ti
Laptop (12 GB), Windows 11. Model: Qwen3-8B Q4_K_M (5.0 GB). Runtime: bundled
llama.cpp `0.4.0-dev` build 10809 (commit `5266f24da`, CUDA). Thinking was
disabled for the suite so the numbers describe visible generation.

Prompt suite (each run cold, then warm with the prompt cache primed):

| Prompt | Input | Output cap | What it represents |
|---|---|---|---|
| chat | ~590 tokens (identity prompt + question) | 300 | Novel prose |
| prefill | ~8,100 tokens (a source file + question) | 80 | Prompt processing |
| edit | ~1,150 tokens (a 140-line file) | 1,400 | "Return the file with one rename": what a coding agent does all day |
| tooljson | ~600 tokens | 120 | One tool-call envelope |

## GPU (32K context, full offload)

Generation tok/s (cold pass unless noted); prompt processing was 3,150-3,750
tok/s in every variant, differences inside run-to-run noise.

| Variant | chat | edit | tooljson | Notes |
|---|---|---|---|---|
| A. previous app flags (`--batch-size 512`, f16 KV, no speculation) | 92.9 | 78.9 | 84.3 | baseline |
| B. runtime default batch (2048/512) | 92.6 | 81.2 | 93.9 | no loss; fewer decode calls |
| C. B + `--cache-reuse 256` | 83.3* | 80.9 | 84.2 | *thermal noise; reuse only matters when a prompt diverges |
| D. C + `--spec-type ngram-mod` | 86.5 | **989.7** (warm 1,400) | 69.8 | big edit win, 6-18% cost on novel/short text |
| E. C + `--spec-type ngram-map-k` | 88.2 | 81.6 | — | drafts rarely accepted (8/96) |
| **F. C + `--spec-type ngram-simple`** | **90.9** | **1,198** (warm 1,212) | **100.3** | chosen: no cost on prose, +19% on tool JSON, 15x on rewrites |
| G. C + KV cache q8_0 | 88.0 | 79.5 | 83.0 | about 5% slower decode at short context; halves cache memory |
| H. D + KV q8_0 | 88.0 | 988 | 67.1 | |

Draft acceptance for F on the edit prompt: 1,066 of 1,104 drafted tokens.
Speculative decoding is lossless: the target model verifies every drafted
token, so output is identical to plain decoding.

## CPU only (`--device none`, 8K context, all 24 physical cores unless noted)

Generation tok/s (cold pass); prompt processing in tok/s in parentheses.

| Variant | chat | edit | tooljson | prefill 1.8K prompt |
|---|---|---|---|---|
| A. previous CPU flags (`--batch-size 128`, flash attention off) | 10.9 | 12.4 | 12.0 | 102 |
| B. runtime default batch, FA off | 13.9 | 12.4 | 12.3 | 101 |
| C. default batch, FA auto | 11.4 | 11.5 | 12.5 | 104 |
| **D. C + `--cache-reuse 256 --spec-type ngram-simple`** | **13.1** | **47.2** | **14.3** | 109 |
| E. C + KV cache q8_0 | 12.8 | 11.6 | 13.4 | 85 (about 15% slower prefill) |
| T8. C with `--threads 8` (P-cores only) | 10.0 | 9.2 | 9.8 | 61 |
| T16. C with `--threads 16` | 12.7 | — | — | 99 |

Variant C ran while a compiler was busy in the background, which explains its
slightly lower row; the direction of every other comparison held on both
passes. On CPU the same speculative mode gave 4x on the rewrite and lost
nothing elsewhere; q8_0 KV costs prompt speed on CPU, so it stays opt-in; and
the runtime's default thread choice (all physical cores) beat the
performance-core-only choice by 40% on prompt processing, so Companion leaves
threads to the runtime. A Core Ultra 5 125U (2 P + 8 E + 2 LP-E) was not
available to measure; if its two low-power E-cores drag the barrier-synchronised
matmuls, `Settings > Performance > manual > CPU threads = 10` is the value to
try.

## What Companion now sends the runtime

Automatic mode: `--n-gpu-layers auto --flash-attn auto --cache-type-k f16
--cache-type-v f16 --cache-reuse 256 --spec-type ngram-simple`, no
`--batch-size`, no `--threads`. CPU fallback keeps those and adds `--device
none --no-op-offload --no-kv-offload` with a RAM-aware context cap (8,192
when memory is comfortable). Settings > Performance exposes speculative
decoding (auto/off) and KV cache precision (f16/q8_0); they apply on the next
model load and show up under the runtime configuration together with the
planned placement.

## Memory-aware context sizing (GPU, hybrid, CPU)

Context size is fitted to the machine at every load, not read from a
preference and hoped for. The numbers come from two places: free VRAM,
re-measured after the previous worker has exited, and the KV-cache cost per
token from the GGUF header (`block_count × kv_heads × (key_length +
value_length) × 2 bytes` at f16; q8_0 costs 17/32 of that).

- **GPU budget** = free VRAM − a compute reserve of max(weights/12, 768 MiB)
  − 5% of the card for the driver and other applications.
- **Whole model on the GPU** when weights + KV at the requested context fit
  the budget with f16. Otherwise the context is halved down to 4,096, trying
  q8_0 at each step, and the largest fitting combination is launched.
- **Hybrid** when the weights alone do not fit: llama.cpp spills layers to
  system RAM, the KV cache is kept on the GPU at no more than a quarter of the
  budget (q8_0), and the remainder is checked against RAM (available − 1/8 −
  half the compute reserve). If even that fails the placement is reported as
  *oversubscribed* and the load still proceeds with a note, because the
  runtime's own `--fit` can shrink further and the user can lower the context.
- **CPU only**: the comfort cap of 8,192 is lowered by free RAM (a machine
  with 7 GiB free gets 4,096; below that 2,048 with a note), and KV stays f16
  because q8_0 costs about 15% of prefill on CPU for no memory benefit that
  matters there.
- The plan (placement, context, precision, notes) is shown under Settings >
  Performance > runtime configuration for the loaded session and the next
  load.

Measured effect: Qwen2.5-Coder 14B at the requested 32K with f16 spilled
layers and decoded at 18 tok/s; fitted to 8,192 with q8_0 it decodes at
55 tok/s on the same card. Qwen3 8B fits 32K with f16 or q8_0 depending on
how much VRAM other applications hold at load time.

## Agent protocol robustness (what the end-to-end runs taught)

See `docs/validation/2026-09-13-e2e.md` for the runs. The parts that mattered:

- **Accept what models actually write.** Qwen2.5-Coder 14B ends every action
  at the closing brace and never writes the closing fence; the strict parser
  rejected all of them. The action parser now accepts an action that ends the
  turn, `json` and bare fences that name a registered tool, the `<tool_call>`
  tag Qwen/Hermes models were trained on, a bare object, `arguments` or
  `parameters` for `args`, raw newlines inside JSON strings, and prose after a
  closed action. Two actions in one reply are still ambiguous and rejected.
  The stream stops at the closing brace, so a closing fence is never
  generated at all.
- **Recovery ladder.** Unreadable reply → native retry with the format
  reminder and thinking off → schema-constrained JSON envelope → stop.
- **Failure budget.** Six consecutive steps without a successful action end
  the run; identical repeated results warn at two and stop at five; any
  successful action resets both. A finished task is never failed for the
  number of attempts it took.
- **Every failure carries a correction.** Tool errors echo the arguments
  received; a missing `path` names the file the model last worked on; an
  `edit_file` whose `old` text is not found shows the closest lines; an
  ambiguous `old` reports how many times it occurs; a rejected completion
  claim is journaled in the model's words next to the review verdict.

## Request-path changes that matter as much as the flags

These were found by reading the request path rather than by the harness; each
is a multiplier on the numbers above.

- **One KV-cache prefix for every request of a conversation.** Routing (Ask /
  Plan / Agent) used to send its own system prompt before every Code message,
  which evicted the conversation from llama-server's single slot and forced a
  full re-prefill of the whole history twice per message. Routing now sends the
  exact chat prefix plus one instruction turn, and the agent's completion review
  rides on the agent transcript the same way. The prompt cache (`cache_n` in the
  engine timings) serves the unchanged part; on a CPU-only machine this is the
  difference between seconds and minutes per message.
- **Streaming agent steps with early stop.** The agent used to wait for the
  whole non-streamed response. It now streams: partial text reaches the UI as it
  is produced, and the moment a complete action envelope has arrived the
  connection is closed, so the runtime never generates prose after the fence
  (which the strict parser would have rejected anyway).
- **Context-aware pruning instead of a turn count.** Old tool results are the
  bulk of an agent transcript; their bodies are released first, then whole turns,
  and the estimate is checked against the actual context window before each
  request. The history window sent to chat is sized from the loaded context too,
  so an 8K CPU context no longer overflows on a long conversation.
- **Reasoning off means off.** Thinking models (Qwen 3, Gemma 4) are asked not
  to think when Reasoning is off; previously they thought anyway and the hidden
  tokens were paid for silently. Reasoning capability is now read from the
  GGUF's chat template, so the toggle is truthful per model.
- **Engine-measured telemetry.** Output and prefill tok/s, cached tokens and
  draft acceptance come from the runtime's `timings`, not from wall-clock
  guesses, and the extra tokenizer round-trip after each reply is gone.
- **Long generations are not killed.** The sidecar HTTP client no longer has a
  whole-request timeout (only connect and idle-read bounds) and is shared, so
  every turn reuses the connection. The same fix applies to model downloads,
  which used to fail after ten minutes.
- **Blocking work off the async runtime.** Repository indexing, file reads,
  searches and shell commands run on the blocking pool; a long `cargo test`
  inside an agent run no longer stalls other requests.

## Should Companion fork llama.cpp?

No. A fork cannot make the kernels faster, and upstream lands new model
architectures and decoding modes weekly; a fork rots. What a custom build buys
is size and provenance, and that is a build recipe, not a fork:
`scripts/build-runtime.ps1` pins an upstream commit and produces a trimmed
package (server only; one CUDA architecture instead of all; optionally a CPU
backend compiled for the host). Expected effect on this machine: `ggml-cuda.dll`
from 145 MB to roughly 30 MB and forty unused tools removed; no change in
tokens per second. The recipe is documented and reviewed but not executed here,
because this machine has no CMake or CUDA toolkit installed. Two leftover
archives in `models/bin` (`cuda.zip`, `cudart.zip`, 540 MB together) are
downloads that were already extracted and can be deleted by hand.
