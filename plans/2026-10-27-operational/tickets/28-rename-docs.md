# 28: Rename: docs, README, AGENTS.md and examples

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`, becoming `harmonik-attractor`). Not harmonik-v3.

**Verify:** `cargo test --workspace --no-fail-fast`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**Design:** [plans/2026-10-27-rename/README.md](../../2026-10-27-rename/README.md) (operator-approved 2026-10-28). Rename outright: no `pas` alias, no reading old names. Crates keep `attractor-*`.

**What to build:** Every doc says `harmonik-attractor`/`hka`,
`.harmonik-attractor/config.toml`, `.harmonik-attractor/` and `HKA_*`:
docs/, README, AGENTS.md (keep its 80-line, 80-column limits), examples,
templates and `.dot` prompts that mention the old names. The project is
"harmonik-attractor" in prose; "PAS" goes. CHANGELOG Unreleased: one entry
listing every renamed surface. Leave finished plans and the CHANGELOG's
history as they are.

**Blocked by:** 27

**Status:** open

- [ ] `grep -rnw 'pas\|PAS' docs README.md AGENTS.md` finds only history
- [ ] The doc-pinning tests still pass
