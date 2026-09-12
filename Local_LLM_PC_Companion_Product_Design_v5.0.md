# Local LLM PC Companion — Product Design & Engineering Specification v5.0

**Status:** Proposed product architecture  
**Supersedes:** Design Document v4.1 as the recommended implementation source of truth after approval  
**Product class:** Local-first desktop AI assistant and coding workspace  
**Primary inference engine:** `llama.cpp`  
**Primary model format:** GGUF  
**Initial platform:** Windows first; architecture remains portable to Linux/macOS  
**Frontend:** React + TypeScript in a desktop WebView shell  
**Core host:** Rust  
**Inference boundary:** `llama.cpp` worker process behind a stable inference protocol

---

# 1. Executive Decision

v4.1 contains many good subsystem ideas, but it accumulated multiple overlapping architectures, MVP definitions, navigation models, optimization systems, and future capabilities. The result is a strong research/design notebook but not yet a single buildable product contract.

v5.0 deliberately reduces the architecture to one product model:

> **A local desktop AI application where every conversation is a persistent session, a session may optionally be attached to a workspace, the model reasons, deterministic tools act, and the application owns safety, context, persistence, scheduling, and recovery.**

The product MUST feel simple even though the runtime is sophisticated.

The first production release is not a universal computer agent. It is a reliable local AI assistant with a high-quality coding/workspace mode.

The key product promise is:

> **Install the app, choose or import a local model, start chatting, optionally open a project, let the assistant inspect and safely modify that project, and resume everything after a restart.**

Everything that does not strengthen that promise is deferred.

---

# 2. What Changes from v4.1

## 2.1 Keep

The following ideas from v4.1 remain core:

- Local-first inference and storage.
- `llama.cpp` as the primary inference runtime.
- Model-agnostic application interfaces.
- The LLM is never the security boundary.
- Structured tool calls instead of direct OS access.
- Explicit workspace boundaries.
- Permission-gated dangerous operations.
- Deterministic software for deterministic operations.
- Persistent sessions separated from ephemeral inference state.
- Context compaction without destroying canonical conversation history.
- Runtime metrics derived from real measurements.
- Cancellation and crash recovery as first-class requirements.
- Capability-aware UX.

## 2.2 Change

v5.0 changes the following architectural assumptions:

1. **There are not two separate runtimes called Chat and Code.**
   There is one session runtime. A session with no workspace behaves as a general chat. A session attached to a workspace gains coding/project tools and project context.

2. **The UI does not directly depend on a local HTTP server in production.**
   The desktop shell communicates with the Rust core using desktop IPC/commands and a structured event stream. A versioned HTTP/SSE adapter MAY exist for development, CLI, or future remote use, but is not the security boundary.

3. **`llama.cpp` does not run inside the UI/control process.**
   Inference runs in a supervised worker process. An OOM, model crash, backend fault, or model reload MUST NOT take down the entire application shell.

4. **A user message creates a first-class Run.**
   Agent execution, model generations, tool calls, permissions, cancellation, metrics, and recovery belong to that Run.

5. **The first release supports many persistent sessions but only one active primary inference generation at a time.**
   Other sessions remain instantly navigable and can queue work. True concurrent model generations are deferred until resource scheduling is proven.

6. **DAIO is split into a safe Runtime Advisor for v1 and an adaptive optimizer later.**
   Hardware detection and safe automatic configuration are product-critical. Continuous self-optimization, NPU routing, thermal optimization, and learned runtime profiles are not v1 requirements.

7. **Resources and Artifacts are not primary navigation destinations.**
   Resource status is ambient and inspectable. Generated files appear where they were created. This avoids turning the application into an operations dashboard.

## 2.3 Defer

The following capabilities are explicitly outside the first production release:

- Browser automation.
- General PC mouse/keyboard automation.
- Plugin marketplace or public plugin SDK.
- Multi-agent orchestration.
- Multi-model routing.
- Multiple simultaneous large-model generations.
- Persistent semantic user memory.
- General local RAG/knowledge-base indexing.
- Full office-document generation/editing.
- Advanced browser/web research.
- NPU execution optimization.
- Continuous thermal/power-aware self-tuning.
- Cross-device sync.
- LAN inference.
- Background scheduled agents.
- Advanced per-session GPU attribution.

These may be added later without changing the core domain model.

---

# 3. Product Definition

The product has four user-facing concepts:

```text
Application
│
├── Chats
│   └── Sessions without a workspace
│
├── Projects
│   └── Workspaces containing one or more sessions
│
├── Models
│   └── Installed/imported/downloadable GGUF models
│
└── Settings
    └── General / Inference / Security / Diagnostics
```

The important simplification is:

```text
General Chat = Session(workspace_id = null)
Project Chat = Session(workspace_id = <workspace>)
```

There is one conversation model, one event protocol, one context builder, one agent runtime, and one persistence model.

The UI MAY visually specialize a project session for code review, diffs, terminal actions, and project files, but this MUST NOT create a second backend architecture.

---

# 4. Product Principles

## 4.1 Local by default

Core chat, project analysis, coding tools, persistence, and inference work without a cloud AI API.

Network access is a separate capability and is disabled unless the user enables a network-dependent feature.

## 4.2 Simple by default

Users should not need to understand:

- GPU layers,
- KV caches,
- prompt caches,
- quantization internals,
- context compaction algorithms,
- tool schemas,
- inference slots,
- agent state machines.

The application chooses safe defaults and exposes advanced controls only when requested.

## 4.3 Transparent under inspection

When the application is acting on the computer, the user can inspect:

- what the model requested,
- what tool will run,
- what path or command is affected,
- what permission was granted,
- what changed,
- what failed,
- what resources are being used.

## 4.4 Deterministic execution

The model decides **what should be done**. Application code decides **how the operation is executed safely**.

## 4.5 Capability honesty

