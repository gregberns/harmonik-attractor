# 27: Rename: `HKA_*` env vars, state dirs and commit trailers

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`, becoming `harmonik-attractor`). Not harmonik-v3.

**Verify:** `cargo test --workspace --no-fail-fast`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**Design:** [plans/2026-10-27-rename/README.md](../../2026-10-27-rename/README.md) (operator-approved 2026-10-28). Rename outright: no `pas` alias, no reading old names. Crates keep `attractor-*`.

**What to build:**
1. Every `PAS_*` variable becomes `HKA_*`: the ones set for agents
   (`HKA_RUN_ID`, `HKA_NODE_ID`, `HKA_ATTEMPT`, ...), the ones read
   (`HKA_STATE_DIR`, `HKA_TRUST_THIS`, ...) and the test-only ones.
2. State and trust dirs: `$XDG_STATE_HOME/harmonik-attractor`
   (`~/.local/state/harmonik-attractor`), `$XDG_CONFIG_HOME/harmonik-attractor`.
3. Run commits: subject `hka(<run-id>): ...`, trailers `Hka-*`
   (`Hka-Status:`, `Hka-Attempt:`, ...). Anything that reads the
   trailers back (resume) reads the new ones.
4. The session-identity strip list (ticket 14) keeps working for the new
   names where it lists PAS's own.
5. Found after 26 (a grep of crates/*/src): the worktree lock
   `<git-dir>/pas-run.lock` (run_lock.rs) becomes
   `harmonik-attractor-run.lock`; the run branch prefix `pas/run/`
   (run_worktree.rs) becomes `hka/run/`; the journal field `pas_version`
   becomes `hka_version` (run-folder.md lists it); temp-file and stdin
   names (`pas-prompt-file-*`, `pas-stdin`); user-facing messages
   (`pas: trust this manifest?`, monitor `Cannot run pas`). Test fixtures
   and Rust identifiers (`pas_exe`) may stay unless touched anyway.

**Blocked by:** 26

**Status:** open

- [ ] A fake agent sees `HKA_RUN_ID` etc. and no `PAS_*` (seam 2)
- [ ] Resume after a kill still finds the interrupted attempt by its trailers
- [ ] `grep -rn 'PAS_\|Pas-\|pas-run\|pas/run\|pas_version' crates/*/src` finds only tests that check old names aren't read
