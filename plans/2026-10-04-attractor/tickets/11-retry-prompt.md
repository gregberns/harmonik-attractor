# 11 (low priority): Retry prompt names the previous failure

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace` in the repo. Every check below runs with the shell-script fakes in temporary git repos; no real agent, API key or subscription.

**What to build:** When a node runs again after a failed attempt, the engine adds the previous attempt's failure class and reason to the prompt it assembles, e.g. "the previous attempt timed out after 600 s" or "the previous attempt failed: tests failed" (design.md §1 "Prompt on a re-run", Q55). Nothing else changes.

**Blocked by:** 08

**Status:** ready-for-agent

- [ ] The fake records its prompt: after a reported failure, the retry's prompt contains the class and the reason; after a timeout, it says it timed out
- [ ] A first attempt's prompt is unchanged from before this ticket
