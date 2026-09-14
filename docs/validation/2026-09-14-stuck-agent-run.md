# The stuck "energy-drink" agent run — 14 September 2026

## What the user saw

A Code session in the "Test Python" workspace (the whole `Code` folder) with
Qwen2.5-Coder 14B, Auto mode, Reasoning off. The prompt: create a project
called "energy-drink" with a modern website for an imaginary energy drink, a
hype line, and interactions and animations. The run circled for 15 steps and
ended with "I produced the same result three times without addressing what
the check found". A window also appeared over the app mid-run.

## What the journal showed

| Evidence | Meaning |
|---|---|
| `context_limit: 4096` on every step; the saved preference was 32,768 | The memory fit cut the window to 4K |
| `turns: 4` from step 3 on, `pruned_turns` +2 per step | The model only ever saw the rules, the task and its latest exchange |
| `mkdir energy-drink` followed by three identical "A command may have changed the project. Run a relevant test, build or validation…" | The completion rule could not be met: a static site has no test or build |
| `create_document energy-drink.zip`, then `create_document energy-drink.txt`, then `open_path energy-drink.txt` → "opened" | The document tool was misused, its result text read as "call open_path", and open_path launched Explorer on a path that did not exist |
| Two replies rejected as unreadable, cause not recorded | The journal kept only the first 900 characters and no parser reason |

## Causes and fixes

1. **Unsatisfiable completion rule.** Any shell command other than a
   recognised test or build set a flag that only a passing test or build could
   clear. Now a command marks its working folder as unobserved, and a listing
   of that folder (or `dir`/`ls`/`git status` there), or a passing test or
   build from there or above, clears it. Writes the host reads back byte for
   byte count as confirmed; edits still need a look.
2. **No room for history on small windows.** Pruning reserved the whole
   output cap (8K with thinking off) against a 4K window, so the history
   budget was zero. The reserve is now a quarter of the window.
3. **Automatic compaction** (owner request): at 90% of the usable context (the
   window minus room for the reply), between steps only, the run pauses in a
   `COMPACTING` state, the model writes a progress note from the transcript it
   already has cached, and the host adds its exact record of completed actions
   and what completion still requires. Compaction is applied only when it
   frees at least a tenth of the room and repeats only after a sixth of the
   room of new growth. Chat compacts saved history before a reply the same
   way, in window-sized chunks. Setting: Settings > Assistant > Automatic
   compaction and Compact at (default 90%).
4. **Window sizing.** The fit halved from the request (32K, 16K, 8K, 4K) and
   kept a margin of the compute reserve plus 5% of the card. Measured on this
   12 GB card with 1.7 GB held by other applications: the 14B model ran fully
   on the GPU at 11,264 tokens with an 8-bit cache (72 tok/s) and spilled at
   12,288 (49 tok/s). The fit now searches in 1,024-token steps with a margin
   of the compute reserve plus max(total/48, 256 MiB), and a model whose
   weights fit but whose smallest cache misses by less than that margin stays
   on the GPU at 4K instead of a hybrid plan with a large cache. Result under
   the same load: 8,192 tokens on the GPU instead of 4,096.
5. **Tools that act outside the project.** `create_document` and `open_path`
   are offered to an agent only when the task asks for a document or for
   something to be opened; shell commands that open windows (`start`,
   `explorer`, `xdg-open`, `Start-Process`…) are refused on the same rule.
   `open_path` refuses paths that do not exist. The document result no longer
   mentions "Open/Reveal actions".
6. **Diagnostics and repairs.** An unreadable reply is journaled with the JSON
   parser's reason and its ending. That revealed the model writing
   `\'` and `` \` `` inside JSON strings for JavaScript; those now repair to
   plain quotes and backticks, other stray backslashes are kept literally.
   The retry asks for shorter content only after a real cut-off (it had made
   the model replace an animation script with an alert).
7. **Crash found while testing.** Positions from the repaired JSON copy were
   used to slice the original reply; a multi-line write without escaped
   newlines panicked the agent task. Positions are now measured on the
   original text.
8. **Completion check.** A tool-free refusal ("I am unable to create
   projects") is sent back once with the list of tools the agent has. The
   check's evidence is sized to the window (it overflowed 4K), it no longer
   sees its own earlier verdicts (it repeated one after the page already had
   the missing hype line), and it judges the latest version of each file. The
   repeat stop applies only when nothing changed between checks; a checker
   that repeats itself after changes ends the run as completed with its
   finding quoted.

## Reruns of the exact prompt

The agent-created `energy-drink` folder was moved to the Recycle Bin before
each rerun (the originals and both intermediate outputs remain recoverable
there). All runs: Qwen2.5-Coder 14B, Auto mode, "Test Python" workspace.

| Run | Build | Window | Outcome |
|---|---|---|---|
| Scratch, 4K forced | after fixes 1–3 | 4,096 | Completed in 29 s, one compaction, basic page |
| Scratch | after the crash fix | 4,096 | Hit the 30-step limit: a 3 KB page does not fit a 4K window next to the instructions |
| Real, rerun 1 | before fixes 6 and 8 | 8,192 | Completed in 23 s; one reply rejected for `\'`, the retry shrank the script to an alert |
| Real, rerun 2 | before fix 8 | 8,192 | Good page (hype line, fade and slide animations, hover and click interactions) but ended FAILED by the anchored check |
| Scratch ×2 | final | 8,192 | Completed in 42 s and 49 s; one check sent the model back for a missing hype line, then passed |
| Real, final | final | 8,192 | Completed in 46 s: header, fading description, bouncing button with hover scale; the script is a stub |

Output quality varies from run to run at this model size and window; the host
now completes the work instead of circling or failing it wrongly. The two
intermediate real sessions are titled "energy-drink rerun 1/2 (before final
fixes)".
