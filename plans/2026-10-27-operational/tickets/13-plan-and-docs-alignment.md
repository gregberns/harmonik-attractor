# 13: Plan and AGENTS.md alignment

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**What to build:** Docs only. A plans index (`plans/README.md`) naming the
active plan, so AGENTS.md, docs/git-workflow.md and the build-branches
workflow point at the index instead of a dated plan. The finished plan's
README records its two deferred steps. AGENTS.md loses its transient
"Until ticket 02a" line, gains a rule that it holds no transient state, and
has no line over 80 characters.

**Blocked by:** none

**Status:** done

- [ ] `plans/README.md` lists both plans; the new plan has its ticket table
- [ ] 2026-10-04-attractor README: 03 shows its deferred step 8; links the follow-up plan
- [ ] AGENTS.md has no ticket numbers, dates or statuses, and no line over 80 characters
- [ ] AGENTS.md, docs/git-workflow.md and .harmonik-v3/workflows/build-branches.yaml point at `plans/README.md`
