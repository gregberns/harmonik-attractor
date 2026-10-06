#![cfg(unix)]
//! A git worktree and branch per Run (ticket 06, design.md §3), observed
//! through `pas run` with the fake agents in temporary git repos.

// This crate uses a subset of the shared harness.
#[allow(dead_code)]
mod fake_agent;

use std::fs;
use std::path::{Path, PathBuf};

use fake_agent::{stderr, FakeAgent};

/// One Claude node running fake `scenario`.
fn agent_node(scenario: &str) -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            work [shape="box", llm_provider="claude", timeout="30s", prompt="scenario={scenario}"]
            done [shape="Msquare"]
            start -> work -> done
        }}"#
    )
}

/// One tool node running `command`.
fn tool_node(command: &str) -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            step [shape="parallelogram", tool_command="{command}"]
            done [shape="Msquare"]
            start -> step -> done
        }}"#
    )
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "pas run failed ({:?})\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        stderr(output)
    );
}

#[test]
fn run_creates_worktree_on_run_branch_at_base() {
    let fake = FakeAgent::new();
    let base = fake.git(&["rev-parse", "HEAD"]);
    let main_branch = fake.git(&["rev-parse", "--abbrev-ref", "HEAD"]);

    assert_success(&fake.run(&agent_node("edit_commit")));

    let meta = fake.run_meta();
    let run_id = meta["run_id"].as_str().unwrap();
    let branch = format!("pas/run/{run_id}");
    let wt = fake.worktree();
    assert_eq!(
        wt,
        canonical(&fake.repo()).join(".pas/worktrees").join(run_id)
    );
    assert_eq!(meta["branch"], branch.as_str());
    assert_eq!(meta["base"], "HEAD");
    assert_eq!(meta["base_sha"], base.as_str());
    // run.json keeps the caller's directory as the workdir.
    assert_eq!(meta["workdir"], canonical(&fake.repo()).to_str().unwrap());

    assert_eq!(
        fake.git_in(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]),
        branch
    );
    assert_eq!(
        fake.git_in(&wt, &["log", "-1", "--format=%s"]),
        "fake-claude edit"
    );
    assert_eq!(fake.git_in(&wt, &["rev-parse", "HEAD~1"]), base);
    assert!(wt.join("fake-edit.txt").is_file());

    // The main checkout is untouched.
    assert!(!fake.repo().join("fake-edit.txt").exists());
    assert_eq!(fake.git(&["rev-parse", "HEAD"]), base);
    assert_eq!(
        fake.git(&["rev-parse", "--abbrev-ref", "HEAD"]),
        main_branch
    );
}

#[test]
fn subdirectory_workdir_maps_into_the_worktree() {
    let fake = FakeAgent::new();
    fs::create_dir_all(fake.repo().join("sub")).unwrap();
    fs::write(fake.repo().join("sub/keep"), "").unwrap();
    fake.git(&["add", "sub/keep"]);
    fake.git(&["commit", "-q", "-m", "sub"]);

    let output = fake
        .command_in(&tool_node("pwd -P > where"), &fake.repo().join("sub"), &[])
        .output()
        .unwrap();
    assert_success(&output);

    let sub = fake.worktree().join("sub");
    let recorded = fs::read_to_string(sub.join("where")).unwrap();
    assert_eq!(recorded.trim(), sub.to_str().unwrap());
    assert!(!fake.repo().join("sub/where").exists());
}

