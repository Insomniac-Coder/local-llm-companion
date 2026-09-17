# Harness and Mica patterns: how to incorporate them

Written 2026-09-16 while the GPU benchmark track ran. Source research:
`docs/research/2026-09-16-runtime-harness-research.md`, sections "DeepSeek Harness: patterns for our
harness" (items 1–12, Part 1) and "Mica: llama.cpp integration and optimisations" (items 1–24,
Part 2). This document turns those findings into designs for this codebase, checked
against the code **as it is now** (after the audit's uncommitted changes; line numbers are current as
of this date and will drift).

Nothing here is implemented yet unless marked **done**. No model was loaded to write it.

# Part 1 — DeepSeek Harness

## Ground rules carried into every design

- **Frontend first, backend second** (owner rule): anything a user can set wrong is blocked in the UI,
  and the backend still validates.
- **No model or hardware names** in code. Every rule is global and reads model facts at runtime.
- **Measure before claiming.** Each item lists how its effect is measured; several need the machine
  free and wait for the benchmark tracks to finish.
- Keep our **text action envelope** (fenced blocks with raw `<<<CONTENT … CONTENT>>>` bodies). It
  exists so file bodies never need JSON escaping. The harness patterns are applied *around* it.

## Facts established while writing this (answers to the research's open questions)

| Question | Answer | How known |
| --- | --- | --- |
| Does the pinned llama-server (b10809) expose `chat_template_caps` on `GET /props`? | **Yes.** The strings `chat_template_caps`, `supports_tools`, `supports_tool_calls`, `supports_system_role`, `supports_parallel_tool_calls`, `supports_preserve_reasoning`, `supports_reasoning_effort` are all in the pinned build's `llama-server-impl.dll` / `llama-common.dll`. | Binary strings of the pinned build; server source `server-context.cpp` puts `chat_template_caps` in `/props`. Confirm the exact JSON shape on the first live load. |
| Does b10809 report context overflow as `exceed_context_size_error`? | **Yes.** HTTP 400, body `{"error":{"code":400,"message":…,"type":"exceed_context_size_error","n_prompt_tokens":…,"n_ctx":…}}`. Raised before any token is generated ("request (N tokens) exceeds the available context size (M tokens)"). | Strings in the pinned `llama-server-impl.dll`; `server-common.cpp` `format_error_response`, `server-context.cpp` `send_error`. |
| How does a mid-stream error arrive? | As an SSE frame `data: {"error": {code, message, type}}`. Our stream parser skips frames without `choices[].delta` (`llamaserver.rs` stream loop), so such an error currently looks like a clean end. | `server-context.cpp` `format_error` inside the streaming `set_next`. |
| Does a stream that ends without `finish_reason` count as success today? | Yes; it should be an error class (see item 4). | `llamaserver.rs:1309-1391`. |

## Where each item stands now

| # | Pattern | State now | Remaining |
| --- | --- | --- | --- |
| 1 | Capability gating ("claim less") | **Partly done.** Chat no longer teaches any tool unless the message asks for a document (`api.rs:1658` `build_system_prompt`, `api.rs:1716` `document_tool_note`), and only offered tools run, stop a round or get corrected (`api.rs:1727` `ChatToolOffer`). | Read real template capabilities from `/props`; fulfil documents host-side on models whose template has no tool support; choose the agent's action mode from capabilities. |
| 2 | Stream splitter | Not started. Every content delta is sent as a `token` event (`api.rs:2832`); the agent streams raw `thought_delta` (`agent_runner.rs:1991`). Frontend regexes hide some envelopes afterwards. | Server-side splitter shared by chat and agent. |
| 3 | Committed content vs attempt records | **Partly done.** History replays prose only: `without_action_envelopes` (`agent_runner.rs:597`) applied in `build_turns_budgeted` (`api.rs:3444`). | The *saved* reply is still every round's raw text joined (`api.rs:2995-3012`), including rejected rounds; Stop saves raw partial text (`generation.rs:94-104`). |
| 4 | Error classes, retries, overflow recovery | Not started. Every sidecar failure is `InferenceError::Generation(String)` (`inference.rs:14-20`, `llamaserver.rs:441-469, 1037-1043`). The agent appends "The model call failed (…)" as a user turn and says "Attempt N of 4" (`agent_runner.rs:2028-2035`). | Typed failures, overflow → prune/compact + one retry, transient → backoff, nothing written into the transcript. |
| 5 | Head+tail tool output | Not started. Command output keeps the first 50,000 chars per stream (`terminal.rs:15, 120-124`), stdout before stderr (`terminal.rs:128-150`); agent and chat keep the first 16,000 (`agent_runner.rs:20, 34`; `api.rs:1945, 2099`). | Head+tail with the full copy on disk; stderr tail first on failure. |
| 6 | Byte-stable prompt prefix | Not started. The reasoning preface is inserted at index 1, **before the history** (`api.rs:2662-2673`), although its comment claims the cached prefix survives. Memory and the workspace snapshot are inside the system prompt. | Move every changing block after the history. |
| 7 | Token accounting from reported usage | Not started. Compaction is decided on chars/4 (`agent.rs:122`, `agent_runner.rs:1019-1060`); chat's output budget uses the same estimate (`api.rs` `chat_output_budget` call before each round). | Anchor on the last reported `prompt_tokens`. |
| 8 | One schema per tool | Not started. `ToolDescriptor` has name, description, risk, permission (`tools.rs:11-16`); argument rules are hand-written in prompts; the structured schema accepts any `args` (`llamaserver.rs:931-946`). | Schema + example per tool; generated docs; generic validation; `oneOf` response format. |
| 9 | Durable record of each model request | Not started. Tables: `storage.rs:257-316` (no request table). | `model_requests` table with retention. |
| 10 | Compaction: truncated notes, prune first | Not started. The note is used whatever its finish reason (`compact_run_transcript`, `agent_runner.rs:1140-1200`); `prune_transcript` (`agent_runner.rs:1253`) runs after compaction. | Two local changes. |
| 11 | Offline replay fixtures | Not started. Tests use hand-written SSE frames. | Captured streams as fixtures. |
| 12 | Loop guards / permissions | Keep ours. | One `permission_decided` journal event. |

