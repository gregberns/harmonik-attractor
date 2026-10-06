#![cfg(unix)]
//! `pas run` end to end with the fake `claude`: the error that ends a Run
//! names the node, the attempt and the agent's files; a node whose edges match
//! nothing stops the Run; tool nodes route as before (ticket 05).

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

// --- Fix (a): no first-edge fallback ---

/// Runs `dot` and expects it to fail; returns `pas run`'s stderr.
fn run_failing(fake: &FakeAgent, dot: &str) -> String {
    let output = fake.run(dot);
    assert!(!output.status.success(), "{}", stderr(&output));
    stderr(&output)
}

/// The position of the first Event of type `kind` (for node `node`, if given).
fn position(events: &[Value], kind: &str, node: Option<&str>) -> Option<usize> {
    events
        .iter()
        .position(|e| e["type"] == kind && node.is_none_or(|n| e["data"]["node_id"] == n))
}

#[test]
fn a_fail_with_only_a_success_edge_stops_the_run() {
    let fake = FakeAgent::new();
    let stderr = run_failing(
        &fake,
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=fail"]
            next [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            done [shape="Msquare"]
            start -> work
            work -> next [condition="outcome=success"]
            next -> done
        }"#,
    );

    let error = "node 'work' outcome fail matched no outgoing edge (conditions: outcome=success)";
    assert!(stderr.contains(error), "{stderr}");
    let events = fake.events();
    let completed = position(&events, "StageCompleted", Some("work")).unwrap();
    assert_eq!(events[completed]["data"]["status"], "fail");
    let failed = position(&events, "PipelineFailed", None).unwrap();
    assert!(completed < failed);
    assert_eq!(events[failed]["data"]["error"], error);
    assert!(position(&events, "StageFailed", Some("work")).is_none());
    assert!(!fake.stages_started().contains(&"next".to_string()));
}

#[test]
fn a_success_with_only_non_matching_edges_stops_the_run() {
    let fake = FakeAgent::new();
    let stderr = run_failing(
        &fake,
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            fix [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            done [shape="Msquare"]
            start -> work
            work -> fix [condition="outcome=fail"]
            fix -> done
        }"#,
    );

    assert!(
        stderr.contains(
            "node 'work' outcome success matched no outgoing edge (conditions: outcome=fail)"
        ),
        "{stderr}"
    );
    assert!(!fake.stages_started().contains(&"fix".to_string()));
}

#[test]
fn a_failed_diamond_agent_ending_with_pass_does_not_take_the_pass_edge() {
    let fake = FakeAgent::new();
    let stderr = run_failing(
        &fake,
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="diamond", llm_provider="claude", timeout="30s", prompt="scenario=fail label=PASS"]
            next [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            done [shape="Msquare"]
            start -> work
            work -> next [label="PASS", condition="outcome=success"]
            next -> done
        }"#,
    );

    assert!(
        stderr.contains("node 'work' outcome fail matched no outgoing edge"),
        "{stderr}"
    );
    assert!(!fake.stages_started().contains(&"next".to_string()));
}

// As in the documented examples, each edge carries the label too: the label
// is extracted from the agent's text only among the edges' `label`s.
#[test]
fn preferred_label_conditions_still_route() {
    let fake = FakeAgent::new();
    let output = fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="diamond", llm_provider="claude", timeout="30s", prompt="scenario=label label=beta_route"]
            a [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            b [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            done [shape="Msquare"]
            start -> work
            work -> a [label="alpha_route", condition="preferred_label=alpha_route"]
            work -> b [label="beta_route", condition="preferred_label=beta_route"]
            a -> done
            b -> done
        }"#,
    );

    assert!(output.status.success(), "{}", stderr(&output));
    let stages = fake.stages_started();
    assert!(stages.contains(&"b".to_string()), "{stages:?}");
    assert!(!stages.contains(&"a".to_string()), "{stages:?}");
}

#[test]
fn a_resume_after_no_matching_edge_does_not_run_the_node_again() {
    let fake = FakeAgent::new();
    let dot = r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=fail"]
            next [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            done [shape="Msquare"]
            start -> work
            work -> next [condition="outcome=success"]
            next -> done
        }"#;
    run_failing(&fake, dot);
    let started = fake.events_of("LlmStarted").len();

    let stderr = run_failing(&fake, dot);

    assert!(
        stderr.contains("Max retries exhausted for node 'work'"),
        "{stderr}"
    );
    assert_eq!(fake.events_of("LlmStarted").len(), started);
}

// --- Tool nodes are unchanged ---

#[test]
fn a_tool_node_exit_still_routes_on_an_outcome_fail_edge() {
    let fake = FakeAgent::new();
    let output = fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            check [shape="parallelogram", timeout="30s", tool_command="exit 3"]
            fixup [shape="parallelogram", timeout="30s", tool_command="true"]
            ok [shape="parallelogram", timeout="30s", tool_command="true"]
            done [shape="Msquare"]
            start -> check
            check -> fixup [condition="outcome=fail"]
            check -> ok [condition="outcome=success"]
            fixup -> done
            ok -> done
        }"#,
    );

    assert!(output.status.success(), "{}", stderr(&output));
    let stages = fake.stages_started();
    assert!(stages.contains(&"fixup".to_string()), "{stages:?}");
    assert!(!stages.contains(&"ok".to_string()), "{stages:?}");
}

#[test]
fn a_tool_node_timeout_still_stops_the_run_with_todays_message() {
    let fake = FakeAgent::new();
    let stderr = run_failing(
        &fake,
        r#"digraph G {
            start [shape="Mdiamond"]
            slow [shape="parallelogram", timeout="1s", tool_command="exec sleep 5"]
            done [shape="Msquare"]
            start -> slow -> done
        }"#,
    );
    assert!(
        stderr.contains("Command timed out after 1000ms"),
        "{stderr}"
    );
}