The application must distinguish:

- model supports capability,
- runtime can execute capability,
- current configuration enables capability,
- capability is unavailable.

The UI must not imply that a small local model has frontier-model capabilities merely because tools exist.

## 4.6 Recoverable by design

A crash, model worker failure, cancelled generation, failed command, or interrupted tool call must leave the canonical conversation and workspace in a coherent state.

---

# 5. Product Architecture

## 5.1 Process topology

Production topology:

```text
┌─────────────────────────────────────────────────────────────┐
│ Desktop Application                                         │
│                                                             │
│  React / TypeScript UI                                      │
│          │                                                  │
│          │ desktop IPC + structured events                  │
│          ▼                                                  │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ Rust Core Host                                        │  │
│  │                                                       │  │
│  │ Sessions  Runs  Context  Models  Permissions          │  │
│  │ Tools     Storage     Scheduler    Telemetry          │  │
│  └───────────────┬──────────────────────┬────────────────┘  │
└──────────────────┼──────────────────────┼───────────────────┘
                   │                      │
            supervised IPC         controlled execution
                   │                      │
          ┌────────▼────────┐      ┌──────▼───────────┐
          │ Inference Worker│      │ Tool Executor    │
          │ llama.cpp       │      │ process/helper   │
          │ no OS tools     │      │ scoped actions   │
          └─────────────────┘      └──────────────────┘
```

### Desktop UI

Responsible for:

- navigation,
- rendering conversations,
- streaming output,
- permission dialogs,
- diff review,
- model management UI,
- settings,
- diagnostics.

It contains no security policy and no direct model control.

### Rust Core Host

This is the product control plane and source of truth for application behavior.

It owns:

- session lifecycle,
- run lifecycle,
- persistence,
- context construction,
- model management,
- inference scheduling,
- tool registry,
- permissions,
- workspace boundaries,
- attachment parsing,
- operation cancellation,
- recovery,
- structured events,
- resource telemetry.

### Inference Worker

The inference worker owns only model/runtime concerns:

- load/unload model,
- tokenize,
- generate/stream,
- prompt/KV cache handling,
- runtime metrics,
- backend-specific inference configuration.

The worker MUST NOT have application tool authority.

### Tool Executor

Filesystem mutation, command execution, and other high-impact actions should be isolated behind a controlled executor boundary.

The implementation MAY initially host safe read-only tools inside the Rust core, but terminal execution and other dangerous operations SHOULD use a child/helper process so process trees can be terminated reliably.

---

# 6. Core Domain Model

v5.0 standardizes the product around the following objects.

## 6.1 Session

A Session is the durable unit the user returns to.

```text
Session
├── id
├── title
├── workspace_id?          optional
├── preferred_model_id?
├── created_at
├── updated_at
├── archived_at?
├── settings_override?
└── status
```

A session does not own a permanent KV cache.

It owns persistent conversation identity and configuration.

## 6.2 Message

A Message is canonical conversation history.

```text
Message
├── id
├── session_id
├── role                  user | assistant | tool_summary | system_note
├── content
├── created_at
├── parent_message_id?
├── run_id?
└── status
```

Messages are not deleted merely because context is compacted.

## 6.3 Run

A Run is one user-request execution lifecycle.

A user sending a prompt creates a Run.

```text
Run
├── id
├── session_id
├── initiating_message_id
├── model_id
├── runtime_profile_id
├── state
├── started_at
├── completed_at?
├── stop_reason?
└── error?
```

Run states:

```text
QUEUED
PREPARING
WAITING_FOR_MODEL
BUILDING_CONTEXT
GENERATING
WAITING_FOR_PERMISSION
RUNNING_TOOL
VERIFYING
COMPLETED
FAILED
CANCELLED
INTERRUPTED
```

A run MAY transition between `GENERATING`, `WAITING_FOR_PERMISSION`, and `RUNNING_TOOL` multiple times.

The product should not expose private chain-of-thought. User-visible planning is a concise task plan/status artifact, not hidden reasoning.

## 6.4 Operation

An Operation is any cancellable long-running application action.

Examples:

- model load,
- model download,
- inference generation,
- terminal command,
- project scan,
- attachment parse,
- file patch,
- benchmark.

```text
Operation
├── id
├── run_id?
├── kind
├── state
├── progress?
├── parent_operation_id?
├── started_at
├── finished_at?
└── error?
```

## 6.5 Tool Call

```text
ToolCall
├── id
├── run_id
├── tool_id
├── input_json
├── normalized_action_digest
├── risk_class
├── permission_decision_id?
├── state
├── output_ref?
└── timing
```

Tool outputs may be too large for the message stream. Large outputs are stored separately and summarized for model context.

## 6.6 Workspace

```text
Workspace
├── id
├── display_name
├── root_path
├── canonical_root_path
├── trust_policy
├── read_policy
├── write_policy
├── command_policy
├── network_policy
└── created_at
```

A workspace is a capability boundary, not merely a convenience path.

## 6.7 Context Manifest

Every model generation MUST have a materialized context manifest describing exactly what was selected.

```text
ContextManifest
├── id
├── run_id
├── model_id
├── tokenizer_revision
├── chat_template_revision
├── system_policy_revision
├── token_budget
├── reserved_output_tokens
├── items[]
└── total_estimated_tokens
```

Context items may reference:

- message IDs,
- summary IDs,
- attachment slices,
- file excerpts,
- tool results,
- workspace instructions.

This is critical for reproducibility, debugging, context inspection, and model switching.

---

# 7. Session and Residency Model

Persistent sessions and inference residency are separate.

A session can be:

```text
PERSISTED
  conversation exists on disk

READY
  lightweight derived context metadata is available

RESIDENT
  the inference worker currently has reusable prompt/KV state

ACTIVE
  the session owns the current inference slot
```

The UI does not need to expose all four technical states.

