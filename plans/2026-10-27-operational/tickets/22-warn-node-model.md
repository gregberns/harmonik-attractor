# 22: Warn when a node sets `model` instead of `llm_model`

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace --no-fail-fast`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**What to build:** Found in the first real smoke test. A node written
`write [agent="claude" model="haiku"]` validates and runs on the profile's
default model: `model` is a graph attribute (docs/dot-dialect.md:311), and
the node attribute is `llm_model` (:212). Nothing tells the author.
1. `pas validate` and `pas run` warn (a lint, not an error) when a node
   sets `model`: name the node and suggest `llm_model`.
2. Mention it next to `llm_model` in docs/dot-dialect.md.

Don't start accepting `model` on nodes; that would change the dialect.

**Blocked by:** none

**Status:** done

- [x] A node with `model="haiku"` gets the warning in validate and run
- [x] A node with `llm_model` and a graph with `model` get no warning
