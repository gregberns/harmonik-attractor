# 10 (low priority): Rate-limit retry window in the claude-p handler

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace` in the repo. Every check below runs with the shell-script fakes in temporary git repos; no real agent, API key or subscription.

**What to build:** A rate-limited Claude attempt is retried inside the handler for a short window before failing (design.md §5, Q48). Profile field `rate_limit_window` (default 2 minutes). A result with `is_error` and a `rate_limit_event` whose status isn't `allowed` (or a rate-limit error text) waits until `resetsAt` or a backoff, within the window, then re-spawns continuing the same session. Each wait is journalled as `LlmRateLimited {invocation_id, wait_s}`, and each re-spawn gets its own `LlmStarted` (`spawn` number), pid and `<inv>.<n>.jsonl`/`.stderr.log` files. After the window: `Failed(reported)` with "rate limited" in the detail. For Pi, set Pi's own retry in the generated `settings.json` to cover the window (read Pi's settings keys first).

**Blocked by:** 08, 09

**Status:** ready-for-agent

- [ ] The fake's "rate limited twice, then success" scenario completes, with two `LlmRateLimited` events, three `LlmStarted` events with distinct spawn numbers, and three transcript files
- [ ] "Always rate limited" with a short window fails with "rate limited" in the detail
- [ ] Re-spawns pass `--resume <session id>`
