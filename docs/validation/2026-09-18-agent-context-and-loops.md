# Why long coding runs got stuck, and what each fix changed (2026-09-18)

Measured on this PC with the app's own HTTP API, one model at a time, an isolated data directory each
time, and the owner's own prompt. The diagnosis behind these numbers is night decision 61; what was built
is decisions 62 and 63.

## 1. The token estimate against what the runtime reported

The estimate decides when context is released, pruned and compacted. It is learned from the ratio between
the characters a request carries and the tokens the runtime reports for it. Counting only the message text —
which is what the app did once models started calling tools through the runtime, where a file's text travels
in the call's `arguments` — produced this:

| run | characters counted before | real ratio of the whole request | estimate vs real prompt |
|---|---|---|---|
| owner's E4B run, step 22 | 0.99 characters per token | 3.47 | 94,587 estimated vs 34,219 real (2.76x) |
| owner's E4B run, step 10 | 1.24 | 3.41 | 14,942 vs 9,649 (1.55x) |
| owner's 27B run, step 30 | 2.27 | 2.62 | 16,002 vs 14,257 (1.12x) |
| reproduction, step 13 | — | — | 7,555 vs 3,123 (2.42x) |

After counting the call arguments and the tool definitions as well, on the same task and model:

| step | estimated | real prompt | ratio |
|---|---|---|---|
| 17 | 13,085 | 14,686 | 0.89x |
| 20 | 18,716 | 19,386 | 0.97x |
| 22 (after a compaction) | 8,932 | 7,860 | 1.14x |
| 24 | 12,979 | 12,630 | 1.03x |
| 26B verification, last step | 7,302 | 7,099 | 1.03x |

## 2. Compaction

Before: `compactions: 0` in all four of the owner's runs and in the reproduction, because the release of old
tool results ran first and always brought usage back under the threshold. The releases themselves fired at
steps 23, 26, 30, 34 and 36 of the run that then looped.

After, on the same prompt and model: 2 compactions in the E4B run (one summarized 39 turns, 22,694 to 7,627
tokens, with the model's note plus the host's record), 3 in the 26B's resume run, 6 in the small-window
measurement below.

## 3. The loop

The owner's second 27B run, which the app described as resuming: 43 tool calls in 6 minutes, 34 reads, 6
listings, 3 commands, **no file changed**, 20 of the 43 identical to an earlier call, 16 targets read two or
three times. Nothing warned and nothing stopped: the repeat guard only compared a result with the one
immediately before it. Step time grew from a median 1.8 s to 18.4 s as the cached prefix was lost (cached
tokens fell to 1,257 of about 15,000).

After: `LoopWatch` counts steps with nothing changed and how many repeat an earlier inspection (8 with 3
repeats warns with the list of what has already been read; 18 with 6, or 30 with nothing changed at all,
stops), and writes of one file with nothing run against it (4 warns; 3 writes of the *same text* stop —
different edits never do, because a long file is written in parts on purpose).

It caught a real run while these measurements were being taken. With no step ceiling in force, a run that
had already written its twelve modules and its report carried on reading them back; at step 72 it ended
itself: "18 steps passed without a single change to the project, and 9 of them repeated something already
inspected. Everything completed so far was kept." The work was complete and kept; only the circling stopped.

## 4. Commands

A command still running at its timeout used to return nothing at all, because the shell was killed while the
program it started held the pipe open. Measured with the same shape as the runner: 0 bytes captured, and
`node.exe` still running after the kill. In the owner's runs every `npm run dev` came back "exit 1, stdout
(empty)", and a model wrote in its journal that "the development server is running successfully".

After, from the reproduction's own record:

```
Command (background):
npm run dev -- --port 3000
Id 1, still running, 10 s so far
--- stdout ---
  VITE v8.3.0  ready in 142 ms
  ➜  Local:   http://localhost:3000/
```

## 5. Seeing the result

`preview_page` on that server, in the same run:

```
Page: http://localhost:3000
Server answered: HTTP/1.1 200 OK
Title: neon-...
Rendered HTML: 689 bytes
--- what the page logged ---
log: [vite] connecting... / log: [vite] connected.
--- what the page shows ---
(only the title: the application itself rendered nothing)
```

Which was true: that run's `App.tsx` did not compile.

## 6. Carrying work between runs

The 26B, asked to "resume from the last checkpoint" — the same words that had sent the 27B into 43 steps of
re-reading — opened with:

```
Continuing work already done in this session: 59 recorded action(s) and the project's own notes.
```

and spent its 58 steps editing and building rather than exploring. Its build went from broken syntax to six
tidy-up errors (unused imports, one missing prop).

## 7. Notes the model keeps

A task that reads `config/service.json`, writes twelve modules (enough to compact six times in an 8K window),
and must state the configuration's values at the end.

| run | steps | compactions | reads of the config | notes kept | modules | values right in the report | time |
|---|---|---|---|---|---|---|---|
| before 1 | 16 | 6 | 2 | 0 | 12/12 | 4/4 | 52.5 s |
| before 2 | 18 | 7 | 2 | 0 | 12/12 | 4/4 | 84.9 s |
| after 1 | 20 | 10 | 2 | 0 | 12/12 | 4/4 | 64.6 s |
| after 2 | 22 | 8 | 3 | 0 | 12/12 | 4/4 | 98.9 s |
| after 3 | 19 | 5 | 2 | 0 | 12/12 | 4/4 | 58.5 s |
| valuenote 1 | 18 | 6 | 2 | 1 | 12/12 | 4/4 | 42.4 s |
| valuenote 2 | 20 | 7 | 1 | 0 | 12/12 | 4/4 | 74.7 s |
| valuenote 3 | 72 | 37 | 3 | 1 | 12/12 | 4/4 | 258.2 s |

`before` is the build without the tool; `after` has the `remember` tool; `valuenote` adds two host-side
changes — the compaction instruction asks for the values themselves rather than where they were read, and
the block left behind reminds the model that `remember` exists at the moment something is about to be lost.

What this says, plainly:

- The tool on its own changed nothing on this model: three runs, no uses, the same two reads of the
  configuration and the same 4/4 values.
- What the model wrote in its note is what mattered. Before: "I have read `config/service.json`
  previously" — a pointer, so it read the file again at the end. After the instruction changed: "The values
  gathered are: Service Name "observatory-relay", Port 8412, Region "eu-west-3", and Retry Count 7", and
  that run wrote its report without going back to the file (1 read instead of 2).
- With the reminder in the block, the model used `remember` in two runs of three, keeping exactly the kind
  of thing it is for:

  - `Service Name: observatory-relay, Port: 8412, Region: eu-west-3, Retry Count: 7`

- Every run produced all twelve modules and all four values, so nothing here was about correctness on this
  task: it was about how much work the model has to redo after its context is summarized away.

## The verification prompt

The owner's "NEON OBSERVATORY" prompt, run start to finish on the E4B (32K) and on the 26B (hybrid, 32K):

| | E4B before the fixes | E4B after | 26B after |
|---|---|---|---|
| steps | 30 (the ceiling then in force) | 45 | 60 + 58 (two runs) |
| what was built | no project: no index.html, no vite config, no entry point | a scaffolded Vite project, server running | 30 files under `src/components`, plus HANDOFF.md and MEMORY.md written by the model |
| ended by | the step ceiling | the repeat guard on a failing edit | the ceiling, then the repeat guard |
| compactions | 0 | 2 | 2 and 3 |

Neither model's app compiled at the end: the E4B's JSX and the 26B's type-only imports are the models' own
limits. The difference is that the host now says so, keeps what was done, and can carry on.
