# 24: Directory mode: each phase starts from the previous phase's result

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace --no-fail-fast`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**What to build:** `pas run <dir>` runs each `.dot` file (a phase) in
lexical order, and `pas launch` generates one phase per spec. Since ticket
06 each phase is its own Run with a worktree from `--base`, so phase 2
doesn't see phase 1's edits (docs/cli-reference.md:93). Before 06 phases
shared one workdir, and `pas launch`'s phases build on each other.
Restore that:
1. Phase N+1's Run branches from phase N's final commit, not `--base`.
   Phase 1 still starts from `--base`.
2. A resumed batch keeps the same chain: a rerun resumes the stopped phase
   on its own branch, and the phases after it chain from where it ends.
3. Each phase is still its own Run (own journal, locks, branch).
4. Update docs/cli-reference.md (:93 and Directory mode) and the CHANGELOG.

**Blocked by:** none

**Status:** open

- [x] Phase 2's worktree has phase 1's committed edit (seam 2, fakes)
- [x] Phase 1 starts from `--base`
- [x] Stop during phase 2, rerun: phase 2 resumes, phase 3 chains from it