## Recommended order

Order is chosen so each step can be tested without a model before the next depends on it, and so the
performance items are measured before and after.

1. **Phase A — safety nets (no behaviour change the user sees):** item 11 fixtures harness, item 9
   request records, item 4 error classes. These make every later change testable and diagnosable.
2. **Phase B — what the user sees and what history carries:** item 2 splitter, then item 3 committed
   content, then item 1 capabilities (it reuses the splitter and the host-side document path).
3. **Phase C — speed on the user's hardware:** run the CPU track's app-pipeline test (C6) **first** as
   the baseline, then item 6 prefix order and item 7 usage meter, then C6 again.
4. **Phase D — agent quality:** items 5, 10, 8, 12.

Effort: S = under half a day, M = about a day, L = two or more days (including tests).

| Phase | Items | Effort |
| --- | --- | --- |
| A | 11 (S), 9 (M), 4 (M) | ~2.5 days |
| B | 2 (M–L), 3 (M), 1 (M) | ~3–4 days |
| C | 6 (M), 7 (S–M) | ~1.5 days + two C6 runs |
| D | 5 (S), 10 (S), 8 (M–L), 12 (S) | ~2.5 days |

---

## Item 4 — typed sidecar failures and recovery

**Design.**

```rust
// inference.rs
pub enum SidecarFailure {
    ContextExceeded { prompt_tokens: u32, context: u32 }, // HTTP 400, type exceed_context_size_error
    TemplateRefusal(String),        // existing reshape path (template_safe_turns) stays
    Unavailable(String),            // connect refused/reset, 503 unavailable_error, process gone
    Timeout,
    Cancelled,
    EmptyResponse,                  // completed with no content, no reasoning, no action
    Truncated,                      // stream ended with no finish_reason and no [DONE]
    BadRequest(String),             // other 4xx
    Server(String),                 // 5xx, mid-stream {"error": …}
}
pub enum InferenceError { …existing…, Sidecar(SidecarFailure) }
```

- `llamaserver.rs`: one function `classify_failure(status, body) -> SidecarFailure` reads
  `error.type` and, for overflow, `n_prompt_tokens`/`n_ctx`. The stream loop treats a frame with a
  top-level `error` object as `Server`/`ContextExceeded`, and a body that ends without
  `finish_reason` as `Truncated`.
- `Display` keeps plain-language messages; nothing raw reaches the UI.
- **Agent** (`agent_runner.rs` model-call match, ~2020-2040): never push the failure text into the
  transcript.
  - `ContextExceeded` → release old tool-result bodies (`prune_transcript`), compact if still over,
    retry once; a second overflow ends the run with a plain message.
  - `Unavailable`/`Timeout`/`Truncated`/`EmptyResponse` → up to 3 retries at 1 s, 2 s, 4 s, each wait
    cancellable by the run's cancel token; only an exhausted retry counts against the progress guard.
  - Anything else → stop with a plain message.
  - The status text reads the guard's real limit instead of "of 4".
- **Chat**: `ContextExceeded` → rebuild turns with the history budget reduced by
  `prompt_tokens − context` plus a margin and retry once; other classes → the existing
  "interrupted" path with the plain message.

**Tests (no model).** The existing mock sidecar returns: 400 `exceed_context_size_error`; 503; a
reset connection; a stream with a mid-stream `error` frame; a stream with no `finish_reason`. Assert
the class, the retry count, that compaction ran for overflow, and that the transcript length is
unchanged by failures.

**Risk.** Retries must not double-charge approvals or re-run tools: retries wrap only the model
call, never tool execution.

## Item 9 — `model_requests` table

**Design.** One table, written after each model call (chat round, agent step, compaction note,
classification):

```sql
CREATE TABLE IF NOT EXISTS model_requests(
  id TEXT PRIMARY KEY, owner_kind TEXT, owner_id TEXT, seq INTEGER,
  kind TEXT,            -- chat | agent | compaction | classify | document
  request_json TEXT,    -- turns + options exactly as sent (sampling_body output)
  raw_output TEXT,      -- content + reasoning as received, before any filtering
  finish_reason TEXT, outcome TEXT,  -- committed | rejected | retried | cancelled | failed
  failure TEXT,         -- SidecarFailure class when failed
  prompt_tokens INTEGER, cached_tokens INTEGER, generated_tokens INTEGER,
  created_at TEXT);
CREATE INDEX IF NOT EXISTS idx_model_requests_owner ON model_requests(owner_kind, owner_id, seq);
```

