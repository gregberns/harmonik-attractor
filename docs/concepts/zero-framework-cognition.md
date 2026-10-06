# Zero framework cognition in PAS

Adapted from harmonik-v3's `docs/concepts/zero-framework-cognition.md`
(Steve Yegge's principle): the framework contains no cognition. All
judgment lives in models and in the people who write graphs; all structure
lives in the engine. The engine is plumbing, never brain.

## The engine may

- **Walk the graph:** compile it, validate its structure, pick the next
  node by the rules the graph states (edge conditions, labels, weights).
- **Route on declared facts:** an outcome status, a failure class, an exit
  code, a context value, a label the graph declared and the agent echoed
  exactly.
- **Run processes:** start, time out, kill and record agents and tools.
- **Keep state:** checkpoints, the journal, the run folder, attempt
  commits.
- **Enforce policy:** deterministic rules such as `max_retries`, budgets,
  timeouts and goal gates.
- **Report failures:** classify how a process ended (reported, timeout,
  crash, no result, launch) and say so clearly. Never hide one.

## The engine must not

- Judge an agent's work: no scoring, no "looks done" heuristics, no
  keyword matching on free text to decide success.
- Decide what to do next in an ambiguous case: if the graph doesn't say,
  stop with a clear error; don't guess or fall back to some edge.
- Rewrite, summarise or rank prompts and outputs.
- Choose a model or agent by guessing difficulty; the graph or a profile
  names it.

## The test

Before adding logic to the engine, ask: is this a mechanical rule the
graph or config states, or does it need to understand meaning? Rules
belong in the engine; meaning belongs to an agent node or to the graph
author. For example, "retry a timed-out attempt up to `max_retries`" is a
rule; "retry if the output looks incomplete" is judgment, so an agent
node must decide it and report an outcome.
