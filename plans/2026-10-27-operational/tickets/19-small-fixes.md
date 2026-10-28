# 19: Small fixes: context order, cost warning, profile cycles

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**What to build:** Three small, independent fixes:
1. The "Context from prior pipeline steps" lines come from a `HashMap`
   (attractor-types/src/context.rs:66; codergen_handler.rs:375-388), so their
   order changes between runs and prompts aren't reproducible. Sort them by key.
2. The preflight "no cost tracking" warning is keyed on the profile names
   `codex`/`gemini` (attractor-pipeline/src/preflight.rs:244). Key it on the
   profile's handler, so `codex-api` (inherits codex) warns and a profile
   named `codex` that uses claude-p doesn't.
3. An `inherit_from` cycle says "inherits from itself" even for `a→b→a`
   (agent-handler config.rs:160, registry.rs:163). Name the chain:
   "agent profile a: inherit_from cycle a → b → a".

**Blocked by:** 13

**Status:** ready

- [ ] Two prior-step outputs give the same prompt text on every run
- [ ] The warning follows the handler, both ways
- [ ] The cycle error prints the chain, for a 1- and a 2-profile cycle
