# Git workflow

Every ticket is built on its own branch and reaches `main` through one
member, the integrator. The dev crew follows it (workflow
`build-branches`).

**One ticket in flight at a time.** hk3 gives the crew one shared checkout,
so a branch switch changes the files under everyone. Only the builder
(cutting a branch) and the integrator (merging) change branches, and only
when the reviewer and tester are idle.

1. **Branch.** The builder cuts `ticket/<nn>-<slug>` (e.g.
   `ticket/02a-handler-crates`) from the current `main`, and may push it.
2. **Build.** The builder commits small steps on that branch, each with
   the docs it changes. Every commit builds; every hand-off to review
   passes `cargo test --workspace`.
3. **Review and test.** The code-reviewer and the tester work on the same
   branch, in the same checkout, and send findings to the builder, who
   fixes them there.
4. **Integrate.** When the tester passes the branch, the integrator, and
   only the integrator, on the ticket branch:
   - rebases it onto `main` and resolves every conflict (with one ticket
     in flight, `main` has usually not moved);
   - runs `cargo test --workspace`, `cargo fmt --all -- --check` and
     `cargo clippy --workspace --all-targets` (no new warnings; see
     [engineering.md](engineering.md#testing));
   - marks the ticket done in `plans/2026-10-04-attractor/README.md` and
     commits that on the branch;
   - fast-forwards `main` without switching the checkout: `git fetch .
     HEAD:main`;
   - pushes `main` to GitHub, then deletes the ticket branch (locally and,
     if pushed, on GitHub).
   If a check fails, the branch goes back to the builder.
5. **Next ticket.** The builder checks out `main` and cuts the next branch.

Rules:
- Nobody but the integrator updates `main`.
- Never rewrite `main`'s history or force-push it.
- Pushing is allowed at any time (operator decision Q61): the integrator
  pushes `main` after each merge; builders may push ticket branches.
