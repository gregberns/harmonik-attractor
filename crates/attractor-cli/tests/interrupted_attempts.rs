#![cfg(unix)]
//! `pas run` end to end with the fake `claude`: an attempt `pas` never
//! finished (SIGKILL, or a stop) is committed as `interrupted` on resume, and
//! the re-run's prompt says so (ticket 07).

#[allow(dead_code)]
mod fake_agent;

use std::fs;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use fake_agent::{stderr, FakeAgent};

/// Kills an agent's process group when dropped, so a failed assertion never
/// leaves the fake running (it outlives a SIGKILLed `pas`).
struct KillGroup(Option<u32>);

impl KillGroup {
    /// Kill the group now and wait until its leader is gone.
    fn kill_now(&mut self) {
        if let Some(pgid) = self.0.take() {
            kill_group(pgid);
            wait_for("the fake to exit", Duration::from_secs(10), || !alive(pgid));
        }
    }
}

impl Drop for KillGroup {
    fn drop(&mut self) {
        if let Some(pgid) = self.0.take() {
            kill_group(pgid);
        }
    }
}

fn kill_group(pgid: u32) {
    // SAFETY: SIGKILL to a process group this test's fake leads.
    unsafe {
        libc::kill(-(pgid as libc::pid_t), libc::SIGKILL);
    }
}

/// Whether `pid` still exists and is not a zombie.
fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
        return false;
    }
    // The orphan is reaped by init; until then `ps` shows it as a zombie.
    std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .map(|o| {
            !String::from_utf8_lossy(&o.stdout)
                .trim_start()
                .starts_with('Z')
        })
        .unwrap_or(false)
}

fn wait_for(what: &str, within: Duration, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The journal so far, skipping a line still being written.
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

/// The pgid of the first agent process, once `LlmStarted` is journalled.
fn agent_pgid(fake: &FakeAgent) -> u32 {
    let mut pgid = None;
    wait_for("LlmStarted", Duration::from_secs(20), || {
        pgid = journal_so_far(fake)
            .iter()
            .find(|e| e["type"] == "LlmStarted")
            .and_then(|e| e["data"]["pgid"].as_u64());
        pgid.is_some()
    });
    pgid.map(|p| p as u32).unwrap()
}

fn spawn_run(fake: &FakeAgent, dot: &str) -> Child {
    fake.command(dot)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// start -> work -> done; `work`'s scenario comes from the node files.
fn work_node(attrs: &str) -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            work [shape="box", agent="fake", prompt="do it", {attrs}]
            done [shape="Msquare"]
            start -> work -> done
        }}"#
    )
}

fn scenario(fake: &FakeAgent, file: &str, line: &str) {
    fs::write(fake.scenarios().join(file), format!("{line}\n")).unwrap();
}

/// One engine commit: sha, `Pas-Status`, `Pas-Attempt`.
#[derive(Debug)]
struct EngineCommit {
    sha: String,
    status: String,
    attempt: String,
}

fn engine_commits(fake: &FakeAgent) -> Vec<EngineCommit> {
    let meta = fake.run_meta();
    let range = format!(
        "{}..{}",
        meta["base_sha"].as_str().unwrap(),
        meta["branch"].as_str().unwrap()
    );
    let log = fake.git(&[
        "log",
        "--reverse",
        "--format=%H%x1f%(trailers:key=Pas-Status,valueonly,separator=)%x1f%(trailers:key=Pas-Attempt,valueonly,separator=)%x1e",
        &range,
    ]);
    log.split('\x1e')
        .map(str::trim)
        .filter_map(|entry| {
            let mut fields = entry.split('\x1f');
            let (sha, status, attempt) = (fields.next()?, fields.next()?, fields.next()?);
            (!status.trim().is_empty()).then(|| EngineCommit {
                sha: sha.to_string(),
                status: status.trim().to_string(),
                attempt: attempt.trim().to_string(),
            })
        })
        .collect()
}

fn files_in(fake: &FakeAgent, sha: &str) -> String {
    fake.git(&["show", "--name-only", "--format=", sha])
}

fn worktree(fake: &FakeAgent) -> PathBuf {
    PathBuf::from(fake.run_meta()["worktree"].as_str().unwrap())
}

