# 32: An example role that gives an agent its own skills

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `harmonik-attractor`/`hka`). Not harmonik-v3.

**Verify:** `cargo test --workspace --no-fail-fast`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**Design:** [agent-setup.md](../agent-setup.md). Split from 30 (its item 3
and the skills part of its item 2).

**What to build:**
1. docs/examples: a `reviewer` role (a plugin dir with one skill, a
   settings file, the profile, a node using it).
2. cli-reference: a "give an agent its own skills" section, written for
   whatever default isolation operator item 17 settles.
3. CHANGELOG Unreleased.

**Blocked by:** 30, operator item 17 (the default isolation; under
`--safe-mode` Claude loads a `plugin_dirs` plugin but not its skills)

**Status:** open

- [ ] The example validates (`hka validate`)
- [ ] With the fake, the example's node gets the role's flags (seam 2)