#[test]
fn non_git_workdir_runs_in_place_with_a_warning() {
    let fake = FakeAgent::new();
    let plain = fake.root().join("plain");
    fs::create_dir_all(&plain).unwrap();

    let output = fake
        .command_in(&tool_node("touch here"), &plain, &[])
        .output()
        .unwrap();
    assert_success(&output);

    assert!(plain.join("here").is_file());
    assert!(!plain.join(".pas").exists());
    let meta = fake.run_meta();
    assert!(meta["worktree"].is_null(), "{meta}");
    assert!(meta["branch"].is_null(), "{meta}");
    assert!(
        stderr(&output).contains("not a git repository"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn main_checkout_stays_clean_with_default_logs_and_worktrees() {
    let fake = FakeAgent::new();

    // No --workdir or --logs: both default to the repo (cwd).
    let output = fake
        .pas_run(&agent_node("edit_commit"))
        .current_dir(fake.repo())
        .output()
        .unwrap();
    assert_success(&output);

    assert!(fake.repo().join(".pas/logs").is_dir());
    assert!(fake.repo().join(".pas/worktrees").is_dir());
    assert_eq!(
        fs::read_to_string(fake.repo().join(".pas/.gitignore")).unwrap(),
        "*\n"
    );
    assert_eq!(fake.git(&["status", "--porcelain"]), "");
}

#[test]
fn existing_pas_gitignore_is_left_alone() {
    let fake = FakeAgent::new();
    fs::create_dir_all(fake.repo().join(".pas")).unwrap();
    fs::write(fake.repo().join(".pas/.gitignore"), "worktrees/\nlogs/\n").unwrap();

    let output = fake
        .pas_run(&agent_node("edit_commit"))
        .current_dir(fake.repo())
        .output()
        .unwrap();
    assert_success(&output);

    assert_eq!(
        fs::read_to_string(fake.repo().join(".pas/.gitignore")).unwrap(),
        "worktrees/\nlogs/\n"
    );
}

#[test]
fn default_logs_in_a_subdirectory_are_ignored_too() {
    let fake = FakeAgent::new();
    let sub = fake.repo().join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("keep"), "").unwrap();
    fake.git(&["add", "sub/keep"]);
    fake.git(&["commit", "-q", "-m", "sub"]);

    // cwd = <repo>/sub: the logs go to <repo>/sub/.pas/logs.
    let output = fake
        .pas_run(&tool_node("true"))
        .current_dir(&sub)
        .output()
        .unwrap();
    assert_success(&output);

    assert!(sub.join(".pas/logs").is_dir());
    assert_eq!(
        fs::read_to_string(sub.join(".pas/.gitignore")).unwrap(),
        "*\n"
    );
    assert_eq!(fake.git(&["status", "--porcelain"]), "");
}

/// The `RunStarted` event's warnings.
fn run_started_warnings(fake: &FakeAgent) -> Vec<String> {
    let started = fake.events_of("RunStarted");
    assert_eq!(started.len(), 1, "{started:?}");
    started[0]["warnings"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|w| w.as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn run_json_warnings(fake: &FakeAgent) -> Vec<String> {
    serde_json::from_value(fake.run_meta()["warnings"].clone()).unwrap()
}

fn assert_dirty_warning(warnings: &[String]) {
    assert!(
        warnings.iter().any(|w| w.contains("uncommitted")),
        "{warnings:?}"
    );
}

#[test]
fn dirty_checkout_warns_and_stays_out_of_the_worktree() {
    let fake = FakeAgent::new();
    fs::write(fake.repo().join("tracked"), "committed\n").unwrap();
    fake.git(&["add", "tracked"]);
    fake.git(&["commit", "-q", "-m", "tracked"]);
    fs::write(fake.repo().join("tracked"), "changed\n").unwrap();
    fs::write(fake.repo().join("untracked"), "new\n").unwrap();

    let output = fake.run(&tool_node("true"));
    assert_success(&output);

    assert_dirty_warning(&run_json_warnings(&fake));
    assert_dirty_warning(&run_started_warnings(&fake));
    assert!(
        stderr(&output).contains("uncommitted"),
        "{}",
        stderr(&output)
    );

    let wt = fake.worktree();
    assert_eq!(
        fs::read_to_string(wt.join("tracked")).unwrap(),
        "committed\n"
    );
    assert!(!wt.join("untracked").exists());
    assert_eq!(
        fs::read_to_string(fake.repo().join("tracked")).unwrap(),
        "changed\n"
    );
    assert!(fake.repo().join("untracked").is_file());
}

#[test]
fn dirt_outside_a_subdirectory_workdir_still_warns() {
    let fake = FakeAgent::new();
    fs::create_dir_all(fake.repo().join("sub")).unwrap();
    fs::write(fake.repo().join("sub/keep"), "").unwrap();
    fake.git(&["add", "sub/keep"]);
    fake.git(&["commit", "-q", "-m", "sub"]);
    fs::write(fake.repo().join("at-root"), "dirt\n").unwrap();

    let output = fake
        .command_in(&tool_node("true"), &fake.repo().join("sub"), &[])
        .output()
        .unwrap();
    assert_success(&output);

    assert_dirty_warning(&run_json_warnings(&fake));
    assert_dirty_warning(&run_started_warnings(&fake));
}

#[test]
fn clean_checkout_has_no_warnings() {
    let fake = FakeAgent::new();

    assert_success(&fake.run(&tool_node("true")));

    assert!(run_json_warnings(&fake).is_empty());
    assert!(run_started_warnings(&fake).is_empty());
    let started = fake.events_of("RunStarted");
    assert!(started[0].get("warnings").is_none(), "{started:?}");
}
