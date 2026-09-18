# Architecture (Stages 21–44: shell, sessions, agent project loop, RAG, release, UI guide)

Source of truth: `Local_LLM_PC_Companion_Design.md` (§§1–107).

## Stack decision
- Backend: **Rust** (spec §6: C++ preferred, Rust acceptable). Chosen over C++
  because this machine has `cargo/rustc` but no `g++/cmake`, and Axum +
  rusqlite + sysinfo give a fast, testable skeleton. `IInferenceEngine`
  (§100) keeps the door open for a C++ llama.cpp core later.
- Frontend: React + TypeScript + Vite (§6). Tailwind deferred; plain CSS
  shell now to keep Stage 1 dependency-free.
- DB: SQLite via rusqlite `bundled` (§20, §70).

## Modules (§101) → `backend/src/`
| Spec module | File | Status |
|---|---|---|
| Inference Engine (§100) | `inference.rs` | `IInferenceEngine` trait + `StubEngine` + `LlamaCppEngine` notes |
| llama.cpp sidecar (§6) | `llamaserver.rs` | spawn/health/OpenAI-compatible client, single-run manager |
| Model Manager (§8–16) | `models.rs` | registry, validate, switch, remove, auto-config |
| Downloads (§10) | `downloads.rs` | URL→GGUF with resume/pause/cancel/SHA-256, estimates |
| Context Manager (§21) | — | Stage 6 |
| Conversation Manager (§20) | `storage.rs` | edit+truncate, attachments, tool audit |
| Agent Engine (§28–30) | `agent.rs` + `agent_runner.rs` | LLM loop, approvals, registry, SSE tail |
| Tool Registry (§60) | `tools.rs` + `terminal.rs` | real write/patch/search/delete, timed and background commands |
| Page preview (§119) | `preview.rs` | loopback page: status, rendered text, console, layout, picture |
| Browser control | `cdp.rs` | DevTools protocol over a hand-written WebSocket |
| Project map | `outline.rs` | a file's definitions with line numbers, a folder's files with sizes |
| Project check | `project_check.rs` | finds the project's own build and tests, reports parsed problems |
| Permission Manager (§25) | `permissions.rs` | RiskLevel + AutonomyLevel gate |
| Workspace Manager (§27) | `workspace.rs` | traversal-proof `resolve()` |
| Hardware Monitor (§11/47) | `hardware.rs` | sysinfo CPU/RAM; GPU stubbed |
| Settings Manager (§49–51) | `settings.rs` | structs + presets |
| Storage Layer (§70) | `storage.rs` | WAL file DB + get/delete, cascade messages |
| API Server (§57–60) | `api.rs` | 15 route groups incl. /api/inference/*, sidecar-preferring chat |
| App Config (§85) | `config.rs` | loopback-only default, data/models dir resolution |

## Stages 21–30 contracts
- Shell: collapsible drawer, Ctrl+1/2/3 + Ctrl+K + Esc, offline banner,
  staged load progress + cancel, density + reduce-motion settings.
- API v1: `/api/v1/*` mirrors the stable surface; `GET /api/v1/status`
  publishes the typed event protocol (state from events, never log text).
- Settings: 16 sections (general→diagnostics); artifacts gain Open/Reveal/
  Save As + generating indicator + info endpoint.
- Code: composer Plan/Run/Diff, full-diff modal, AGENTS.md discovery card
  (behavior-only, never permissions), drag-drop attach, share picker with
  explicit summary/messages/attachments/memory includes.
- Context: breakdown + health + output reserve on `GET .../context`;
  `POST .../compact` returns before/after/saved stats; timeline endpoint.
- Memory: `memory_entries` table, scoped visibility (global + own conv/ws),
  explicit share copies, UI panel with scope picker.
- Sessions: activity classes, priority + related_to, portable export,
  recovery (stale vs loaded model, resume/discard), row actions
  pause/stop/reduce; restart test via recovery endpoint.
- Attachments: office bytes as base64 → magic sniff (PDF /Count, ZIP entry
  scan, strings) with ready/partial/unsupported status; budget endpoint;
  OCR honestly reports missing engine; viewer serves bytes from disk.
- Resources: active-session attribution labeled measured/unknown (nulls,
  never guesses), row actions, cache-budget endpoint.
- DAIO: `daio.rs` profiler (SIMD candidates, nvidia-smi VRAM, NPU stays
  unavailable without evidence), capability DB, workload classes with
  objectives, full-memory-model placement, `/optimize` + `/calibrate`
  (honest baseline-uncalibrated) endpoints, UI card with rationale.

## Stages 39–44 contracts (UI/UX Design Guide v1.0)
- Tokens + primitives: flagship blue-tinted dark palette, spacing/radii/type/
  motion scale tokens; `src/ui/` (Button, Toggle, Badge, Tooltip, Popover,
  Divider, Chip, Tabs, useEscape). Token rule: no raw colors/spacing in
  feature CSS. Light theme preserved.
- Shell: CSS Grid sidebar/center/right-panel; topbar owns mode tabs,
  searchable model selector with readiness, Reasoning/Search mirrors, status
  pill (Loading/Switching/Preparing/Ready/Error/Standby), session menu
  (Clear/Compact/Fork/Share/Export/Rename/Permissions), theme, panel toggle.
  Sidebar owns New chat, workspace picker, nav with icons, Pinned, sessions
  with hover menu + inline rename + auto-titles; icon-rail collapse.
- Right panel: Context/Files/Tools/Plan/Resources tabs composing existing
  cards; collapsible, drawer under 1100 px, hidden under 850 px.
- Composer + messages: multiline auto-grow textarea, attachment chips with
  processing state, elevated card; hover/focus message actions; search
  sources as chips; agent events as tool-timeline rows; approvals as modal.
- Code header: project/root/branch/build-system + Build/Test/Diff/Terminal
  secondary actions. Settings legend + Auto badges + Shortcuts table.
  Recommended badges on Recommend/Optimize outputs and model cards.
- A11y/responsive: labeled icon-only controls, Escape on all modals,
  aria-modal dialogs, skip-to-composer link, no color-alone states,
  1400/1100/850 breakpoints.

## Stages 31–38 contracts
- Chat tool loop: code sessions read files in-chat (SAFE tools only,
  workspace-confined, ≤3 rounds, audited as `chat:*`); writes/commands are
  refused in-chat with an Agent-panel pointer. Usage line shows file reads.
- Repo index: `repo_index.rs` file/symbol map (build dirs skipped), cached
  per workspace, `?q=` ranked lookup + refresh; card with counts + finder.
- RAG: `knowledge_chunks` table, folder/file ingest (chunked 800 chars),
  keyword retrieval into code chats with citations; embeddings staged.
- Telemetry: `generation_metrics` table recorded per turn, `.../metrics`
  endpoint with excerpts; bubbles keep Avg tok/s after reload.
- Git: read-only status/log/branch endpoint + card; `git_commit` MODERATE
  tool (single-line message, approval + confirm); destructive git refused
  in the terminal denylist.
- Office output: std-only docx/pptx (stored-zip) + PDF writers, validated
  by reopen (zip check / %PDF magic); `create_document` covers 9 formats.
- Automation: `list_processes` (SAFE), `open_path` (MODERATE, OS handler);
  screenshots honestly unavailable (no capture backend).
- Plugins: `plugins/*/plugin.json` registry, declared-tool cross-check
  (unknown ignored), `.../run` executes through the same permission gate
  + audit trail; UI card with per-tool Run.
- Doctor: infallible per-subsystem rows (never 500s); card with re-run.
- Setup/benchmark: `/setup/status` 5-step wizard data + Models-tab wizard;
  `/system/benchmark` times live generation, 503 when idle — never faked.
- Chat list renders newest 150 with a "show older" control (perf).

## Stages 16–20 contracts
- Recommend: workload × profile engine with quant-aware weights, q8 KV model,
  15% reserves, per-candidate verdicts; measured VRAM preferred over stubs.
- Switching: last_model per thread, blocking banner, staged SSE prepare,
  compatibility matrix, 409 guard with wait/stop/switch.
- Vision: JPEG data URLs to vision models; honest OCR-less fallback; mmproj
  passed through on start.
- Documents: JSON-spec rendering (txt/md/json/csv/html/xlsx + std-only
  docx/pdf/pptx), validated by reopen, artifact rows + download endpoint.
- Units: sysinfo 0.30 reports bytes; all GB math divides by 2^30 (regression
  tested after a 64 TB headroom incident).
- Reasoning: per-message override → conversation default → settings; native
  vs extended honest labeling; `supports_reasoning` never defaults true.
- Search: toggle-only web access (DuckDuckGo keyless default, Brave keyed);
  `web_search` tool runs in the async layer with consent; citations persist;
  failures degrade to local-only, never fabricated.
- Commands: `/`-prefixed deterministic shortcuts; SAFE run inline,
  MODERATE/HIGH spawn supervised agent runs; only implemented ones listed.
- Sessions: conversations carry mode/workspace/defaults; workspaces CRUD with
  build detection; fork copies thread+files; share posts explicit packages;
  residency computed (active/warm/cold), KV stays llama-server's job.
- Resources: 2 s sampler (sysinfo + nvidia-smi), 1 h ring, windowed graphs,
  honest measured/estimated/unknown attribution, actionable alerts.
- UI: token CSS variables, dark/light/system themes, toasts, highlighted code,
  mode switcher, command menu, agent approvals, settings form, resources tab.

## Stages 6–10 contracts
- Chat sends the recent thread (20 turns / 60k chars) + attachment excerpts as
  real roles; `GET .../context` shows kept/dropped/estimate (§21).
  `PATCH .../messages/:mid` edits and truncates later turns.
- Attachments: text stored under `data/attachments/<conv>/`, excerpt in DB.
- Tools execute only via `POST /api/tools/execute` (gate → run → audit);
  session grants cover MODERATE, DANGEROUS always asks.
- Agent: `POST /api/agent/run` spawns the LLM loop; `.../events` tails SSE;
  `.../resume` approves/denies; `.../stop` cancels. Code-assist is read-only.

## Runtime performance contract (2026-09-13, see docs/PERFORMANCE.md)
- Launch flags are measured, not assumed: automatic mode sends
  `--cache-reuse 256 --spec-type ngram-simple` and leaves batch/threads to the
  runtime; `Settings > Performance` adds speculative (auto/off) and KV cache
  precision (f16/q8_0). CPU fallback keeps speculation and the runtime's
  thread choice; the context is capped by free RAM (`inference::cpu_context_cap`,
  8,192 when comfortable) and KV stays f16.
- Memory fit at every load (`inference::fit_to_memory` → `MemoryPlan`):
  free VRAM is re-measured after the old worker exits, the KV cost per token
  comes from the GGUF header (`ModelMetadata::kv_bytes_per_token`), and the
  plan chooses placement `gpu` / `hybrid` / `oversubscribed` / `cpu` with the
  largest fitting context in 1,024-token steps (f16 or q8_0, whichever gives
  more), under a margin of the compute reserve plus max(total/48, 256 MiB)
  calibrated on a 12 GB card (docs/validation/2026-09-14-stuck-agent-run.md).
  A model whose weights fit but whose smallest cache misses by less than that
  margin stays on the GPU at 4,096 tokens rather than a hybrid plan. The resolved
  policy carries `placement` and notes; the policy endpoint reports the plan
  for the next load (VRAM fitting is skipped while a model is running, since
  its own memory would be counted as used). A `model_id` that is not
  installed (a saved default after the file was removed, or an empty models
  folder) yields the generic plan plus `missing_model`, never an error; an
  empty models folder still seeds the in-memory demo entry (§88) so the
  selector is never blank, and loading that entry fails with a clear message.
- One shared HTTP client to the sidecar (connect + idle-read bounds only; no
  whole-request timeout). Downloads use a separate client with the same shape.
- `SidecarClient::stream` is the single streaming core: visible deltas,
  `reasoning_content` deltas, phase changes, `finish_reason`, the engine's
  `timings` (prompt/decode tok/s, `cache_n`, draft acceptance) and an early
  `should_stop` predicate that closes the connection so the slot is released.
- Every request of a conversation shares one KV prefix
  (`api::assemble_request_context`): chat and the agent's completion review
  (appended to the run transcript). Code sessions have no routing request:
  every message goes to the agent, which answers or acts within the session's
  permission mode (ask, accept edits, plan, auto). A question answered before
  anything was read is sent once to read the files it concerns
  (`asks_for_information`, `READ_BEFORE_ANSWERING`); a change request gets the
  "start the work" push instead. A plan run ends with the plan-only
  `present_plan` tool (as Claude Code's ExitPlanMode); only then does the run
  summary carry `plan_ready` and the conversation show the approval card, whose
  "Yes" switches the mode and continues the planned task. Reasoning off sends
  `chat_template_kwargs.enable_thinking=false`; `supports_reasoning` and
  `tool_calling` are read from the GGUF chat template, and a lone `mmproj`
  next to the weights enables vision.
- Chat: `max_tokens` is derived from the loaded context (8K/12K/16K caps),
  history is sized from `n_ctx` (`history_char_budget`), `done` carries
  `truncated`, and `event: reasoning` streams native thinking (never stored).
  A dead worker produces an error, never an echo.
- Agent: streamed action requests emit `thought_delta` events (live only,
  never journaled), stop at the first complete action object, prune the
  transcript by estimated size (release old tool-result bodies first), run
  tools on the blocking pool, and honour the run's `reasoning` flag.
- Tool calling per model (2026-09-17, night decision 59, `tooling.rs`). At a
  model's first load (and again when the runtime build, the model's chat
  template or the check itself changes) `tooling::check` offers a listing
  tool and a file write through the runtime's tool interface (OpenAI-style
  `tools`; the template renders them in the model's own format and the
  runtime's parser returns `tool_calls`), and requires a parsed call and a
  small code file whose text arrives byte for byte. If either fails, the same
  two checks run in the text action format below. The result is written to
  `models/<folder>/tooling.json` (git-ignored): method native | text | none and
  `can_write`. The load returns `tooling_notice`; `GET /api/models` adds
  `tooling` and `tooling_state` (current | stale | unchecked); `POST
  /api/models/:id/tooling/check` re-checks the loaded model.
  - Native method: the agent sends `tools` (only those the run may use),
    `tool_choice: auto`, `parallel_tool_calls: false`, and a system prompt
    without the text tool list or envelope rules. The first parsed call is the
    step's action; the transcript keeps it as an assistant turn with
    `tool_calls` only (the note written before it is journaled, not kept: a
    template may render it after the result, where it read as the next step
    and the call was repeated) and its result as a `tool` turn with
    `tool_call_id`, so every
    later request renders history in the model's own format. Every outcome of
    a call (result, refusal, denial, blocked in read-only) answers it as a tool
    turn. `repair_tool_pairs` runs after pruning and compaction so no call or
    result is left alone. File text in call arguments stays verbatim (a note
    in the content argument was copied as file content by a small model) and
    is released oldest-first only when the window is short, like old results.
    Templates that demand strict alternation get calls and results flattened
    to text.
  - `can_write` false: the run is read-only whatever the permission mode
    (writes answered with the reason), and the UI offers only Ask.
  - Method none (or no current check with a template the runtime reports
    without tools): the schema-constrained envelope from the first step.
- Action parsing (`agent_runner::locate_action`) accepts every shape local
  models produce: `tool`/`json`/bare fences (the latter two only for a
  registered tool name), an unterminated fence when the turn ends after the
  object, the `<tool_call>` tag, the Gemma envelope, a bare object or the
  structured `kind` envelope, `arguments`/`parameters` aliases, raw newlines
  inside JSON strings, prose after a closed action. Two actions stay
  ambiguous. The transcript copy of an unterminated action gets its closing
  marker so later turns imitate a well-formed example.
- Recovery and budgets (`ActionResponsePolicy`, `agent_progress`): unreadable
  reply → native retry with reminder → schema-constrained envelope → stop;
  six consecutive non-successful steps end a run, identical repeats warn at
  two and stop at five, any successful action resets both. The same call
  failing three times stops the run only with no successful file change in
  between (a test failing again after a fix is new evidence). Every tool error
  echoes the received arguments; `edit_file` reports the closest region for a
  missing `old` and the count for an ambiguous one; a missing `path` names the
  last file; rejected completion claims are journaled as `thought` events.
  An unreadable reply is journaled with the JSON parser's reason and its
  ending; the retry asks for smaller content only after a real cut-off. JSON
  repairs also turn `\'` and `` \` `` into plain quotes and keep other stray
  backslashes literally; offsets are always measured on the original reply.
