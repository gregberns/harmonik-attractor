# Rename: a harmonik name for PAS (draft)

Status: planning. No tickets until the operator picks a name.

The operator doesn't want the tool called `pas`; it is a tool of harmonik
(hk3 drives it). This plan lists what a rename touches so it can be cut
into tickets once the name is chosen.

## Name

Candidates (none is on this machine's PATH today):
- `hka`: harmonik attractor; matches the repo name and hk3's `hka-*` labels.
- `hkp`: harmonik pipeline.

Pick one before any ticket. The name is also the run folder (`.<name>/`),
the config file (`<name>.toml`) and the env prefix (`<NAME>_*`).

## What a rename touches

Counted on main at 3fc3820 (files in crates/, docs/, README, AGENTS.md):

| Surface | Today | Files |
|---|---|---|
| Binary and CLI help | `pas` | ~144 |
| Env vars given to agents and read by pas | `PAS_*` | ~55 |
| Run folder | `.pas/` | ~28 |
| Project config | `pas.toml` | ~37 |
| Crate names | `attractor-*` | ~78 |

## Open choices (recommendations)

1. **Crates keep `attractor-*`.** They name the Attractor spec they
   implement, not the product; renaming them is churn with no user effect.
2. **No compatibility shims.** No released users besides hk3, which
   doesn't call pas yet: rename outright, no `pas` alias, no reading of an
   old `pas.toml` or `.pas/`.
3. **One ticket per surface**, binary first, then config and run folder,
   then env vars, then docs; or one ticket if the diff stays mechanical.
4. **Do it before hk3 integration** (operational plan, item 12), so hk3
   is written against the final name.
