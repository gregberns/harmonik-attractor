#![cfg(unix)]
//! `pas run` end to end with the fake `claude`: the error that ends a Run
//! names the node, the attempt and the agent's files (ticket 05).

#[allow(dead_code)]
mod fake_agent;

use std::path::PathBuf;

use serde_json::Value;

use fake_agent::{stderr, FakeAgent};

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

/// The absolute transcript and stderr paths of one `LlmStarted`, from the
/// Run folder as `pas` names it (canonical: on macOS the temp folder is
/// under `/private`).
fn files_of(fake: &FakeAgent, started: &Value) -> (PathBuf, PathBuf) {
    let run_dir = std::fs::canonicalize(&fake.run_dirs()[0]).unwrap();
    let transcript = run_dir.join(started["transcript"].as_str().unwrap());
    let stderr = run_dir.join(started["stderr"].as_str().unwrap());
    (transcript, stderr)
}

/// `PipelineFailed`'s error.
fn pipeline_error(fake: &FakeAgent) -> String {
    let failed = fake.events_of("PipelineFailed");
    assert_eq!(failed.len(), 1, "{failed:?}");
    failed[0]["error"].as_str().unwrap().to_string()
}

#[test]
fn an_agent_timeout_names_the_node_the_attempt_and_its_files() {
    let fake = FakeAgent::new();
    let output = fake.run(&one_node(
        r#"timeout="1s", max_retries=1, prompt="scenario=hang""#,
    ));
    let stderr = stderr(&output);
    assert!(!output.status.success(), "{stderr}");

    let started = fake.events_of("LlmStarted");
    let attempts: Vec<u64> = started
        .iter()
        .map(|data| data["attempt"].as_u64().unwrap())
        .collect();
    assert_eq!(attempts, [1, 2], "{started:?}");
    let (transcript, stderr_log) = files_of(&fake, &started[1]);
    assert!(transcript.is_file(), "{}", transcript.display());
    assert!(stderr_log.is_file(), "{}", stderr_log.display());

    let message = format!(
        "node 'work' attempt 2 failed: timeout after 1000ms; transcript {}; stderr {}",
        transcript.display(),
        stderr_log.display()
    );
    assert!(stderr.contains(&message), "{stderr}");
    assert_eq!(pipeline_error(&fake), message);
}

#[test]
fn an_agent_crash_names_the_exit_status_its_stderr_and_its_files() {
    let fake = FakeAgent::new();
    let output = fake.run(&one_node(r#"timeout="30s", prompt="scenario=crash""#));
    let stderr = stderr(&output);
    assert!(!output.status.success(), "{stderr}");

    let started = fake.events_of("LlmStarted");
    assert_eq!(started.len(), 1, "{started:?}");
    let (transcript, stderr_log) = files_of(&fake, &started[0]);

    let message = format!(
        "Handler 'codergen' failed on node 'work': attempt 1: \
         Claude Code exited with exit status: 3; last stderr lines:\n\
         fake-claude: crashed\n\
         transcript {}; stderr {}",
        transcript.display(),
        stderr_log.display()
    );
    assert!(stderr.contains(&message), "{stderr}");
    assert_eq!(pipeline_error(&fake), message);
}
