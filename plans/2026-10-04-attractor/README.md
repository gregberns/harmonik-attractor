# Attractor: build plan

The build inputs for this repo's Attractor work: what to build and in which
order. This copy is the one the dev crew reads and updates (ticket status
below). The research and the operator's decisions stay in harmonik-v3:
[research](https://github.com/gregberns/harmonik-v3/blob/main/plans/2026-10-04-attractor/research/), [decisions.md](https://github.com/gregberns/harmonik-v3/blob/main/plans/2026-10-04-attractor/decisions.md),
[requirements.md](https://github.com/gregberns/harmonik-v3/blob/main/plans/2026-10-04-attractor/requirements.md).

Status: READY. Build tickets in number order, one ticket in flight at a
time (the crew shares one checkout; see
[docs/git-workflow.md](../../docs/git-workflow.md)).

- [spec.md](spec.md): what to build, testing decisions, out of scope, risks
- [design.md](design.md): the design, with code references and reasoning
- [CONTEXT.md](CONTEXT.md): glossary; use its terms in code and docs
- [tickets/](tickets/): one file per ticket

## Tickets

Tracer-bullet order. Every ticket is verified with `cargo test
--workspace` and the shell-script fakes; no real agent or subscription.
Tickets 10-12 are low priority. The integrator updates a ticket's status
here in the commit that merges it to main.

| # | Ticket | Blocked by | Status |
|---|---|---|---|
| 01 | [Fake-agent harness (digital twin of claude -p)](tickets/01-fake-agent-harness.md) | none | ready |
| 02a | [Agent handler interface, process runner and the claude-p handler](tickets/02a-handler-crates-claude-p.md) | 01 | ready |
| 02b | [Agent start events, live stderr and graceful kill](tickets/02b-agent-start-events-stderr-kill.md) | 02a | ready |
| 03 | [Agent profiles in config, test-only profiles and reasoning level](tickets/03-agent-profiles-config.md) | 02a | ready |
| 04 | [Codex and Gemini as handler crates; remove LlmProvider](tickets/04-port-codex-gemini.md) | 03 | ready |
| 05 | [Clear run-ending errors and the two failure-hiding fixes](tickets/05-failure-reporting-and-hidden-failures.md) | 02b | ready |
| 06 | [A worktree and branch per run](tickets/06-worktree-and-branch.md) | 01 | ready |
| 07 | [Attempt commits, interrupted attempts and the end-of-run report](tickets/07-attempt-commits-and-end-of-run.md) | 05, 06 | ready |
| 08 | [Continue a node's agent session on retry and loop-back](tickets/08-sessions-continue.md) | 04, 07 | ready |
| 09 | [The Pi handler for DeepSeek, GLM and the hosted Qwen](tickets/09-pi-handler.md) | 08 | ready |
| 10 | [(low) Rate-limit retry window in the claude-p handler](tickets/10-rate-limit-window.md) | 08, 09 | ready |
| 11 | [(low) Retry prompt names the previous failure](tickets/11-retry-prompt.md) | 08 | ready |
| 12 | [(low) Prompt file and run-folder documentation](tickets/12-prompt-file-and-run-folder-doc.md) | 07 | ready |
