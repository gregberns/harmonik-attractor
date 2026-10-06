# The Run Folder

What `pas run` writes to disk for each Run: every file, its format, who
writes it and when, and its version field. Tools that watch a Run (the
Monitor, `pas runs`, scripts, other agents) read these files. This page is
the contract between `pas` and those readers.

## What the run folder is for

The run folder is **observability** (design Q20). Another process reads it
to watch a Run, spot a hung agent and read results. The engine never reads
it to decide anything, with three exceptions:

- `checkpoint.json` is the resume state. A resumed Run continues from it.
- `control/stop` is an input. The engine polls for it between stages.
- `answers/` is an input. A waiting Human Gate polls for its answer file.

To set up a resume, `pas run` also reads its own bookkeeping back:
`run.json` for the worktree and branch, and `events.jsonl` for the next
attempt number and `seq`. It never routes on them.

The record of what each attempt did is its **attempt commit** on the branch
`pas/run/<run-id>`, not the run folder. Per-node results are also in the
journal (`StageCompleted`, `StageFailed`).

The run folder is **not for secrets**. A prompt file or a transcript holds
what the agent got and said, including anything the graph author put in a
prompt. Environment variable values are never written; their names are.
The argv in a prompt file is recorded as is, so a secret passed as an
argument (for example an MCP server token inside `--mcp-config` or
`--settings` JSON from `[codergen.claude]`) appears in it.

