# 16: `pas validate --workdir` reads the same pas.toml as run

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**What to build:** `pas validate` reads the current directory's pas.toml
(crates/attractor-cli/src/commands/validate.rs:63-74, no `--workdir` in
main.rs:210-220), while `pas run --workdir X` reads X's (run.rs:339).
Reproduced both ways: validate passes and run fails, and the reverse. Add
`--workdir` to `validate` (and to `info` if it resolves profiles the same
way) with the same meaning and default as `run`, so a check means the same
thing as the run it checks. Document it in docs/cli-reference.md.

**Blocked by:** 13

**Status:** ready

- [ ] Seam 2: from dir A (broken pas.toml), `pas validate --workdir B` passes and `pas run --workdir B` runs; from B, `validate --workdir A` fails with the same error `run --workdir A` gives
- [ ] Without `--workdir`, behaviour is unchanged
