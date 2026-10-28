# 15: A Codex run with no agent message is not a success

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**What to build:** The codex handler counts any non-empty stdout without an
error event as `Completed`, at any exit code
(crates/attractor-handler-codex-exec/src/parse.rs:125-143). Reproduced: a
stub that prints garbage and exits 0 completes the pipeline, and one that
prints only `thread.started` and exits 137 (killed) completes it too. That
hides a failure, which the previous plan exists to prevent (its Q47).

Classify like the claude-p handler: `Completed` only with a final agent
message (or `turn.completed`) and exit 0; a non-zero exit with no
`turn.failed` is a crash with the exit code; exit 0 with no agent message
is `NoResult`. Keep `turn.failed`/`error` as reported failures. Update the
codex row of the failure table in the docs.

**Blocked by:** 13

**Status:** done

- [ ] Seam 1: garbage stdout + exit 0 → NoResult; `thread.started` only + exit 137 → Crash with the exit code; a normal run → Completed with the message
- [ ] fake-codex gains the scenarios it needs (and tests/agents/README.md lists them)
- [ ] Seam 2: the garbage scenario stops the run with a no-result error naming the node
- [ ] Existing codex tests and the recorded fixture still pass
