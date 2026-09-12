# Agent recovery, full Auto, and stable elapsed time — 12 September 2026

## Incident and diagnosis

The reported calculator run had already ended in failure when the stop request
arrived. Its journal recorded 14 model attempts; the inspected later attempts
each consumed the previous fixed 1,024-token output allowance. Empty output and
unreadable-action corrections alternated, resetting separate retry counters.
This was not useful task progress. Existing project files and history were kept.

The old completion adapter discarded `finish_reason` and reasoning-channel
presence, so the runner could not distinguish a cut-off answer from a completed
one. Completion-token usage alone does not establish how many tokens were
reasoning. The updated adapter retains only scalar diagnostics, never hidden
reasoning text.

## Implemented response policy

- Initial action allowance: up to 2,048 output tokens, constrained by estimated
  remaining context with 256 tokens of additional headroom.
- One recovery before new successful evidence: up to 4,096 output tokens with
  the model-template native-thinking override disabled for this run. This does
  not change ordinary Chat settings.
- At the user's suggestion, a nonempty length-truncated visible response can be
  continued once. The request carries its exact partial text and asks for the
  missing suffix. The app assembles and validates the full response before any
  action executes. Exact full-prefix replay is deduplicated; approximate overlap
  is not guessed. Partial and assembled text have size caps.
- Empty, unsupported or malformed output receives one fresh compact correction
  with a runtime-enforced JSON schema distinguishing a tool action from a final
  answer. This schema stays enabled after successful recovery. Suffix continuation
  requests are deliberately unconstrained, then the assembled result is validated.
  A failed continuation can also use that clean fallback; it cannot chain more
  continuations or renew the fallback allowance. If the fallback is invalid the
  run stops (at most three invalid-output requests: initial, continuation, clean
  fallback). Partial or still-cut-off actions never execute.
- A shared four-attempt no-progress ceiling additionally covers failed requests,
  unknown tools, denied actions, repeated reads and rejected completion claims.
  Only new successful tool evidence resets it. The whole-run iteration limit
  remains independent. The short completion-review request is also separately
  bounded.
- Context accounting includes the carried partial response and continuation
  instruction. It reports the latest individual request, not a sum of all prior
  contexts. Character estimates are not represented as exact tokenizer counts.

The agent currently obtains action responses non-streamingly. Continuation is
internal to response assembly, not a claim that incomplete tool payloads are
streamed as finished actions. Ordinary Chat continuation was not changed here.

## Auto permission behavior

At the user's explicit request, Auto now authorizes registered actions including
file modification, commands, deletion and Git operations without per-action
approval prompts. Ask behavior is unchanged. Both settings entry points persist
the preference before applying it and release eligible already-waiting actions.

The runner no longer holds a permission read lock while awaiting a decision.
After publishing a pending action, it rechecks Auto under the settings-update
lock, closing both possible orders of an Ask-to-Auto transition. This creates no
session grant that could survive a later return to Ask.

Auto does not override read-only interaction modes, file-tool path validation,
unknown-tool rejection or blocked-command checks. Search still requires the
task's Search toggle and respects an explicit Deny policy. Native shell commands
are not OS-sandboxed merely because they start in the selected project folder.

## Elapsed time

Agent runs expose a stable `started_at`, shared with the persisted assistant
placeholder. Chat uses the send-time timestamp. The indicator derives elapsed
wall time from that timestamp instead of its component mount time, including
permission waits and time spent viewing other tabs. Unknown legacy start times
remain unknown. Conversation identity, stale-poll guards and pending-start guards
prevent another session or replayed terminal event from resetting the clock.

## Automated verification

- Backend: **264 passed, zero failed, two intentionally ignored** (266 tests).
  The opt-in local-model action-format probe also passed when run explicitly.
- Frontend: **59 passed, zero failed**, production build successful.
- Coverage includes continuation assembly and context accounting; empty/malformed
  alternation; parseable-but-truncated tool rejection; duplicate-action rejection;
  progress budgets; Auto persistence and both pending-publication timing orders;
  Search denial; elapsed-time remount and cross-session isolation.

The running app was updated after idle checks and verified SQLite snapshots in
`data/backups/20260911T183630915415Z`, which also contains the preceding executable.
The previously loaded Gemma model and the existing Auto preference were restored.
No user conversation was created for the isolated follow-up evaluation.

## Live-model follow-up

The test uses Gemma 4 12B IT Q4_K_M at the existing 32,768-token runtime context,
the actual application API, and a new disposable folder. It asks the agent to
fix one arithmetic operator, delete only an explicitly disposable fixture file,
and execute a local Node assertion script. No packages or network access are
needed, and no user project is linked to the run.