- Completion floor (`VerificationState`): a command marks its working folder
  unobserved until it is listed there (list_directory, `dir`/`ls`, `git
  status`) or a test/build passes from there or above; writes the host read
  back byte for byte (`tools::WRITE_VERIFIED`) are confirmed, edits need a
  read. A tool-free first answer to a change request is sent back once with
  the tool list. The model check sees neither its earlier verdicts nor more
  evidence than the window holds; the repeat stop fails a run only when
  nothing changed between checks, and a check repeating itself after changes
  ends the run completed with its finding quoted.
- A run is ended by its own lack of progress, not by a step count.
  `agent_progress::LoopWatch` counts steps that changed nothing in the project
  and how many of them repeat an earlier inspection: at 8 steps with 3 repeats
  the model is shown what it has already inspected and asked for the next
  change, and at 18 with 6 — or 30 steps with nothing changed at all — the run
  stops with its work kept. A run that may not write (plan, question) is judged
  on repetition alone. `ProgressGuard` still ends a run after six steps with no
  successful tool and after five identical results in a row.
  There is no step ceiling: a count cannot tell a long build from a loop, and
  real work runs to hundreds of steps (owner, 2026-09-18). The run ends on
  evidence or on the user; `max_iterations` is gone from settings, the API and
  the UI, and an old settings file's key is ignored.
