# Fake agents

`fake-claude` is a digital twin of `claude -p` for tests (design.md §6). It
prints scripted stream-json, so any engine behaviour can be tested end to
end through `pas run` with no real agent, API key or subscription.

Tests select it by agent profile (ticket 03): the test repo's `pas.toml`
defines a `fake` profile (`mechanism = "claude-p"`, `command` = this script,
`test_only = true`), pipelines use `agent="fake"`, and `pas run` gets
`--allow-test-agents`. PAS runs it with its own environment minus the
profile's `env.remove` plus `env.set` and the `PAS_*` ids; the test sets
`FAKE_AGENT_SCENARIOS`. Extra profile fields (`kill_grace`, `timeout`,
`reasoning_args`, `env.set`, ...) go in the same `pas.toml`. Only the test of
the `llm_provider="claude"` alias still puts this script first on `PATH` as
`claude`. A seam 1 test can instead give `Agents` a profile whose command is
this script (`crates/attractor-handler-claude-p/tests/claude_p.rs`). The Rust
helper is `crates/attractor-cli/tests/fake_agent/mod.rs`; the tests are in
`crates/attractor-cli/tests/fake_agent_harness.rs` and
`crates/attractor-cli/tests/agent_profiles.rs`.

## Contract

- **Flags.** It accepts exactly the flags PAS passes to `claude` (the
  built-in `claude` profile's args, the `[codergen.claude]` and node flags,
  `--model`, `--effort`, and the `claude-p` handler's own); value flags take
  their value. An unknown flag
  exits 2 with `fake-claude: unknown flag <x>`, so argv drift shows up in
  tests. `-p` is required.
- **Scenario.** The scenario text is the first match of:
  1. the first line of `$FAKE_AGENT_SCENARIOS/<PAS_NODE_ID>.<PAS_ATTEMPT>`
     (only when both variables are set and the file exists);
  2. the first line of `$FAKE_AGENT_SCENARIOS/<PAS_NODE_ID>` (when
     `PAS_NODE_ID` is set and the file exists);
  3. otherwise the `-p` prompt, from the text after its last `Task (`: that
     is the node's own `prompt`. The text before it holds the goal and
     earlier nodes' results.

  Put `scenario=<name>` in that text; if it appears more than once, the
  last one wins. Optional tokens, taken the same way: `label=<L>`,
  `fails=<N>` and `exit=<N>` (letters, digits, `_` and `-` only; `exit`
  must be a number). An unknown or missing scenario exits 2. A node file
  lets one node act differently on each attempt, e.g. `work.1` holding
  `scenario=bad_result exit=1` and `work` holding `scenario=success`.
- **Environment.** `FAKE_AGENT_SCENARIOS` (required): an existing folder
  where the fake keeps its state. `FAKE_HANG_SECS`: how long `hang` sleeps
  (default 30). `PAS_NODE_ID`, `PAS_ATTEMPT`: optional, for the scenario
  lookup above.
- **stdin** is read only by the `stdin` scenario.
- **Output.** A `system`/`init` line with a `session_id`, `assistant`
  message lines, and a final `result` line (`subtype`, `is_error`,
  `result`, `session_id`, `num_turns`, `total_cost_usd`), in the shape of
  `crates/attractor-pipeline/tests/fixtures/providers/claude-2.1.282.stream.jsonl`.

## State files

In `$FAKE_AGENT_SCENARIOS`:

| File | Contents |
|---|---|
| `attempts.<scenario>` | how many times this scenario started |
| `env.log` | per start: a `--- start` line, a `FAKE_PID=<pid>` line (the fake's own pid: PAS execs it directly, so this is the agent's pid), then the sorted `PAS_*`, `ANTHROPIC_*`, `OPENAI_*` and `CLAUDE_CODE_USE_*` environment variables |
| `invocations.log` | per start: a `--- start` line, then each argument on its own line, without the `-p` value |
| `prompts.log` | per start: a `--- start` line, then the full `-p` prompt |
| `prompt.<PAS_NODE_ID>.<PAS_ATTEMPT>` | per start when both variables are set: the full `-p` prompt and a newline, overwritten on each start of that attempt |
| `committed` | created (empty) by `commit_hang` after its commit |
| `stdin.bytes` | written by `stdin`: how many bytes it read from stdin |
| `go` | created by the test: lets `stderr_live` finish |
| `term` | written by `hang_term` and `hang_ignore_term` when they get TERM (`hang_ignore_term` appends a line per TERM) |

The counter is per scenario name, not per node, and its read-modify-write
isn't atomic: use one `flaky` node per test and no parallel branches.

## Scenarios

| Name | Behaviour | Exit |
|---|---|---|
| `success` | result `success`, text `fake-claude: success` | 0 |
| `fail` | result `error_during_execution` with `is_error:true`; with `label=<L>`, its text ends with `<L>` on its own line | 1 |
| `error_max_turns` | result `error_max_turns` with `is_error:false` | 0 |
| `label` | success whose last line is the `label=` token | 0 |
| `crash` | init and an assistant line, `fake-claude: crashed` on stderr, no result | 3 |
| `crash_after_result` | a full success stream, then a stderr line | 3 |
| `garbage` | non-JSON lines only | 0 |
| `silent` | no output | 0 |
| `hang` | init, then `exec sleep $FAKE_HANG_SECS` (killed by the node timeout) | - |
| `slow` | init, three assistant lines 0.2 s apart, then success | 0 |
| `edit_commit` | writes `fake-edit.txt` in its cwd, commits it as `fake-claude edit`, then success | 0 |
| `edit_hang` | writes `partial.txt` in its cwd (uncommitted), then hangs like `hang` | - |
| `commit_hang` | writes `committed.txt` in its cwd, commits it as `fake-claude commit` (like `edit_commit`), creates `committed`, then hangs like `hang` | - |
| `pas_dir` | creates `.pas/x` (holding `x`) and `visible.txt` in its cwd, then success | 0 |
| `flaky` | hangs while its start count is at most `fails` (default 1), then success | 0 |
| `bad_result` | init, then `{"type":"result","subtype":"success","is_error":false,"num_turns":"many"}`, a result line that doesn't deserialize (`num_turns` is a string) | `exit` (default 0) |
| `stdin` | reads stdin to the end, writes the byte count to `stdin.bytes`, then success | 0 |
| `stderr_live` | init, `fake-claude: working` on stderr, waits for the file `go` (polled every 0.05 s, at most 10 s, else exit 2), then `fake-claude: done` on stderr and success | 0 |
| `hang_term` | traps TERM (writes `term`, exits 143), init, then `sleep 30 & wait` | 143 on TERM |
| `hang_ignore_term` | traps TERM (appends to `term`, carries on), init, then loops `sleep 1 & wait` forever (killed only by KILL) | - |

`flaky` fails by hanging because a timeout is the only error PAS retries
today.

`hang_term` and `hang_ignore_term` don't `exec` their `sleep`, so the shell
itself gets TERM and runs its trap. A TERM sent to the process group (as
PAS sends it) also hits the backgrounded `sleep`; `hang_term` exits anyway,
and `hang_ignore_term`'s loop just starts another `sleep`.

## Adding a scenario

1. Add a `case` arm to `fake-claude`, built from `init`, `assistant`,
   `result`, `succeed` and `hang`. Result and assistant texts are plain
   ASCII with no quotes or backslashes (a JSON escape such as `\n` is fine),
   and never contain `scenario=`, `label=`, `fails=` or `exit=`: a later node's
   prompt carries earlier results. Print JSON with `printf '%s\n' "$json"`;
   never put data in a printf format string.
2. Add a row to the table above.
3. Add a test in `fake_agent_harness.rs` that runs it through `pas run`.

## fake-codex and fake-gemini

Twins of `codex exec --json` and the Gemini CLI, for the `codex-exec` and
`gemini` handlers. They find their scenario like `fake-claude` (the node
files, else `scenario=<name>` in the node's prompt), log the environment to
`env.log` and argv to `invocations.log` (the positional prompt as
`<prompt>`), and the prompt to `prompts.log`. The CLI harness selects them
with `agent="fake-codex"` / `agent="fake-gemini"` (its `pas.toml` profiles
inherit the built-in `codex`/`gemini` profiles with the fake as `command`),
or shims them on `PATH` as `codex`/`gemini` for the `llm_provider` aliases.

| Scenario | fake-codex | fake-gemini |
|----------|------------|-------------|
| `success` | `agent_message` `fake-codex: success`, `turn.completed` | answer `fake-gemini: success` in the requested format |
| `turn_failed` / `failure` | a message, then `turn.failed`, exit 1 | an error (json) or error `result` (stream-json), exit 1 |
| `crash` | no stdout, `fake-codex: crashed` on stderr, exit 3 | no stdout, `fake-gemini: crashed` on stderr, exit 3 |
| `garbage` | non-JSON stdout, exit 0 | non-JSON stdout, exit 0 |
| `silent` | nothing, exit 0 | (none) |
| `hang` | starts, then sleeps `FAKE_HANG_SECS` | sleeps `FAKE_HANG_SECS` |
| `label` | message ending with `label=<L>` on its own line | (none) |

`fake-gemini --help` answers the format probe: it appends one line (`$0`,
its arguments, its pid, and `via=$FAKE_VIA`) to `probes.log` and writes
nothing to the other logs. The first line of `gemini-help` in the scenario
folder picks the answer: `json` (help without `stream-json`), `fail`
(exit 1), `hang` (writes its pid to `probe.pid` and sleeps); anything else,
or no file, lists `stream-json`.
