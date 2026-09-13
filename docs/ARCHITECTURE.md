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
| Tool Registry (§60) | `tools.rs` + `terminal.rs` | real write/patch/search/delete, timeout cmds |
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
  largest fitting context (halving to 4,096, f16 then q8_0). The resolved
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
  (`api::assemble_request_context`): chat, routing (`/api/chat/classify`
  appends one instruction turn after the identical prefix) and the agent's
  completion review (appended to the run transcript). Reasoning off sends
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
  two and stop at five, any successful action resets both. Every tool error
  echoes the received arguments; `edit_file` reports the closest region for a
  missing `old` and the count for an ambiguous one; a missing `path` names the
  last file; rejected completion claims are journaled as `thought` events.
- Storage: indexes on every conversation/workspace-keyed table, one query for
  all journals of a conversation, `busy_timeout`.

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
- Binary search: `COMPANION_LLAMA_SERVER_BIN` → PATH → `models/bin/` (§52 errors tell the user where to put it).
- Setup: download a llama.cpp release, place `llama-server(.exe)`, add a GGUF at `models/<id>/model.gguf` + `metadata.json`, Scan → Load → Start inference.

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
- Default autonomy = Level 1 Assisted.

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
