# Local LLM PC Companion

Privacy-first local AI companion (llama.cpp + GGUF). See `docs/ARCHITECTURE.md`
and [the v5.1 product review](docs/PRODUCT_REVIEW_2026-09-11.md) for implemented improvements, verification and the remaining full-product roadmap.

## Layout (§71)
```
local-llm-companion/
  backend/   # Rust Axum API: chat, models, tools, agent, search, vision, docs, memory, daio, index, knowledge, plugins
  frontend/  # React+TS+Vite: chat/code modes, resources, settings, themes
  plugins/filesystem/plugin.json
  docs/ARCHITECTURE.md
  models/    # GGUF files in root/subfolders; metadata.json optional
```

## Run
```powershell
.\run.ps1                           # builds UI and starts one local process
# Open http://localhost:5173
# Press Ctrl+C once to stop the server and llama sidecar cleanly.
```

For frontend development with hot reload, run Vite separately from `frontend/`;
the normal user runtime is the single backend process, which serves both the UI
and `/api/*` on port 5173. The backend still supports `COMPANION_ADDR` when
started directly, defaulting to port 3877.

## Working modes

- **Chat** for conversation and attachments.
- **Code / Ask** for read-only project questions (default).
- **Code / Plan** for inspection and a proposed implementation plan without edits.
- **Code / Agent** for changes and verification. **Ask** requests action approvals; explicitly selecting **Auto** permits registered actions, including commands and deletion, without per-action prompts. File boundaries and tool safety limits still apply; shell commands are not process-sandboxed by their working directory. Web search requires the task's Search switch and is blocked when its saved policy is Deny.
- Use **Ctrl+K** to find a session or action. The inspector can be resized by dragging its edge or focusing the separator and using arrow keys.
- Models, Resources, Runtime & diagnostics, Tools & plugins and Settings are grouped at the bottom left. There is no duplicate configuration menu in the header.

## Source exploration

Code questions can use up to 24 read-only actions per reply. The model is guided
to search for named symbols, inspect source and tests, and verify documentation
against implementation instead of treating documentation as a complete audit.

`read_file` accepts a workspace-relative `path` and optional 1-based, inclusive
`start_line`/`end_line`. Any line is addressable, including lines beyond 17,000.
Reads default to 200 lines, capped at 500 lines and approximately 12,000 content
characters per chunk. Results include total lines, numbered content, and exact
continuation coordinates; `start_column` handles exceptionally long lines.
Search can target a directory or one file and no longer skips source files
merely because they exceed 1 MB. Broad searches remain capped at 50 matches;
narrow the path or query when that limit is reported.

For read-only chat exploration, each result has an evidence ID. The model can
use `manage_context` with `keep` and `release` ID arrays to select evidence after
reading it. Under context pressure, unmarked chunks with fewer matches to the
recent user requests are released first (oldest first for ties). This relevance
score is a heuristic, not semantic certainty. Explicitly kept chunks and the
latest result are protected. Released chunks retain their path/range so they
can be read again; the original activity log and conversation are not deleted.
Selection is request-local, not permanent memory. Context and action limits
still apply; incomplete inspection must not be presented as a full audit.

## Storage and configuration

Launch location no longer changes the default storage root. Existing `backend/data/companion.db` is preserved when present; otherwise data lives in `data/`. If both exist, neither is merged or removed. Set `COMPANION_DATA_DIR` to an explicit absolute directory to choose one. `COMPANION_MODELS_DIR` and `COMPANION_FRONTEND_DIR` override models and compiled UI locations.

Back up your active data directory before migrations or moving an installation. Model header validation detects structural problems; it is not a checksum verification of all weights.

## Checks

```powershell
cd backend
cargo test
cd ../frontend
npm test       # Node.js 24+ (native TypeScript test imports)
npm run build
```

The complete v5.1 design is a product roadmap, not a checklist of already-shipped features. See the review for remaining durable tasks, sandboxing, vector retrieval, automation and multi-model orchestration work.
