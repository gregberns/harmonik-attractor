# 29: Claude agent setup fields: isolation, settings, skills, tools

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `harmonik-attractor`/`hka` after 25). Not harmonik-v3.

**Verify:** `cargo test --workspace --no-fail-fast`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**Design:** [agent-setup.md](../agent-setup.md) (operator-approved 2026-10-28). Config is `.harmonik-attractor/config.toml` after 26.

**What to build:** New `claude-p` profile fields, resolved like every
other field (set replaces the parent's whole value; `inherit_from` fills
the rest): `isolation` (`safe` | `bare` | `inherit` with
`setting_sources`), `settings`, `plugin_dirs`, `tools`,
`disallowed_tools`, `mcp_config`.
1. Each becomes its flag (table in the design). `disallowed_tools` is one
   comma-separated value (the flag is variadic).
2. The built-in `claude` profile: `isolation = "safe"` and ticket 23's
   list as `disallowed_tools`; its `args` keep only the base flags. The
   argv a plain `claude` node gets doesn't change (order aside).
3. Relative paths resolve against the project root (the folder holding
   `.harmonik-attractor/`), made absolute before the spawn.
4. A field on a non-`claude-p` profile fails validation, naming it.

**Blocked by:** 28

**Status:** open

- [ ] A `reviewer` inheriting `claude` with `plugin_dirs` and `settings` gets those flags plus `--safe-mode` and `--disallowedTools` (seam 2)
- [ ] A profile setting `disallowed_tools = []` gets no `--disallowedTools`
- [ ] Relative paths arrive absolute; a codex profile with `plugin_dirs` fails validation
