# 04: Codex and Gemini as handler crates; remove LlmProvider

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace` in the repo. Every check below runs with the shell-script fakes in temporary git repos; no real agent, API key or subscription.

**What to build:** Today's Codex and Gemini commands, the Gemini `--help` format probe and their parsers move into their own handler crates (`codex-exec`, `gemini`), unchanged in behaviour (design.md §1, Q5, Q32). Built-in profiles `codex` and `gemini` reproduce today's argv; `llm_provider="codex"`/`"gemini"` map to them. Then the closed `LlmProvider` enum and the old provider code are deleted (contract). Adding a handler is one crate and one registry line.

**Blocked by:** 03

**Status:** ready-for-agent

- [ ] A fake `codex` (shell script printing Codex JSONL) covers success, `turn.failed` (reported failure) and crash through `pas run`, with the same outcomes as before this ticket
- [ ] A fake `gemini` answers the `--help` probe and covers success and failure, with the same outcomes as before
- [ ] Existing Codex and Gemini tests (handler and regression tests with recorded fixtures) pass against the new crates
- [ ] No reference to the `LlmProvider` enum remains; `cargo build` and the full suite are green