- Retention: keep the newest 300 rows and at most ~50 MB of text; prune on insert.
- Written after the response completes (the storage mutex is held only for the insert).
- **Owner decision (2026-09-16):** on by default, and included in "Export logs". The records can hold
  file contents the agent read, so the export dialog says so before the file is saved.

**Tests.** Round-trip: a stored `request_json` re-sent through `sampling_body` produces an identical
body; retention prunes oldest first.

## Item 11 — replay fixtures

**Design.** `backend/tests/fixtures/streams/<name>.sse` holding raw SSE bodies, plus a tiny axum mock
that serves a fixture byte-for-byte (the existing mock helpers in `llamaserver.rs` tests already do
most of this). Each fixture has a sidecar `.expect.json`: visible text, action events, saved content,
history text. First fixtures: fenced action; tagged `<tool_call>`; Gemma `<|tool_call>`; bare JSON;
an ordinary ```json code block in an answer (must stay visible); an envelope cut off mid-body; a raw
`<<<CONTENT` block; a mid-stream error frame; a stream with no `finish_reason`. New malformed-output
reports add a fixture taken from `model_requests.raw_output`.

## Item 2 — stream splitter

**Design.** New module `stream_split.rs`, used between `SidecarClient::stream`'s `on_token` and the
SSE channel in chat, and for the agent's `thought_delta`:

```rust
pub enum Piece { Prose(String), ActionStarted { name: String, path: Option<String> },
                 ActionFinished { name: String }, ActionIncomplete }
