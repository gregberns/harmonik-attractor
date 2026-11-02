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

- [x] Every existing `[codergen.claude]` and `--codergen-claude-*` test still passes

**Notes:**
- The `settings_mode` → `isolation` fold landed in 29 (a boundary call the
  captain accepted), so `--safe-mode` is never passed twice.
- `settings_json`, `tools`, `plugin_dirs` and `mcp_config_json` now set
  the profile fields; a profile setting the same field replaces it and
  passes the flag once
  (`a_profiles_own_settings_win_over_codergen_claude_with_one_flag`).
  `agents_json` has no field and stays in `args` as `--agents`.
- `execute_resolved`/`execute` (and `claude_settings_args`) are reached
  only from tests and the public library API; the engine calls
  `execute_configured` (`engine.rs:673`). The compatibility path builds
  the same flags from a `ClaudeSetup`.
- Item 3 (the example `reviewer` role), with item 2's "give an agent its
  own skills" section, moved to ticket 32 (the default isolation is
  operator item 17: with `--safe-mode`, Claude loads a plugin from
  `plugin_dirs` but not its skills).
- `docs/emergence-analysis.md` has a Historical design banner: "This
  analysis describes the engine as of 2026-02 (PAS)."
