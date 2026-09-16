# Template refusals, long actions and the context chain — 2026-09-16

Four faults reported together: a chat model that answered 400 to everything, a
backend thread that panicked mid-generation, a long file write that failed and
took the run with it, and a context size that read as a third of what was set.

## 1. Every request to one model returned 400

**Reported.** Photographs of the console on a CPU-only machine running a 2B
model: repeated `request routing failed: … llama-server returned 400 Bad
Request: {"error":{"code":400,"message":"Unable to generate parser for this
template. Automatic parser generation failed: … raise_exception('Conversation
roles must alternate user…`, then `sidecar chat failed` with the same body.

**Reproduced exactly**, against the same model file on the development
machine, by sending two same-role turns in a row:

```
messages: [{user: "You are helpful."}, {user: "hi"}]
-> 400 "Conversation roles must alternate user/assistant/user/assistant/..."
```

The model's template refuses a system role outright and requires strict
alternation. llama.cpp folds the system turn into the first user turn itself,
which places it next to the real user turn — two user turns in a row — and the
template raises. The app sent a system turn on every request, so every request
failed. The same list merged into one user turn answers normally:

```
messages: [{user: "You are helpful.\n\nhi"}]  -> "Hi! 😊 How can I help you"
```

**Fixed.** `ChatTemplateShape` is read from the GGUF's own template by what its
`raise_exception` calls say — "system" means no system role, "alternate" means
strict alternation. No model name or template text appears in the logic; the
reader is driven entirely by the file. For a restricted template the system
text is folded into the first user turn and consecutive same-role turns are
merged. Permissive templates are returned untouched, so their cached prompt
prefix is unchanged. A 400 whose body reports a template refusal is retried
once under the strict shape, for templates whose wording is not recognized.

**Verified.** The model's own file now reports
`"chat_template":{"system_role":false,"strict_alternation":true}` at scan.
Loaded CPU-side, `write linked list in python` streamed 222 token events with
no error event, and a multi-turn follow-up with reasoning enabled streamed 552
more. No 400, no refusal retry, nothing in the log.

## 2. A worker thread panicked mid-generation

**Reported.** `thread 'tokio-rt-worker' panicked at axum-0.7.9/src/response/
sse.rs:363: assertion 'left == right' failed: SSE field value cannot contain
newlines or carriage returns`.

**Cause.** `Event::data` splits its payload on newlines but asserts that no
carriage return remains. The sidecar's error text carried one. This was never
limited to error frames: any model output containing a carriage return would
have killed the same thread.

**Fixed.** `sse_text` normalizes line endings and every event in the module
sends through it (`safe_data`).

## 3. A long file write failed and cost the run

**Found in the app's own history**, not reported second-hand: the run
"develop a website with modern UI that has a clock, timer and stopwatch".

| Step | What happened |
| --- | --- |
| 5 | `write_file` for `app.js`: 12,084 characters, complete, closing tag present. JSON failed at column 4221 — one stray character inside 10 KB of JavaScript. All 12,084 discarded. |
| 6 | The retry produced a shorter, different `app.js` with a syntax error. |
| 8 | `node --check` found it; five further steps went on locating and repairing it. |
| 17 | `ls -la` failed: commands run through `cmd`, which the prompt never said. |
| 10, 29 | Two compactions, at 95% and 94% of usable room. |
| 30 | Step limit reached. Run failed with the work incomplete. |

**Fixed.** Four changes, in the order they bite:

- File bodies no longer pass through JSON escaping. `content`, `old` and `new`
  may be left out of the argument object and written raw between markers
  (`<<<CONTENT` … `CONTENT>>>`). Verified end to end: a body carrying an
  apostrophe, an escaped quote, a regex backslash, a template literal and a
  Windows path reaches disk byte for byte, and a second part appends to it.
- When JSON is used anyway and breaks inside one long value, `salvage_action`
  recovers it by anchoring on the envelope's own end. Registered tools only,
  never on a truncated reply, and the journal records what was recovered.
- The prompt states the reply budget in characters and directs a longer file
  to `write_file` then `append_file`, which is the only way to produce one at
  all on a 5K window. `append_file` is new and host-verified like `write_file`.
- The prompt names the shell commands run through.

## 4. The context size read as a third of what was set

**Reported.** 32,768 set; the code chat showed 11k.

**Measured.** Three numbers, of which only the last was shown:

| | tokens | decided by |
| --- | --- | --- |
| Setting | 32,768 | the user |
| Window loaded | 16,384 | the memory fit |
| Shown in the meter | 11,776 | window minus the reply reserve |

The model in use is 9.6 GB of weights against about 10.5 GB of free GPU
memory, so the weights alone do not fit and it runs hybrid. Its cache costs
about 138 KB per token even at 8-bit — 65 layers with 256-wide keys and values
— so 16,384 tokens is 2.3 GB. The fit caps the cache at a quarter of GPU
memory to keep as many layers as possible on the GPU, which is where 16,384
came from; 32,768 would want 4.6 GB and push roughly half the model to the CPU.

**Fixed.** The meter shows the chain and the loader's one-sentence reason.
`settings.runtime.context_fit` adds the choice: fit it to memory (faster) or
use the size as written (slower), with the note saying which happened and what
it cost.

**Verified.** With a model whose own trained limit is the binding constraint,
the endpoint reports `configured_limit: 32768`, `limit: 8192`,
`history_room_tokens: 4657`, `context_fit: "fit"`.

## 5. Compaction ran after the threshold was passed, not at it

**Reported.** The limit is 90%, but usage reached 112% before compaction ran.

**Cause.** Compaction is checked between steps, which is correct, but one step
could add more than the margin: a tool result was capped at a fixed 16,000
characters — a third of the usable room on a 16K window, more than all of it
on a 5K one — and the stored reply kept a second full copy of any file the
model had just written.

**Fixed.** A tool result keeps at most a fifth of the room. A file body the
host has verified on disk is replaced in the stored reply by a note naming the
path and size; the progress note and the action survive, and bodies under
1,500 characters are untouched.

## 6. Logs and chats could not leave the machine

The failures above were reported as photographs of a console, because the
application log went only to stdout and the export carried a summary.

**Fixed.** The export now carries the full transcript with message ids, every
tool execution with arguments and result, the agent journal per reply, a
runtime snapshot (window, cache types, detected template shape, policy) and
the tail of the application log, which is now kept in memory (2,000 lines).

## Suite

332 backend tests, 64 frontend tests, types clean, no new clippy warnings.