- Commands: `terminal::run` collects both streams as they arrive and kills the
  whole process tree at a timeout, so what a command printed before it was
  stopped is still reported (killing the shell alone left the program running
  and the pipe open, and every timed-out command came back empty). A command
  meant to keep running goes to the background instead —
  `execute_command {"background": true}` watches it settle and returns an id,
  `{"background_id": N}` reads it later, `"stop": true` ends it — and every
  background command a run started is stopped when the run ends. An unknown
  command is answered with this shell's own equivalent.
- `preview_page {"url": "http://localhost:5173"}` waits for the server, then
  drives the installed browser through the DevTools protocol (`cdp.rs`, a
  hand-written WebSocket client: handshake, masked frames out, unmasked frames
  in). It reports the status line, title, the text a reader would see, what the
  page logged (from `Runtime.consoleAPICalled`, `Runtime.exceptionThrown` and
  `Log.entryAdded`), and **where things ended up**: window and page size, how
  far the page scrolls sideways and what is too wide, what sits below the first
  screen, what is outside the window, and boxes with content but no size. A
  page that renders perfectly below the fold is the failure reading its text
  can never catch. A screenshot is taken through the same session and stored
  in the conversation's Artifacts panel, because the user can see it even
  though a local model cannot. If the browser cannot be driven on a PC, the
  older dump-the-page path still answers.
  Loopback addresses only. Offered on windows of 16K or more, where the report
  fits beside the work; the same threshold decides whether the prompt asks for
  HANDOFF.md and MEMORY.md to be kept in the project.
