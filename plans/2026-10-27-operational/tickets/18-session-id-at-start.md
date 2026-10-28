# 18: Keep the session id when pas is killed mid-attempt

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**What to build:** A node's agent session id reaches the checkpoint only
after the attempt returns (crates/attractor-pipeline/src/engine.rs:687-693),
so a SIGKILL of `pas` mid-attempt loses it: the resume starts a new session
with no context, and the interrupted commit has no `Pas-Session` trailer
(engine.rs:864 passes `session: None`). For profiles where PAS chooses the
id (claude `--session-id`), save it to the checkpoint before the agent is
started; for agents that report it (codex `thread_id`), save it when first
seen if that's cheap, else document the limit. The interrupted commit uses
the saved id.

**Blocked by:** 13

**Status:** ready

- [ ] Seam 2: SIGKILL `pas` during a claude-fake attempt (wait for the fake's init line, not just LlmStarted), resume: the next attempt passes `--resume <the same id>`
- [ ] The interrupted commit has `Pas-Session: <id>`
- [ ] A normal retry still resumes as before
