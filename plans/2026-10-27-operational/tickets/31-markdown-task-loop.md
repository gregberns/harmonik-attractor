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

- [x] The template validates with `bd` absent from the PATH
- [x] With fakes, a two-task list runs both tasks, ticks both boxes and exits (seam 2)
- [x] The guide's first pipeline pattern needs no beads

**Notes:**
- The list's path is `TASKS.md` in the repo root, written in each prompt
  and in the `tick` tool node, not once: `${...}` expands in prompts only,
  not in `tool_command` (`transforms.rs`), and the ticket rules out an
  engine change. The template's header and the guide say to replace it
  throughout for another file.
- `tick` is a tool node (awk turns the first `- [ ] ` into `- [x] `;
  it fails with no open box), so ticking is exact and testable with fakes.
  The commit is the engine's attempt commit, not a separate step.
- `test` runs `make test`, a placeholder the user edits.
- Tests: `crates/attractor-cli/tests/task_loop.rs` (validate with an empty
  `PATH`; the template run on the fake `claude` with a list of two open
  tasks and one ticked, checking the stage order and the branch's
  `TASKS.md`). `documented_dot_contracts` also validates every template.
- `--max-steps` (default 200) caps a list's length: each task takes at
  least 5 steps, and the step count is saved with the Run. The guide and
  the template say so;
  `a_task_loop_stopped_by_max_steps_resumes_with_a_larger_max_steps`
  checks that the same cap stops again and a larger one resumes.