User-facing states can remain:

```text
Ready
Running
Queued
Needs approval
Stopped
Failed
```

## 7.1 Important invariant

> **SQLite conversation/session data is canonical. Prompt/KV caches are disposable accelerators.**

If all runtime caches are deleted, the application must still be able to reconstruct a valid next prompt.

## 7.2 MVP scheduler

v1 supports:

- many open sessions,
- one loaded primary text model,
- one active generation at a time,
- queued runs from other sessions,
- background tool execution only where safe,
- instant UI switching between sessions.

This gives the user a multi-tab product without pretending consumer hardware can efficiently execute many large contexts simultaneously.

---

# 8. Context Engine

The Context Engine is a first-class service, not a collection of prompt concatenation helpers.

Its output is a `ContextManifest`.

## 8.1 Inputs

```text
Context Request
├── session
├── current user message
├── target model
├── workspace
├── attachment references
├── recent tool results
├── summaries
└── token budget
```

## 8.2 Selection order

The default policy should roughly prioritize:

1. System/security/tool contract.
2. Current user request.
3. Recent conversation turns.
4. Explicitly attached content.
5. Workspace instructions.
6. Retrieved project excerpts relevant to the task.
7. Durable conversation summary.
8. Older conversation turns as remaining budget permits.

Exact allocation is model- and workload-dependent.

## 8.3 Output reserve

The context builder MUST reserve token budget for output before packing input context.

It MUST also leave a safety margin for tokenizer/template variance.

## 8.4 Compaction

Compaction creates derived summaries.

It MUST NOT destroy canonical messages.

```text
Canonical messages
       │
       ├────> recent messages used directly
       │
       └────> older region summarized
                       │
                       ▼
                 Summary record
```

The summary stores the range of message IDs it represents and the model/configuration that generated it.

Manual `/compact` MAY remain as a power-user command, but automatic compaction is the normal product behavior.

## 8.5 Project context

The first release does not require embeddings.

Project discovery should use deterministic methods first:

- directory tree,
- file type filtering,
- filename search,
- fast text/regex search,
- language-aware symbol extraction where available,
- import/include relationships where inexpensive.

The model receives only relevant excerpts, not the whole repository.

---

# 9. Agent Runtime

The agent is an orchestration loop over model generations and tool calls.

It is not a separate personality or a second application.

## 9.1 Runtime loop

```text
User message
   ↓
Create Run
   ↓
Build context
   ↓
Model generation
   ↓
┌───────────────────────────┐
│ final answer? ────────────┼──► complete
│                           │
│ tool request?             │
└──────────────┬────────────┘
               ↓
Validate tool request
               ↓
Evaluate permission policy
               ↓
      approval if required
               ↓
Execute tool
               ↓
Store structured result
               ↓
Build next context
               └────────────► Model generation
```

## 9.2 Run limits

The runtime enforces:

- maximum model turns,
- maximum tool calls,
- maximum retries per tool,
- command timeout,
- output-size limits,
- context budget,
- cumulative run duration limit where configured.

The limits are application policy, not prompt suggestions.

## 9.3 Verification

For coding tasks, a run is not considered successful merely because a patch applied.

Where appropriate the agent should verify through deterministic tools:

```text
edit
→ inspect diff
→ build/lint
→ test
→ report result
```

If verification cannot be performed, the final response must state that clearly.

---

# 10. Tool Runtime

## 10.1 Tool contract

Every tool implements a stable contract:

```text
ToolDefinition
├── id
├── version
├── title
├── description
├── input_schema
├── output_schema
├── risk_class
├── required_capabilities
├── timeout_policy
├── output_limit
└── handler
```

## 10.2 v1 tool set

Read-only:

```text
filesystem.list
filesystem.read
filesystem.search
workspace.tree
system.info
```

Mutating:

```text
filesystem.create
filesystem.write
filesystem.patch
filesystem.move
filesystem.delete
```

Execution:

```text
process.exec
```

Optional read-only VCS integration:

```text
git.status
git.diff
git.log
```

Git mutation commands can use `process.exec` initially and remain permission-gated.

## 10.3 Structured patch editing

Code edits should use a patch/edit protocol instead of rewriting whole files by default.

The executor must:

1. verify the target path,
2. verify the expected base content/hash where supplied,
3. apply atomically,
4. report changed ranges,
5. generate a diff,
6. fail safely on mismatch.

## 10.4 Terminal execution

The default command tool uses:

```text
program + argv[] + cwd + env_delta
```

rather than passing arbitrary strings to a shell.

A separate `process.shell` capability MAY be added for commands that genuinely require shell syntax and should carry a higher risk class.

The executor must capture:

- stdout,
- stderr,
- exit code,
- duration,
- timeout/cancellation result.

Large stdout/stderr is truncated in the event stream and stored in a referenced result file/record.

---

# 11. Permission and Security Model

The security model assumes model output, file content, and tool arguments are untrusted.

## 11.1 Capability flow

```text
Model output
   ↓
Structured tool request
   ↓
Schema validation
   ↓
Normalization
   ↓
Workspace/capability validation
   ↓
Risk classification
   ↓
Permission policy
   ↓
User approval if required
   ↓
Tool executor
```

No layer may be skipped because the model appears trustworthy.

## 11.2 Risk classes

```text
READ
  non-mutating operation inside an allowed workspace

WRITE
  creates or changes user data in an allowed workspace

EXECUTE
  starts a process or build/test command

DESTRUCTIVE
  deletes, overwrites, force-resets, mass-moves, or performs similar actions

NETWORK
  sends data outside the local device

SYSTEM
  changes configuration outside the workspace or affects OS/application state
```

## 11.3 Permission decisions

Permission choices:

```text
Allow once
Allow for this run
Allow for this workspace policy
Deny
```

Persistent permission rules must be explicit, inspectable, and revocable.

## 11.4 Action digest

