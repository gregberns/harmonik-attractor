# PAS (harmonik-attractor): agent context

PAS is an Attractor: a pipeline engine that walks a DOT graph of agent and
tool nodes. Binary `pas`; Rust workspace; MIT OR Apache-2.0. This is the
operator's fork of citadelgrad/pascals-discrete-attractor, driven by
harmonik-v3 (hk3). `CLAUDE.md` is a symlink to this file; edit `AGENTS.md`.

## Rules for this file

Loaded into every agent session, so every line costs context. Only: what
the project is, rules to follow, and one-line pointers to `docs/`. Put
explanations in `docs/` and link them here. Keep it under 80 lines.

## Where the work comes from

- The plan: [plans/2026-10-04-attractor/](plans/2026-10-04-attractor/README.md)
  (`spec.md`, `design.md`, `CONTEXT.md` glossary, ticket table and status).
- Work comes from its `tickets/`, in order, one at a time. Build only what
  a ticket asks; no speculative options. Use the glossary's terms.
- No other tracker for our own work: no `bd`/beads, TodoWrite or TODO
  lists. (PAS's beads integration, docs/guide.md, is a product feature.)
- Until ticket 02a, codergen runs the local `claude` CLI (`claude -p`), with
  no API key; tests never do, they use the fakes.

## Build and test

```bash
cargo build --release                    # binary: target/release/pas
cargo test --workspace                   # all tests
cargo clippy --workspace --all-targets   # lint: no new warnings (baseline 1)
cargo fmt --all -- --check               # format check
```

All crates share one version in the root `Cargo.toml` (`[workspace.package]`);
crates use `version.workspace = true`. Never set a version in a crate.

## Programming rules

Pure core, imperative shell; detail in [docs/engineering.md](docs/engineering.md).
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
[docs/concepts/zero-framework-cognition.md](docs/concepts/zero-framework-cognition.md).

## Git workflow

One ticket at a time on its own branch from `main` (the crew shares one
checkout); only the integrator merges to `main`, after the full checks, and
pushes it. Builders may push ticket branches. See
[docs/git-workflow.md](docs/git-workflow.md).

## Docs

- [docs/dot-dialect.md](docs/dot-dialect.md): the DOT dialect; read before writing `.dot` files
- [docs/guide.md](docs/guide.md): pipeline patterns and handler dispatch
- [docs/cli-reference.md](docs/cli-reference.md): CLI commands, flags, environment
- [docs/task-verification.md](docs/task-verification.md): goal gates, edge routing, budget guards
- [docs/execution-capabilities.md](docs/execution-capabilities.md): what the engine supports and rejects
