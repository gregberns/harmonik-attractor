# 25: Rename: the binary is `harmonik-attractor`, alias `hka`

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`, becoming `harmonik-attractor`). Not harmonik-v3.

**Verify:** `cargo test --workspace --no-fail-fast`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**Design:** [plans/2026-10-27-rename/README.md](../../2026-10-27-rename/README.md) (operator-approved 2026-10-28). Rename outright: no `pas` alias, no reading old names. Crates keep `attractor-*`.

**What to build:**
1. crates/attractor-cli/Cargo.toml: `[[bin]]` `harmonik-attractor` and a
   second `[[bin]]` `hka`, both `src/main.rs`, so `cargo install` puts
   both on the PATH.
2. Help, usage and version text name the program as typed
   (`harmonik-attractor` or `hka`); error and hint messages that say
   `pas <command>` say `hka <command>`.
3. Tests that run the binary (`CARGO_BIN_EXE_pas`, ~27 files) use the new
   name; add one test that `hka --version` works.
4. AGENTS.md's build line (`target/release/pas`). Other docs are ticket 28.

**Blocked by:** none

**Status:** open

- [ ] `cargo build --release` makes `harmonik-attractor` and `hka`, no `pas`
- [ ] No user-facing message says `pas ` (grep the CLI crate's strings)
