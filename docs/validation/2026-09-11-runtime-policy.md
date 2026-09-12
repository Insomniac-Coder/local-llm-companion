# Managed model runtime validation — 2026-09-11

## Confirmed mismatch

The saved advanced `kv_cache_type: "Q8_0"` preference was not connected to
`server_args`. The installed worker's actual default was f16 for both K and V.
The change does not activate that previously inert preference as an unexpected
manual override. Model weight quantization such as Q4_K_M is separate from the
temporary attention cache's numeric representation.

Read-only inspection of `models/bin/llama-server.exe --version` reported
`0.4.0-dev`, build `10809`, commit `5266f24da`. Its `--help` confirms:

- `--cache-type-k` and `--cache-type-v` default to `f16`; Q4_K_M is not an
  accepted cache type.
- `--flash-attn` accepts `on`, `off`, and `auto`.
- `--n-gpu-layers` accepts an exact number, `auto`, or `all`.
- `--no-kv-offload` disables cache offloading.
- `--parallel` controls the number of server slots.

The native implementation validates attention-cache compatibility using the
actual model: quantized V requires Flash Attention, and quantized cache block
sizes must divide the relevant per-layer head dimensions. It constructs memory
through the model implementation rather than assuming one universal cache
layout. This is why the app does not derive KV precision from a weight filename
or invent support from an architecture name.
[Official llama.cpp context implementation](https://github.com/ggml-org/llama.cpp/blob/master/src/llama-context.cpp).

## Implemented policy

Automatic mode resolves a fresh launch policy for the selected model. It keeps
the configured context preference as a cap, clamps it to the model's advertised
limit, selects a batch up to 512, and delegates CPU thread selection, GPU
placement and Flash Attention compatibility to the native runtime. Automatic
threads are represented as zero in policy metadata but the `--threads` flag is
omitted; zero is never sent as the worker's actual thread count. K/V f16 is
explicit and compatibility-first; this is not claimed to be the fastest or
smallest cache for every model. Native GGUF metadata still determines the model's
attention, sliding-window and recurrent-state layout and chat template.

Manual mode retains active thread, GPU-layer, batch, attention and offload
controls. The old inactive advanced cache selector does not suddenly take
effect. The policy reports requested versus effective context and resolved
launch choices; these are not presented as measured GPU residency.

Each model load starts a new worker and cache. One server slot preserves the
selected per-request context budget. No saved KV-slot state or custom chat
template is injected. Subsequent requests submit the saved message transcript
to the newly selected model's native tokenizer/template.

Startup now drains both worker-output pipes continuously, keeping only an 8 KiB
diagnostic tail. An occupied port is rejected before launch; a dead worker cannot
be mistaken for another server's successful health response. Failed startup
reaps the child and reports bounded diagnostics.

## Validation and limits

At this integration snapshot, the full backend suite passed **218 tests**, with
zero failures and one intentionally ignored installed-model diagnostic. Added
tests cover independent per-model context/architecture resolution, preserving
preferences, explicit launch flags, automatic/manual controls, a 128 KiB log
stream without pipe blockage, the bounded diagnostic tail, early process exit,
and refusing an already occupied port before worker launch.

No live user model, conversation or worker was modified for these checks. This
policy change has unit/mock coverage and installed-binary option verification;
it was not a new real-GGUF load or throughput benchmark. The earlier real-model
Chat/Ask/Plan/edit evaluation remains documented separately. Context-memory
recommendations are heuristic and cannot fully model every attention or
recurrent architecture; actual native-runtime load validation remains decisive.
