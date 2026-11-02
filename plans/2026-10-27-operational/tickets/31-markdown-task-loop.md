# 31: A reusable task-loop pipeline over a markdown task list

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `harmonik-attractor`/`hka`). Not harmonik-v3.

**Verify:** `cargo test --workspace --no-fail-fast`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (baseline 1 warning). Checks use the shell fakes in temp git repos; no real agent, API key or subscription.

**What to build:** Operator-approved 2026-11-02. The only reusable
"loop over tasks" pipeline shipped, `templates/epic-runner.dot`, takes its
tasks from beads (`bd`). A user should need only a markdown file and a
`.dot` file. Nothing in the engine requires beads (it checks for `bd`
only when a pipeline has `beads.*` nodes), so this is a template and docs,
no engine change.
1. `templates/task-loop.dot`: a fixed pipeline over a markdown checklist
   (`- [ ] task` lines, path given once in the graph's goal or a variable):
   pick the first unchecked task (agent node, `MORE`/`DONE`), implement,
   test (tool node), verify, tick the box and commit, loop back; `DONE`
   exits. Plain node types only; no `beads.*`.
2. An example task list next to it, and the template validates with no
   `bd` on the PATH.
3. docs/guide.md leads with this pattern ("one good pipeline, reused");
   beads (`decompose`/`scaffold`/`epic-runner`) becomes an optional
   section. README's quick start uses it.
4. CHANGELOG Unreleased.

**Blocked by:** 30

**Status:** open

- [ ] The template validates with `bd` absent from the PATH
- [ ] With fakes, a two-task list runs both tasks, ticks both boxes and exits (seam 2)
- [ ] The guide's first pipeline pattern needs no beads
