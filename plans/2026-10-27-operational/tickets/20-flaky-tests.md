# 20 (backlog): Flaky tests and test hygiene

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**What to build:** Tests that fail on a loaded machine. All are test bugs
(no product change): mostly fresh shell stubs run against short real-time
deadlines (a new script's first run takes ~0.2 s on macOS, far more under
load). Fix by waiting on a file, fifo or signal, warming or sharing stubs,
and dropping upper time bounds:
- attractor-cli tests/cli_semantics.rs
  `validate_info_and_run_share_the_same_resolved_provider_and_boundaries`:
  `timeout=1s` on a fresh shim (also lines 425-613); it also runs in the
  crate folder (set `current_dir`), and `built_in_budget_warning_...` (533)
  writes run folders into the source tree.
- attractor-cli tests/kill.rs `sigterm_ignoring_run_is_killed_after_grace`:
  wait for a ready file after the trap instead of a 300 ms sleep; drop the
  3 s upper bound.
- attractor-pipeline codergen_handler_tests transcripts:: (three tests):
  wait for the transcript line before dropping; warm the stub.
- attractor-monitor tests/controls.rs (three tests): write the fake once per
  binary (likely "text file busy" from concurrent write-and-exec) and the
  2 s startup window.
- run_lifecycle.rs THREE_STAGES uses `llm_provider="claude"` with the real
  PATH (safe only via --dry-run): put blocking agent stubs first on PATH in a
  shared tests helper.
- Optional: fold FakeAgent's `command`/`command_with`/
  `command_without_test_agents` into `command_in`.

**Blocked by:** 13

**Status:** backlog (operator: not an immediate priority)

- [ ] Each listed test passes 20x under load (a busy-loop load script)
- [ ] No test writes into the source tree
