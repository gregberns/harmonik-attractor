# Design: configure what each Claude agent starts with (item 15)

Status: approved by the operator 2026-10-28. Tickets 29-30.

## Goal

Like an hk3 role: a `reviewer` node starts Claude with the reviewer's skills
and settings, a `builder` node with the builder's, from `pas.toml` alone.

## How hk3 does it

hk3's `compose-role` merges `config/base.yaml`, the role's YAML and the
project's overlay, then launches Claude with `--settings <role>/settings.json`
(e.g. `disableBundledSkills`, `skillOverrides`) and `--plugin-dir <role>/plugin`,
a plugin it builds from the role's skill names.

## What PAS has today

- `[codergen.claude]` (settings mode, `settings_json`, `tools`, `agents_json`,
  `plugin_dirs`, `mcp_config_json`): one setup for every Claude node. Its
  flags are appended to the `claude` profile's `args`, so profiles that
  inherit from `claude` get them.
- A profile's `args`: any flag, but setting it replaces the whole list, so
  a `reviewer` that adds `--plugin-dir` this way silently loses
  `--safe-mode` and ticket 23's `--disallowedTools`.

## Proposal

New `claude-p` profile fields, resolved like every other field (a field the
profile sets replaces the parent's; `inherit_from` fills the rest):

| Field | Becomes | Default on `claude` |
|---|---|---|
| `isolation` | `--safe-mode` / `--bare` / `--setting-sources <list>` | `safe` (`--safe-mode`) |
| `settings` | `--settings <file or JSON>` | none |
| `plugin_dirs` | `--plugin-dir <dir>` each (skills live here) | none |
| `tools` | `--tools <list>` | none (Claude's default) |
| `disallowed_tools` | `--disallowedTools <a,b,...>` | ticket 23's list |
| `mcp_config` | `--mcp-config <file or JSON>` | none |

```toml
[agents.reviewer]
inherit_from = "claude"
plugin_dirs = ["agents/reviewer"]        # a Claude plugin with its skills
settings = "agents/reviewer/settings.json"
```

- `args` keeps only the base flags, so a profile adding skills no longer
  has to copy them and can't drop the isolation flags by accident.
- Relative paths resolve against the `pas.toml`'s folder, made absolute
  before the spawn (agents run in a worktree).
- Setting a field on a non-`claude-p` profile fails validation, naming it.
- `[codergen.claude]` stays and fills these fields on the `claude` profile
  (the CLI flags still override it), so nothing that works today breaks.
  The rename plan can drop it later.

## Left out (later, if wanted)

- hk3-style `skills = ["name"]` built into a plugin from a skills library:
  a plugin dir per role does the same with no build step.
- A role's system prompt (`--append-system-prompt`): the node's prompt
  already carries the instructions.

## Tickets once approved

1. The fields, their resolution and validation; move `--safe-mode` and the
   23 list from `args` into `isolation` and `disallowed_tools`.
2. `[codergen.claude]` folds into the fields; docs and an example role.
