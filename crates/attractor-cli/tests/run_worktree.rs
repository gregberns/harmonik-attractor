#![cfg(unix)]
//! A git worktree and branch per Run (ticket 06, design.md §3), observed
//! through `pas run` with the fake agents in temporary git repos.

// This crate uses a subset of the shared harness.
mod common;
#[allow(dead_code)]
mod fake_agent;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

use common::wait_for_go;

use fake_agent::{stderr, FakeAgent};

/// One Claude node running fake `scenario`.
fn agent_node(scenario: &str) -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            work [shape="box", agent="fake", timeout="30s", prompt="scenario={scenario}"]
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

/// The one Run's branch, from its `run.json`.
fn run_branch(fake: &FakeAgent) -> String {
    fake.run_meta()["branch"].as_str().unwrap().to_string()
}

/// `path` as committed on the Run's branch. A successful Run removes its
/// worktree and keeps the branch (ticket 07), so tests read results here.
fn on_branch(fake: &FakeAgent, path: &str) -> String {
    fake.git(&["show", &format!("{}:{path}", run_branch(fake))])
}

/// Whether `path` is committed on the Run's branch.
fn is_on_branch(fake: &FakeAgent, path: &str) -> bool {
    fake.git(&["ls-tree", "-r", "--name-only", &run_branch(fake)])
        .lines()
        .any(|line| line == path)
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

    // The agent worked on the Run's branch: its commit is there, then the
    // engine's attempt commit (ticket 07). The successful Run's worktree is
    // removed; the branch stays.
    assert!(!wt.exists(), "{}", wt.display());
    assert_eq!(
        fake.git(&["log", "-1", "--format=%s", &format!("{branch}~1")]),
        "fake-claude edit"
    );
    assert_eq!(fake.git(&["rev-parse", &format!("{branch}~2")]), base);
    assert!(is_on_branch(&fake, "fake-edit.txt"));

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
    let recorded = on_branch(&fake, "sub/where");
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
    // pas's own logs folder is not dirt.
    assert!(
        !stderr(&output).contains("uncommitted"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_refused_run_leaves_its_default_logs_ignored() {
    let fake = FakeAgent::new();

    let output = fake
        .pas_run(&tool_node("true"))
        .args(["--base", "nosuchref"])
        .current_dir(fake.repo())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));

    assert_eq!(fake.git(&["status", "--porcelain"]), "");
}

#[test]
fn worktree_root_inside_the_repo_stays_out_of_git_status() {
    let fake = FakeAgent::new();
    fs::write(
        fake.repo().join("pas.toml"),
        "[project]\nname = \"demo\"\n\n[run]\nworktree_root = \"wt\"\n",
    )
    .unwrap();
    fake.git(&["add", "pas.toml"]);
    fake.git(&["commit", "-q", "-m", "pas.toml"]);

    assert_success(&fake.run(&tool_node("true")));
    assert_eq!(fake.git(&["status", "--porcelain"]), "");

    let second = fake.run_with(&tool_node("true"), &["--fresh"]);
    assert_success(&second);
    assert!(
        !stderr(&second).contains("uncommitted"),
        "{}",
        stderr(&second)
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

    // `git show` output is trimmed.
    assert_eq!(on_branch(&fake, "tracked"), "committed");
    assert!(!is_on_branch(&fake, "untracked"));
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

#[test]
fn base_flag_starts_the_branch_at_that_commit() {
    let fake = FakeAgent::new();
    let older = fake.git(&["rev-parse", "HEAD"]);
    fake.git(&["commit", "-q", "--allow-empty", "-m", "newer"]);

    assert_success(&fake.run_with(&tool_node("true"), &["--base", &older]));

    let meta = fake.run_meta();
    assert_eq!(meta["base"], older.as_str());
    assert_eq!(meta["base_sha"], older.as_str());
    // The tool node's attempt commit sits directly on the base (ticket 07).
    assert_eq!(
        fake.git(&["rev-parse", &format!("{}~1", run_branch(&fake))]),
        older
    );
}

#[test]
fn worktree_root_flag_puts_the_worktree_there() {
    let fake = FakeAgent::new();
    let root = fake.root().join("elsewhere");

    assert_success(&fake.run_with(
        &tool_node("pwd -P > where"),
        &["--worktree-root", root.to_str().unwrap()],
    ));

    let run_id = fake.run_meta()["run_id"].as_str().unwrap().to_string();
    assert_eq!(fake.worktree(), canonical(&root).join(&run_id));
    // The node ran there; the successful Run then removed it (ticket 07).
    assert_eq!(on_branch(&fake, "where"), fake.worktree().to_str().unwrap());
    assert!(!fake.worktree().exists());
    assert!(!fake.repo().join(".pas/worktrees").exists());
}

fn write_pas_toml(fake: &FakeAgent, worktree_root: &str) {
    fs::write(
        fake.repo().join("pas.toml"),
        format!("[project]\nname = \"demo\"\n\n[run]\nworktree_root = \"{worktree_root}\"\n"),
    )
    .unwrap();
    fs::write(
        fake.repo().join(".gitignore"),
        format!("/{worktree_root}/\n"),
    )
    .unwrap();
    fake.git(&["add", "pas.toml", ".gitignore"]);
    fake.git(&["commit", "-q", "-m", "pas.toml"]);
}

#[test]
fn pas_toml_worktree_root_is_relative_to_pas_toml() {
    let fake = FakeAgent::new();
    write_pas_toml(&fake, "wt");

    assert_success(&fake.run(&tool_node("true")));

    let run_id = fake.run_meta()["run_id"].as_str().unwrap().to_string();
    assert_eq!(
        fake.worktree(),
        canonical(&fake.repo()).join("wt").join(&run_id)
    );
}

#[test]
fn worktree_root_flag_beats_pas_toml() {
    let fake = FakeAgent::new();
    write_pas_toml(&fake, "wt");
    let root = fake.root().join("flag-root");

    assert_success(&fake.run_with(
        &tool_node("true"),
        &["--worktree-root", root.to_str().unwrap()],
    ));

    let run_id = fake.run_meta()["run_id"].as_str().unwrap().to_string();
    assert_eq!(fake.worktree(), canonical(&root).join(&run_id));
    assert!(!fake.repo().join("wt").exists());
}

#[test]
fn unknown_base_is_refused_before_any_branch_or_worktree() {
    let fake = FakeAgent::new();

    let output = fake.run_with(&tool_node("true"), &["--base", "nosuchref", "--json"]);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let first: serde_json::Value = serde_json::from_str(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(first["error"]["code"], "invalid_base", "{first}");
    assert_eq!(fake.git(&["branch", "--list", "pas/run/*"]), "");
    assert!(!fake.repo().join(".pas/worktrees").exists());
    assert_eq!(
        fake.git(&["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
}

/// A `pas run` in the background, killed if the test fails first.
struct Spawned(Child);

impl Spawned {
    fn wait(&mut self) -> ExitStatus {
        wait_until("the Run to exit", || self.0.try_wait().unwrap())
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// Polls `check` every 20 ms for up to 60 s.
fn wait_until<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Waits until the one Run's `run.json` names its worktree.
fn wait_for_worktree(fake: &FakeAgent) -> PathBuf {
    wait_until("run.json with a worktree", || {
        let run = fake.run_dirs().into_iter().next()?;
        let meta: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(run.join("run.json")).ok()?).ok()?;
        meta["worktree"].as_str().map(PathBuf::from)
    })
}

#[test]
fn second_run_of_same_pipeline_is_refused_before_any_worktree() {
    let fake = FakeAgent::new();
    let dot = tool_node(wait_for_go!());
    let mut a = Spawned(fake.command_with(&dot, &[]).spawn().unwrap());
    let wt = wait_for_worktree(&fake);

    let b = fake.run_with(&dot, &["--json"]);

    assert_eq!(b.status.code(), Some(5), "{}", stderr(&b));
    let first: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&b.stdout).lines().next().unwrap()).unwrap();
    assert_eq!(first["error"]["code"], "pipeline_locked", "{first}");
    assert_eq!(
        fake.git(&["branch", "--list", "pas/run/*"]).lines().count(),
        1
    );
    assert_eq!(
        fake.git(&["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        2
    );
    assert_eq!(
        fs::read_dir(fake.repo().join(".pas/worktrees"))
            .unwrap()
            .count(),
        1
    );

    fs::write(wt.join("go"), "").unwrap();
    assert!(a.wait().success());
}

/// `edit` (fake `edit_commit`) -> `gate` (waits for `go`) -> `check`
/// (sees the edit, writes `seen`).
fn edit_gate_check() -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            edit [shape="box", agent="fake", timeout="30s", prompt="scenario=edit_commit"]
            gate [shape="parallelogram", tool_command="touch at-gate; {wait}"]
            check [shape="parallelogram", tool_command="test -f fake-edit.txt && echo ok > seen"]
            done [shape="Msquare"]
            start -> edit -> gate -> check -> done
        }}"#,
        wait = wait_for_go!()
    )
}

/// Starts `pas run` with `extra`, waits for `gate`, stops the Run and lets
/// `gate` finish; returns the worktree the Run stopped in.
fn run_until_stopped_at_gate(fake: &FakeAgent, extra: &[&str]) -> PathBuf {
    let mut run = Spawned(
        fake.command_with(&edit_gate_check(), extra)
            .spawn()
            .unwrap(),
    );
    let wt = wait_for_worktree(fake);
    wait_until("gate to start", || {
        wt.join("at-gate").exists().then_some(())
    });
    let run_id = fake.run_meta()["run_id"].as_str().unwrap().to_string();
    let stop = std::process::Command::new(env!("CARGO_BIN_EXE_pas"))
        .args(["stop", &run_id])
        .env("PAS_STATE_DIR", fake.root().join("state"))
        .current_dir(fake.root())
        .output()
        .unwrap();
    assert!(stop.status.success(), "{}", stderr(&stop));
    fs::write(wt.join("go"), "").unwrap();
    assert_eq!(run.wait().code(), Some(0), "a stopped Run exits 0");
    assert!(!wt.join("seen").exists(), "check ran before the stop");
    wt
}

#[test]
fn resume_continues_in_the_same_worktree() {
    let fake = FakeAgent::new();
    let wt = run_until_stopped_at_gate(&fake, &[]);
    let before = fake.run_meta();
    // Dirt after the Run started: a resume does not check it again.
    fs::write(fake.repo().join("later"), "dirt\n").unwrap();

    let output = fake.run_with(&edit_gate_check(), &[]);
    assert_success(&output);

    assert_eq!(fake.run_meta(), before, "run.json is unchanged");
    assert!(
        !stderr(&output).contains("uncommitted"),
        "{}",
        stderr(&output)
    );
    assert_eq!(fake.worktree(), wt);
    // Read on the branch: the successful Run removed its worktree (ticket 07).
    assert_eq!(on_branch(&fake, "seen"), "ok");
    assert!(!wt.exists());
    assert_eq!(
        fs::read_dir(fake.repo().join(".pas/worktrees"))
            .unwrap()
            .count(),
        0,
        "no second worktree was made; the Run's one was removed on success (ticket 07)"
    );
    assert_eq!(fake.attempts("edit_commit"), 1);
    assert_eq!(
        fake.git(&["log", "--format=%s", &run_branch(&fake)])
            .matches("fake-claude edit")
            .count(),
        1
    );
    assert_eq!(fake.events_of("RunStarted").len(), 1);
}

#[test]
fn resume_ignores_a_changed_worktree_root() {
    let fake = FakeAgent::new();
    let root = fake.root().join("first-root");
    let wt = run_until_stopped_at_gate(&fake, &["--worktree-root", root.to_str().unwrap()]);
    assert!(wt.starts_with(canonical(&root)), "{}", wt.display());

    let output = fake.run_with(&edit_gate_check(), &[]);
    assert_success(&output);

    assert_eq!(fake.worktree(), wt);
    // Read on the branch: the successful Run removed its worktree (ticket 07).
    assert_eq!(on_branch(&fake, "seen"), "ok");
    assert!(!wt.exists());
    let err = stderr(&output);
    assert!(
        err.contains("worktree") && err.contains(wt.to_str().unwrap()),
        "{err}"
    );
    assert!(!fake.repo().join(".pas/worktrees").exists());
    assert_eq!(
        fake.git(&["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1,
        "only the main checkout is left: the Run's worktree was reused, then removed on success (ticket 07)"
    );
}

#[test]
fn resume_recreates_a_deleted_worktree_from_its_branch() {
    let fake = FakeAgent::new();
    let wt = run_until_stopped_at_gate(&fake, &[]);
    fs::remove_dir_all(&wt).unwrap();

    let output = fake.run_with(&edit_gate_check(), &[]);
    assert_success(&output);

    assert_eq!(fake.worktree(), wt);
    // `check` sees the edit committed on the Run's branch.
    // Read on the branch: the successful Run removed its worktree (ticket 07).
    assert_eq!(on_branch(&fake, "seen"), "ok");
    assert!(!wt.exists());
    assert_eq!(fake.attempts("edit_commit"), 1);
}
