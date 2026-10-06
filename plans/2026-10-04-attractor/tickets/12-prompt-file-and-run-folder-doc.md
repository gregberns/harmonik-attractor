# 12 (low priority): Prompt file and run-folder documentation

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace` in the repo. Every check below runs with the shell-script fakes in temporary git repos; no real agent, API key or subscription.

**What to build:** Each invocation also writes `transcripts/<inv>.prompt.txt` (the prompt, the argv, and the environment variable names without values) before the agent starts (design.md §4). A `docs/run-folder.md` in the fork lists every file and event in the run folder, its format, when it is written and its version field, including `LlmStarted`, the stderr file and `final.json`, and states that the run folder is observability and the attempt commits are the record.

**Blocked by:** 07

**Status:** ready-for-agent

- [ ] After a run with the fake, each invocation has a `prompt.txt` containing the prompt and argv, and no environment values (a secret set in the env is absent from the file)
- [ ] `docs/run-folder.md` exists and every file a test run creates is listed in it (a test lists the run folder and checks each name against the doc)
