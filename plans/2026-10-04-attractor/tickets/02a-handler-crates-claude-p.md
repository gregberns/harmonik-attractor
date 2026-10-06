# 02a: Agent handler interface, process runner and the claude-p handler

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace` in the repo. Every check below runs with the shell-script fakes in temporary git repos; no real agent, API key or subscription.

**What to build:** Claude nodes run through the new public agent handler interface (design.md §1, Q31, Q35). New crates: the interface crate (handler trait, request and result types, observer, built-in default profiles, and the registry the engine calls), the shared local process runner, and the `claude-p` handler. The codergen node calls the registry for Claude nodes; Codex and Gemini stay on today's path until ticket 04 (expand first).

The runner: cwd = workdir, stdin `/dev/null`, own process group with today's drop guard, and an environment = the caller's minus a built-in strip list of API-key variables (Q54; it becomes the profile's `env.remove` in ticket 03) plus `PAS_RUN_ID`, `PAS_NODE_ID`, `PAS_ATTEMPT`, `PAS_INVOCATION_ID`. Timeout keeps today's kill for now (ticket 02b adds TERM first).

The `claude-p` handler adds only `-p <prompt> --output-format stream-json --verbose`; today's other flags come from the built-in `claude` profile. Its failure table treats a `subtype` starting `error` as a failure. `llm_provider="claude"` maps to the `claude` profile. The engine maps `Completed` and `Failed(reported)` to today's Success and Fail, and timeout, crash, no result and launch to today's errors. The existing Claude handler tests (the program-override stub tests and the recorded-output regression tests) move to the new crate and stay green.

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] All of ticket 01's scenarios pass unchanged through the new path, except `error_max_turns` with `is_error:false`, which is now a failure
- [ ] The fake sees `PAS_NODE_ID` and `PAS_ATTEMPT`, and no `ANTHROPIC_API_KEY` even when the caller sets one; the fake's scenario lookup can now use `PAS_NODE_ID`/`PAS_ATTEMPT`
- [ ] The fake's stdin is `/dev/null` (a scenario that reads stdin gets EOF at once)
- [ ] Seam 1: tests call the registry's `run` directly with the fake and check status, failure class, transcript and environment for each scenario
- [ ] The migrated Claude handler and regression tests pass; the full suite stays green (Codex and Gemini still on today's path)