Approval must be bound to the normalized action the user saw.

If the model changes:

- command,
- target path,
- write contents materially,
- network destination,
- destructive scope,

a new permission decision is required.

## 11.5 Filesystem protections

All path-based tools must:

- canonicalize paths,
- resolve `..`, symlinks, junctions, and equivalent redirections,
- verify the resolved target remains inside allowed roots,
- reject device/special paths where inappropriate,
- use safe temporary files for atomic writes,
- prevent silent overwrite unless policy allows it.

## 11.6 Secrets

The system must never automatically place secrets into model context.

Default-deny patterns should cover common secret files and directories such as:

- `.env`,
- SSH keys,
- cloud credential stores,
- browser credential databases,
- password stores.

The user may explicitly grant access, but the application should warn before exposing such content to the model context.

## 11.7 Network

The inference worker has no reason to require network access for core operation.

Network-enabled tools are separate capabilities.

The first production release may ship with network tools disabled entirely.

## 11.8 Local API exposure

Production IPC is local to the desktop application.

If a local HTTP API is enabled:

- bind to loopback by default,
- require a random per-install/session token,
- reject cross-origin browser access by default,
- do not expose it to LAN interfaces without explicit configuration.

---

# 12. Inference Architecture

## 12.1 Stable interface

The core talks to an inference abstraction, never directly to `llama.cpp` internals.

```text
InferenceEngine
├── probe_model(path)
├── load(model, runtime_config)
├── unload()
├── tokenize(input)
├── generate(request, stream_sink, cancel_token)
├── snapshot_cache?(session_key)
├── restore_cache?(session_key)
└── metrics()
```

`LlamaCppWorker` implements this interface.

## 12.2 Worker protocol

The worker protocol must be versioned.

Core-to-worker commands:

```text
hello
probe_model
load_model
unload_model
tokenize
generate
cancel_generation
get_metrics
shutdown
```

Worker-to-core events:

```text
worker.ready
model.load_progress
model.loaded
model.failed
generation.started
generation.delta
generation.metrics
generation.completed
generation.failed
worker.warning
worker.crashed
```

## 12.3 Crash isolation

If the inference worker exits unexpectedly:

1. the desktop remains open,
2. the active run becomes `INTERRUPTED`,
3. the failure is recorded,
4. runtime memory is considered invalid,
5. the user can restart/reload the model,
6. the session remains intact.

## 12.4 One loaded primary model in v1

v1 supports one loaded primary LLM at a time.

Installed models may be numerous.

Changing models performs:

```text
finish/cancel active generation
→ persist run state
→ unload worker model
→ release resources
→ load target model
→ build context for target tokenizer/template
→ continue session
```

A session's canonical history is model-agnostic.

---

# 13. Model Manager

## 13.1 Model record

```text
Model
├── id
├── display_name
├── family/architecture
├── format
├── file_path
├── size_bytes
├── quantization
├── parameter_count?
├── trained_context_length?
├── chat_template?
├── capabilities
├── projector_path?
├── source_metadata?
├── verified_hash?
└── probe_revision
```

Model properties should be probed from the GGUF/runtime where possible rather than trusting a manually maintained `metadata.json` file.

## 13.2 Acquisition

A production app should support both:

- **Import local GGUF** — works offline.
- **Curated download** — optional network flow with size, quantization, compatibility estimate, resumable download, and hash verification.

The first-run product should not require users to understand Hugging Face file naming conventions.

## 13.3 Capability registry

Capabilities are normalized application capabilities:

```text
chat
structured_output
tool_calling
vision
native_reasoning
long_context
```

A capability record may be:

```text
supported
unsupported
unknown
experimental
```

The UI adapts to this registry.

---

# 14. Runtime Advisor (v1 replacement for full DAIO)

The full DAIO concept is valuable as a long-term subsystem, but too broad for the first product release.

v1 implements a **Runtime Advisor**.

## 14.1 Responsibilities

- detect CPU/RAM/GPU,
- detect supported `llama.cpp` backend,
- inspect model size and architecture,
- estimate model memory,
- estimate KV/context memory,
- choose a safe context size,
- choose a safe GPU offload level,
- choose thread count,
- choose conservative batch size,
- detect likely OOM before load,
- optionally run a short calibration benchmark,
- persist the recommended profile per hardware + model + app/runtime version.

## 14.2 Non-goals for v1

The Runtime Advisor does not:

- route to an NPU,
- continuously change configuration while a session is active,
- optimize thermal behavior,
- learn from every prompt,
- maintain a global hardware capability knowledge base,
- search a huge parameter space automatically.

## 14.3 Recommendation output

```text
RuntimeProfile
├── id
├── hardware_fingerprint
├── model_id
├── backend
├── gpu_layers/offload
├── threads
├── batch_size
├── context_size
├── flash_attention
├── kv_type/placement
├── memory_estimate
├── source              estimated | calibrated | user
├── confidence
└── runtime_revision
```

The user sees:

```text
Performance
● Recommended
○ Low memory
○ Maximum performance
○ Custom
```

Advanced values are available behind an expander.

---

# 15. Persistence

SQLite is the canonical application database.

Enable WAL mode and schema migrations.

## 15.1 Core tables

```text
sessions
messages
runs
operations
workspaces
models
runtime_profiles
context_manifests
context_items
summaries
attachments
tool_calls
permission_decisions
artifacts
generation_metrics
settings
schema_migrations
```

## 15.2 Canonical vs derived data

Canonical:

- sessions,
- messages,
- workspace definitions,
- user permission rules,
- model installation metadata.

Derived/rebuildable:

- context manifests,
- summaries,
- project indexes,
- prompt/KV caches,
- search indexes,
- telemetry aggregates.

Caches must never become the only copy of user content.

## 15.3 Message immutability

Once committed, message content should not be silently mutated.

