# 01: Fake-agent harness (digital twin of claude -p)

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace` in the repo. Every check below runs with the shell-script fakes in temporary git repos; no real agent, API key or subscription.

**What to build:** A fake `claude` executable (shell script) and a test helper, so any engine behaviour can be tested end to end through `pas run` with no real agent (design.md §6, decision Q19, Q53). Today's code already runs `claude` from `PATH` and passes the caller's environment through, so this ticket needs no engine change: the helper puts the fake first on `PATH` and points it at a scenario directory.

The fake accepts every flag PAS passes to `claude` today, reads its scenario from a marker in the node's prompt attribute (e.g. `scenario=hang`; ticket 02a adds `PAS_NODE_ID`/`PAS_ATTEMPT`) and an attempt counter file, never reads stdin (it is still inherited until 02a), and prints stream-json: an init line with a session id, an assistant message, and a final `result` line. Scenarios: success; reported failure (`is_error:true`); `error_max_turns` with `is_error:false`; label routing (result text ends with a label); crash (partial output, stderr, exit 3, no result); crash after result; garbage; silent; hang (sleeps past the timeout; never reads stdin); slow stream; edit and commit (writes files, runs `git commit`); flaky (fails N times, then succeeds).

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] A one-node pipeline run with `pas run` and the fake, scenario success, completes and the node's result text is the fake's
- [ ] Reported failure routes on an `outcome=fail` edge; label routing follows the labelled edge
- [ ] Crash, garbage and silent each end the run with today's error; hang ends it with today's timeout error (short node timeout)
- [ ] Edit and commit leaves the fake's file and commit in the workdir, and the journal shows `CommitsCreated`
- [ ] Flaky with `max_retries` is retried on timeouts (the only retryable class the fake can produce), matching today's retry rules
- [ ] `error_max_turns` with `is_error:false` is recorded today as Success; the test asserts today's behaviour and is marked to flip in ticket 02a
- [ ] The fake and scenario format are documented next to the script (how to add a scenario)