- Editing by line number (`replace_lines {path, from, to, text, expect?}`) sits
  beside `edit_file`. Exact-text replacement asks a model to reproduce code
  byte for byte, which is what every run that died on 2026-09-18 got wrong;
  the line numbers are already in front of it, printed by `read_file`. The
  optional `expect` carries the first line being replaced and refuses when it
  does not match, because a number that has moved would otherwise destroy the
  wrong lines. The result says how the numbering shifted.
- `outline {path}` answers "what is in here" without the text: a file's
  definitions with their line numbers (which feed `replace_lines`), or a
  folder's files with their sizes and line counts. Reading whole files to find
  things is what filled the window in the runs that got stuck.
- `project_check {path?}` finds what the project is (package.json scripts,
  Cargo.toml, go.mod, Python tests), runs its build and then its tests, and
  reports the problems as `file:line: message`, deduplicated and capped. A
  failed build stops it before the tests. A toolchain this PC does not have is
  named as missing (looked for on PATH, `.exe`/`.cmd`/`.bat` on Windows,
  Python under python/python3/py) rather than run; a folder that is not a
  project is told so.
- The project's own instructions reach code sessions: the first of AGENTS.md,
  CLAUDE.md, MUSE.md or PROJECT.md is appended to the system prompt, cut to the
  window's share, and framed as how to work in this project — explicitly not as
  something that changes the host's rules or what needs approval, so a file in
  a repository cannot talk its way past them.