Editing a prior user message creates a branch/fork relationship or an explicit revision.

This prevents invisible history corruption and makes run reconstruction possible.

---

# 16. Event Protocol

The frontend derives all long-running state from structured events.

It must never parse human-readable logs to infer state.

## 16.1 Envelope

```json
{
  "event_id": "evt_...",
  "seq": 42,
  "type": "tool.completed",
  "timestamp": "...",
  "session_id": "ses_...",
  "run_id": "run_...",
  "operation_id": "op_...",
  "payload": {}
}
```

`seq` is monotonic within a run or stream scope so the client can detect gaps/reordering.

## 16.2 Core events

```text
session.updated
run.created
run.state_changed
run.completed
run.failed
run.cancelled

context.build_started
context.built
context.pressure

model.load_started
model.load_progress
model.loaded
model.unloaded
model.failed

inference.started
inference.delta
inference.metrics
inference.completed
inference.failed

tool.requested
tool.permission_required
tool.started
tool.output
tool.completed
tool.failed

permission.resolved
artifact.created
system.resource_sample
operation.progress
operation.cancelled
error
```

## 16.3 Event persistence

Not every token delta needs permanent storage.

Persist important lifecycle transitions and final records; keep high-frequency token/resource events ephemeral or sampled.

---

# 17. Core API / IPC Contract

The domain API is versioned independently of the UI.

Conceptual commands:

```text
sessions.list
sessions.create
sessions.get
sessions.rename
sessions.archive
sessions.delete
sessions.send_message
sessions.cancel_run

workspaces.add
workspaces.remove
workspaces.get_policy
workspaces.update_policy

models.list
models.import
models.download
models.load
models.unload
models.delete
models.probe

permissions.resolve

settings.get
settings.update

diagnostics.snapshot
diagnostics.export_bundle
```

The UI subscribes to structured events after issuing commands.

An optional `/api/v1` adapter may map these same domain commands to HTTP later; business logic must not live in the adapter.

---

# 18. Attachment Pipeline

v1 supports a deliberately small attachment set:

```text
text
markdown
source-code files
json
yaml
toml
csv (size-limited preview)
images only when a verified vision model is loaded (optional v1.1)
```

Complex office formats and large PDFs are deferred until the parsing/indexing pipeline is production-quality.

## 18.1 Pipeline

```text
Attachment
   ↓
Type detection
   ↓
Safety/size check
   ↓
Parser
   ↓
Normalized content record
   ↓
Context selection
   ↓
ContextManifest item
```

The model does not automatically receive the entire raw file.

Large content is chunked and selected.

---

# 19. Workspace and Project Index

Opening a project creates a Workspace record and a lightweight Project Index.

## 19.1 Initial index

Store:

- directory tree,
- file paths,
- file type/language,
- size/mtime,
- optional content hash,
- symbols where cheap and reliable,
- imports/includes where cheap and reliable.

Do not create embeddings by default in v1.

## 19.2 Incremental updates

Watch filesystem changes and update only affected index entries.

Do not continuously re-scan the entire repository.

## 19.3 Ignore policy

Honor sensible exclusions:

- `.git`,
- build output,
- dependency/vendor directories,
- binary assets,
- user-defined ignore rules,
- optionally `.gitignore`.

The index is local and rebuildable.

---

# 20. User Experience Architecture

## 20.1 Primary navigation

Recommended navigation:

```text
New Chat
Chats
Projects
Models
Settings
```

Diagnostics/resource information is accessible from a compact status area and Settings > Diagnostics.

Artifacts appear inline in the session and in the relevant project/session history.

## 20.2 Why this is better than Chat / Code / Resources / Artifacts as equal modes

- “Projects” is a user concept; “Code mode” is an implementation concept.
- A single session runtime avoids duplicated UI/backend state.
- Resource monitoring supports the work but is not the work.
- Artifacts belong to the task that produced them.
- The navigation remains useful even when future project types expand beyond coding.

## 20.3 Session header

General chat:

```text
Architecture discussion        Qwen 14B        Context 42%
```

Project session:

```text
RageV / RT Ghosting            Qwen 14B        Context 68%
D:\Development\RageV
```

## 20.4 Composer

Default:

```text
[ + Attach ]  Ask anything…                                  [Send]
```

Project sessions may additionally expose compact controls such as:

```text
[Plan] [Permissions]
```

“Reasoning” should not be a universal decorative toggle unless the loaded model/runtime actually exposes a meaningful capability. If shown, the UI must describe what it changes.

Web Search is deferred from v1 core.

## 20.5 Agent activity

Show concise observable work:

```text
Searching project…
Read 4 files
Proposed 2-file patch
Waiting for permission to run: cmake --build build
Build failed
Inspecting compiler error…
Build passed
```

Do not expose private reasoning traces.

## 20.6 Diff review

File mutations should produce an inspectable diff before or immediately after execution according to policy.

For risky edits, the permission dialog should allow the user to review the diff before approval.

---

# 21. Resource and Performance UX

Resource telemetry is important for a local inference product but should remain subordinate to the task.

## 21.1 Status area

Show compactly:

```text
Model: Qwen 14B Q4_K_M
GPU 72% · VRAM 9.8/12 GB · RAM 21/64 GB · 38 tok/s
```

Clicking opens diagnostics.

## 21.2 Per-generation telemetry

Measured generation speed can appear subtly after generation:

```text
38.4 tok/s · TTFT 0.42 s
```

Detailed metrics are behind an inspector.

Do not mix prompt processing speed and generation speed under one ambiguous label.

## 21.3 No fake precision

Metrics must be labeled:

```text
Measured
Estimated
Unavailable
```

Hardware-dependent performance is never presented as a guaranteed product SLO.

---

# 22. Cancellation

Cancellation is a protocol, not a button-only UI feature.

Every long-running operation has a cancellation token or equivalent control path.

