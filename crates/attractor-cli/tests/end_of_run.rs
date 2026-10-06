#![cfg(unix)]
//! `pas run` end to end with the fake `claude`: every end the process lives
//! through writes `final.json`; a successful Run removes its worktree and
//! keeps its branch (ticket 07).

#[allow(dead_code)]
mod common;
#[allow(dead_code)]
mod fake_agent;

use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

use fake_agent::{stderr, FakeAgent};
use serde_json::Value;

/// start -> work -> done, where `work` is a claude node with `attrs`.
fn one_node(attrs: &str) -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            work [shape="box", llm_provider="claude", {attrs}]
            done [shape="Msquare"]
            start -> work -> done
        }}"#
    )
}

/// The one Run's `final.json`.
fn final_json(fake: &FakeAgent) -> Value {
    let runs = fake.run_dirs();
    assert_eq!(runs.len(), 1, "expected one Run folder: {runs:?}");
    let path = runs[0].join("final.json");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn run_id(fake: &FakeAgent) -> String {
    fake.run_meta()["run_id"].as_str().unwrap().to_string()
}

fn branch(fake: &FakeAgent) -> String {
    fake.run_meta()["branch"].as_str().unwrap().to_string()
}

fn branch_exists(fake: &FakeAgent) -> bool {
    let branch = branch(fake);
    fake.git(&["branch", "--list", &branch]).contains(&branch)
}

/// The fields every report of a Run with a worktree shares.
fn assert_git_fields(fake: &FakeAgent, report: &Value) {
    let meta = fake.run_meta();
    assert_eq!(report["v"], 1);
    assert_eq!(report["run_id"], meta["run_id"]);
    assert_eq!(report["branch"], meta["branch"]);
    assert_eq!(report["base"], meta["base"]);
    assert_eq!(report["base_sha"], meta["base_sha"]);
    assert_eq!(
        report["final_commit"],
        fake.git(&["rev-parse", &branch(fake)]).as_str()
    );
}

/// Polls `ready` until it holds, failing after `within`.
fn wait_for(what: &str, within: Duration, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_exit(child: &mut Child, within: Duration) -> ExitStatus {
    let deadline = Instant::now() + within;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("pas did not exit within {within:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The journal of the one Run so far, skipping a line still being written.
fn journal_so_far(fake: &FakeAgent) -> Vec<Value> {
    let Some(run) = fake.run_dirs().into_iter().next() else {
        return vec![];
    };
    fs::read_to_string(run.join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn spawn(fake: &FakeAgent, dot: &str) -> Child {
    fake.command_with(dot, &[])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn success_removes_the_worktree_keeps_the_branch_and_reports() {
    let fake = FakeAgent::new();
    let output = fake.run_with(
        &one_node(r#"timeout="30s", prompt="scenario=edit_commit""#),
        &["--json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));

    let worktree = fake.worktree();
    assert!(!worktree.exists(), "{} still exists", worktree.display());
    assert!(branch_exists(&fake));
    assert!(!fake
        .git(&["worktree", "list", "--porcelain"])
        .contains(&worktree.display().to_string()));

    let report = final_json(&fake);
    assert_git_fields(&fake, &report);
    assert_eq!(report["status"], "success");
    assert!(report["worktree"].is_null(), "{report}");
    assert!(report.get("error").is_none(), "{report}");
    assert_eq!(report["warnings"], serde_json::json!([]));
    // The fake's commit and the engine's are both on the branch.
    assert!(!fake
        .git(&["show", &format!("{}:fake-edit.txt", branch(&fake))])
        .is_empty());

    // D7: the first line is unchanged; the last is the same report.
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    let first: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["ok"], true);
    assert_eq!(first["run_id"], run_id(&fake).as_str());
    let last: Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(last, report);
}

#[test]
fn a_pas_folder_in_the_worktree_does_not_block_its_removal() {
    let fake = FakeAgent::new();
    let output = fake.run(&one_node(r#"timeout="30s", prompt="scenario=pas_dir""#));
    assert!(output.status.success(), "{}", stderr(&output));

    assert!(!fake.worktree().exists());
    assert!(branch_exists(&fake));
    let report = final_json(&fake);
    assert_eq!(report["status"], "success");
    assert!(report["worktree"].is_null(), "{report}");
    assert_eq!(report["warnings"], serde_json::json!([]));
}

#[test]
fn a_failed_run_keeps_its_worktree_and_reports_the_error() {
    let fake = FakeAgent::new();
    let output = fake.run_with(
        &one_node(r#"timeout="30s", prompt="scenario=crash""#),
        &["--json"],
    );
    assert!(!output.status.success());

    let worktree = fake.worktree();
    assert!(worktree.is_dir(), "{} is gone", worktree.display());
    assert!(branch_exists(&fake));

    let report = final_json(&fake);
    assert_git_fields(&fake, &report);
    assert_eq!(report["status"], "failed");
    assert_eq!(report["worktree"], worktree.display().to_string().as_str());
    let error = report["error"].as_str().unwrap();
    assert!(error.contains("fake-claude: crashed"), "{error}");
    assert!(stderr(&output).contains(error), "{}", stderr(&output));
    // The branch tip is the failed attempt's commit.
    let tip = fake.git(&["log", "-1", "--format=%B", &branch(&fake)]);
    assert!(tip.contains("Pas-Node: work"), "{tip}");
    assert!(tip.contains("Pas-Status: fail"), "{tip}");

    let stdout = String::from_utf8(output.stdout).unwrap();
    let last: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(last, report);
}

#[test]
fn a_stop_between_stages_keeps_the_worktree_and_reports_stopped() {
    let fake = FakeAgent::new();
    let dot = format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            wait [shape="parallelogram", timeout="120s", tool_command="{go}"]
            work [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            done [shape="Msquare"]
            start -> wait -> work -> done
        }}"#,
        go = common::wait_for_go!()
    );
    let mut child = spawn(&fake, &dot);
    wait_for("wait to start", Duration::from_secs(20), || {
        journal_so_far(&fake)
            .iter()
            .any(|e| e["type"] == "StageStarted" && e["data"]["node_id"] == "wait")
    });

    let stop: Output = Command::new(env!("CARGO_BIN_EXE_pas"))
        .args(["stop", &run_id(&fake)])
        .env("PAS_STATE_DIR", fake.root().join("state"))
        .current_dir(fake.root())
        .output()
        .unwrap();
    assert!(stop.status.success(), "{}", stderr(&stop));
    fs::write(fake.worktree().join("go"), "").unwrap();
    let status = wait_exit(&mut child, Duration::from_secs(30));

    assert_eq!(status.code(), Some(0), "{status:?}");
    let worktree = fake.worktree();
    assert!(worktree.is_dir());
    assert!(branch_exists(&fake));
    let report = final_json(&fake);
    assert_git_fields(&fake, &report);
    assert_eq!(report["status"], "stopped");
    assert_eq!(report["worktree"], worktree.display().to_string().as_str());
    assert!(report.get("error").is_none(), "{report}");
}

#[test]
fn sigterm_mid_agent_keeps_the_worktree_and_reports_stopped() {
    let fake = FakeAgent::new();
    let mut child = spawn(
        &fake,
        &one_node(r#"timeout="120s", prompt="scenario=hang_term""#),
    );
    wait_for("LlmStarted", Duration::from_secs(20), || {
        journal_so_far(&fake)
            .iter()
            .any(|e| e["type"] == "LlmStarted")
    });

    // SAFETY: sending a signal to our own child's pid.
    let sent = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(sent, 0, "kill -TERM failed");
    let status = wait_exit(&mut child, Duration::from_secs(30));

    assert_eq!(status.code(), Some(143), "{status:?}");
    let worktree = fake.worktree();
    assert!(worktree.is_dir());
    assert!(branch_exists(&fake));
    let report = final_json(&fake);
    assert_git_fields(&fake, &report);
    assert_eq!(report["status"], "stopped");
    assert_eq!(report["worktree"], worktree.display().to_string().as_str());
    assert!(report.get("error").is_none(), "{report}");
}

#[test]
fn a_non_git_workdir_reports_null_git_fields() {
    let fake = FakeAgent::new();
    let plain: PathBuf = fake.root().join("plain");
    fs::create_dir(&plain).unwrap();
    let output = fake
        .command_in(
            &one_node(r#"timeout="30s", prompt="scenario=success""#),
            &plain,
            &[],
        )
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(plain.is_dir());

    let report = final_json(&fake);
    assert_eq!(report["v"], 1);
    assert_eq!(report["run_id"], run_id(&fake).as_str());
    assert_eq!(report["status"], "success");
    for key in ["branch", "base", "base_sha", "final_commit", "worktree"] {
        assert!(report[key].is_null(), "{key}: {report}");
    }
    assert!(report.get("error").is_none(), "{report}");
}
