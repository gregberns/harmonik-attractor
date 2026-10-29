# 30: `[codergen.claude]` fills the new fields; docs and an example role

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `harmonik-attractor`/`hka` after 25). Not harmonik-v3.

**Verify:** `cargo test --workspace --no-fail-fast`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**Design:** [agent-setup.md](../agent-setup.md) (operator-approved 2026-10-28). Config is `.harmonik-attractor/config.toml` after 26.

**What to build:**
1. `[codergen.claude]` and its `--codergen-claude-*` CLI flags set the
   new fields on the `claude` profile instead of appending raw args (the
   CLI still overrides the config). Profiles inheriting `claude` keep
   getting them.
2. docs/cli-reference.md: the fields in the profile table, a "give an agent
   its own skills" section, and the `[codergen.claude]` section pointing
   at the fields.
3. docs/examples: a `reviewer` role (a plugin dir with one skill, a
   settings file, the profile, a node using it).
4. CHANGELOG Unreleased.

**Blocked by:** 29

**Status:** open

- [ ] Every existing `[codergen.claude]` and `--codergen-claude-*` test still passes
- [ ] The example validates (`hka validate`)
