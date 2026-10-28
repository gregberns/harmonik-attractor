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
- attractor-pipeline codergen_regression_tests (the Gemini cases, e.g.
  `nonzero_exit_with_final_result_gives_pre_streaming_outcome`): under load
  the `gemini --help` probe times out, so the stub runs with
  `--output-format json` instead of `stream-json`.
- attractor-monitor tests/controls.rs (every test that spawns the fake,
  including `resume_output_goes_to_the_same_runs_console_log`): write the
  fake once per binary (likely "text file busy" from concurrent write-and-exec) and the
  2 s startup window.
- run_lifecycle.rs THREE_STAGES uses `llm_provider="claude"` with the real
  PATH (safe only via --dry-run): put blocking agent stubs first on PATH in a
  shared tests helper.
- attractor-handler-codex-exec `a_timed_out_attempt_still_reports_the_thread_id`:
  flaked once under load.
- attractor-cli tests/pi.rs
  `a_retry_after_a_timeout_continues_the_same_pi_session`: a 1 s timeout;
  failed once under load ~50, passed 5/5 alone.
- Optional: fold FakeAgent's `command`/`command_with`/
  `command_without_test_agents` into `command_in`.

**Blocked by:** 13

**Status:** done

- [x] Each listed test passes 20x under load (a busy-loop load script)
- [x] No test writes into the source tree

**Findings:**
- controls.rs: passed 14/14 with `--test-threads=1` but failed 3/14 in
  parallel, both under load. The cause was not "text file busy" but the
  monitor's 2 s startup window: the fake `pas` could not start within it,
  so the monitor answered "started" before it had run. A hidden,
  tests-only `AppState::set_startup_window` gives the tests 60 s.
- Gemini regression cases: each run took 13-31 s under load ~140, so the
  10 s `--help` probe timed out (1 failure in 20); the test `Agents` give
  the probe 60 s.
- codex thread id: the id is in the fake's first line, so a 500 ms
  timeout could fire before it; now 5 s. pi retry: the 1 s node timeout
  also covers the retry; now 5 s.
- The FakeAgent `command` fold was left out (optional).