## 22.1 Generation cancellation

```text
UI Stop
→ Core marks cancellation requested
→ Core signals inference worker
→ worker stops decoding
→ worker reports final partial metrics
→ Core persists partial assistant message with stopped state
→ Run becomes CANCELLED or returns to tool/agent control as appropriate
```

## 22.2 Process cancellation

Command execution must terminate the process tree, not merely the parent process.

Platform-specific process management is hidden behind an interface.

## 22.3 Non-cancellable operations

If an operation cannot safely be interrupted, the UI must say so rather than pretending cancellation is immediate.

---

# 23. Crash Recovery

## 23.1 Transaction boundaries

Persist before beginning consequential external actions where appropriate.

For example:

```text
record tool call as approved
→ commit
→ execute mutation
→ record outcome
```

## 23.2 Startup recovery

On startup, detect records left in transient states:

```text
GENERATING
RUNNING_TOOL
WAITING_FOR_MODEL
MODEL_LOADING
```

Mark them `INTERRUPTED` unless the subsystem can prove the operation is still active.

## 23.3 Destructive operations

Never automatically retry a destructive tool call after a crash.

The user or agent must re-evaluate current state first.

## 23.4 File writes

Use atomic replace where possible:

```text
write temp
→ fsync/close as appropriate
→ validate
→ rename/replace
```

Backups or VCS diffs should be available for important project edits.

---

# 24. Error Model

Every user-facing error should answer:

1. What happened?
2. What was affected?
3. What can the user do next?

Example:

```text
Model could not be loaded.

The selected configuration is estimated to require 13.6 GB of VRAM,
but 10.9 GB is currently available.

[Use Recommended Settings] [Try CPU/GPU Hybrid] [Choose Model]

Technical details ▸
```

Errors have stable machine codes in addition to user text.

```text
MODEL_OOM_ESTIMATE
MODEL_LOAD_FAILED
WORKSPACE_PATH_DENIED
TOOL_SCHEMA_INVALID
TOOL_PERMISSION_DENIED
PROCESS_TIMEOUT
INFERENCE_WORKER_CRASHED
CONTEXT_BUDGET_EXCEEDED
DATABASE_MIGRATION_FAILED
```

---

# 25. Logging and Diagnostics

Logs are structured internally and rendered human-readably.

Separate logical channels:

```text
app
model
inference
run
tool
security
storage
```

Sensitive content is redacted by default.

## 25.1 Diagnostic bundle

Users should be able to export a diagnostic bundle containing:

- app version,
- OS/hardware summary,
- model metadata without model file contents,
- runtime configuration,
- recent errors,
- selected sanitized logs,
- database schema version.

The export UI must clearly state what will be included.

## 25.2 Doctor command

A developer/power-user `/doctor` command may test:

- database,
- inference worker launch,
- backend acceleration,
- model path access,
- workspace path handling,
- tool registration,
- command executor,
- resource telemetry.

---

# 26. Performance Requirements

Product performance targets should cover software responsiveness rather than promise hardware-dependent LLM speed.

## 26.1 UI

- Normal UI actions should not wait on inference.
- Long lists must be virtualized.
- Conversation history should be incrementally loaded.
- Streaming updates should be batched enough to avoid excessive re-rendering.
- Resource samples should be throttled.

## 26.2 Core

- Database operations must not block the UI thread.
- Model loading runs outside the UI event loop.
- File scanning is incremental/cancellable.
- Tool stdout is streamed with backpressure and size caps.
- Token deltas use bounded queues.

## 26.3 Application-side latency objectives

Excluding model/hardware computation:

- Send action should receive an acknowledged Run ID immediately.
- Cancel requests should be acknowledged immediately by the core.
- Switching sessions should not wait for model load or context reconstruction.
- Opening the application should not require loading the default model before the shell becomes usable.

---

# 27. Packaging and Update Model

## 27.1 Package

Recommended Windows-first distribution:

```text
LocalCompanion/
├── desktop executable
├── web assets
├── inference worker
├── llama.cpp runtime libraries/backends
├── migrations
└── signed application resources
```

Models and user data live outside the installation directory.

## 27.2 Data directories

Keep separate:

```text
Application binaries
Application data / SQLite
Models
Caches
Logs
Workspaces (user-owned, external)
Downloads
```

## 27.3 Updates

Application updates and model downloads have separate lifecycles.

Application updates must be signed and must not silently delete user models or conversation data.

Database migrations must be transactional and versioned.

---

# 28. Cross-Platform Boundaries

Windows is the initial platform, but platform-dependent behavior lives behind interfaces:

```text
IProcessManager
IHardwareProbe
IResourceSampler
IPathPolicy
IFileWatcher
IAppShell
```

The domain model, persistence model, tool schemas, event protocol, context engine, and agent runtime must remain platform-neutral.

Do not design the core around Windows registry, drive letters, Job Objects, or PowerShell semantics even if the Windows implementation uses them.

---

# 29. Recommended Repository Structure

```text
local-companion/
│
├── apps/
│   └── desktop/
│       ├── src-ui/                 React / TypeScript
│       └── src-tauri/              desktop bootstrap only
│
├── crates/
│   ├── core/                       application services
│   ├── domain/                     Session/Run/Tool/Workspace models
│   ├── protocol/                   commands + event schemas
│   ├── storage/                    SQLite + migrations
│   ├── inference/                  engine abstraction + worker client
│   ├── model-manager/
│   ├── runtime-advisor/
│   ├── context-engine/
│   ├── agent-runtime/
│   ├── tool-registry/
│   ├── tool-filesystem/
│   ├── tool-process/
│   ├── permissions/
│   ├── workspace/
│   ├── project-index/
│   ├── telemetry/
│   └── platform/
│
├── native/
│   └── llama-worker/               C++ or thin launcher around llama.cpp
│
├── tests/
│   ├── fixtures/
│   ├── integration/
│   ├── e2e/
│   └── security/
│
├── docs/
│   ├── architecture/
│   ├── adr/
│   ├── protocol/
│   └── threat-model/
│
└── scripts/
```