pub struct StreamSplitter { held: String, in_action: bool, at_line_start: bool, reply_started: bool }
impl StreamSplitter {
    pub fn push(&mut self, delta: &str) -> Vec<Piece>;
    pub fn finish(&mut self) -> Vec<Piece>;   // end of round: held text resolved
}
```

- Prose goes out at once. Text that *could* open an action is held: a line-start run of backticks,
  `<tool_call>`, `<|tool_call>`, or `{` as the reply's first non-space character.
- It is resolved with the same recognisers the parser already uses (`fence_opener`,
  `locate_action`), so the splitter and the saver can never disagree about what an action is.
- Held text that turns out to be an ordinary code block is released as prose (a few tokens late,
  never lost). A hold longer than ~400 characters without an opener match is released.
- Inside an action (including `<<<NAME … NAME>>>` raw bodies) nothing is emitted except
  `ActionStarted` once the tool name (and path, when present) has been read.
- SSE: `token` carries prose only; new `action` event `{name, path, state}`; the frontend shows a
  compact action line (it already has one for agent activity) and keeps its regexes only for old
  saved messages.
- `should_stop` keeps reading the raw text, so early stopping is unchanged.

**Tests.** The item 11 fixtures, fed in 1-byte, 7-byte and whole-frame chunks: identical output for
every chunk size.

**Risk.** The largest UI-visible change: a code block appears slightly later. Measured by
fixture tests, not by eye.

## Item 3 — committed content vs attempt records

**Design.**
- Chat keeps two buffers per message: `raw` (every round, as today) and `visible` (the splitter's
  prose from rounds that were **committed**). A round rejected by the document correction
  (`api.rs` correction path) contributes nothing to `visible`.
- The saved message `content` = `visible` + citations. Each round's raw text goes to
  `model_requests.raw_output` with its outcome (item 9). Until item 9 exists, a `raw_round`
  activity in `message_activities` holds it.
- Stop (`generation.rs:94-104`): persist the splitter's prose so far, plus an `interrupted`
  activity; a half-written envelope is never saved as content.
- History (already prose-only): when the loaded model's tool support is `unsupported`/`unknown`,
  replace each removed envelope with a one-line note such as `[read_file src/main.rs]` instead of
  dropping it, so the model still knows the action happened without seeing the envelope to copy.
- Export chat keeps exporting content; "Export logs" keeps raw rounds.

**Tests.** A correction round's text is absent from the saved content; Stop in the middle of an
envelope saves only the prose before it; a count over a **copy** of `companion.db` of assistant
messages containing envelope markers, before and after (expected: new messages 0).

## Item 1 — template capabilities

**Design.**
- After a load reaches `/health`, `GET /props` once; read `chat_template_caps`. Store on the running
  sidecar:

```rust
pub enum Support { Yes, No, Unknown }
pub struct TemplateCaps { tools: Support, tool_calls: Support, system_role: Support,
                          parallel_tool_calls: Support, preserve_reasoning: Support }
```

  Order of truth: user override in the model's `metadata.json` → `/props` → today's template
  heuristic, which can only ever yield `Unknown` or `No`, never `Yes`.
- `ChatToolOffer::for_request(has_workspace, document_requested, caps)`: with `tools = No`, no tool
  text is sent at all.
  - **Owner decision (2026-09-16):** a request for a plain-text document (txt, md, csv, html and
    similar) is fulfilled without a tool. The model is asked to reply with only the file's content,
    and the host saves that reply as the document. A request for a structured document (docx, pptx,
    xlsx, pdf) gets a plain statement that this model cannot create that kind, naming the plain-text
    kinds it can.
- Agent: `tools = No`/`Unknown` → start in the structured action mode (`use_structured`,
  `agent_runner.rs:1955`) instead of fenced envelopes. Raw file bodies still use the continuation
  path.
- `system_role = No` → shape turns up front with `template_safe_turns` instead of waiting for the
  400 refusal and retrying (saves one wasted request on such models).
- Model card and runtime summary show the three states in plain words.
- **"Tools" tag in the model list (owner request, 2026-09-16).** A tag already exists
  (`ModelLibraryItem.tsx`, shown when `tool_calling`), but `tool_calling` comes from a loose substring
  test (`models.rs`: template contains "tool_call" or "tools"). Make it accurate:
  - After a model's first load, store the `/props` capabilities per model file (keyed like calibration:
    file name + size), with the runtime build, and show the tag from that.
  - Before the first load, use a stricter template test that looks for the template actually handling
    a tools list (`tools` used in a Jinja condition or loop, or a tool-call block), not the word alone.
  - The details panel says whether support is "confirmed by the runtime" or "declared by the chat
    template, confirmed on first load". A model whose runtime says no tool support shows no tag.

**Tests.** `build_system_prompt` has no tool text when `tools = No`; `/props` parsing from a recorded
body; `Unknown` never enables fenced mode by itself. Live (machine free): the Gemma 2 2B document
request before/after — saved messages with envelope markers, and whether the document gets created.

## Item 6 — byte-stable prompt prefix

**Design.** Every request is ordered:

1. **Stable system prompt**: identity and rules for the session mode. It must not contain memory,
   the workspace snapshot, reasoning instructions, dates or anything that changes during a
   conversation.
2. **Saved history** (prose only, item 3).
3. **Changing context**, written into the **latest user turn** as a delimited preamble: workspace
   snapshot, saved memory, reasoning preface, web results (already appended there), retrieval hints.
   A late system message is avoided because several templates refuse a second system turn.
4. The user's message.

- Agent runs keep their own system prompt (its rules are large and constant within a run; putting
  them after a growing transcript would re-read them every step). The one re-prefill when a chat
  hands over to an agent is accepted and documented.
- Classification already reuses the chat prefix; with the reasoning preface out of the prefix it
  matches for every reasoning setting.

**Measure.** C6 runs a 5-turn conversation with Reasoning toggled on turn 3 and a memory added
before turn 4. It records per turn: `prompt_tokens`, `cached_tokens` and the wait before the first
word, before and after the change. Expected after: cached tokens ≈ previous prompt on every turn.
On CPU an 8B reads prompts at ~120 tok/s, so each 1,000 tokens re-read costs ~8 s.

## Item 7 — usage meter

**Design.**

```rust
pub struct UsageMeter { reported_prompt: Option<u32>, turns_at_report: usize, chars_per_token: f32 }
```

- After each response: `reported_prompt = prompt_tokens`, remember the transcript length, and update
  a per-model `chars_per_token` (request characters / reported tokens, smoothed). This replaces the
  fixed 4 as the fallback.
- Estimate = `reported_prompt + estimate(turns added since) − estimate(turns removed)`. Added text
  over ~2,000 characters is counted with `/tokenize`.
- Compaction reset: after compaction, estimate until the next report.
- Used by `CompactionPolicy::due`, chat's `chat_output_budget`, and the context bar.

**Tests.** From journals on a **copy** of `companion.db`: the estimate error per request (the
context events carry both numbers) before and after. Unit: the meter's estimate equals the reported
value right after a response and grows by the added turns' estimate.

## Item 5 — head+tail tool output

**Design.**
- `terminal.rs`: capture up to a larger cap (e.g. 2 MB per stream) and write the full output to
  `<data>/runs/<run-or-message-id>/command-<n>.log`.
- `CommandResult` gets `stdout_view`/`stderr_view` built by `head_tail(text, limit)`: one third head,
  two thirds tail, joined by `[… N characters omitted; full output: <path> …]`.
- `format_result`: on a non-zero exit, show the stderr tail **before** stdout.
- One shared `head_tail` replaces the head-only cuts in `agent_runner.rs` (`tool_output_chars`
  users) and `api.rs:1945, 2099`. File-read chunks keep their continuation positions.
- The note tells the model it can `read_file` the log with a line range (read_file already
  supports ranges).

**Tests.** 60K chars of stdout plus 2K of stderr: the stderr tail and the last stdout lines survive at
the smallest room size; the saved log equals the full output.

## Item 10 — compaction fixes

- If the note's `finish_reason` is `length`, trim it to its last complete line, or drop it and keep
  only the host record when fewer than ~3 lines remain.
- In the compaction check: release old tool-result bodies first (`prune_transcript`'s release loop),
  measure again (with item 7's meter), and call the model only if still over the threshold.

**Tests.** Mock sidecar returning `finish_reason: length` for the note; a transcript whose tool
results alone push it over the threshold compacts without a model call.

## Item 8 — one schema per tool

**Design.**
- `ToolDescriptor` gains `args: &'static [ArgSpec]` (name, kind string/integer/boolean/array,
  required, description) and `example: &'static str`. A static registry is enough; no plugin system.