- `changes` lists what the run has created, changed or deleted with each
  file's size now. The host kept that record for compaction from the start; a
  run could not ask for it, so a model whose earlier turns were summarized away
  could not tell what it had already built without reading the folder again.
- `remember {"note": "..."}` keeps up to 12 short facts in the task turn, which
  pruning and compaction never touch, and they are re-attached after each
  compaction. The summary written at a compaction is fresh prose every time, so
  a fact the model worked out and did not repeat in it was lost with the turns
  it came from; this is the part of its context the model itself chooses to
  hold on to. `web_search` and `remember` are carried out by the run, not by the
  tool registry (`tools::runs_in_the_agent`).
- A follow-up run in a session that already did work starts from that work: the
  host's record (rebuilt from `tool_executions`) and the project's own
  HANDOFF.md and MEMORY.md are appended to its task turn, because the previous
  run's working transcript went with the run.
- Tool offer per task: `create_document` only when the task names a document
  type, `open_path` and window-opening shell commands (`start`, `explorer`,
  `xdg-open`, `Start-Process`…) only when it asks for something to be opened;
  otherwise the call is refused with the reason. `open_path` refuses missing
  paths.
- Automatic compaction (`settings.memory.auto_compact` automatic|off,
  `compact_at_pct` 50–98, default 90; legacy "ask" reads as automatic).
  Agent: checked at the top of each iteration only (never during a reply,
  tool, approval, check or pending continuation); the run emits a
  `COMPACTING` status (an active state everywhere via
  `AgentState::is_active`), the model writes a progress note on its cached
  transcript, the host appends its action record and outstanding
  requirements into the task turn, and the latest exchange stays verbatim
  when it fits under 70% of the room. Applied only when it frees a tenth of
  the room; repeated only after a sixth of the room of growth. Pruning
  (`pruning_reserve` = a quarter of the window) is the safety net behind it.
  Pruning goes by size only - a run may have any number of turns while they
  fit (a fixed 60-turn cap, removed 2026-09-18, changed the prompt at the same
  early point on every step once reached and made the model server re-read
  the whole window each time). When it must cut, it cuts back to three
  quarters of the budget in one go, so the following steps extend a prompt
  the cache still holds.