The first test exposed a compatibility edge case: the initial response consumed
2,048 output tokens, but a 44-token continuation did not assemble into a valid
action. The run stopped after two iterations, with zero tools executed and no
files changed. Its start timestamp stayed constant. Terminal state was observed
approximately 67 seconds after start; events do not yet provide an exact end
timestamp. This was evidence of bounded failure, not successful task completion.

That result motivated the single clean-format fallback described above. A model
which cannot reliably produce an exact suffix gets one fresh, small-action
request with the invalid partial text omitted. It does not receive an unlimited
series of increased token allowances.

The second live test stopped after three rounds, with generated counts of
2,048, 60 and 22 tokens and no executed tools. The very short final failure
pointed to action-format compatibility rather than output exhaustion. A direct
visible-output-only probe using the app's prompt exposed repeated fence markers.
After replacing incomplete format descriptions with one complete JSON/fence
example, Gemma returned its native text envelope instead:
`<|tool_call>call:list_directory{args:{"path":"."}}<tool_call|>`.

The agent now normalizes this observed single-call envelope to the same ToolCall
as its Markdown format. Complete JSON-object arguments and the exact ending are
required; quoted examples, multiple calls, missing endings and trailing material
are rejected. Normal registry, interaction-mode, permission, filesystem and
executor checks still apply. The explicit native-format probe then passed in
0.91 seconds with a complete response. This does not claim support for every
model-specific protocol, or change the streamed Chat tool parser.

The next integration run executed four real tools without approval: listing the
fixture, reading the calculator, applying the operator patch, and deleting the
disposable file. Both mutations produced diffs. It then changed format again
before verification and stopped; reading/editing was not reported as a passing
test. An independent host execution of the unchanged assertion script passed,
but was not attributed to that model run.

The clean recovery now additionally uses the server's supported
`response_format` JSON-schema constraint. The schema requires `kind`, `name`,
`args` and `answer`; the host separately validates the tool/final distinction,
registry, tool arguments, permissions and result. The final-format instruction is
included before context accounting, not silently appended by the adapter. This
changes only agent fallback requests; ordinary Chat and the completion critic
remain unchanged. [llama.cpp server API documentation](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md#post-v1chatcompletions-openai-compatible-chat-completions-api).

The host now treats the explicit `kind` as authoritative. An explanatory note in
a tool envelope is not an invalid action or a final answer; inactive name/args
fields in a final envelope never execute. Missing or invalid action arguments
still fail normal executor validation. This avoids rejecting otherwise valid
schema output because of unnecessary cross-field emptiness requirements.

With that correction, the real model fixed the fresh fixture, deleted its
disposable file, and executed `node verify.cjs` with exit code 0 and stdout
`COMPANION_AUTO_CHECK_PASSED`. Five tools succeeded, with zero tool errors or
permission events. [Recorded activity](2026-09-12-recovery-before-verification-fix.json).
The script's SHA-256 stayed
`8DC0CC10C522B2DA7D74BF56476AC82E66CD79B07A266286BD4E9256A9161AC3`.

That run still exposed a host completion-labeling bug: the recognized Node test
names omitted `verify.cjs`, so a successful verification was treated as an unknown
command and repeated completion checks eventually stopped the run. Common exact
`verify`, `check` and `test` script names and `.spec` variants are now recognized;
nonzero exits, unrelated scripts and compound commands do not gain verification
credit. The test suite covers this distinction. Original failure records remain
unchanged; a new verification-only run checks the correction.

The direct tools API also executed the same fixture command with
`approved_once: false` and `grant_session: false`, returning `approved_via: auto`
and exit code 0. This is a separate host permission check, not attributed to the
model. Only explicitly disposable evaluation files were deleted; user projects
and conversation history were not removed.

### Final verification result

The follow-up run **COMPLETED** after five iterations. It read the unchanged
`verify.cjs`, executed `node verify.cjs`, observed exit code 0 and
`COMPANION_AUTO_CHECK_PASSED`, then returned a final answer. There were 25 events,
two successful tools, zero tool errors and **zero permission prompts**. It used
one format recovery and one completion-review correction before doing the work.
Its stable start time was `2026-09-11T19:05:27.490903800+00:00`; completion was
observed approximately 25.02 seconds later (polling upper bound, not an exact
event duration). [Final activity record](2026-09-12-recovery-final-verification.json).

The final request reported 1,482 prompt tokens against a 32,768-token window;
the corresponding estimate was 1,471, with 4,096 output tokens reserved. The
actual verification script was not modified. A separate host run also passed
the assertions. The app remains running with Gemma loaded and Auto selected;
no evaluation run remains active. These small local checks establish the tested
actions and recovery paths, not reliability across all models or large projects.