The worktree and the branch are **not** in the run folder. A Run works in
`<worktree-root>/<run-id>` (default `<project-root>/.pas/worktrees/<run-id>`)
on branch `pas/run/<run-id>`; `run.json` and `final.json` record both. See
[Worktree and branch](cli-reference.md#worktree-and-branch).

## Rules for readers

- Readers ignore event types they don't know, and fields they don't know,
  in every file. `pas`'s own journal reader keeps an unknown event type as
  a raw line and skips it.
- Check the version field (`v`, `schema_version`, or the first line of a
  prompt file) before reading the rest. `pas`'s journal reader refuses a
  line whose `v` is not 1.
- A file can be missing: a Run that crashed or was refused writes less.
  `console.log` exists only for Runs the Monitor started.
- Files ending in `.tmp` are a write in progress (written, then renamed).
  Ignore them.

## Layout

The logs folder (`<logs>`) is `.pas/logs/` under the directory `pas run`
was started in. Each Pipeline gets its own folder in it, `<stem>-<hash>`:
the `.dot` file's stem and the FNV-1a 32-bit hash of its canonical path, as
eight hex digits. With `--logs <dir>`, `<dir>` is the Pipeline folder.

```
<logs>/<stem>-<hash>/                      the Pipeline folder
  checkpoint.json                          resume state
  run.lock                                 Pipeline lock
  runs/<run-id>/                           the Run folder
    run.json                               Run metadata
    events.jsonl                           Run Journal
    console.log                            pas run output (Monitor starts only)
    final.json                             end-of-run report
    transcripts/<inv>.jsonl                agent stdout
    transcripts/<inv>.stderr.log           agent stderr
    transcripts/<inv>.prompt.txt           prompt, argv, env names
    answers/<question-id>.json             Human Gate answer
    answers/<question-id>.json.rejected    an unusable answer, moved aside
    control/stop                           stop request

<state>/runs.jsonl                         Run Index, one line per Run
```

`<run-id>` is a UUID v7, lowercase and hyphenated. `<inv>` is an Invocation
ID, the same form, one per Model Invocation (each agent call, so each retry
gets a new one). `<question-id>` is `q-<node>-<n>`, 1 to 128 characters of
`[A-Za-z0-9_-]`.

`<state>` is the PAS state folder: `$PAS_STATE_DIR`, else
`$XDG_STATE_HOME/pas`, else `~/.local/state/pas`. It is machine-wide, not
per Pipeline.

## Files

| Path (from `<logs>`) | Format | Written by, when | Appended or replaced | Version |
|---|---|---|---|---|
| `<stem>-<hash>/checkpoint.json` | pretty JSON | `pas run`, before each node attempt, after each node and before a stop; deleted when the Pipeline completes and by `--fresh` | replaced (temp file, fsync, rename) | `schema_version` (3) |
| `<stem>-<hash>/run.lock` | JSON `{"pid":…,"run_id":…}` | `pas run`, when the Attempt starts; `flock`ed until it ends | contents replaced; the file stays | none |
| `<stem>-<hash>/runs/<run-id>/run.json` | pretty JSON | `pas run`, once, when a new Run starts; never rewritten on resume | written once (temp file, rename) | `v` (1) |
| `<stem>-<hash>/runs/<run-id>/events.jsonl` | JSON Lines | `pas run`, through the Attempt, all Attempts in one file | appended | `v` (1) on every line |
| `<stem>-<hash>/runs/<run-id>/console.log` | text | the Monitor, when it starts or resumes a Run: stdout and stderr of that `pas run` | appended | none |
| `<stem>-<hash>/runs/<run-id>/final.json` | pretty JSON | `pas run`, at the end of each Attempt, after `AttemptEnded`; not after SIGKILL | replaced (temp file, rename) | `v` (1) |
| `<stem>-<hash>/runs/<run-id>/transcripts/<inv>.jsonl` | the agent's stdout, as written (`stream-json` lines for `claude -p`, JSONL for `codex exec --json`, `json`/`stream-json` for Gemini) | the agent process runner, created when the agent process starts, streamed while it runs | created once, then appended | none (the provider's format) |
| `<stem>-<hash>/runs/<run-id>/transcripts/<inv>.stderr.log` | text | the agent process runner, as above, for stderr | created once, then appended | none |
| `<stem>-<hash>/runs/<run-id>/transcripts/<inv>.prompt.txt` | text, see below | the agent registry (`Agents::run`), before the handler is called | written once | first line `pas prompt file v1` |
| `<stem>-<hash>/runs/<run-id>/answers/<question-id>.json` | JSON, one line | `pas answer`, the Monitor (through `pas answer`), or `pas run`'s terminal prompt | written once; the first writer wins | `v` (1) |
| `<stem>-<hash>/runs/<run-id>/answers/<question-id>.json.rejected` | as found | `pas run`, when an answer file is complete but unusable (wrong question, unknown choice, not JSON) | renamed; replaces an older one | as found |
| `<stem>-<hash>/runs/<run-id>/control/stop` | JSON, one line | `pas stop` or the Monitor; removed by `pas run` at the start of the next Attempt | written once; a repeat is a no-op | `v` (1) |
| `runs.jsonl` (in `<state>`) | JSON Lines | `pas run`, once, when a new Run starts (not on resume), under `flock`, fsynced | appended | `v` (1) on every line |

### `checkpoint.json`

Fields: `current_node_id`, `completed_nodes`, `node_outcomes`,
`context_snapshot`, `timestamp`, `run_id`, `step_count`, `total_cost`,
`schema_version`, `quality_loop_counters`, `quality_last_footprint`,
`previous_node_id`, `total_handler_attempts`, `active_node_id`,
`active_node_attempts`, `active_attempt_number`, `active_attempt_head`,
`execution_fingerprint`. A resume refuses a checkpoint whose
`execution_fingerprint` does not match the current graph. It belongs to the
Pipeline folder, not a Run: `run_id` names the Run it resumes. Source:
`crates/attractor-pipeline/src/checkpoint.rs`.

### `run.lock` and the Worktree lock

