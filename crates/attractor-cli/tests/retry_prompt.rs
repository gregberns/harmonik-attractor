#![cfg(unix)]
//! The retry prompt names the previous failure (ticket 11, design §1
//! "Prompt on a re-run"), through `pas run` with the fake, read from the
//! prompt files the fake writes per start.

// This crate uses a subset of the shared harness.
#[allow(dead_code)]
mod fake_agent;

use std::fs;
use std::process::{Child, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use fake_agent::{stderr, FakeAgent};
use serde_json::Value;

/// The phrase every failure note starts with.
const PREVIOUS: &str = "The previous attempt";

fn assert_success(output: &std::process::Output) {
    assert!(output.status.success(), "{}", stderr(output));
}

/// The scenario line the fake reads for one start of a node (`work@2`) or
/// one attempt (`work.2`).
fn scenario(fake: &FakeAgent, file: &str, line: &str) {
    fs::write(fake.scenarios().join(file), format!("{line}\n")).unwrap();
}

/// The prompt the fake got, from its `prompt.<file>` (e.g. `work@2`).
fn prompt(fake: &FakeAgent, file: &str) -> String {
    let path = fake.scenarios().join(format!("prompt.{file}"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `work` loops back to itself on a fail, and ends on a success.
fn fail_loop(timeout: &str) -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            work [shape="box", agent="fake", timeout="{timeout}", prompt="do it"]
            done [shape="Msquare"]
            start -> work
            work -> work [condition="outcome=fail"]
            work -> done [condition="outcome=success"]
        }}"#
    )
}

#[test]
fn a_retry_after_a_timeout_says_it_timed_out() {
    let fake = FakeAgent::new();
    fake.commit_pas_toml("kill_grace = \"1s\"");
    scenario(&fake, "work.1", "scenario=hang");
    scenario(&fake, "work.2", "scenario=success");

    assert_success(&fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="box", agent="fake", timeout="1s", max_retries=1, prompt="do it"]
            done [shape="Msquare"]
            start -> work -> done
        }"#,
    ));

    assert!(!prompt(&fake, "work.1").contains(PREVIOUS));
    assert_eq!(
        prompt(&fake, "work.2"),
        "Task (work): do it\n\nThe previous attempt timed out after 1 s.\n"
    );
}

#[test]
fn a_rerun_after_a_reported_failure_names_the_class_and_reason() {
    let fake = FakeAgent::new();
    scenario(&fake, "work@1", "scenario=fail");
    scenario(&fake, "work@2", "scenario=success");

    assert_success(&fake.run(&fail_loop("30s")));

    let second = prompt(&fake, "work@2");
    assert!(
        second.contains("The previous attempt failed (reported): fake-claude: failed"),
        "{second}"
    );
    assert!(!prompt(&fake, "work@1").contains(PREVIOUS));
}

#[test]
fn a_first_attempts_prompt_is_unchanged() {
    let fake = FakeAgent::new();
    scenario(&fake, "work.1", "scenario=success");

    assert_success(&fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="box", agent="fake", timeout="30s", prompt="do it"]
            done [shape="Msquare"]
            start -> work -> done
        }"#,
    ));

    // Exactly what the engine assembled before this ticket.
    assert_eq!(prompt(&fake, "work.1"), "Task (work): do it\n");
}

/// `implement` -> `review`, which sends it back once (`again`), then ends
/// (`done`); `implement` fails or succeeds per start as the test sets.
fn review_loop() -> &'static str {
    r#"digraph G {
        start [shape="Mdiamond"]
        implement [shape="box", agent="fake", timeout="30s", prompt="build it"]
        review [shape="diamond", agent="fake", timeout="30s", prompt="review it"]
        done [shape="Msquare"]
        start -> implement
        implement -> implement [condition="outcome=fail"]
        implement -> review [condition="outcome=success"]
        review -> implement [label="again"]
        review -> done [label="done"]
    }"#
}

#[test]
fn a_loop_back_after_a_success_gets_no_note() {
    let fake = FakeAgent::new();
    scenario(&fake, "implement@1", "scenario=success");
    scenario(&fake, "implement@2", "scenario=success");
    scenario(&fake, "review@1", "scenario=label label=again");
    scenario(&fake, "review@2", "scenario=label label=done");

    assert_success(&fake.run(review_loop()));

    assert!(!prompt(&fake, "implement@2").contains(PREVIOUS));
}

#[test]
fn a_success_clears_the_note() {
    let fake = FakeAgent::new();
    scenario(&fake, "implement@1", "scenario=fail");
    scenario(&fake, "implement@2", "scenario=success");
    scenario(&fake, "implement@3", "scenario=success");
    scenario(&fake, "review@1", "scenario=label label=again");
    scenario(&fake, "review@2", "scenario=label label=done");

    assert_success(&fake.run(review_loop()));

    assert!(prompt(&fake, "implement@2").contains("The previous attempt failed (reported)"));
    // Back from review after implement@2 succeeded: no note.
    assert!(!prompt(&fake, "implement@3").contains(PREVIOUS));
    // The note never reaches another node's prompt.
    assert!(!prompt(&fake, "review@1").contains(PREVIOUS));
}

// --- Resume ---

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The journal so far, tolerating a partly written last line.
fn journal_so_far(fake: &FakeAgent) -> Vec<Value> {
    let Some(run) = fake.run_dirs().into_iter().next() else {
        return Vec::new();
    };
    fs::read_to_string(run.join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn wait_exit(child: &mut Child) -> ExitStatus {
    let mut status = None;
    wait_until("pas run to exit", || {
        status = child.try_wait().unwrap();
        status.is_some()
    });
    status.unwrap()
}

#[test]
fn the_first_attempt_after_a_resume_names_the_failure_before_the_stop() {
    let fake = FakeAgent::new();
    scenario(&fake, "work@1", "scenario=fail");
    scenario(&fake, "work@2", "scenario=hang_term");
    let dot = fail_loop("120s");

    let mut run = fake
        .command(&dot)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // LlmStarted is journalled at spawn, before the fake has counted the
    // start or set hang_term's TERM trap; its init line comes after both.
    wait_until("the second start's init line", || {
        let started: Vec<Value> = journal_so_far(&fake)
            .into_iter()
            .filter(|e| e["type"] == "LlmStarted")
            .collect();
        let (Some(second), Some(run)) = (started.get(1), fake.run_dirs().into_iter().next()) else {
            return false;
        };
        let transcript = run.join(second["data"]["transcript"].as_str().unwrap());
        fs::read_to_string(transcript).is_ok_and(|text| text.contains(r#""subtype":"init""#))
    });
    // SAFETY: sending a signal to our own child's pid.
    let sent = unsafe { libc::kill(run.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(sent, 0);
    assert_eq!(wait_exit(&mut run).code(), Some(143));

    // A stopped attempt changes nothing: the note is still work@1's failure.
    scenario(&fake, "work@3", "scenario=success");
    assert_success(&fake.run(&dot));

    let resumed = prompt(&fake, "work@3");
    assert!(
        resumed.contains("The previous attempt failed (reported): fake-claude: failed"),
        "{resumed}"
    );
}
