# 23: Claude agents can't reach the operator's other sessions

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace --no-fail-fast`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**What to build:** Found in the smoke test (a wrapper logged the real argv
and init line, Claude Code 2.1.302). The `claude` profile passes
`--safe-mode`, which drops the operator's skills, but Claude's built-in
session tools stay on: the agent's tool list had `SendMessage`,
`ListAgents` and `Workflow`, so a pipeline agent can message the
operator's live sessions (the hk3 crew). The env is already clean (14).
1. The built-in `claude` profile (crates/attractor-agent-handler/src/agents.toml)
   passes `--disallowedTools` with the tools that reach outside the Run:
   `SendMessage`, `ListAgents`, `Workflow`, `RemoteTrigger`,
   `PushNotification`, `CronCreate`, `CronDelete`, `ScheduleWakeup`.
   Pass it as one comma-separated value: the flag is variadic
   (`<tools...>`), so separate words would swallow what follows. Keep the
   list in one place and say why next to it.
2. A profile that sets its own `args` replaces the list (as with any
   field); document that in docs/cli-reference.md next to the `claude`
   profile, and how to allow one back.
3. CHANGELOG Unreleased.

**Blocked by:** none

**Status:** open

- [x] A fake claude run's argv has `--disallowedTools` with the whole list (seam 2)
- [x] A `pas.toml` profile with its own `args` gets exactly those args