#[test]
fn a_sigkilled_attempt_is_committed_as_interrupted_and_the_node_runs_again() {
    let fake = FakeAgent::new();
    scenario(&fake, "work.1", "scenario=edit_hang");
    scenario(&fake, "work.2", "scenario=success");
    let dot = work_node(r#"timeout="120s", max_retries=1"#);
    let mut pas = spawn_run(&fake, &dot);
    let mut agent = KillGroup(Some(agent_pgid(&fake)));
    let partial = worktree(&fake).join("partial.txt");
    wait_for("partial.txt", Duration::from_secs(20), || partial.is_file());

    pas.kill().unwrap();
    pas.wait().unwrap();
    // The orphaned agent must not touch the worktree during the resume.
    agent.kill_now();

    let output = fake.command(&dot).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));

    let commits = engine_commits(&fake);
    assert_eq!(commits.len(), 2, "{commits:#?}");
    assert_eq!(
        (commits[0].status.as_str(), commits[0].attempt.as_str()),
        ("interrupted", "1")
    );
    assert!(files_in(&fake, &commits[0].sha).contains("partial.txt"));
    assert_eq!(
        (commits[1].status.as_str(), commits[1].attempt.as_str()),
        ("success", "2")
    );
    // The re-run was told: the base is the attempt's start, the commit is
    // the interrupted one.
    let base = fake.run_meta()["base_sha"].as_str().unwrap().to_string();
    let prompt = fs::read_to_string(fake.scenarios().join("prompt.work.2")).unwrap();
    assert!(
        prompt.contains(&format!(
            "Your previous attempt was interrupted; its changes since {base} are recorded in \
             commit {}. Review git diff {base} before continuing.",
            commits[0].sha
        )),
        "{prompt}"
    );
}

#[test]
fn a_stopped_attempt_is_committed_as_interrupted_and_does_not_count() {
    let fake = FakeAgent::new();
    scenario(&fake, "work.1", "scenario=edit_hang");
    scenario(&fake, "work.2", "scenario=success");
    // No retries: the re-run still happens because a stopped attempt
    // doesn't count (02b), and it is numbered 2 (numbers never repeat).
    let dot = work_node(r#"timeout="120s", max_retries=0"#);
    let mut pas = spawn_run(&fake, &dot);
    let _agent = KillGroup(Some(agent_pgid(&fake)));
    let partial = worktree(&fake).join("partial.txt");
    wait_for("partial.txt", Duration::from_secs(20), || partial.is_file());

    // SAFETY: SIGTERM to our own child.
    unsafe {
        libc::kill(pas.id() as libc::pid_t, libc::SIGTERM);
    }
    assert_eq!(pas.wait().unwrap().code(), Some(143));

    let output = fake.command(&dot).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));

    let commits = engine_commits(&fake);
    assert_eq!(commits.len(), 2, "{commits:#?}");
    assert_eq!(
        (commits[0].status.as_str(), commits[0].attempt.as_str()),
        ("interrupted", "1")
    );
    assert!(files_in(&fake, &commits[0].sha).contains("partial.txt"));
    assert_eq!(
        (commits[1].status.as_str(), commits[1].attempt.as_str()),
        ("success", "2")
    );
    let prompt = fs::read_to_string(fake.scenarios().join("prompt.work.2")).unwrap();
    assert!(
        prompt.contains("Your previous attempt was interrupted"),
        "{prompt}"
    );
}

#[test]
fn an_agent_commit_during_an_interrupted_attempt_is_noticed() {
    let fake = FakeAgent::new();
    scenario(&fake, "work.1", "scenario=commit_hang");
    scenario(&fake, "work.2", "scenario=success");
    let dot = work_node(r#"timeout="120s", max_retries=1"#);
    let mut pas = spawn_run(&fake, &dot);
    let mut agent = KillGroup(Some(agent_pgid(&fake)));
    let marker = fake.scenarios().join("committed");
    wait_for("the fake's commit", Duration::from_secs(20), || {
        marker.is_file()
    });

    pas.kill().unwrap();
    pas.wait().unwrap();
    agent.kill_now();

    let output = fake.command(&dot).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));

    let commits = engine_commits(&fake);
    assert_eq!(commits.len(), 2, "{commits:#?}");
    // The tree was clean, but HEAD moved: an empty interrupted commit on top
    // of the fake's.
    assert_eq!(commits[0].status, "interrupted");
    assert_eq!(files_in(&fake, &commits[0].sha).trim(), "");
    assert_eq!(
        fake.git(&["log", "-1", "--format=%s", &format!("{}~1", commits[0].sha)]),
        "fake-claude commit"
    );
    // The note's base is the attempt's start, before the fake's commit, so
    // `git diff <base>` shows the fake's work.
    let base = fake.run_meta()["base_sha"].as_str().unwrap().to_string();
    let prompt = fs::read_to_string(fake.scenarios().join("prompt.work.2")).unwrap();
    assert!(
        prompt.contains(&format!("Review git diff {base} ")),
        "{prompt}"
    );
    let diff = fake.git(&["diff", "--name-only", &base, &commits[0].sha]);
    assert!(diff.lines().any(|l| l == "committed.txt"), "{diff}");
}

