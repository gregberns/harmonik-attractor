# 02b: Agent start events, live stderr and graceful kill

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace` in the repo. Every check below runs with the shell-script fakes in temporary git repos; no real agent, API key or subscription.

**What to build:** The process runner from 02a gains what a watcher needs and a gentler kill (design.md §1 invariants, §4, Q20). A new journal event `LlmStarted` (invocation, spawn, node, attempt, profile, model, host, pid, pgid, transcript, stderr) is written after spawn and before the agent's first output. The agent's stderr is written live to `transcripts/<inv>.stderr.log` (today it is kept only in memory). Timeout and cancel send TERM to the agent's process group, wait the grace period (default 10 s, from the profile once ticket 03 lands), then KILL; the drop guard stays as the backstop.

**Blocked by:** 02a

**Status:** ready-for-agent

- [ ] `LlmStarted` appears in `events.jsonl` before the first transcript line, with the fake's pid and pgid and the transcript and stderr paths
- [ ] The fake's stderr lines appear in `<inv>.stderr.log` while it runs (slow stream scenario), and stay after a crash
- [ ] Hang: the fake receives TERM first (a trap writes a marker file), exits on it, and the run reports the timeout; a fake that ignores TERM is killed after a short grace period
- [ ] Dropping the run (stop) still kills the fake's process group
