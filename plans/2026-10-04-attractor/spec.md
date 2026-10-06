# Spec: Attractor (PAS fork)

Source: [requirements.md](https://github.com/gregberns/harmonik-v3/blob/main/plans/2026-10-04-attractor/requirements.md) and the operator's decisions in
[decisions.md](https://github.com/gregberns/harmonik-v3/blob/main/plans/2026-10-04-attractor/decisions.md) (Q12-56). Design: [design.md](design.md), which
holds the detail, the code references and the reasoning; this spec states
what to build. Glossary: [CONTEXT.md](CONTEXT.md). Research:
[research/](https://github.com/gregberns/harmonik-v3/blob/main/plans/2026-10-04-attractor/research/).

All code goes in the operator's fork **`~/github/harmonik-attractor`** (Rust
workspace, binary `pas`, base commit `50945da`), not in harmonik-v3.
harmonik-v3 only invokes `pas`.

## Problem Statement

hk3 needs an Attractor (a pipeline engine that walks a graph of agent and
tool steps) that an external driver can run. PAS, the chosen base, already
runs sequential pipelines with checkpoints, human gates and a run journal,
but:
- it runs only three hard-coded agents (`claude`, `codex`, `gemini` on
  `PATH`, fixed flags); a new agent, or a model like DeepSeek or GLM, needs
  a code change, and reasoning level is rejected;
- every node works directly in the caller's directory: no worktree, no
  branch, no commits, so there is no record of what each step did and runs
  can trample each other;
- an agent's stderr is never written down, and nothing on disk says which
  transcript belongs to which running node, so a hung agent is hard to spot
  and a timeout's error ("Command timed out after 120000ms") doesn't say
  which node or where to look;
- some failures are hidden: a failed node can follow a success edge when no
  edge condition matches, a node still asking to retry after its last
  attempt ends the run as complete, and a Claude error result whose
  `is_error` is false counts as success;
- a node that runs again (a retry, or a loop such as implement → review →
  implement) starts a fresh agent with no memory of its last attempt;
- there is no way to test the engine end to end without a real agent and
  subscription.

## Solution

`pas` gains a small, public **agent handler** interface: each way of driving
an agent is its own crate (`claude-p` and `pi` new; `codex-exec` and
`gemini` moved from today's code), sharing a local process runner. Agents
are chosen by named **agent profiles** in config (defaults built in,
overrides in `pas.toml`), so a new model or agent that a handler can read
is a config edit. Pi is the multi-model handler for DeepSeek, GLM and the
hosted Qwen.

Each run gets its own **git worktree** (default
`<project-root>/.pas/worktrees/<run-id>`) on branch `pas/run/<run-id>`, and
the engine commits after every **attempt**, with `Pas-*` trailers, never
deleting partial work. On success the worktree is removed and the run
reports its branch, base and final commit; it merges nothing.

A node that runs again **continues its agent session** by default
(`fidelity=full`), and `thread_id` lets nodes share one.

The run folder records a start event per agent spawn, the agent's stderr
and a `final.json`. Failures are never hidden: reported errors route as
today, timeouts and crashes stop the run with an error that names the node
and the files to read, and the two failure-hiding bugs stop the run
instead.

A **digital twin** (shell-script fakes of `claude -p` and `pi`) lets every
change be tested end to end with `cargo test`, with no real agent.

## User Stories

1. As a pipeline author, I want to name an agent profile on a node, so that
   I choose the agent and model without code changes.
2. As a pipeline author, I want existing `llm_provider="claude"` (and
   `codex`, `gemini`) nodes to keep working, so that my pipelines don't
   break.
3. As an operator, I want to add a model by adding a profile in `pas.toml`,
   so that a new model version never needs a new `pas` binary.
4. As an operator, I want to set a node's reasoning level, so that hard
   steps can think longer.
5. As an operator, I want DeepSeek, GLM and my hosted Qwen to run through
   one handler (Pi) configured by profile, so that I don't need a handler
   per vendor.
6. As an operator, I want API-key variables stripped from the agent's
   environment unless the profile sets them, so that a stray key never
   switches a subscription run to API billing.
7. As an operator, I want each run in its own worktree and branch, so that
   runs don't touch my checkout or each other.
8. As an operator, I want a commit after every attempt, including failed
   and interrupted ones, so that I can see exactly what each attempt did.
9. As an operator, I want `.pas/` never committed and ignored in my
   checkout, so that run artefacts don't pollute my repo.
10. As an operator, I want a run that resumes after a crash to keep the
    partial work and tell the agent about it, so that good work is never
    lost.
11. As an operator, I want the worktree removed after a successful run and
    kept after a failed one, so that I can inspect failures.
12. As a driver, I want the run's branch, base, final commit and status in
    `pas run --json` and `final.json`, so that I can merge or open a PR
    myself.
13. As a driver or agent, I want a start event naming the node, pid and
    transcript and stderr files for every agent spawn, so that I can watch
    a running agent and spot a hung one.
14. As an operator, I want the agent's stderr on disk, so that I can see
    why it crashed.
15. As an operator, I want a timeout or crash to stop the run with an error
    naming the node, attempt, failure class and files to read, so that I
    know where to look.
16. As an operator, I want a failed node that matches no edge to stop the
    run, so that a failure never silently follows a success edge.
17. As an operator, I want a node still retrying after its last attempt to
    stop the run, so that it never ends as "complete".
18. As an operator, I want Claude error results counted as failures even
    when `is_error` is false, so that errors aren't hidden.
19. As a pipeline author, I want a retried or looped-back node to continue
    its agent's previous session, so that the agent keeps its context.
20. As a pipeline author, I want `thread_id` to let several nodes share one
    session, and `fidelity=fresh` to opt out.
21. As an operator, I want a session that can't be continued to fail the
    attempt, so that a lost context is never hidden.
22. As a developer, I want a fake `claude` and a fake `pi` (selected via
    `PATH` in tickets 01-02b, then by a test-only profile from ticket 03),
    so that I can test the engine end to end without a real agent.
23. As a developer, I want the test-only profile refused unless I pass
    `--allow-test-agents`, so that a fake can't run by accident.
24. As a developer, I want to add a seventh handler as one new crate and one
    registry line, so that the engine and interface don't change.
25. As an operator (low priority), I want a rate-limited Claude attempt
    retried for a short window, so that a brief limit doesn't fail the run.
26. As an operator (low priority), I want a retry prompt to say why the last
    attempt failed, so that the agent can fix it.

## Implementation Decisions

The detail for each is in design.md (section in brackets).

1. **Handler crates** [§1]. A public interface crate (handler trait,
   request and result types, observer, profile config and the registry the
   engine calls), a shared local-process runner crate, and one crate per
   handler: `claude-p`, `codex-exec`, `gemini`, `pi`. All are compiled into
   `pas`; a registry line per handler. The handler's `run` never returns an
   error; every end is a status (`Completed`, `Failed(class)`,
   `Cancelled`) with a failure class (`reported`, `timeout`, `crash`,
   `no_result`, `launch`). The invariant table in §1 says which guarantees
   the registry enforces and which each handler must keep.
2. **Child process** [§1]. cwd = the run's worktree; stdin `/dev/null`; own
   process group; environment = the caller's minus the profile's `remove`
   list (API keys by default), then the profile's `set`, plus `PAS_RUN_ID`,
   `PAS_NODE_ID`, `PAS_ATTEMPT`, `PAS_INVOCATION_ID`, `PAS_SESSION_ID`;
   transcript and stderr written live; timeout and cancel send TERM, wait
   the profile's grace, then KILL.
3. **Profiles** [§2]. Fields: `mechanism`, `command`, `args`,
   `session_args`, `resume_args` or `resume_command`, `model`,
   `model_args`, `reasoning`, `reasoning_args`, `timeout`, `kill_grace`,
   `env.remove`, `env.set`, `test_only`, `inherit_from`; Pi adds
   `provider`, `base_url`, `api_key_env`, `limits`. Templates `{model}`,
   `{reasoning}`, `{session_id}`. Defaults built into the binary; a project
   overrides whole profiles by name in `pas.toml` `[agents.*]`. Nodes name
   `agent=`; `llm_provider` and `[codergen.claude]` keep working.
   `reasoning_effort` is accepted.
4. **Claude handler** [§1]. Adds only `-p <prompt> --output-format
   stream-json --verbose`; failure table with `subtype` starting `error`
   as a failure.
5. **Codex and Gemini** [§1]. Today's commands and parsers, moved into
   their own crates, unchanged in behaviour.
6. **Pi handler** [§1]. `pi --mode json --model <provider/id>
   [--thinking] --session-dir --session-id`, whatever `pi` is installed;
   a per-invocation `PI_CODING_AGENT_DIR` with an engine-written
   `models.json` (holding the API key, 0600) and `settings.json`, plus
   `PI_TELEMETRY=0`, `PI_OFFLINE=1`, `PI_SKIP_VERSION_CHECK=1`; failure =
   last assistant `stopReason` `error` or `aborted`, or no final message.
7. **Sessions** [§1]. `New(id)` or `Continue(id)`; `fidelity=full` (default
   for profiles that can resume) continues the thread's session; thread key
   = node id unless `thread_id` is set; the map lives in `checkpoint.json`.
   A profile with no resume form (gemini) defaults to `fidelity=fresh`, and
   an explicit `full` on it fails validation. No fallback when continuing
   fails.
8. **Worktree and branch** [§3]. Branch `pas/run/<run-id>` from `--base`
   (default `HEAD`); a dirty source checkout starts anyway with a warning.
   Worktree root configurable in `pas.toml` and `--worktree-root`. pas
   writes `.pas/.gitignore` containing `*` in each `.pas/` it creates. Locks
   first: Pipeline lock, then checkpoint or new run id, then worktree, then
   Worktree lock.
9. **Attempt commits** [§3]. After every attempt, in the attempt loop
   before the result is matched: `git add -A -- . ':(exclude).pas'`, `git
   commit --allow-empty --no-verify`, trailers `Pas-Run`, `Pas-Node`,
   `Pas-Attempt`, `Pas-Status`, `Pas-Failure-Class`, `Pas-Session`; a fixed
   identity when the repo has none.
10. **Resume** [§3]. PAS's checkpoint resume, unchanged. On resume, if the
    checkpoint shows an attempt in progress and the worktree has changes,
    commit them as `Pas-Status: interrupted` and tell the agent to review
    `git diff <last attempt commit>`.
11. **End of run** [§3]. Success: remove the worktree, keep the branch.
    Failure: keep both. Report `branch`, `base`, `final_commit`, `status` in
    `pas run --json` and `final.json`. No merge.
12. **Run folder** [§4]. New journal event `LlmStarted` per spawn
    (invocation, spawn, node, attempt, profile, model, host, pid, pgid,
    transcript, stderr); `<inv>.stderr.log`; `final.json`. The run folder is
    observability only.
13. **Failures** [§5]. Reported error → routable Fail (as today); timeout →
    retryable up to `max_retries`, then stops the run (as today); crash, no
    result, launch → stop the run. Messages change only where today's
    can't be understood (Q56): a timeout's gains the node, attempt and
    transcript and stderr paths; a crash's gains its stderr tail and path.
    Fix (a): no first-edge fallback; a node with outgoing edges that match
    none stops the run, whatever its outcome (for a non-FAIL outcome this
    deliberately diverges from spec lines 390-392, which end the run
    normally). Fix (b): a Retry on the last attempt stops the run. Tool
    nodes unchanged.
14. **Low priority** [design "Priority"]: rate-limit window in `claude-p`
    (Q48); failure class and reason in the retry prompt (Q55);
    `<inv>.prompt.txt`; `docs/run-folder.md`.

## Testing Decisions

- **Seams (Q53):** (1) the registry's `run` with a fake executable, for the
  handler contract: start event, files, environment, failure table, kill;
  (2) `pas run` end to end with a fake, for engine behaviour: routing,
  retries, commits in the worktree, stop and resume, run-folder contents.
  Tests assert what a caller sees (results, events, files, git history),
  not internals.
- **Fakes:** shell scripts. `fake-claude` prints `claude -p` stream-json and
  picks a scenario from a scenario directory keyed by `PAS_NODE_ID` and
  `PAS_ATTEMPT`; `fake-pi` prints Pi's `--mode json` JSONL and always exits
  0. Scenarios are listed in design.md §6. Selected by a `test_only`
  profile behind `--allow-test-agents`.
- **No real agent or subscription** is needed for any acceptance check;
  `cargo test --workspace` runs everything, in temporary git repos.
- **Prior art in PAS:** shell stubs written per test
  (`codergen_handler_tests.rs`), `PATH` shims through the real `pas`
  binary (`attractor-cli/tests/cli_semantics.rs`), the recorded Claude
  fixture under `tests/fixtures/providers/`, and in-process handler doubles
  in `engine_tests.rs`, which stay.
- The 1,289 existing tests stay green. No test asserts the first-edge
  fallback directly; any test that depends on it shows up when ticket 05
  runs the suite and is updated then.

## Out of Scope

From design.md §7: ACP, tmux and remote handlers; a generic `exec`
handler; dynamic handler loading; parallel fan-out and manager loops;
fidelity `truncate`, `compact` and `summary:*`; the spec's failure-routing
rework (routable crashes and timeouts, `retry_target` for FAIL); an idle
timeout; falling back to a new session; automatic orphan reaping; per-node
result and tool-output files; a heartbeat progress field; git-derived
resume, the engine-owned run ref and history-rewrite detection; merging,
`pas merge`, PRs and pushing; sandboxing; an HTTP driver API; renaming
`pas`.

## Risks and Unknowns

**Unknowns** (cheapest check first):
- ~~`claude -p --resume <id>` keeps the same session id~~ Resolved (Q62,
  research/claude-resume-check.md): `--session-id` sets the id, `-p
  --resume` keeps it and the context, and a missing session is an
  `is_error` result (`error_during_execution`).
- `codex exec resume` verified on the ChatGPT login (harmonik-v3
  research/claude-resume-check.md): it keeps the `thread_id` and the
  context; a missing thread exits 1 with no JSON and "no rollout found" on
  stderr. `-m` on resume is still untested.
- Pi's `settings.json` keys for its retry window (low-priority ticket):
  read Pi's source when that ticket starts.
- A worktree nested under the project's `.pas/` behaves with every git
  command PAS runs: covered by ticket 06's tests.

**Assumptions:** the fork stays the operator's to change; the existing
checkpoint resume is good enough (Q43); shell fakes are portable enough
(macOS and Linux CI).

**Risks:**
- Moving `CodergenHandler`'s process, stream and parse code into crates
  breaks many of the 1,289 tests at once (medium). Mitigation: ticket 02a
  moves Claude first (with its tests), ticket 04 Codex and Gemini, and
  `LlmProvider` is removed last (expand, migrate, contract).
- Pi is not pinned (Q52) and releases almost daily (medium). Accepted by
  the operator; a breaking release shows up as a reported or no-result
  failure with Pi's output in the transcript, never as a hidden success.
- Tickets 02a/02b, 05 and 06 all touch the engine and run preparation;
  building them in parallel will cause merge conflicts (low, nuisance).
- TERM-grace-KILL needs an async kill path where today a `Drop` sends
  SIGKILL (medium). Mitigation: keep the drop guard as the backstop;
  tested with the hang scenario.
- Fix (a) changes routing for graphs that relied on the first-edge
  fallback (low): it now stops with a clear error instead.
- Behaviour changes users will notice (design.md §5 list): worktrees,
  commits, continued sessions, persisted session files, stripped keys.

**Irreversible or outward-facing steps:** none in the tickets. Pushing the
fork to GitHub is allowed at any time (Q61).

**Resolve first:** nothing outstanding; the `claude -p --resume` and
`codex exec resume` checks are done (Q62).

## Tickets

In [tickets/](tickets/), tracer-bullet order; see [README.md](README.md)
for the table with blocking edges.
