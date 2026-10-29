# PAS (harmonik-attractor): agent context

PAS is an Attractor: a pipeline engine that walks a DOT graph of agent and
tool nodes. Binary `pas`; Rust workspace; MIT OR Apache-2.0. This is the
operator's fork of citadelgrad/pascals-discrete-attractor, driven by
harmonik-v3 (hk3). `CLAUDE.md` is a symlink to this file; edit `AGENTS.md`.

## Rules for this file

Loaded into every agent session, so every line costs context. Only: what
the project is, rules to follow, and one-line pointers to `docs/`. Put
explanations in `docs/` and link them here. Keep it under 80 lines, each
line under 80 characters. No transient state: no ticket numbers, dates,
statuses or "until X" notes; those belong in the plan.

## Where the work comes from

- The active plan is named in [plans/README.md](plans/README.md); its
  README links the ticket table, `tickets/` and the glossary (`CONTEXT.md`).
- Build only what a ticket asks; use the glossary's terms.
- No other tracker for our own work: no `bd`/beads, TodoWrite or TODO
  lists. (PAS's beads integration, docs/guide.md, is a product feature.)

## Build and test

```bash
cargo build --release                    # binaries harmonik-attractor, hka
cargo test --workspace                   # all tests
cargo clippy --workspace --all-targets   # lint: no new warnings (baseline 1)
cargo fmt --all -- --check               # format check
```

All crates use the root `Cargo.toml` version (`version.workspace = true`);
never set a version in a crate.

## Programming rules

Pure core, imperative shell ([docs/engineering.md](docs/engineering.md)).
- Decisions (graph compilation, routing, outcome and failure mapping) are
  pure functions over immutable data.
- Side effects (processes, files, git, clock) live at the edges behind
  traits, injected, never created deep inside.
- Total functions in new and changed code: no `unwrap`/`expect`/`panic!`
  outside tests; exhaustive `match`; errors are typed values (`Result`).
  Don't rewrite existing code just for this.
- Small composable functions; newtypes for ids; no shared mutable state.

## Testing rules

Detail in [docs/engineering.md](docs/engineering.md#testing).
- New tests go through the two public seams: the agent registry's `run`
  with a fake, and `pas run` end to end with a fake.
- No real agent, API key or subscription in any test: use the shell fakes.
- Every bug fix starts with a failing test.
- Deterministic tests: no wall-clock sleeps where a file, fifo or signal
  can be waited on; temp dirs and temp git repos only.

## Zero framework cognition

The engine provides structure (graph, routing, commits, process control)
and never judgment; judgment belongs to agents and graph authors. Read
[zero-framework-cognition.md](docs/concepts/zero-framework-cognition.md).

## Git workflow

Each ticket in its own worktree, `.worktrees/<nn>-<slug>` on branch
`ticket/<nn>-<slug>` from `main`; tickets run in parallel. The root checkout
stays on a clean `main`: never switch or edit it. Only the integrator merges
to `main` (fast-forward), then pushes it and removes the worktree. See
[docs/git-workflow.md](docs/git-workflow.md).

## Docs

- [dot-dialect.md](docs/dot-dialect.md): read before writing `.dot` files
- [guide.md](docs/guide.md): pipeline patterns and handler dispatch
- [cli-reference.md](docs/cli-reference.md): commands, flags, environment
- [task-verification.md](docs/task-verification.md): goal gates, routing
- [execution-capabilities.md](docs/execution-capabilities.md): what's rejected
- [run-folder.md](docs/run-folder.md): every run file and journal event