If embedding `llama.cpp` through Rust FFI proves robust, the inference worker may still be Rust with a native binding. The process boundary is more important than the worker implementation language.

---

# 30. Testing Architecture

## 30.1 Fake inference engine

Build a deterministic fake engine before relying on real model inference in automated tests.

It should simulate:

- token streaming,
- tool-call responses,
- delays,
- cancellation,
- worker crash,
- malformed output,
- context limit errors.

This makes the application runtime testable without multi-GB model files.

## 30.2 Unit tests

Required:

- path normalization and traversal rejection,
- permission policy,
- tool schema validation,
- context budgeting,
- session/run state transitions,
- event sequencing,
- model metadata parsing,
- runtime profile estimates,
- patch validation,
- output truncation,
- database migrations.

## 30.3 Contract tests

The inference worker protocol and tool protocol require compatibility tests.

Unknown optional fields should be forward-compatible where appropriate; incompatible protocol versions fail clearly.

## 30.4 Integration tests

Test:

```text
real SQLite
+ real core
+ fake inference worker
+ fake/temporary workspace
```

Then separately test real `llama.cpp` on supported CI/dev hardware.

## 30.5 End-to-end product tests

Minimum scenarios:

### Chat

```text
launch
→ import model
→ load model
→ create session
→ send message
→ stream response
→ restart app
→ continue session
```

### Project agent

```text
open fixture project
→ ask to fix known bug
→ agent searches files
→ proposes patch
→ permission evaluated
→ patch applied
→ build/test command approval
→ tests pass
→ final summary
```

### Security

```text
workspace root = fixture/project
→ model requests ../../secret.txt
→ tool rejects path
→ run continues safely
```

### Crash recovery

```text
start generation
→ kill inference worker
→ app remains open
→ run becomes interrupted
→ reload model
→ session continues
```

## 30.6 Fault injection

Deliberately test:

- worker crash,
- disk full,
- database locked/corrupt simulation,
- file changed between read and patch,
- command timeout,
- cancellation race,
- tool process crash,
- model OOM,
- malformed model output.

A product-grade local agent must be designed around these failures.

---

# 31. v1 Production Scope

The first release is complete only if the following vertical slice is reliable.

## 31.1 Installation and onboarding

- Windows installer.
- First-run hardware detection.
- Import local GGUF.
- Optional curated model download.
- Safe recommended runtime configuration.
- Load/unload model with progress and error handling.

## 31.2 Chat

- Create/rename/archive/delete sessions.
- Streaming Markdown/code responses.
- Session persistence across restart.
- Stop generation.
- Edit/retry through explicit branching/revision behavior.
- Context usage indicator.
- Automatic compaction.
- Real generation metrics.

## 31.3 Projects

- Add/remove workspace.
- Multiple sessions per workspace.
- Project tree/search.
- Read relevant source files.
- Patch-based edits.
- Diff presentation.
- Build/test/process execution with permissions.
- Agent loop with limits.
- Clear tool activity timeline.

## 31.4 Safety

- Workspace path boundary.
- Permission policy.
- Path canonicalization.
- No unrestricted shell by default.
- No network tools in core v1.
- Cancellation.
- Audit trail for tool actions.
- Secret-sensitive path warnings/blocks.

## 31.5 Reliability

- SQLite migrations.
- Inference worker isolation.
- Crash recovery.
- Interrupted-run detection.
- Atomic file edits.
- Error codes and actionable user errors.
- Diagnostic bundle.

## 31.6 Performance/diagnostics

- CPU/RAM/GPU/VRAM monitoring where supported.
- Current model/runtime profile.
- Generation tok/s and TTFT.
- Basic model memory estimate.
- No advanced per-session hardware attribution requirement.

---

# 32. Explicitly Not in v1

The following are not allowed to creep into v1 unless a v1 requirement depends on them:

```text
Web research
Browser automation
Computer-use automation
PDF knowledge base
Office artifact suite
Persistent semantic memory
Plugin SDK
Plugin marketplace
Multi-model routing
Model swarms
Background scheduled tasks
Concurrent inference slots
Remote/LAN API
Cloud sync
Voice
Image generation
Fine-tuning
NPU optimizer
Adaptive thermal optimizer
Advanced resource graphs
Cross-session context sharing UI
```

This list is a scope-control mechanism.

---

# 33. Phase 1.1 / 1.5

After v1 stability:

- Vision model/projector support.
- PDF/document parsing.
- Git first-class tools.
- Optional web search with explicit network indicator.
- Better artifact handling.
- Model catalog/recommendations.
- Lightweight workspace retrieval improvements.
- More model/runtime backends if justified.

---

# 34. Phase 2

Only after the core session/run/tool/security architecture is proven:

- Local knowledge/RAG.
- Persistent semantic workspace memory.
- Browser automation.
- General PC automation.
- Plugin SDK.
- Advanced sandboxing.
- Multiple concurrent runs with resource scheduler.
- More sophisticated runtime optimization.
- Optional NPU support where the inference stack actually benefits.

---

# 35. Phase 3

Potential future capabilities:

- Multi-model routing.
- Specialist model pipelines.
- LAN/remote inference.
- Encrypted sync.
- Scheduled/background agents.
- Community extensions.
- Speech.
- Image generation.
- Fine-tuning workflows.

These features must reuse the same Session, Run, Tool, Permission, Operation, and Event primitives rather than invent new execution systems.

---

# 36. Implementation Sequence

The implementation should be built as vertical slices, not subsystem islands.

## Stage 0 — Architecture contracts

Before UI polish:

