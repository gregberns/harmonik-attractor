# 21: Strip the Gemini API keys like the others

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace --no-fail-fast`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**What to build:** The default `env.remove`
(crates/attractor-agent-handler/src/agents.toml) strips the Anthropic and
OpenAI keys, so claude and codex bill the subscription, but not
`GEMINI_API_KEY` or `GOOGLE_API_KEY`. Google isn't a supported provider
yet; this only keeps the three providers consistent.
1. Add `GEMINI_API_KEY` and `GOOGLE_API_KEY` to the default strip list.
2. Keep the `codex-api` recipe in docs/cli-reference.md in sync: it lists
   the default list minus `OPENAI_API_KEY`, so it gains the two names.
3. Update every other doc that lists the stripped names (grep for
   `CLAUDE_CODE_USE_VERTEX`). No `gemini-api` recipe: not supported yet.
4. Note it under the CHANGELOG's Unreleased section.

**Blocked by:** none

**Status:** open

- [x] A fake agent run with both keys set sees neither (seam 2)
- [x] A profile with its own `env.remove` still replaces the list (unchanged)
- [x] The `codex-api` recipe matches the default list minus `OPENAI_API_KEY`