- Generated from it:
  - the agent's and chat's tool documentation, so the two can no longer disagree;
  - a small generic validator run before any tool executes, with one error format naming what is
    missing ("read_file needs path (text)");
  - for structured mode, a `response_format` whose `args` is a `oneOf` over the tools allowed in this
    run, each with its required fields, so llama.cpp's grammar enforces them.
- Raw `<<<CONTENT` / `<<<OLD` / `<<<NEW` bodies stay outside the schema.

**Tests.** Generated docs snapshot; validator rejects each missing required argument; generated
`oneOf` schema snapshot. From journals: "invalid tool arguments" events per run, before and after.

## Item 12 — permission audit event

One `permission_decided` activity with `once | session | auto | denied | cancelled`, written on every
decision path (including timeout = denied). Unit test per path.

---

## What is deliberately not adopted

- Session event sourcing, the plugin runtime, parallel tool execution and permission presets: sized for
  a multi-session product; one user and one `--parallel 1` server do not need them (research item 12).
- Native `tools` JSON for file writes: it would bring back JSON escaping of file bodies. It may be
  worth an A/B later for *read-only* tools on templates with `supports_tool_calls = Yes`.

# Part 2 — Mica

Mica links llama.cpp directly and therefore hand-wrote features that llama-server already has. The
lessons are about **using server features and keeping prompts stable**, not about copying Mica's
native code. Facts below were checked against the pinned b10809 build (strings in
`llama-server-impl.dll` / `llama-common.dll`) and the current code on 2026-09-16.

## Where each Mica item stands now

| Mica # | Topic | State now | Action |
| --- | --- | --- | --- |
| 1 | Static linking vs sidecar | **Decided**: keep the llama-server process, built from pinned source inside the project (`runtime/llama.cpp.lock.json`, `scripts/build-runtime.*`). | Finish the build (handoff). |
| 2, 3, 19, 20, 23, 24 | Android Vulkan priority patch, Hexagon NPU, template matcher, Mica defects, "do not copy" list | Not applicable to a desktop CUDA/Vulkan build. | None. Keep the idea of re-runnable, anchored build patches if the build script ever patches llama.cpp. |
| 4 | Prompt prefix stability | Not fixed. Same root as harness item 6, plus: attachment excerpts (up to 20,000 chars, `api.rs:75`), images (`api.rs:1504`) and web/repo/knowledge blocks are attached to **whichever user turn is newest**, so they move every turn. | Item M4 below. |
| 5 | Warm the cache before the first message | Not done. Every load starts an empty cache. | Item M5. |
| 6 | Save the conversation cache to disk | Not done. The pinned build has `--slot-save-path` and `/slots` save/restore. | Item M6 (needs an owner storage decision). |
| 7 | RAM prompt cache (`--cache-ram`, default 8192 MiB) | Not accounted: the app never passes it and the memory plan ignores it. Option present in b10809. | Item M7. |
| 8 | Hybrid/sliding-window models and cache checkpoints | Not measured. `--checkpoint-min-step` exists in b10809's common library. | Item M8 (measure first). |
| 9 | KV size formula | **Partly done**: GPU loads now size context with `llama-fit-params` (exact for every architecture, `runtime_fit.rs`). The header formula still drives the CPU cap and first estimates. | Item M9. |
| 10 | Hard cap on reasoning tokens | Not done: the reasoning budget only changes `max_tokens`. b10809 accepts per-request `reasoning_budget_tokens` / `thinking_budget_tokens` and `reasoning_budget_message`. | Item M10. |
| 11 | Images: EXIF rotation, which images are sent | **Two bugs confirmed**: `vision.rs` never applies EXIF orientation (it only resizes to `MAX_SIDE_PX` 1568); requests attach the **first** four images of the conversation (`api.rs:1504`, `attachments_for` ordered by rowid), not the latest. | Item M11. |
| 12 | Markdown re-parsed on every token | Confirmed: `App.tsx:668-680` calls `setMsgs` with the whole text on every token; `MessageView` re-renders the full markdown. | Item M12. |
| 13 | Token counting | Same as harness item 7. | Merged: per-model characters-per-token ratio from reported usage. |
| 14 | Micro-batch size | **Measured**: CPU 512 best; GPU 512 within 2% of larger sizes, which cost more VRAM. Under partial offload: being measured in the GPU track (G6). | Decide after G6. |
| 15 | Thread count | **Done and measured**: all physical cores fastest; topology detection; per-model calibration profiles. | None. |
| 16 | Stop during prefill | Not measured. | Add "Stop latency during an 8K CPU prefill" to the CPU track (C6). |
| 17 | mmap / no-mmap / mlock | CPU track C4 measures speed and load time. | Test under memory pressure only if C4 shows a difference. |
| 18 | Sampling: `repeat_penalty` 1.1 | Still 1.1 on every request (`inference.rs:168`, `settings.rs:393`). Ollama uses 1.0. It may lower draft acceptance and exactness on file rewrites. | Item M18 (measure). |
| 21 | Trimming history breaks the prefix | Not measured. | Item M21 (measure, then fix). |
| 22 | Persist cache hits per request | **Partly done**: `generation_metrics.timing_json` stores engine timing (including cached tokens) per chat message. Agent steps, compaction and classification are not stored. | Covered by harness item 9. |