`run.lock` is the Pipeline lock: a second `pas run` of the same Pipeline
exits with code 5. The Worktree lock is `pas-run.lock` in the Run worktree's
git dir (`<repo>/.git/worktrees/<run-id>/pas-run.lock`), **not** in the run
folder. Both hold `{"pid":…,"run_id":…}`; the OS releases them when the
process exits. See [Concurrency locks](cli-reference.md#concurrency-locks).

### `run.json`

Fields: `v`, `run_id`, `pipeline_path`, `pipeline_name`, `workdir` (the
source directory), `git_worktree`, `logs_dir`, `started_at`, `argv`,
`pas_version`, `epic_id`, `worktree`, `branch`, `base`, `base_sha`,
`warnings`. `worktree`, `branch`, `base` and `base_sha` are `null` when the
Run works in place (no git repository, or `--dry-run`). Source:
`crates/attractor-journal/src/meta.rs`.

### `events.jsonl`

Each line is one Event:

```json
{"v":1,"seq":42,"ts":"2026-09-24T10:03:11.123Z","run_id":"0192...","attempt":2,"type":"StageStarted","data":{"node_id":"implement","handler_type":"codergen"}}
```

`seq` increases across all Attempts of the Run, from 1. `ts` has
millisecond precision. `attempt` is the Attempt (one `pas run` process)
that wrote the line. The writer fsyncs after `AttemptEnded` and
`HumanInputRequested`, and otherwise at least every 5 seconds. A torn last
line, left by a killed process, is cut off when the next Attempt opens the
file. The event types are listed [below](#events).

### `final.json`

Fields: `v`, `run_id`, `status` (`success`, `failed`, `stopped`),
`branch`, `base`, `base_sha`, `final_commit`, `worktree`, `error` (failures
only), `warnings`. With `--json`, the same object is the last stdout line.
See [End of run: `final.json`](cli-reference.md#end-of-run-finaljson).

### `transcripts/<inv>.prompt.txt`

Plain text, so `cat` and `less` work:

```
pas prompt file v1
invocation: <id>
node: <node-id>
attempt: <n>

argv:
<one argument per line; the argument equal to the prompt is shown as <prompt>>

env (names only):
<one name per line, sorted>

prompt:
<the prompt, verbatim, to the end of the file>
```

The prompt goes last because it spans lines. Environment variable values
never appear, and neither does the prompt's text inside the argv section.
The file is written before the agent starts, so an invocation that fails to
launch still has one. Writing it is observability: if it fails, `pas` logs
a warning and runs the agent anyway.

### Transcripts and stderr logs

Both files are created only once the agent process has started, so a
reader can match a growing transcript to its `LlmStarted` event and tell a
silent agent from its file's size and modification time. `LlmStarted`
and `LlmInvoked` record the paths relative to the Run folder.

### `answers/<question-id>.json`

```json
{"v":1,"question_id":"q-review-1","choice":"approve","source":"cli","answered_at":"2026-09-24T10:00:00Z"}
```

`source` is `terminal`, `cli` or `monitor`. The file is written to a temp
file and hard-linked into place, so only the first answer lands and no
reader sees half a file. Source: `crates/attractor-journal/src/answer.rs`.

### `control/stop`

```json
{"v":1,"source":"cli","requested_at":"2026-09-24T10:00:00.000Z"}
```

`source` is `cli` or `monitor`. The Run notices the file between stages,
journals `StopRequested`, saves its checkpoint and ends the Attempt as
`stopped`. Re-running the same `pas run` command resumes it.

### `runs.jsonl` (the Run Index)

```json
{"v":1,"run_id":"0192...","started_at":"2026-09-24T10:00:00Z","workdir":"/abs/repo","pipeline_path":"/abs/repo/p.dot","run_dir":"/abs/repo/.pas/logs/p-1a2b3c4d/runs/0192..."}
```

It never stores a status: `pas runs` derives status from the journal. A
Run whose `run_dir` is gone is shown as missing. Lines that don't parse
are skipped. Source: `crates/attractor-journal/src/index.rs`.

## Events

Every type `events.jsonl` can hold, in the order of `EventData::KNOWN_TYPES`
(`crates/attractor-journal/src/event.rs`). The golden file for each type
is the field-level reference.

| Event | Written when |
|---|---|
| [`RunStarted`](../crates/attractor-journal/tests/golden/journal/RunStarted.jsonl) | Once, when a new Run starts (not on resume); the first line. |
| [`AttemptStarted`](../crates/attractor-journal/tests/golden/journal/AttemptStarted.jsonl) | At the start of every Attempt, the first and each resume. |
| [`AttemptEnded`](../crates/attractor-journal/tests/golden/journal/AttemptEnded.jsonl) | When an Attempt ends, with `reason`. Never written when the process is killed; its absence marks a crashed Attempt. |
| [`Heartbeat`](../crates/attractor-journal/tests/golden/journal/Heartbeat.jsonl) | Every 30 seconds while an Attempt runs, with the `pas` pid. |
| [`PipelineStarted`](../crates/attractor-journal/tests/golden/journal/PipelineStarted.jsonl) | When the engine starts walking the graph, in every Attempt. |
| [`PipelineCompleted`](../crates/attractor-journal/tests/golden/journal/PipelineCompleted.jsonl) | When the graph reaches its exit node and its goal gates pass. |
| [`PipelineFailed`](../crates/attractor-journal/tests/golden/journal/PipelineFailed.jsonl) | When the Attempt fails with an error (not for a stop). |
| [`StageStarted`](../crates/attractor-journal/tests/golden/journal/StageStarted.jsonl) | Before each attempt of a node. |
| [`StageCompleted`](../crates/attractor-journal/tests/golden/journal/StageCompleted.jsonl) | When a node attempt ends with an outcome the engine routes on. |
| [`StageFailed`](../crates/attractor-journal/tests/golden/journal/StageFailed.jsonl) | When a node fails with no retries left, or its handler errors. |
| [`StageRetrying`](../crates/attractor-journal/tests/golden/journal/StageRetrying.jsonl) | When a node attempt will be retried. |
| [`EdgeSelected`](../crates/attractor-journal/tests/golden/journal/EdgeSelected.jsonl) | When the engine picks the edge to the next node. |
| [`GoalGateChecked`](../crates/attractor-journal/tests/golden/journal/GoalGateChecked.jsonl) | At the exit node, once per goal-gate node, sorted by node ID. |
| [`CheckpointSaved`](../crates/attractor-journal/tests/golden/journal/CheckpointSaved.jsonl) | After each write of `checkpoint.json`. |
| [`ContextUpdated`](../crates/attractor-journal/tests/golden/journal/ContextUpdated.jsonl) | When a node's outcome sets context keys (names only, sorted). |
| [`EpicSnapshot`](../crates/attractor-journal/tests/golden/journal/EpicSnapshot.jsonl) | When a beads node reads the Epic and its Tasks. |
| [`TaskClaimed`](../crates/attractor-journal/tests/golden/journal/TaskClaimed.jsonl) | When a beads node claims the next Task. |
| [`TaskSelectionBlocked`](../crates/attractor-journal/tests/golden/journal/TaskSelectionBlocked.jsonl) | When open Tasks remain but all are blocked. |
| [`TaskClosed`](../crates/attractor-journal/tests/golden/journal/TaskClosed.jsonl) | When a beads node closes a Task, with its Run Commits. |
| [`LlmStarted`](../crates/attractor-journal/tests/golden/journal/LlmStarted.jsonl) | When an agent process is spawned, before its first output: pid, pgid, host, transcript and stderr paths. |
| [`LlmInvoked`](../crates/attractor-journal/tests/golden/journal/LlmInvoked.jsonl) | When a Model Invocation ends (or is dropped): status, tokens, cost, duration. |
| [`CommitsCreated`](../crates/attractor-journal/tests/golden/journal/CommitsCreated.jsonl) | When HEAD moved during a node attempt, listing the new commits. |
| [`HumanInputRequested`](../crates/attractor-journal/tests/golden/journal/HumanInputRequested.jsonl) | When a Human Gate asks its question; fsynced, so a reader can answer at once. |
| [`HumanInputAnswered`](../crates/attractor-journal/tests/golden/journal/HumanInputAnswered.jsonl) | When the Human Gate takes an answer, with its `source`. |
| [`StopRequested`](../crates/attractor-journal/tests/golden/journal/StopRequested.jsonl) | When the engine finds `control/stop` between stages. |
