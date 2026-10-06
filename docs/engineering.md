# Engineering principles

How code and tests are written in this repo. AGENTS.md has the one-line
rules; this file explains them.

## Programming: functional core, imperative shell

The engine is easiest to trust when its decisions are pure and its effects
are few and visible. Rust supports this well; the rules adapt pure
functional programming to it.

- **Pure core.** Graph compilation, edge selection, outcome and failure
  mapping, prompt assembly and checkpoint transitions take values and
  return values. They read no files, spawn nothing, and don't look at the
  clock. They are tested with plain values, no setup.
- **Imperative shell.** Processes, the filesystem, git, environment and
  time sit at the edges, behind small traits (the agent handler trait, the
  process runner; a git trait if tickets 06-07 need one). The shell gathers inputs, calls the core,
  and carries out what the core returned. Dependencies are passed in, never
  constructed deep inside a function.
- **Immutable data.** Prefer owned values and new values over mutation.
  Mutate only local state inside a function. No global or shared mutable
  state; no interior mutability (`RefCell`, `Mutex`) unless concurrency
  requires it, and then keep it in the shell.
- **Total functions.** Every function returns for every input. In new and
  changed code: no `unwrap`, `expect`, `panic!` or indexing that can panic
  outside tests (existing code has many; don't rewrite it just for this);
  `match` exhaustively; avoid `_ =>` on enums you own, so a new variant
  forces every site to be reconsidered.
- **Errors as values.** Return `Result` with a typed error enum
  (`thiserror`); keep the cause; don't stringify early. A failure an
  agent or graph should route on is data (an outcome), not an error.
- **Make illegal states unrepresentable.** Enums over flags and options
  that must agree; newtypes for ids (`RunId`, `NodeId`); constructors that
  validate once.
- **Small composable functions.** One job each; iterators and combinators
  over index loops; no boolean parameters that switch behaviour.

## Testing

- **Seams.** New tests test behaviour at the two agreed seams (plan
  decision Q53); the existing tests use other seams and stay:
  1. the agent registry's `run` with a fake agent: start event, files,
     environment, failure table, kill;
  2. `pas run` end to end with a fake: routing, retries, commits in the
     worktree, stop and resume, run-folder contents.
  Pure core functions are also tested directly with values. Don't test
  private helpers or assert on call order.
- **Fakes, never real agents.** Shell-script fakes of `claude -p` and `pi`
  print scripted output for a named scenario. No test may need a real
  agent, an API key or a subscription.
- **Bug fixes start red.** Write the failing test that shows the bug, then
  fix it.
- **Deterministic.** Each test makes its own temp dir and temp git repo.
  Don't sleep to wait for something: wait on a file, a fifo, a process
  exit or a signal. Where a timeout is the behaviour under test, use the
  shortest one that still proves it.
- **Expected values come from outside the code**: a literal, a recorded
  fixture, the spec. Never recompute the expected value the way the code
  does.
- Before a merge to `main`: `cargo test --workspace` passes, `cargo fmt
  --all -- --check` passes, and `cargo clippy --workspace --all-targets`
  shows no new warnings. Baseline on 2026-10-05 (`50945da`): one warning,
  `attractor-monitor/src/controls.rs:104` (`nonminimal_bool`).