## M4 — every piece of request context stays on the turn it was sent with

Extends harness item 6.

- **Attachments**: the excerpt text for a message is stored with that message (or re-derived
  deterministically from the stored attachment) and rendered into that turn on every later request,
  never appended to the newest turn. The per-turn cap (`ATTACH_CHARS_PER_TURN`) stays.
- **Images**: images are sent with the turn that carried them. The request selects the most recent
  images that fit the room, not the first four; older image turns become a one-line note ("[image:
  photo.jpg, sent earlier]") when they no longer fit.
- **Web results, repo-index hints, knowledge snippets**: stored with the user message they answered
  and replayed at that position, never re-attached to a later turn.
- **Reasoning preface, workspace snapshot, memory**: the changing block inside the latest user turn
  (harness item 6).

**Measure** (C6, before and after): five turns of plain chat; a chat with one attached document; a
web-search turn; Reasoning flipped on turn 3; a code session where the agent creates a file. Per turn:
`cached_tokens` vs the previous request's `prompt_tokens`, and the wait before the first word. The
research estimated (unmeasured) that re-reading a 5–7K-token excerpt each turn costs about 2 s on the
GPU and about a minute on CPU.

## M5 — warm the cache before the first message

- Trigger: a model load completes with a conversation open, or the first keystroke in a conversation
  whose history is not warm. Opening a chat alone does not trigger it (most opened chats are only
  read).
- Action: one background request built by the same `assemble_request_context` as a real send, with
  the history and `max_tokens` 1, output discarded. With `--parallel 1` a real send queues behind it
  and reuses the prefix.
- Skip while an agent run is active; drop the request on conversation switch or model unload.
- Decide from numbers, not a constant: warm only when estimated history tokens ÷ the model's measured
  prompt speed (calibration `prompt_tps`, else the last observed rate) exceeds about a second.
- The composer shows "Preparing conversation…" while it runs.
- **Depends on M4 / harness 6**: without stable prefixes the warm-up is wasted.

**Measure.** Wait before the first word of the first message after a load, with and without warming,
at 8K and 32K history, GPU and CPU.

## M6 — save and restore the conversation cache on disk

- Launch with `--slot-save-path <data>/slots`.
- Save (`POST /slots/0?action=save`) when a turn completes and the slot is idle; write to a temporary
  name and rename.
- Restore (`action=restore`) when a conversation opens after a model load, before any warm-up.
- A JSON stamp beside each file records model file identity (path, size, modified time), `n_ctx`, K/V
  cache types, flash-attention mode, a hash of the system prompt and chat template, the last message
  id and the runtime build. Every field must match before restore; otherwise the file is deleted.
- Never save a slot whose prompt holds image chunks (the server saves text tokens only, so tokens and
  cells would disagree).
- Disk policy is an **owner decision**: a cap in GB, least-recently-used eviction, a per-conversation
  switch, and a free-space floor measured at save time.

**Measure.** Bytes per token of a saved slot (dense, sliding-window, hybrid); restore time from the
`/slots` response vs the cold prompt time, 8K and 32K, GPU and CPU.

## M7 — size the RAM prompt cache explicitly

- Pass `--cache-ram <MiB>` computed by the memory plan: RAM left after CPU-side weights, KV and compute
  buffers, capped at the server default (8192). A low-RAM machine gets a smaller cache instead of
  paging.
- Show it in the runtime plan notes like the other decisions.

**Measure.** llama-server private bytes after switching between 4–5 long conversations, against the
plan's budget.

## M8 — cache checkpoints on hybrid and sliding-window models

Measure first. On the hybrid 27B (`full_attention_interval` 4) and a sliding-window model: six short
turns, logging `cached_tokens` per turn, the server's checkpoint lines and host RAM. If short chats
re-read from the start because checkpoints sit at least 8,192 tokens apart, set
`--checkpoint-min-step` from that measurement (the option is in b10809's common library; confirm the
server accepts it).

## M9 — measured cache sizes

- After each load, parse the server's `KV buffer size`, `RS buffer size` and `compute buffer size`
  lines as they stream (`drain_worker_log` keeps only an 8 KB tail, so capture them early).
- Store bytes per token per (model file, cache type); use them for the CPU context cap and pre-load
  estimates.
- Teach the header formula `full_attention_interval` and `sliding_window`, for a model's first load
  only.
- Correct `docs/PERFORMANCE.md` where it describes halving and a 5% margin (the code uses 1024-token
  steps).

## M10 — hard reasoning cap

- Per request, when the template supports reasoning (`/props` caps, harness item 1): set
  `reasoning_budget_tokens` from the budget setting as a share of the request's output allowance
  (e.g. low ¼, medium ½, high ¾), plus a short `reasoning_budget_message` so the model moves on to
  the answer. Keep the existing hint.
- Test before making it the default: on a fixed reasoning prompt set, compare reasoning tokens, the
  wait before the first visible word and answer correctness, cap off vs on.

## M11 — image fixes

1. **Bug:** read the decoder's EXIF orientation and apply it before resizing (`vision.rs`
   `prepare_image`). Test: a portrait JPEG with Orientation = 6 comes out upright (unit test on
   pixels, no model).
