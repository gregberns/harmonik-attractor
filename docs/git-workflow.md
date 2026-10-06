# Git workflow

Tickets are built in parallel, each in its own git worktree on its own
branch, and reach `main` one at a time through the integrator. The dev crew
follows it (workflow `build-branches`). Decisions Q61, Q63, Q64.

## Layout

- **Main checkout** (the repo root, where every crew member's session
  starts): stays on `main` with no tracked changes (hk3's `HANDOFF-*.md`
  and `.harmonik/` there are gitignored). Nobody switches its branch or
  edits tracked files in it; only the integrator updates it, by
  fast-forward.
- **Ticket worktree:** `.worktrees/<nn>-<slug>` on branch
  `ticket/<nn>-<slug>` (e.g. `.worktrees/02a-handler-crates`,
  `ticket/02a-handler-crates`). `.worktrees/` is gitignored. Everyone
  working on a ticket uses its path (`cd` into it, or `git -C <path>`).
- **Sub-worktrees** for a builder's subagents:
  `.worktrees/<nn>-<slug>--<part>` on `ticket/<nn>-<slug>--<part>`, cut
  from the ticket branch.
- Each worktree has its own `target/`, so parallel builds don't block each
  other; the first build in a new worktree is a full one.

## Steps

1. **Hand out.** The captain hands out every ticket whose blockers are
   merged (edges in [the plan README](../plans/2026-10-04-attractor/README.md)),
   one per free builder.
2. **Branch.** The builder creates the ticket worktree from the current
   `main`:
   `git worktree add .worktrees/<nn>-<slug> -b ticket/<nn>-<slug> main`.
3. **Build.** The builder commits small steps in the ticket worktree, each
   with the docs it changes. Every commit builds; every hand-off to review
   passes `cargo test --workspace`. Builders may push their ticket branch.
4. **Parallelise inside a ticket.** For independent pieces, the builder
   cuts sub-worktrees off the ticket branch (`git worktree add
   .worktrees/<nn>-<slug>--<part> -b ticket/<nn>-<slug>--<part>
   ticket/<nn>-<slug>`), points a subagent at each, then merges each part
   into the ticket branch and removes its sub-worktree and branch.
   Explicit sub-worktrees give a known base (the ticket branch); where the
   Agent tool's `isolation: worktree` branches from is untested.
5. **Review and test.** The code-reviewer and the tester work in the
   ticket worktree and send findings to the builder, who fixes them there.
6. **Integrate**, one ticket at a time, by the integrator only. In the
   ticket worktree:
   - rebase onto `main` (or merge `main` in when a rebase would be
     painful) and resolve every conflict. Parallel tickets that touch the
     engine (the plan names 02a/02b, 05 and 06) will conflict; resolving
     that is the integrator's job;
   - run `cargo test --workspace`, `cargo fmt --all -- --check` and
     `cargo clippy --workspace --all-targets` (no new warnings; see
     [engineering.md](engineering.md#testing));
   - mark the ticket done in `plans/2026-10-04-attractor/README.md` and
     commit that on the ticket branch;
   - fast-forward `main` in the main checkout: `git -C <repo root> merge
     --ff-only ticket/<nn>-<slug>` (`git fetch . <branch>:main` is refused
     while `main` is checked out there);
   - push `main`. Don't push the rebased ticket branch (its remote copy,
     if the builder pushed one, has diverged); it is deleted in clean-up,
     never force-pushed.
   If a check fails, the branch goes back to the builder.
7. **Clean up** (integrator), from the root checkout, since git won't
   remove the worktree you're in: `git -C <root> worktree remove
   .worktrees/<nn>-<slug>`, `git -C <root> branch -d ticket/<nn>-<slug>`
   (merged into `main`, so `-d` succeeds), `git -C <root> push origin
   --delete ticket/<nn>-<slug>` if it was pushed, and `git -C <root>
   worktree prune`.
8. **Next.** Tickets whose blockers are now merged branch from the updated
   `main`. A ticket still in progress picks up `main` at integration.

## Rules

- Nobody but the integrator updates `main`, and only by fast-forward.
- Never rewrite `main`'s history or force-push it. Rebasing your own
  ticket branch is fine.
- Pushing is allowed at any time (Q61): the integrator pushes `main` after
  each merge; builders may push ticket branches.
- The operator starts the crew from a plain terminal in the repo (Q64);
  see `.harmonik-v3/crews/dev.yaml`.
