# Rename: `pas` becomes `harmonik-attractor` (alias `hka`)

Status: proposal, waiting on the operator's OK of the layout below. No
tickets yet.

The operator's call (2026-10-28): the tool is `harmonik-attractor`, with
`hka` as a short alias. Agents mostly start it, so the long name is fine.

## How hk3 does it

- The executable is the full name, `harmonik-v3`; README step 2 links it
  as `hk3` on the PATH (`ln -s .../harmonik-v3 ~/.local/bin/hk3`).
- Per-project state lives in one folder, `.harmonik-v3/` (`config.env`,
  `build/`, `crews/`, `teams/`, `workflows/`).
- Its env vars use the short name: `HK3_*`.

## Proposed layout

Same split: the long name for files, the short one for typing.

| Surface | Today | Proposed |
|---|---|---|
| Binary | `pas` | `harmonik-attractor`, and `hka` |
| Project config | `pas.toml` | `.harmonik-attractor/config.toml` |
| Run folder (logs, worktrees) | `.pas/` | `.harmonik-attractor/` (git-ignored parts) |
| Env vars | `PAS_*` (`PAS_STATE_DIR`, `PAS_TRUST_THIS`, the agent ids) | `HKA_*` |
| State and trust store | `~/.local/state/pas`, `~/.config/pas` | `.../harmonik-attractor` |
| Commit trailers, run commit subject | `Pas-Status:`, `pas(<run>):` | `Hka-Status:`, `hka(<run>):` |
| Crates | `attractor-*` | unchanged |

`hka` as a second `[[bin]]` of the same `main.rs`, so `cargo install` puts
both on the PATH with no manual symlink step; help text shows whichever
name was typed.

## Recommendations

1. **Crates keep `attractor-*`.** They name the Attractor spec they
   implement; renaming them is churn with no user effect.
2. **No compatibility shims.** Nobody but us runs it, and hk3 doesn't call
   it yet: no `pas` alias, no reading an old `pas.toml` or `.pas/`.
3. **Before hk3 integration** (operational plan, item 12), so hk3 is written
   against the final names.
4. **Tickets:** binary and help text; config and run folder; env vars,
   state dirs and trailers; docs and README. Each leaves the suite green.

Counted on main at 3fc3820 (files in crates/, docs/, README, AGENTS.md):
`pas` ~144, `PAS_` ~55, `.pas/` ~28, `pas.toml` ~37.
