# 07: Attempt commits, interrupted attempts and the end-of-run report

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace` in the repo. Every check below runs with the shell-script fakes in temporary git repos; no real agent, API key or subscription.

**What to build:** The engine commits after every attempt and reports the run's result (design.md §3-4, Q16, Q29, Q38-40, Q43-44). After each attempt, in the attempt loop before the result is matched: `git add -A -- . ':(exclude).pas'` and `git commit --allow-empty --no-verify` with message `pas(<run-id>): <node> attempt <n> (<status>)` and trailers `Pas-Run`, `Pas-Node`, `Pas-Attempt`, `Pas-Status`, `Pas-Failure-Class` (on failure); a fixed identity when the repo has none. On resume, if the checkpoint shows an attempt in progress and the worktree has changes, commit them with `Pas-Status: interrupted` and add to the agent's prompt that its previous attempt was interrupted and to review `git diff <last attempt commit>`. Success removes the worktree and keeps the branch; failure keeps both. `pas run --json` and a new `final.json` report `branch`, `base`, `final_commit`, `status` and, on failure, ticket 05's error.

**Blocked by:** 05, 06

**Status:** ready-for-agent

- [ ] A three-node run with the fake leaves three commits on the run branch with correct trailers; a retried node leaves one commit per attempt, failed ones included
- [ ] The fake's own commit during an attempt is kept, with the engine's commit on top
- [ ] A `.pas/` directory created inside the worktree is never committed
- [ ] A repo with no `user.name`/`user.email` still gets attempt commits
- [ ] A target repo with a failing pre-commit hook does not block attempt commits
- [ ] Kill `pas` (SIGKILL) during the slow-stream scenario after the fake writes a file, then re-run: an `interrupted` commit holds that file, the node re-runs on top, and the fake's recorded prompt contains the interrupted note (teardown kills the fake's process group, which can outlive `pas`)
- [ ] Success: the worktree directory is gone, the branch remains, and `final.json` and `--json` output match the branch's tip
- [ ] Failure: worktree and branch remain, and `final.json` holds the error