- The completion check and the compaction note are sent on the run's own
  transcript with the run's tool list and `tool_choice: "none"`
  (`chat_turns_on_run_prompt`): the template writes the tool definitions at the
  top of the prompt, and without them neither request matched any of the cache
  (a 35K check and the step after it: ~56 s on the 26B, ~4 s now). The check
  is the run's transcript turn for turn: earlier review turns stay in, and
  when any are in view the instruction says they may already be resolved
  (decisions 70-71).
  Compaction is decided before anything is released: releasing old results is
  free and a note costs a model call, so the free step used to run first and
  dropped usage back under the threshold every time, which meant no run ever
  wrote a note (night decision 61). The note carries the host's record of
  completed actions **and of every place the run has already looked** (a line
  per file read, folder listed, search and preview), so a released file is not
  read again; the note left in a released result says so instead of inviting a
  repeat.
  Chat: before a reply, saved history at the threshold of
  `RequestContext::history_room_chars` is folded into the conversation
  summary in window-sized chunks (`fold_conversation_summary`), keeping what
  fits in two fifths of the room; the stream shows a `compacting` phase. The
  context gauge reports usage against the same room.
- File bodies travel outside JSON (2026-09-16). An action may leave
  `content`, `old` and `new` out of its argument object and put each one raw
  between markers after it (`<<<CONTENT` … `CONTENT>>>`), in any envelope
  shape. The text is taken verbatim, so no escaping can damage it; JSON
  arguments remain accepted and unchanged. A block that has opened and not
  closed keeps the action incomplete, because the stream stops on the first
  complete action. When JSON is used anyway and breaks inside one long value,
  `salvage_action` recovers that value by anchoring on the envelope's own end
  (registered tools only, never on a truncated reply). The prompt states the
  reply budget in characters (`reply_char_budget`) and directs longer files to
  `write_file` then `append_file`, and names the shell commands run through.
