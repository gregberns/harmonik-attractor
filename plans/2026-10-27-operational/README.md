# Operational: build plan

Follow-up to [2026-10-04-attractor](../2026-10-04-attractor/README.md), whose
13 tickets are merged. Goal: PAS is safe and reliable to run real agent
pipelines, driven by hk3. The work comes from an audit of main at 20aedd5
(2026-10-27): every ticket below was reproduced or confirmed in the code.

Same rules as before: one worktree and branch per ticket
([docs/git-workflow.md](../../docs/git-workflow.md)); every check runs with
the shell fakes, never a real agent. Design background stays in the
previous plan's [design.md](../2026-10-04-attractor/design.md) and
[CONTEXT.md](../2026-10-04-attractor/CONTEXT.md) glossary.

## Tickets

In priority order. The integrator marks a ticket done here when merging it.

| # | Ticket | Blocked by | Status |
|---|---|---|---|
| 13 | [Plan and AGENTS.md alignment](tickets/13-plan-and-docs-alignment.md) | none | done |
| 14 | [Never pass the parent session's identity to agents](tickets/14-strip-session-identity-env.md) | 13 | done |
| 15 | [A Codex run with no agent message is not a success](tickets/15-codex-no-hidden-success.md) | 13 | done |
| 16 | [`pas validate --workdir` reads the same pas.toml as run](tickets/16-validate-workdir.md) | 13 | done |
| 17 | [Docs match the code](tickets/17-docs-drift.md) | 14, 15, 16 | done |
| 18 | [Keep the session id when pas is killed mid-attempt](tickets/18-session-id-at-start.md) | 13 | done |
| 19 | [Small fixes: context order, cost warning, profile cycles](tickets/19-small-fixes.md) | 13 | done |
| 20 | [Flaky tests and test hygiene](tickets/20-flaky-tests.md) | 13 | done |
| 21 | [Strip the Gemini API keys like the others](tickets/21-strip-gemini-keys.md) | none | done |
| 22 | [Warn when a node sets `model` instead of `llm_model`](tickets/22-warn-node-model.md) | none | done |
| 23 | [Claude agents can't reach the operator's other sessions](tickets/23-no-cross-session-tools.md) | none | done |
| 24 | [Directory mode: each phase starts from the previous phase's result](tickets/24-chain-phases.md) | none | done |

## Waiting on the operator

These change how the product behaves, so they are not tickets yet. Each
"yes" becomes a ticket here.

1. **Directory mode chains phases?** Decided: yes (captain, 2026-10-28; it
   restores the pre-06 behaviour `pas launch` relies on). Ticket 24.
2. **A first real run.** Done 2026-10-27 at bd0592d: claude haiku, a tool
   check, one loop-back (the claude session resumed), codex, a final check;
   one commit per node, $0.003. Findings: ticket 22 and item 14. Not yet
   exercised: codex resume, gemini, pi. Before it: nothing has run with a real agent since the plan
   (only pre-plan code, in harmonik-v3 research/pas-spike). A hand-run smoke
   test (claude haiku + codex + a tool check + one loop-back; a few cents of
   subscription) would check what the fakes can't: each CLI accepts PAS's
   argv, codex resume with `--model`, commits after real edits.
3. **`env.remove` replaces the default strip list** (design decision 8;
   documented at docs/cli-reference.md:408). Adding one name re-admits the
   API keys. Making it additive needs an `env.keep` and breaks the
   codex-api recipe. Recommended: keep as is.
4. **Signed attempt commits.** Hooks are skipped by Q40; signing is off too.
   Only matters if run branches must be signed. Recommended: keep.
5. **Resume after a NoMatchingEdge or StillRetrying stop** says "Max
   retries exhausted". Repeat the original error, or route again?
6. **Loop-back after "No conversation found"** keeps resuming the dead
   session (no fallback, by design). Clear the id after a not-found?
7. **A broken profile no node uses fails every run.** Keep failing loudly,
   or check only the profiles in use?
8. **Leftover `pi-agent/` after a SIGKILL** (holds the API key, 0600).
   Clear leftovers when the Pi handler starts? (Out of scope listed
   "automatic orphan reaping".)
9. **Catch handler panics** (02b step 8): today pas exits 101, the agent's
   process group is killed, no `RunEnded`; a resume sees an interrupted
   attempt.
10. **03 step 8:** `allowed_tools`/`max_budget_usd` as profile templates
    instead of `extra_args`. Internal tidiness only; validation already
    rejects them on non-claude nodes.
11. **Version:** cut 0.12.0 for the plan's work (the CHANGELOG stops at
    0.11.0)?
12. **hk3 integration:** no harmonik-v3 code calls `pas` yet. That work
    lives in harmonik-v3.
13. **Strip the Gemini API keys?** Operator: yes, keep the providers in
    sync; Google isn't supported yet, so no `gemini-api` recipe. Ticket 21.
14. **Agents see the operator's Claude setup.** Checked with a wrapper:
    `--safe-mode` is passed and drops the operator's skills, but Claude's
    built-in session tools (`SendMessage`, `ListAgents`, `Workflow`) stay.
    Operator: blocking messaging is right. Ticket 23.
15. **Configure what each agent starts with, like hk3 roles.** hk3 gives
    each role a `--settings` file and a `--plugin-dir` of its skills.
    PAS can do this today only globally (`[codergen.claude]`: settings,
    tools, plugin dirs) or by rewriting a profile's whole `args`.
    Operator: wants it. Proposed: `settings`, `plugin_dirs`, `tools` and
    `disallowed_tools` as `claude` profile fields, so a `reviewer` profile
    (`inherit_from = "claude"`) carries its own skills. Design proposal:
    [agent-setup.md](agent-setup.md).
