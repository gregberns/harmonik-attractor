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

#[test]
fn the_task_loop_runs_both_tasks_ticks_both_boxes_and_exits() {
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
    let branch = fake.run_meta()["branch"].as_str().unwrap().to_string();
    assert_eq!(
        fake.git(&["show", &format!("{branch}:TASKS.md")]),
        "# Tasks\n\n- [x] first task\n- [x] already done\n- [x] second task"
    );
}
