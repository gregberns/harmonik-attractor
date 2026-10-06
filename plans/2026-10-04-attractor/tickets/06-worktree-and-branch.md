# 06: A worktree and branch per run

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace` in the repo. Every check below runs with the shell-script fakes in temporary git repos; no real agent, API key or subscription.

**What to build:** Every run works in its own git worktree on its own branch (design.md §3, Q15, Q17, Q36, Q37). Branch `pas/run/<run-id>` from `--base <ref>` (default `HEAD`); worktree at `<worktree_root>/<run-id>`, default `<project-root>/.pas/worktrees/<run-id>`, settable in `pas.toml` and with `--worktree-root`. A dirty source checkout starts anyway with a warning in `RunStarted` and `run.json`. pas writes `.pas/.gitignore` containing `*` in each `.pas/` it creates. Order in run preparation: Pipeline lock, then checkpoint or new run id, then create or reuse the worktree, then the Worktree lock. Resume reuses the same worktree. `run.json` records worktree, branch and base.

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] A run in a scratch repo creates `.pas/worktrees/<run-id>` on branch `pas/run/<run-id>` at the base commit; the fake's edits land there and the main checkout is untouched
- [ ] `git status` in the main checkout shows nothing from `.pas/` after a run
- [ ] `--base` and `--worktree-root` (and the `pas.toml` setting) are honoured
- [ ] A dirty main checkout: the run starts, the warning is in `run.json` and the journal, and the uncommitted changes are not in the worktree
- [ ] Two concurrent `pas run` of the same pipeline: the second is refused before creating a worktree or branch
- [ ] Stop mid-run, then re-run the same command: it resumes in the same worktree with the earlier edits present
