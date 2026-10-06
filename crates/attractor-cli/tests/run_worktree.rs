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