- The estimate the thresholds use is measured against what the runtime
  reports, and counts everything a request carries: message text, each call's
  `arguments` (where a file's text travels) and the tool definitions. Counting
  the text alone learned 0.9–1.4 characters per token where requests held
  3.0–3.6, which made every estimate two to three times the real size and
  released file text at a third of the real usage (night decision 61).
- A write whose text is the host's own release note is refused: a model that
  saw its earlier write rewritten that way copied the note back as content,
  and two components were written as 169-byte placeholder files.
- Transcript growth per step is bounded so the compaction threshold is not
  crossed before it is checked: a tool result keeps at most a fifth of the
  room (`tool_output_chars`), and a file body the host verified on disk is
  replaced in the stored reply by a note naming the path and size
  (`condensed_reply`, bodies of 1,500 characters and over only).
- Chat templates that refuse a message list are respected
  (`ChatTemplateShape`, read from the GGUF's own template by what its
  `raise_exception` calls say, never from the model's name). For a restricted
  template the system turn is folded into the first user turn and consecutive
  same-role turns are merged, so user and assistant alternate; permissive
  templates are untouched and keep their cached prefix. A 400 whose body
  reports a template refusal is retried once under that shape, covering
  templates whose wording is not recognized.
- Every SSE payload is normalized before it is sent (`sse_text`): axum splits
  `data` on newlines but panics on a carriage return, which killed a worker
  thread mid-generation when a sidecar error carried Windows line endings.
- Context size is reported as the chain it is: the saved setting, the window
  the model was loaded with, and the room left for history after the reply's
  reserve, with the loader's one-sentence reason (`context_note`).
  `settings.runtime.context_fit` = `fit` (default, shrink the window to keep
  the model on the GPU) or `requested` (keep the saved size and let layers run
  on the CPU); the note says which happened and what it cost.
- Session export carries the whole record: full transcript with message ids,
  every tool execution with its arguments and result, the agent journals per
  reply, the runtime snapshot (window, cache types, template shape, policy)
  and the tail of the application log (`logbuf::LogTail`, 2,000 lines), so a
  failure on one machine can be read on another.
- Logs on disk (`logfile.rs`), in `logs/` under the data folder, so a restart
  does not erase them: `companion.log` (every record the backend logs) and
  `model-server.log` (the model server's output a line at a time with its
  arrival time, and Companion's own notes: the command line it was started
  with, ready, stopped by Companion, ended by itself with the exit status in
  plain words). 4 MB a file, two older files kept, rotated by renaming.
- A model server that ends by itself is named wherever it matters: in the
  run's last message (how it ended, where its output is, load it again), in
  `InferenceStatus.stopped`, and in the UI's "The model server stopped" notice
  with a Load it again button. "Companion isn't answering" needs two failed
  status checks in a row (`services/runtimeHealth.ts`; a failure is checked
  again after 3 s).
- Closing (`AppState::shut_down`): on Ctrl+C / Ctrl+Break, the window
  closing, signing out or shutting down (Windows), or SIGTERM / SIGHUP (unix),
  running tasks are cancelled and recorded with the reason first, then chat
  generation and the model server stop. Stopping the model server first had
  made a task in flight blame the model. Tasks left running by a hard kill are
  still marked interrupted on the next start
  (`storage::recover_interrupted_activity`).
- Storage: indexes on every conversation/workspace-keyed table, one query for
  all journals of a conversation, `busy_timeout`.
- Routing (`request_router`, Code sessions only; Chat never routes): the
  model's schema-constrained decision is read leniently (fences, prose, extra
  fields, a cut-off object after the intent); when it is unavailable
  (unreadable, runtime error, timeout, which is 60 s on a GPU and 240 s when
  the model is on the CPU) `heuristic_intent` routes by wording, sentence by
  sentence: work anywhere wins, then a plan request, else a question. The
  response says `source: model | heuristic`; the UI shows no notice for a
  heuristic route and the backend log records why the model's decision was
  unusable.
- Documents (experimental, owner-parked 2026-09-13): `create_document` is
  chat-safe (it renders into the app's artifacts folder, never the project),
  so Chat and a Code session's Ask path can produce
  txt/md/json/csv/html/xlsx/docx/pdf/pptx from a JSON spec. Output is plain
  (no styling, images or charts) and small models need the corrections
  below to get there, so treat it as a preview rather than a feature. The
  spec reader accepts `type`/`format` for `kind`, unwraps a `content`/`spec`
  wrapper, and refuses a blank document with the expected fields named. A
  request that names a file type (`documents::requested_document_kind`) is
  fulfilled only by a produced file: the chat loop nudges once, the agent's
  verification keeps the run open until `create_document` (or a write with
  that extension) succeeded. An unreadable tool call in chat gets one
  correction quoting the JSON parser's error.
- Machine panel: CPU and RAM rows always, GPU and VRAM rows when a GPU
  reports; nothing is hidden for screen height. The collapsed rail shows
  graphics memory on a GPU machine and system memory otherwise.

## Stage 5 streaming contract
- Chat is true token-passthrough: sidecar deltas forward into the SSE channel
  immediately (`token` events); a terminal `done` event carries
  `{prompt_tokens, generated_tokens}` for the context bar (§19, §21).
- `done` also carries measured `gen_ms`, `ttft_ms`, `gen_tps` (output tok/s);
  the bubble shows live rolling tok/s while streaming, then `Avg. X tok/s`
  bottom-right with Copy (hover reveals TTFT). Commands/stubs send null.
- Every chat turn is prefixed with a system prompt (§22): identity +
  capability truth, and in Code mode a capped project snapshot (tree, build,
  instructions) so the model answers from the project and points at
  /init /review /plan instead of denying it can see anything.
- Context display % is real usage (reserve affects health only), so an empty
  chat reads ~0% instead of jumping to the reserve size.
- Sidecar failures mid-stream arrive as `error` events; the user turn is
  already persisted, the assistant turn only on success (§83).
- Frontend (`MessageView.tsx`): react-markdown + GFM, fenced code blocks with
  per-block Copy, message Copy, Regenerate (re-sends last user turn), ▍ caret
  while generating, usage in the context bar (§18).
- Stop is server-side (§46, `generation.rs` + `POST /api/chat/stop`): the Stop
  button aborts the UI fetch AND cancels the tracked run — cancel flag, task
  abort, and the partial streamed so far persisted verbatim as the assistant
  turn (§83). A new turn supersedes a running one the same way.
- GPU reclaim: aborting drops the reqwest stream, closing the HTTP connection;
  llama-server aborts that decode slot, so GPU time goes to the next request
  instead of finishing unread tokens. Verify: start a long generation, Stop,
  send a new turn — it should start promptly rather than queue.

## Stage 4 model contract
- `GET /api/models/:id` → metadata + on-disk facts + `recommended` auto-config (§14, §16).
- `DELETE /api/models/:id` stops inference if current, unregisters, removes the dir.
- Downloads (§10, inference-independent): `POST /api/models/downloads {id, url, sha256?}`,
  `GET` list/one, `POST .../pause|resume|cancel`. Resume uses HTTP Range + `.part`;
  SHA-256 mismatch blocks finalize; cancel removes `.part`, pause keeps it.

## Stage 3 inference contract
- No C++ toolchain on this box → sidecar, not FFI: backend spawns
  `llama-server` (127.0.0.1, default port 3888) and talks OpenAI-compatible
  HTTP (`/health`, `/v1/chat/completions`, `/tokenize`).
- `GET /api/inference/status` → `{engine, running, base_url, model, context_size, binary_found, last_error}` (§80).
- `POST /api/inference/start {model_id?, model_path?, port?, n_ctx?, n_gpu_layers?, n_threads?}` builds argv from settings + overrides, waits for `/health` (90 s), marks model loaded.
- `POST /api/inference/stop` kills the child; `POST /api/models/unload` also stops it (§15).
- Chat prefers the sidecar when running, stub fallback otherwise (collect-then-stream; token-passthrough is Stage 5).
- Binary search: `COMPANION_LLAMA_SERVER_BIN` → `runtime/bin/` (built from the pinned commit in `runtime/llama.cpp.lock.json` by `scripts/build-runtime.ps1` / `build-runtime.sh`) → PATH (§52 errors tell the user what to do).
- Setup: build the runtime with the build script (see README), add a GGUF under `models/<id>/`, Scan → Load → Start inference.

## Stage 2 API contract
- Errors: `{error, hint}` with 400/404/413/500 (§52). Frontend surfaces both.
- Conversations: `GET/POST /api/conversations`, `GET/DELETE /api/conversations/:id`,
  `GET/POST /api/conversations/:id/messages` (roles user|assistant|tool, 200k char cap).
- Chat `POST /api/chat {conversation_id, message}` persists both turns when the id
  is known, then streams `event: token` (§19–20, §83 crash recovery).
- Models: `POST /api/models/scan` registers `<models>/*/metadata.json` (§9).
- Settings `PUT` validates context &gt; 0, temp 0–2, iterations 1–200.
- Agent `POST /api/agent/run` requires non-empty task + workspace (§27).
- DB: `data/companion.db` (WAL). Falls back to in-memory with a loud error log.

## Security invariants (§92–93)
- `LLM → ToolRequest → PermissionManager → Tool → OS`. No direct OS access.
- `WorkspaceManager::resolve` is the only path joiner; lexical + canonical checks.
- MODERATE/DANGEROUS tools require `approved=true` (permission UX sets it, §26).
- Permission modes map to autonomy levels (`permissions::autonomy_for_mode`): ask →
  WorkspaceAgent (reads free, everything else asks; the default), accept edits → AcceptEdits
  (reads and file edits in the project free), plan → read-only runs, auto → Autonomous.
- `PermissionManager::decide_call` sees the call's arguments: an edit inside any `.git` folder
  (config, hooks) asks even in Accept edits, because git runs commands named there. The app's own
  git reads pass `-c core.fsmonitor=false --no-ext-diff --no-textconv`.
- Changing the mode releases waiting actions the new mode allows; a waiting web search is released
  only by Auto. Shift+Tab cycles ask → accept edits → plan (never Auto) and saves where it stops;
  saves go one at a time (`PermissionModeSaver`).

## Limitations (honest, per §79)
- Sidecar only: no in-process GGML (needs a C++ toolchain); needs a downloaded binary + GGUF.
- Stop aborts the UI fetch, not the sidecar request (see Stage 5 gap above).
- GPU enumeration returns CPU fallback; auto-config degrades gracefully.
- Terminal tool is approval-gated stub (Stage 8 adds timeouts + capture).
- No vision/docs/RAG/automation yet (Phases 2–3).

## Run
```powershell
cd backend
cargo test        # 154 tests
$env:COMPANION_ADDR='127.0.0.1:3877'; cargo run --bin companion-backend
cd ..\frontend
& "C:\Program Files\nodejs\npm.cmd" install
& "C:\Program Files\nodejs\npm.cmd" run dev
```