#[test]
fn an_attempt_that_left_nothing_gets_no_interrupted_commit_or_note() {
    let fake = FakeAgent::new();
    scenario(&fake, "work.1", "scenario=hang");
    scenario(&fake, "work.2", "scenario=success");
    let dot = work_node(r#"timeout="120s", max_retries=1"#);
    let mut pas = spawn_run(&fake, &dot);
    let mut agent = KillGroup(Some(agent_pgid(&fake)));
    // The fake has started (its env.log exists) before pas is killed.
    let env_log = fake.scenarios().join("env.log");
    wait_for("the fake to start", Duration::from_secs(20), || {
        env_log.is_file()
    });

    pas.kill().unwrap();
    pas.wait().unwrap();
    agent.kill_now();

    let output = fake.command(&dot).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));

    let commits = engine_commits(&fake);
    assert_eq!(commits.len(), 1, "{commits:#?}");
    assert_eq!(
        (commits[0].status.as_str(), commits[0].attempt.as_str()),
        ("success", "2")
    );
    let prompt = fs::read_to_string(fake.scenarios().join("prompt.work.2")).unwrap();
    assert!(!prompt.contains("interrupted"), "{prompt}");
}

#[test]
fn a_resume_after_a_failed_attempt_makes_no_interrupted_commit() {
    let fake = FakeAgent::new();
    scenario(&fake, "work.1", "scenario=crash");
    scenario(&fake, "work", "scenario=success");
    let dot = work_node(r#"timeout="30s", max_retries=1"#);
    let first = fake.command(&dot).output().unwrap();
    assert!(!first.status.success(), "the crash ends the first run");

    let output = fake.command(&dot).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));

    // The failed attempt was committed when it ended; nothing was left.
    let commits = engine_commits(&fake);
    let summary: Vec<(&str, &str)> = commits
        .iter()
        .map(|c| (c.status.as_str(), c.attempt.as_str()))
        .collect();
    assert_eq!(summary, [("fail", "1"), ("success", "2")], "{commits:#?}");
    let prompt = fs::read_to_string(fake.scenarios().join("prompt.work.2")).unwrap();
    assert!(!prompt.contains("interrupted"), "{prompt}");
}

#[test]
fn repeated_resumes_with_no_attempts_left_record_the_interruption_once() {
    let fake = FakeAgent::new();
    scenario(&fake, "work.1", "scenario=edit_hang");
    // No retries: the SIGKILLed attempt counts, so every resume ends with
    // "Max retries exhausted".
    let dot = work_node(r#"timeout="120s", max_retries=0"#);
    let mut pas = spawn_run(&fake, &dot);
    let mut agent = KillGroup(Some(agent_pgid(&fake)));
    let partial = worktree(&fake).join("partial.txt");
    wait_for("partial.txt", Duration::from_secs(20), || partial.is_file());
    pas.kill().unwrap();
    pas.wait().unwrap();
    agent.kill_now();

    for _ in 0..2 {
        let output = fake.command(&dot).output().unwrap();
        assert!(!output.status.success());
        assert!(
            stderr(&output).contains("Max retries exhausted for node 'work'"),
            "{}",
            stderr(&output)
        );
    }

    let commits = engine_commits(&fake);
    let summary: Vec<(&str, &str)> = commits
        .iter()
        .map(|c| (c.status.as_str(), c.attempt.as_str()))
        .collect();
    assert_eq!(summary, [("interrupted", "1")], "{commits:#?}");
}
