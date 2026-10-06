# 08: Continue a node's agent session on retry and loop-back

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace` in the repo. Every check below runs with the shell-script fakes in temporary git repos; no real agent, API key or subscription.

**What to build:** A node that runs again continues its agent's previous session (design.md §1 Sessions, Q25, Q27, Q28, Q30, Q50; Q62 verified the Claude flags on a real login). The engine mints a session id for a new session and records the id the agent reports; the thread key (node id, or `thread_id`) → session id map lives in `checkpoint.json`. `fidelity=full` (default for profiles that can resume) continues; `fidelity=fresh` starts new. Profile fields `session_args` (new session), `resume_args` (replaces only `session_args`) or `resume_command` (replaces command and args; `model_args` and `reasoning_args` appended). Defaults: Claude `--session-id {session_id}` / `--resume {session_id}`; Codex `codex exec resume {session_id} ...`; the defaults drop `--no-session-persistence` and `--ephemeral`. A profile with no resume form (gemini) defaults to `fresh`, and an explicit `full` on it fails validation. A session that can't be continued fails the attempt (no fallback). `PAS_SESSION_ID`, `Pas-Session` trailer, and the agent session id in the result and journal.

**Blocked by:** 04, 07

**Status:** ready-for-agent

- [ ] The fake records its argv: first run of a node gets `--session-id <uuid>`, a retry of that node gets `--resume <same id>` and keeps `--safe-mode` and the other profile args
- [ ] A loop implement → review → implement: the second implement continues implement's session; review has its own
- [ ] Two nodes with the same `thread_id` share one session
- [ ] `fidelity=fresh` gets a new session every time
- [ ] The map survives stop and resume (the resumed node continues the recorded id)
- [ ] The fake Codex gets `exec resume <thread id>` on a retry
- [ ] A gemini node defaults to fresh; `fidelity=full` on it fails `pas validate`
- [ ] The fake's "session not found" scenario, matching real Claude (research/claude-resume-check.md, Q62: a result line with `is_error: true`, `subtype: error_during_execution`, an `errors` array, and "No conversation found with session ID" on stderr), is a reported failure carrying that reason; no fallback to a new session
