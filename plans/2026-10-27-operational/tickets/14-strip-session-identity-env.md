# 14: Never pass the parent session's identity to agents

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**What to build:** Agents get the parent environment minus the profile's
`env.remove` (crates/attractor-agent-handler/src/env.rs:21-42). When `pas`
runs inside a Claude Code or hk3 session, every agent also gets that
session's identity: `CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`,
`CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_MESSAGING_SOCKET`/`_TOKEN`,
`HK3_AGENT_ID`/`_ROLE`/..., `KEEPER_*`. So an agent's `hk3 crew` command
acts as the parent, and a child claude can be unmessageable (hk3 strips the
same set for its own children: harmonik-v3 modules/session/main:61,
`SCRUB_RE`, and docs/architecture.md:74-87).

Add an engine-owned list, stripped always and not configurable, next to the
`PAS_*` handling in env.rs: `CLAUDECODE`, `CLAUDE_PID`, `CLAUDE_EFFORT`,
the session-identity `CLAUDE_CODE_*` names (`CHILD_SESSION`,
`SESSION_ID`, `MESSAGING_SOCKET`, `MESSAGING_TOKEN`, `ENTRYPOINT`,
`EXECPATH`, `ENABLE_FUNCTION_HOOKS`, `SESSION_ATTENDED`), every `HK3_*`,
every `KEEPER_*`, and `HARMONIK_AGENT`. hk3's `SCRUB_RE` strips every
`CLAUDE_CODE_*`; PAS strips only the session-identity names above, since
other `CLAUDE_CODE_*` names are user settings (e.g.
`CLAUDE_CODE_MAX_OUTPUT_TOKENS`) and pass as before. `env.set` can still
set any of them on purpose. The meaning of `env.remove` doesn't
change. Document the list in docs/cli-reference.md (agent environment) and
docs/run-folder.md if it lists env names.

**Blocked by:** 13

**Status:** done

- [ ] Seam 1 (`Agents::run` with a fake): with `CLAUDECODE`, `CLAUDE_CODE_MESSAGING_TOKEN`, `HK3_AGENT_ID` and `KEEPER_X` in the parent env, none reach the agent; `CLAUDE_CODE_MAX_OUTPUT_TOKENS` and an unrelated `FOO` do (`CLAUDE_CODE_USE_BEDROCK` is on the default strip list already)
- [ ] The same with a profile that sets its own `env.remove`
- [ ] `env.set` of a stripped name reaches the agent
- [ ] Seam 2 (`pas run` with the fake): the agent's recorded env has none of the identity names
- [ ] The prompt file lists none of the stripped names
