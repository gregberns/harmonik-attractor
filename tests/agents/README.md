# Fake agents

`fake-claude` is a digital twin of `claude -p` for tests (design.md §6). It
prints scripted stream-json, so any engine behaviour can be tested end to
end through `pas run` with no real agent, API key or subscription.

PAS runs `claude` from `PATH` and passes its environment through, so a test
puts this script first on `PATH` as `claude` and sets
`FAKE_AGENT_SCENARIOS`. The Rust helper that does this is
`crates/attractor-cli/tests/fake_agent/mod.rs`; the tests are in
`crates/attractor-cli/tests/fake_agent_harness.rs`.

## Contract

- **Flags.** It accepts exactly the flags PAS passes to `claude` today
  (`codergen_provider.rs`); value flags take their value. An unknown flag
  exits 2 with `fake-claude: unknown flag <x>`, so argv drift shows up in
  tests. `-p` is required.
- **Scenario.** Read from the `-p` prompt, from the text after its last
  `Task (`: that is the node's own `prompt`. The text before it holds the
  goal and earlier nodes' results. Put `scenario=<name>` in the node's
  prompt; if it appears more than once, the last one wins. Optional tokens,
  taken the same way: `label=<L>` and `fails=<N>` (letters, digits, `_` and
  `-` only). An unknown or missing scenario exits 2.
- **Environment.** `FAKE_AGENT_SCENARIOS` (required): an existing folder
  where the fake keeps its state. `FAKE_HANG_SECS`: how long `hang` sleeps
  (default 30).
- **stdin** is never read.
- **Output.** A `system`/`init` line with a `session_id`, `assistant`
  message lines, and a final `result` line (`subtype`, `is_error`,
  `result`, `session_id`, `num_turns`, `total_cost_usd`), in the shape of
  `crates/attractor-pipeline/tests/fixtures/providers/claude-2.1.282.stream.jsonl`.

Ticket 02a adds scenario lookup by `PAS_NODE_ID` and `PAS_ATTEMPT`.

## State files

In `$FAKE_AGENT_SCENARIOS`:

| File | Contents |
|---|---|
| `attempts.<scenario>` | how many times this scenario started |
| `invocations.log` | per start: a `--- start` line, then each argument on its own line, without the `-p` value |
| `prompts.log` | per start: a `--- start` line, then the full `-p` prompt |

The counter is per scenario name, not per node, and its read-modify-write
isn't atomic: use one `flaky` node per test and no parallel branches.

## Scenarios

| Name | Behaviour | Exit |
|---|---|---|
| `success` | result `success`, text `fake-claude: success` | 0 |
| `fail` | result `error_during_execution` with `is_error:true` | 1 |
| `error_max_turns` | result `error_max_turns` with `is_error:false` | 0 |
| `label` | success whose last line is the `label=` token | 0 |
| `crash` | init and an assistant line, `fake-claude: crashed` on stderr, no result | 3 |
| `crash_after_result` | a full success stream, then a stderr line | 3 |
| `garbage` | non-JSON lines only | 0 |
| `silent` | no output | 0 |
| `hang` | init, then `exec sleep $FAKE_HANG_SECS` (killed by the node timeout) | - |
| `slow` | init, three assistant lines 0.2 s apart, then success | 0 |
| `edit_commit` | writes `fake-edit.txt` in its cwd, commits it as `fake-claude edit`, then success | 0 |
| `flaky` | hangs while its start count is at most `fails` (default 1), then success | 0 |

`flaky` fails by hanging because a timeout is the only error PAS retries
today.

## Adding a scenario

1. Add a `case` arm to `fake-claude`, built from `init`, `assistant`,
   `result`, `succeed` and `hang`. Result and assistant texts are plain
   ASCII with no quotes or backslashes (a JSON escape such as `\n` is fine),
   and never contain `scenario=`, `label=` or `fails=`: a later node's
   prompt carries earlier results. Print JSON with `printf '%s\n' "$json"`;
   never put data in a printf format string.
2. Add a row to the table above.
3. Add a test in `fake_agent_harness.rs` that runs it through `pas run`.