2. **Bug:** select the most recent images, on their own turns (M4). Test: two images in separate
   turns; the second is sent when only one fits.
3. Measure image tokens at 768 / 1024 / 1568 for the projectors in use, and derive the size cap from
   the projector's metadata instead of one constant.

## M12 — render streamed text at frame rate

- `App.tsx`: accumulate tokens in a ref and push to state once per animation frame, flushing
  immediately on done, stop and error; the same for the reasoning map. No fixed millisecond constant.
- **Measure:** browser performance profile during a ~1,400-token rewrite (n-gram drafting delivers
  hundreds of tokens a second) and a long prose reply: long tasks over 50 ms and main-thread scripting
  time, before and after.

## M18 — repeat penalty

Measure before changing: `repeat_penalty` 1.0 vs 1.1 on the rewrite task already in the benchmark
scripts. Record draft acceptance, generation speed and whether the rewritten code matches the
requested change exactly. It can run inside the CPU track's drafting test (8B) or on the GPU.

## M21 — history trimming

Measure how often a request is sent with history trimmed: in a long chat pushed past the budget, log
turns dropped and `cached_tokens` per request. If it is common, trim in larger steps so the prefix
stays the same for several turns, or make compaction always run before trimming starts.

# Part 3 — Relevance to this PC app (owner check, 2026-09-16)

The owner asked that every item be checked against what this app is: **a desktop app for one user,
running local models on whatever PC it is installed on (GPU, hybrid or CPU-only), whose goal is fast,
correct answers that fit the hardware, without over-building.** Mica is a phone app; several of its
solutions exist because of phone limits (the OS kills apps, battery, a slow chip, one APK per CPU),
not because they help a PC. DeepSeek Harness is a multi-session product; some of its machinery is
sized for that.

Verdicts: **Adopt** (clearly helps this app, low risk to the goal), **Measure first** (plausible on a
PC but the size of the gain is unknown; build only if the measurement says so), **Defer** (real but
costly or risky relative to the gain now), **Drop** (not relevant to a PC app, or works against the
goal).

## Verdicts

