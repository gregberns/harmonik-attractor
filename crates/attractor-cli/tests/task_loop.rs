#![cfg(unix)]
//! `templates/task-loop.dot`, the task loop over a markdown checklist: it
//! validates without Beads, and `hka run` with the fake `claude` works
//! through a two-task list, ticking both boxes.

mod blocked_agents;
#[allow(dead_code)]
mod fake_agent;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use fake_agent::{stderr, FakeAgent};

fn template(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../templates/{name}"))
}

fn node_file(fake: &FakeAgent, name: &str, line: &str) {
    fs::write(fake.scenarios().join(name), format!("{line}\n")).unwrap();
}

#[test]
fn the_task_loop_validates_without_bd_on_the_path() {
    let empty = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_harmonik-attractor"))
        .arg("validate")
        .arg(template("task-loop.dot"))
        .env("PATH", empty.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
}

/// A repo with a list of two open tasks and one ticked, a passing
/// `make test`, and the fake `claude` scripted to pick twice, pass both,
/// then answer DONE.
fn two_task_list() -> FakeAgent {
    let fake = FakeAgent::new();
    fake.shim_claude_on_path();
    fs::write(
        fake.repo().join("TASKS.md"),
        "# Tasks\n\n- [ ] first task\n- [x] already done\n- [ ] second task\n",
    )
    .unwrap();
    fs::write(fake.repo().join("Makefile"), "test:\n\t@true\n").unwrap();
    fake.git(&["add", "TASKS.md", "Makefile"]);
    fake.git(&["commit", "-q", "-m", "tasks"]);
    node_file(&fake, "pick@1", "scenario=label label=MORE");
    node_file(&fake, "pick@2", "scenario=label label=MORE");
    node_file(&fake, "pick@3", "scenario=label label=DONE");
    node_file(&fake, "implement", "scenario=success");
    node_file(&fake, "verify", "scenario=label label=PASS");
    fake
}

fn tasks_on_the_run_branch(fake: &FakeAgent) -> String {
    let branch = fake.run_meta()["branch"].as_str().unwrap().to_string();
    fake.git(&["show", &format!("{branch}:TASKS.md")])
}

const ALL_TICKED: &str = "# Tasks\n\n- [x] first task\n- [x] already done\n- [x] second task";

#[test]
fn the_task_loop_runs_both_tasks_ticks_both_boxes_and_exits() {
    let fake = two_task_list();
    let dot = fs::read_to_string(template("task-loop.dot")).unwrap();
    let output = fake.run(&dot);
    assert!(output.status.success(), "{}", stderr(&output));

    assert_eq!(
        fake.stages_started(),
        [
            "start",
            "pick",
            "implement",
            "test",
            "verify",
            "tick",
            "pick",
            "implement",
            "test",
            "verify",
            "tick",
            "pick",
            "done",
        ]
    );
    assert_eq!(tasks_on_the_run_branch(&fake), ALL_TICKED);
}

/// Each task costs at least five steps, so `--max-steps` caps a list's
/// length; the same command with a larger `--max-steps` resumes the Run.
#[test]
fn a_task_loop_stopped_by_max_steps_resumes_with_a_larger_max_steps() {
    let fake = two_task_list();
    let dot = fs::read_to_string(template("task-loop.dot")).unwrap();
    let stopped = fake.run_with(&dot, &["--max-steps", "8"]);
    assert!(
        stderr(&stopped).contains("exceeded maximum step count (8)"),
        "{}",
        stderr(&stopped)
    );
    // The same cap again stops at once: the step count is in the checkpoint.
    let again = fake.run_with(&dot, &["--max-steps", "8"]);
    assert!(
        stderr(&again).contains("exceeded maximum step count (8)"),
        "{}",
        stderr(&again)
    );

    let resumed = fake.run_with(&dot, &["--max-steps", "200"]);
    assert!(resumed.status.success(), "{}", stderr(&resumed));
    assert_eq!(fake.run_dirs().len(), 1, "a resume, not a new Run");
    assert_eq!(tasks_on_the_run_branch(&fake), ALL_TICKED);
}
