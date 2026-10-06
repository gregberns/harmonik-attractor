# 09: The Pi handler for DeepSeek, GLM and the hosted Qwen

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace` in the repo. Every check below runs with the shell-script fakes in temporary git repos; no real agent, API key or subscription.

**What to build:** A `pi` handler crate, the multi-model handler (design.md §1 "The Pi handler", §2 profiles, Q33, Q34, Q52). It runs whatever `pi` is installed: `pi --mode json --model <provider/id> [--thinking <level>] --session-dir <run folder>/pi-sessions --session-id <id> <prompt>`, stdin `/dev/null`, with a per-invocation `PI_CODING_AGENT_DIR` holding an engine-written `models.json` (provider, `baseUrl`, `api: openai-completions`, model id and limits, the API key read from the profile's `api_key_env`; file mode 0600) and `settings.json`, both in the run folder (never in the worktree) and removed when the invocation ends, plus `PI_TELEMETRY=0`, `PI_OFFLINE=1`, `PI_SKIP_VERSION_CHECK=1`. Failure: the last assistant `message_end` has `stopReason` `error` or `aborted`, or there is no final assistant message (json mode exits 0 even on provider errors). New and continued sessions are the same command. Profile fields `provider`, `base_url`, `api_key_env`, `limits`; example profiles `deepseek`, `glm` (Z.ai pay-as-you-go URL) and `qwen` (endpoint TBD).

**Blocked by:** 08

**Status:** ready-for-agent

- [ ] A fake `pi` (shell script, always exits 0) covers: `stop` → Completed with that text; `error` with `errorMessage` → reported failure with the message in the detail; `aborted` → reported failure; no final message → no-result failure
- [ ] The fake checks that `PI_CODING_AGENT_DIR/models.json` exists with the profile's provider, base URL, model and key, is mode 0600, and that the three `PI_*` variables are set; the key is not on argv
- [ ] After the invocation, no `models.json` holding the key remains, and none is ever in the worktree or a commit
- [ ] A retry continues: the same `--session-id` is passed both times
- [ ] `pas validate` rejects a pi profile missing `provider` or `model`
- [ ] The three example profiles load and validate