| Item | Verdict | Why, for a PC app | Risk to our goal and how it is contained |
| --- | --- | --- | --- |
| H1 template capabilities | **Adopt** | Small local models without tool training are what users run on modest PCs; the Gemma 2 malformed-tool case happened on the owner's own CPU-only PC. | A wrong capability reading could withhold tools from a model that handles them: `/props` is authoritative, the heuristic may only say "unknown", and a per-model override exists. |
| H2 stream splitter | **Adopt** | Users on any platform currently see raw action text streaming. | Hiding real content: every hold rule is fixture-tested; ordinary code blocks are released, only slightly later. |
| H3 committed content vs attempts | **Adopt** | Saved replies with rejected rounds and half envelopes act as bad examples for the next turn on every platform. | Losing information: raw rounds are kept in the request records (H9) or the activity journal. |
| H4 error classes and recovery | **Adopt** | Local models have small windows (the owner's 2B has ~4.8K), so context overflow is a normal event on PCs; today an overflow makes the next request bigger. | Retry loops: retries wrap only the model call, are bounded and cancellable, and never re-run tools. |
| H5 head+tail command output | **Adopt** | Agent builds and tests on a PC print long output; the error lines at the end are what matter. | Disk use for full logs: per-run folder, cleaned with the run's retention. |
| H6 + M4 stable prompt prefixes | **Adopt, measured before and after** | Prompt reading happens on the user's own hardware. Measured on this machine: an 8B on CPU reads ~120 tok/s, a 27B on GPU ~930 tok/s. Every re-read turn costs seconds on GPU and tens of seconds to minutes on CPU. This is platform-independent and the largest speed item in the plan. | Moving context after the history could change model behaviour: re-run the Gemma 2 tool-output check and the chat regression prompts before and after; keep the change only if answers are unchanged. |
| H7 usage meter (+ Mica 13) | **Adopt** | Fixes the compaction overshoot the owner reported (112%); code and non-English text break the fixed chars/4 on every platform. | None significant; falls back to today's estimate until the first report. |
| H9 request records | **Adopt (on by default, in log exports — owner)** | Diagnosing a malformed output needs the exact request; a PC has the disk for a capped table. | Privacy (records can hold file contents): stored locally, capped; the export dialog states that exports include them. |
| H10 compaction fixes | **Adopt** | Small windows compact often; a cut-off note replacing history hurts every platform. | None significant. |
| H11 replay fixtures | **Adopt** | Development tooling; lets streaming and saving fixes be tested without loading a model. | None. |
| H8 tool schemas | **Defer** | Useful, but the largest change for a moderate gain; the structured mode is a fallback path. Revisit after H1–H3 show how often argument errors still happen. | A large refactor of the agent prompt could change agent behaviour: not before the fixtures exist. |
| H12 permission audit event | **Defer** | Small audit value for one user. | None; low priority. |
| Harness session event sourcing, plugins, parallel tools, presets | **Drop** | Sized for a multi-session product; one user and one `--parallel 1` server gain nothing. | — |
| M7 `--cache-ram` sizing | **Adopt** | PCs vary widely in RAM; the server silently allows 8 GiB of prompt cache while a hybrid placement already fills RAM with weights (e.g. an 18.6 GB MoE model with its experts in RAM on a 32 GB PC). Paging would wreck the speed the placement work gains. | A smaller cache means fewer instant conversation switches; sized from the memory plan, never below what one conversation needs. |
| M8 checkpoints on hybrid/SWA models | **Measure first** | The owner runs a hybrid 27B and sliding-window Gemma models on this PC; if short chats re-read from the start every turn, that is a large PC-side cost. | Only a launch knob changes, and only if measured. |
| M10 hard reasoning cap | **Measure first** | Runaway thinking is slow on any PC (an 8B on CPU at ~15 tok/s spends over 2 minutes on 2,000 thinking tokens). | A cap can make answers worse: adopt only if correctness holds on a fixed prompt set. |
| M11 EXIF rotation and latest images | **Adopt (bug fixes)** | Desktop users attach phone photos too; sending the first four images instead of the latest is wrong on any platform. | None. |
| M12 frame-rate rendering | **Measure first** | Desktop browsers are fast, but n-gram drafting delivers hundreds of tokens a second on rewrites and each token re-renders the whole markdown. Build only if a profile shows long frames. | Low: display timing only. |
| M18 repeat penalty 1.1 vs 1.0 | **Measure first** | Rewrites and tool JSON must repeat text exactly; affects correctness and drafting on every platform. | Changing sampling changes answers: only with a measured benefit. |
| M21 history trimming | **Measure first** | Platform-independent cache cost; frequency unknown. | — |
| M5 warm-up before the first message | **Defer** | Mica needs it because a phone kills the app and prefill is slow. A PC app stays running; after the first message the cache is warm anyway. On a GPU the first-message wait for a typical history is a second or two. It only matters for CPU-only PCs with long histories right after a model load, and it spends compute on chats the user may only read. | Wasted work and a busy slot. Revisit after H6/M4 and C6 show the first-message wait on the CPU-only machine. |
| M6 save the cache to disk | **Defer** | Built for phones, where the OS kills the app. On a PC the costly moments are app restarts and model switches, which are rarer. The files are large (hundreds of MB to GB at 32K), and a stale file restored against a changed model or template gives wrong answers silently. | High complexity and a silent-correctness risk for a modest PC gain. Revisit only if users restart or switch models often with long chats. |
| M9 measured cache sizes from logs | **Drop for GPU, Defer for CPU** | GPU context is now sized by `llama-fit-params`, which is exact for every architecture. Only the CPU-only context cap still uses the header formula. | — |
| M14, M15, M17 batch, threads, load mode | **Done or being measured on this PC** | Measured on the owner's hardware (M17 in CPU track C4). | — |
| M16 stop latency | **Measure (CPU track)** | Matters only on CPU-only PCs. | — |
| Mica 1–3, 19, 20, 22–24 (static JNI linking, Vulkan/Adreno priority patch, Hexagon NPU, built-in template matcher, Mica's own defects, phone constants) | **Drop** | Android-, phone-GPU- or JNI-specific, or already handled by llama-server on the PC. | — |

## Order (revised after the relevance check)

| Phase | Work | Why here |
| --- | --- | --- |
| A — safety nets | H11 fixtures, H9 request records, H4 error classes | Everything after it becomes testable and diagnosable |
| Quick fixes (independent) | M11 image bugs, H10 compaction note, H5 head+tail output, M7 cache-ram sizing | Small, low risk, user-visible |
| B — what the user sees | H2 splitter, H3 committed content, H1 capabilities | Correct chat and history on small local models |
| C — speed on the user's hardware | C6 baseline (GPU and CPU-only) → H6 + M4 stable prefixes → H7 usage meter → C6 again. Measurements alongside: M8 checkpoints, M10 reasoning cap, M12 render profile, M18 repeat penalty, M21 trimming | The one large platform-independent speed item, proven by before/after numbers |
| Revisit only if measurements justify | M5 warm-up, M6 disk cache, H8 schemas, H12 audit event | Deferred: modest gain on a PC relative to cost or risk |

# Open decisions for the owner

All decided by the owner on 2026-09-16:

1. **`model_requests` (H9): on by default, and included in log exports.** Exports can carry file
   contents the agent read, so the export dialog states that plainly before the user saves the file.
2. **Safety nets first:** A → quick fixes → B → C, as proposed.
3. **Models without tool support:**
   - Say plainly that this model cannot create structured documents (docx, pptx, xlsx, pdf; the kinds
     that need the `create_document` tool's structured spec).
   - Still allow **basic plain-text documents** (txt, md, csv, html and similar), with no tool call
     and no JSON: the model writes the file body as its ordinary reply, instructed to reply with only
     the file's content, and the host saves that reply as the document.
   - This replaces the JSON-schema host-side path proposed in H1 for these models.
   - `documents::requested_document_kind` needs a `txt` / "text file" rule; today it detects
     pptx, xlsx, pdf, docx, csv, md and html only.
   - Frontend first: when the loaded model lacks tool support, the UI says which document kinds are
     available before the user asks.
4. ~~Agree with deferring warm-up (M5) and the disk cache (M6) for a PC app?~~ **Decided 2026-09-16:
   owner agrees — both deferred.** Revisit only if measurements (C6 on the CPU-only machine) show a
   large first-message wait after model loads.
