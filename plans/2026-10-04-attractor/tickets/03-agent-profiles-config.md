# 03: Agent profiles in config, test-only profiles and reasoning level

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace` in the repo. Every check below runs with the shell-script fakes in temporary git repos; no real agent, API key or subscription.

**What to build:** Agents are chosen by named profiles (design.md §2, Q13). A node names `agent="<profile>"`; a project overrides or adds profiles in `pas.toml` `[agents.*]` over the built-in defaults (whole profiles by name, `inherit_from` allowed). Profile fields: `mechanism`, `command`, `args`, `model`, `model_args`, `reasoning`, `reasoning_args`, `timeout`, `kill_grace`, `env.remove`, `env.set` (applied after `remove`), `test_only`. 02a's built-in strip list becomes the default `env.remove`; `kill_grace` sets the grace period 02b's graceful kill uses. `llm_model` and `reasoning_effort` on a node override the profile; `reasoning_effort` is no longer rejected. `[codergen.claude]` in `pas.toml` keeps working, read into the `claude` profile's `args`. A `test_only` profile is refused unless `pas run --allow-test-agents` is given. `pas validate` checks every profile a pipeline uses.

**Blocked by:** 02a

**Status:** ready-for-agent

- [ ] A `fake` profile (`test_only`, `command` = the fake script) runs ticket 01's success scenario with `--allow-test-agents`, and is refused with a clear message without it; from here on tests select the fake by profile instead of `PATH`
- [ ] A node with `reasoning_effort="high"` on a profile with `reasoning_args = ["--effort", "{reasoning}"]` passes `--effort high` to the fake (the fake records its argv)
- [ ] `llm_model` overrides the profile's model in the argv
- [ ] A profile's `env.set` key reaches the fake even when it is in the `remove` list
- [ ] An unknown profile, or `reasoning_effort` on a profile without `reasoning_args`, fails `pas validate` naming the node
- [ ] From here on tests select fakes by profile; tickets 01-02b selected them via `PATH`
- [ ] A pipeline using `llm_provider="claude"` and a `pas.toml` with `[codergen.claude]` settings produces the same argv as before this ticket