- define Session/Message/Run/Operation schemas,
- define event envelope,
- define tool contract,
- define permission contract,
- define inference worker protocol,
- create database migration framework,
- write ADRs for process boundaries.

## Stage 1 — Fake end-to-end chat

```text
Desktop UI
→ Rust core
→ fake inference worker
→ token events
→ persisted session
```

Definition of done: restart and continue the fake conversation.

## Stage 2 — Real inference worker

- launch `llama.cpp` worker,
- probe model,
- load model,
- stream output,
- cancel,
- surface worker crash.

Definition of done: real GGUF chat survives app restart and worker restart.

## Stage 3 — Model onboarding

- hardware probe,
- import model,
- runtime recommendation,
- load errors,
- model switching.

## Stage 4 — Workspace read tools

- add workspace,
- canonical path security,
- list/read/search,
- project index,
- tool activity UI.

Definition of done: model can answer project questions without write access.

## Stage 5 — Run/permission/write loop

- patch tool,
- permission dialogs,
- diff review,
- process executor,
- build/test loop,
- cancellation.

Definition of done: fixture bug can be safely fixed and verified.

## Stage 6 — Context engine

- context manifests,
- token accounting,
- compaction,
- relevant project excerpt selection,
- context inspector.

## Stage 7 — Reliability

- crash recovery,
- fault injection,
- interrupted-run handling,
- atomic writes,
- diagnostics.

## Stage 8 — Product polish

- onboarding,
- model catalog/download,
- empty/error/loading states,
- accessibility,
- keyboard UX,
- update system,
- installer.

No Phase 2 feature begins until the v1 Definition of Done passes.

---

# 37. Definition of Done — v1

v1 is product-ready only when all of the following are true.

## Product

- A new user can install the application and reach a working local chat without editing configuration files.
- A user can import or obtain a compatible model and understand whether it fits their machine.
- A user can create many sessions and switch among them without losing state.
- A user can attach a project and use the same assistant runtime to inspect and safely modify it.

## Reliability

- Killing the inference worker does not kill the application.
- Restarting the application preserves completed conversation history.
- Interrupted runs are clearly marked.
- Cancellation works for generation and process execution.
- A failed tool does not corrupt the session.
- A stale patch does not overwrite changed code silently.

## Security

- The model cannot bypass workspace boundaries.
- Path traversal and symlink/junction escapes are tested.
- Dangerous actions require policy evaluation.
- Permission approval is bound to the action shown.
- The application does not run as Administrator/root for normal operation.
- Network access is not silently introduced.
- Secrets are not automatically injected into context.

## Agent behavior

- The agent can inspect a deterministic fixture project.
- It can identify a known issue.
- It can propose and apply a patch under policy.
- It can request approval for a build/test command.
- It can interpret failure and retry within limits.
- It reports what changed and what verification succeeded or failed.

## UX

- The default interface is not a control dashboard.
- Model/resource details are available without dominating the conversation.
- Every long-running operation has visible state.
- Permission dialogs show concrete consequences.
- Errors are actionable.
- Advanced inference settings are optional.

## Engineering

- Domain, protocol, inference, tools, and storage have contract tests.
- Core behavior can be tested using a fake inference engine.
- Database migrations are automated and tested.
- Structured events drive UI state.
- Critical security decisions have ADR/threat-model documentation.

---

# 38. Architecture Invariants

These rules must survive future feature growth.

1. **The LLM never directly performs OS actions.**
2. **The UI is not a security boundary.**
3. **The inference worker has no tool authority.**
4. **Persistent conversation state is independent of KV/prompt cache state.**
5. **A Session is durable; a Run is executable; an Operation is cancellable.**
6. **Every consequential tool action passes schema validation and policy evaluation.**
7. **Compaction never destroys canonical history.**
8. **Model capabilities are detected/declared explicitly and never faked.**
9. **Caches are disposable.**
10. **Network access is an explicit capability.**
11. **Dangerous actions are never silently retried after uncertain failure.**
12. **Future features must compose through existing Run/Tool/Permission/Event primitives.**

---

# 39. Final Product Architecture

```text
                           ┌─────────────────────────┐
                           │      Desktop UI          │
                           │ Chats / Projects / Models│
                           └────────────┬────────────┘
                                        │
                                commands + events
                                        │
                           ┌────────────▼────────────┐
                           │       Core Host          │
                           │                          │
                           │ Session Manager          │
                           │ Run Manager              │
                           │ Context Engine           │
                           │ Model Manager            │
                           │ Runtime Advisor          │
                           │ Scheduler                │
                           │ Tool Registry            │
                           │ Permission Engine        │
                           │ Workspace Manager        │
                           │ Persistence              │
                           │ Telemetry                 │
                           └───────┬──────────┬───────┘
                                   │          │
                          inference│          │capability-scoped
                                   │          │execution
                         ┌─────────▼───┐  ┌───▼────────────┐
                         │ llama.cpp   │  │ Tool Executor  │
                         │ Worker      │  │ Files/Process  │
                         └─────────────┘  └────────────────┘
```

Around this runtime sit durable records:

```text
Session
  ├── Messages
  ├── Runs
  │    ├── Context Manifests
  │    ├── Model Generations
  │    ├── Tool Calls
  │    ├── Permission Decisions
  │    └── Metrics
  └── Workspace? 
```

This is the core shape the product should not outgrow.

---

# 40. Final Product Statement

> **Local LLM PC Companion is a local-first desktop AI workspace that turns a user-selected local model into a persistent conversational and coding assistant. The model supplies reasoning; the application supplies context, deterministic tools, permissions, resource management, persistence, recovery, and a safe interface to the user's projects.**

The first release succeeds if it is dependable enough that a user can trust it with a real codebase and simple enough that they do not need to understand local-inference internals to use it.

The objective is not maximum feature count.

The objective is:

> **one coherent local AI runtime, one trustworthy execution model, and one polished product experience.**
