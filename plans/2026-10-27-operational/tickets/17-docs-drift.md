# 17: Docs match the code

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**What to build:** Docs only, after 14-16 merge (they change the same docs).
Fix each mismatch the 2026-10-27 audit found; check each against the code
first:
- docs/execution-capabilities.md:24 and docs/dot-dialect.md:243 say
  `reasoning_effort` on Codex/Gemini nodes is rejected as
  `unsupported_execution_capability`; since 04 those nodes have profiles and
  get the `reasoning_args` error (validation.rs:230). The test named at :24
  no longer exists. Also the stale comment at execution_plan.rs:673-675.
- docs/cli-reference.md: `pas decompose --validate` missing from the
  options table (875-882); it `requires` SPEC_PATH (main.rs:266), so check
  whether "exactly one source" (867) needs it at all; `pi` missing
  from the agent CLIs needed on PATH (1452-1456); `PAS_REQUIRE_BD` and
  `GEMINI_API_KEY` undocumented.
- docs/accept-execute-c4.md:126 and accept-execute-c4-dot.md:135 cite
  `PAS_CLI_PATH` and `server/execute.rs`, which don't exist (the monitor
  uses `current_exe()`, attractor-monitor/src/spawn.rs:24).
- README.md: subcommand list (261) lacks `init` and `trust`; "Codergen
  Provider Integration" (59-66) lacks Pi and agent profiles.
- plans/2026-10-04-attractor/design.md:181 says `Agents` catches handler
  panics; mark it not built (deferred, see the operational plan).
- A short "Differences from the Attractor spec" section in docs/guide.md
  linking the existing inline notes (task-verification.md:135,
  guide.md:460-467): a no-match non-fail stops the run; preferred-label and
  suggested-id routing pick only among unconditional edges.
- CHANGELOG.md: an "Unreleased" section summarising the 2026-10-04 plan's
  user-visible changes (worktree and branch per run, attempt commits,
  profiles, sessions, Pi, rate-limit window, prompt file, stripped env,
  failure fixes). No version bump (operator's call).

**Blocked by:** 14, 15, 16

**Status:** ready

- [ ] Each item above fixed or found already right
- [ ] `run_folder_doc` tests still pass
