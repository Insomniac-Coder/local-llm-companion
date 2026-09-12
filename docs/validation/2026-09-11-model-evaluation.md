# Real-model validation — 11 September 2026

## Scope and isolation

These were actual Gemma inference and application-tool runs, not mocked model
responses. A separately compiled backend ran on `127.0.0.1:3879`, with its own
temporary SQLite database, attachments directory, and three-file test project.
Its llama-server worker used port `8089`. The user's application on port `5173`,
conversations, projects, model files, and permission settings were not changed.
Both evaluation processes were stopped afterward.

Configuration: `gemma-4-12b-it-Q4_K_M.gguf`, 4,096-token runtime context, temperature
0.1, batch 256, 8 CPU threads, GPU layers 999. The installed file's GGUF metadata
identifies architecture `gemma4` and native maximum context 262,144; that native
maximum was **not** used for these tests. Results are single local observations,
not comparative throughput benchmarks or evidence of large-project reliability.

## Results

| Test | Observed result | Total elapsed | Evidence |
|---|---|---:|---|
| Chat | Replied `42` to `17 + 25` | 1.203 s | [Transcript](model-eval-2026-09-11/chat-report.json) |
| Code Ask | Executed `read_file` on README; correctly identified bicycle-repair tickets and unique code `LANTERN-583`; no mutation | 7.024 s | [Transcript and three persisted activities](model-eval-2026-09-11/ask-report.json) |
| Plan | Read the calculator and tests, explained the subtraction bug, produced a plan, and completed after four iterations; no mutation or command execution | 18.831 s | [Transcript and 15 persisted activities](model-eval-2026-09-11/plan-report.json) |
| Agent edit | Changed subtraction to addition, emitted an actual before/after diff, ran `node calculator.test.js` with exit code 0, and completed after six iterations | 22.583 s | [Transcript and 25 persisted activities](model-eval-2026-09-11/edit-report.json) |

The independent post-run test also passed both assertions: `add(2, 3) === 5` and
`add(-2, 3) === 1`. The calculator's SHA-256 stayed
`62382ae0fb154b9081829896eed2639c8e74d0fa4c96bb9f1805f1b523ac602b` through Chat,
Ask, and Plan. After the approved edit it was
`754052599694724afaf234c67ab548b34584aa20a91a46e3f61ecfdca6f5383e`.

Approvals were one-time and restricted to known fixture reads, the exact
calculator operator patch, and the exact test command. No session-wide or global
permission grant was made. The edit run initially supplied `args.diff` instead of
the executor's required `args.patch`; the app emitted a tool error, the model
corrected its arguments, and only the valid retry modified the file. This exposed
a missing argument-schema explanation in the tool prompt and demonstrated real
error recovery. Later prompt-schema improvements were not part of this run.

## Failure discovered and corrected

The first Plan run inspected the files but failed after 118.429 seconds and eight
iterations because its extra completion checker repeatedly returned no decision.
[Baseline evidence](model-eval-2026-09-11/plan-before-report.json).

A direct request to the same model established the cause: the review's entire
220-token budget was consumed by native reasoning. The response had
`finish_reason: "length"` and empty visible `content`. With the same prompt and
`chat_template_kwargs: {"enable_thinking": false}`, it returned `COMPLETE`,
`finish_reason: "stop"`, and two generated tokens in 319 ms. The model's embedded
chat template explicitly supports this flag.
[Non-reasoning probe summary](model-eval-2026-09-11/reviewer-probe-summary.json).

Corrections: read-only Plan/CodeAssist runs no longer require a second model
critic; the implementation critic uses a narrowly scoped
`chat_turns_without_reasoning` request. A regression test verifies that this
request override does not alter ordinary user-generation requests. Seven sidecar
tests passed. The successful Plan and edit runs above include these corrections.

## Limits and remaining verification

- Native reasoning control remains a separate issue: the model server can reason
  internally even when conservatively discovered model metadata says native
  reasoning is unsupported. Ordinary request mapping to the UI Reasoning toggle
  was not changed by this critic-specific fix and needs explicit compatibility
  coverage.
- These small tasks do not validate long-context work, vision, cancellation under
  load, every permission bypass, or all local models. Plan's read-only behavior
  was observed here; exhaustive enforcement belongs to the automated tests.
- A read-only snapshot of the user's earlier run found no inference worker, no
  pending permission, and an unchanged `OBSERVING` event at iteration 6. It was
  not actively generating; the exact historical cause was not established from
  that snapshot. The evaluation did not stop or repair that user run.

The copied reports contain only this synthetic fixture's requests, events, and
results. Original scratch harness, executable, and raw probe evidence remain at
`C:\Users\ism19\AppData\Local\Temp\companion-eval-da35b3dc-8360-4191-b667-229a72df858d`.
The hard-coded scratch launcher was not promoted to a reusable project command:
a safe runner should first verify a dedicated data directory and fixture marker,
not merely trust that a different API port implies isolated user history.

## Later harness and visible-output telemetry regression coverage

After the real-model runs above, the backend suite completed with **210 passed,
zero failed, one ignored** (211 tests). This includes stricter tool-fence parsing,
conversation/continuation intent guards, verification checks, and the following
streaming-telemetry regressions. These are automated mock/pure-function tests,
not another real-model run.

- Empty content and role-only deltas do not start the output timer. Nonempty
  `reasoning_content` changes phase to `thinking` without forwarding or storing
  its text. The first nonempty content delta changes phase to `responding`.
- A mocked response with 500 total completion tokens but only `界!` of visible
  text records two visible tokens using the local tokenizer. Native reasoning
  usage is not the numerator of visible-output speed. Without that endpoint,
  the character-based fallback is explicitly marked estimated.
- Splitting the UTF-8 bytes of `界` across transport chunks preserves the exact
  text. Both LF and CRLF SSE framing are covered.
- Deterministic timing tests exclude preparation, observed reasoning, trailing
  server delay and between-round tool/prompt waits from output duration. A
  single buffered content delta has no measurable emission interval, so its
  output rate is null rather than an invented speed.
- An old metrics table is migrated additively: existing overall rates retain
  `timing: null`. New `visible_output_v1` timing round-trips through storage.

The streamed Chat/Code Ask response and metrics endpoint now carry a versioned
`timing` object with `first_visible_ms`, `output_ms`, `output_tokens`,
`output_tps`, `estimated`, `token_basis`, `thinking_ms`, and `total_ms`. Output
duration is the sum of first-to-last visible delivery spans in each round;
first-visible delay and total wall time remain separate. The local tokenizer
lookup is bounded to 750 ms and excluded from output duration. Thinking duration
is observed channel-delivery time only; absence of a reasoning-channel signal
means unknown, not zero internal reasoning.

Non-streaming AgentRunner turns do not provide a measured visible-output rate.
An explicitly stopped generation can be aborted before final metrics are saved;
it must not be assigned a fabricated final rate. These limitations remain
distinct from the successfully tested completed streaming response path.
