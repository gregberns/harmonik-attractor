# 26: Rename: `pas.toml` and `.pas/` become `.harmonik-attractor/`

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`, becoming `harmonik-attractor`). Not harmonik-v3.

**Verify:** `cargo test --workspace --no-fail-fast`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**Design:** [plans/2026-10-27-rename/README.md](../../2026-10-27-rename/README.md) (operator-approved 2026-10-28). Rename outright: no `pas` alias, no reading old names. Crates keep `attractor-*`.

**What to build:**
1. The project config moves from `pas.toml` to
   `.harmonik-attractor/config.toml` (every reader: run, validate, info,
   trust, launch, init).
2. The run folder moves from `.pas/` to `.harmonik-attractor/` (logs,
   worktrees, anything else written there). The `.gitignore` PAS writes
   (`ensure_pas_gitignore`) ignores the run data but never `config.toml`,
   so the config stays committable.
3. `init` writes the new layout. The repo's own `.gitignore` swaps `.pas/`
   for the new folder's run data.

**Blocked by:** 25

**Status:** open

- [ ] A run with `.harmonik-attractor/config.toml` reads it; a lone `pas.toml` is ignored
- [ ] After a run, `git status` in the project shows no run data, and `config.toml` is still tracked
