#![cfg(unix)]
//! `pas run` end to end with the fake `claude` (`tests/agents/fake-claude`):
//! each test runs a pipeline whose agent nodes name a fake scenario in their
//! prompt, and checks what a caller sees: exit status, stderr, journal, the
//! fake's own logs and the workdir's git history.

mod fake_agent;

use fake_agent::{stderr, FakeAgent};

#[test]
fn success_completes_with_fake_result_text() {
    let fake = FakeAgent::new();
    // `next` sees `work`'s result in its prompt, the only place a caller can
    // read a node's result text after a successful Run.
    let output = fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            next [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            done [shape="Msquare"]
            start -> work -> next -> done
        }"#,
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fake.attempts("success"), 2);
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[1].contains("- work.result: fake-claude: success\n"),
        "{}",
        prompts[1]
    );
    let invocations = fake.invocations();
    let argv = &invocations[0];
    assert!(argv.contains(&"-p".to_string()), "{argv:?}");
    let format = argv.iter().position(|a| a == "--output-format").unwrap();
    assert_eq!(argv[format + 1], "stream-json");
}

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

/// Runs a one-node pipeline expected to fail; returns its stderr.
fn failed_run(fake: &FakeAgent, attrs: &str) -> String {
    let output = fake.run(&one_node(attrs));
    assert!(!output.status.success(), "{}", stderr(&output));
    stderr(&output)
}

#[test]
fn reported_failure_routes_on_outcome_fail_edge() {
    let fake = FakeAgent::new();
    let output = fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=fail"]
            on_ok [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            on_fail [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            done [shape="Msquare"]
            start -> work
            work -> on_ok [condition="outcome=success"]
            work -> on_fail [condition="outcome=fail"]
            on_ok -> done
            on_fail -> done
        }"#,
    );

    assert!(output.status.success(), "{}", stderr(&output));
    let stages = fake.stages_started();
    assert!(stages.contains(&"on_fail".to_string()), "{stages:?}");
    assert!(!stages.contains(&"on_ok".to_string()), "{stages:?}");
}

#[test]
fn label_routing_follows_the_labelled_edge() {
    let fake = FakeAgent::new();
    // beta_route is the second edge, so neither edge order nor a fallback
    // to the first edge can pick it by chance.
    let output = fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="diamond", llm_provider="claude", timeout="30s", prompt="scenario=label label=beta_route"]
            alpha [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            beta [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            done [shape="Msquare"]
            start -> work
            work -> alpha [label="alpha_route", condition="preferred_label=alpha_route"]
            work -> beta [label="beta_route", condition="preferred_label=beta_route"]
            alpha -> done
            beta -> done
        }"#,
    );

    assert!(output.status.success(), "{}", stderr(&output));
    let stages = fake.stages_started();
    assert!(stages.contains(&"beta".to_string()), "{stages:?}");
    assert!(!stages.contains(&"alpha".to_string()), "{stages:?}");
}

#[test]
fn crash_ends_the_run_with_the_exit_status_and_stderr() {
    let fake = FakeAgent::new();
    let stderr = failed_run(&fake, r#"timeout="30s", prompt="scenario=crash""#);
    assert!(stderr.contains("Claude Code exited with"), "{stderr}");
    assert!(stderr.contains("exit status: 3"), "{stderr}");
    assert!(stderr.contains("fake-claude: crashed"), "{stderr}");
}

#[test]
fn garbage_ends_the_run_with_a_parse_error() {
    let fake = FakeAgent::new();
    let stderr = failed_run(&fake, r#"timeout="30s", prompt="scenario=garbage""#);
    assert!(stderr.contains("Failed to parse Claude output"), "{stderr}");
}

#[test]
fn silent_ends_the_run_with_a_no_output_error() {
    let fake = FakeAgent::new();
    let stderr = failed_run(&fake, r#"timeout="30s", prompt="scenario=silent""#);
    assert!(
        stderr.contains("Claude Code produced no output"),
        "{stderr}"
    );
}

#[test]
fn hang_ends_the_run_with_the_timeout_error() {
    let fake = FakeAgent::new();
    let stderr = failed_run(&fake, r#"timeout="1s", prompt="scenario=hang""#);
    assert!(
        stderr.contains("Command timed out after 1000ms"),
        "{stderr}"
    );
    assert_eq!(fake.attempts("hang"), 1);
}

#[test]
fn edit_and_commit_leaves_the_file_and_commit_in_the_workdir() {
    let fake = FakeAgent::new();
    let output = fake.run(&one_node(r#"timeout="30s", prompt="scenario=edit_commit""#));

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(fake.repo().join("fake-edit.txt").is_file());
    assert_eq!(fake.git(&["log", "-1", "--format=%s"]), "fake-claude edit");
    let head = fake.git(&["rev-parse", "HEAD"]);
    let created = fake.events_of("CommitsCreated");
    assert_eq!(created.len(), 1, "{created:?}");
    assert_eq!(created[0]["node_id"], "work");
    assert_eq!(created[0]["commits"][0]["sha"], head.as_str());
    assert_eq!(created[0]["commits"][0]["subject"], "fake-claude edit");
}

#[test]
fn flaky_is_retried_on_timeout_then_succeeds() {
    let fake = FakeAgent::new();
    let output = fake.run(&one_node(
        r#"timeout="1s", max_retries=2, prompt="scenario=flaky fails=1""#,
    ));

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fake.attempts("flaky"), 2);
    let retrying = fake.events_of("StageRetrying");
    assert_eq!(retrying.len(), 1, "{retrying:?}");
    assert_eq!(retrying[0]["node_id"], "work");
}

#[test]
fn flaky_past_its_retries_ends_the_run_with_the_timeout_error() {
    let fake = FakeAgent::new();
    // Today the last attempt's own CommandTimeout ends the Run, not
    // RetriesExhausted.
    let stderr = failed_run(
        &fake,
        r#"timeout="1s", max_retries=1, prompt="scenario=flaky fails=3""#,
    );
    assert!(
        stderr.contains("Command timed out after 1000ms"),
        "{stderr}"
    );
    assert_eq!(fake.attempts("flaky"), 2);
}

#[test]
fn error_max_turns_counts_as_success_today() {
    let fake = FakeAgent::new();
    let output = fake.run(&one_node(
        r#"timeout="30s", prompt="scenario=error_max_turns""#,
    ));

    // FLIPS IN 02a: error_* subtypes become failures (design §5 c).
    assert!(output.status.success(), "{}", stderr(&output));
    let completed = fake.events_of("StageCompleted");
    let work = completed
        .iter()
        .find(|data| data["node_id"] == "work")
        .unwrap();
    assert_eq!(work["status"], "success");
}

// Permanent, not a 02a flip: a final result wins over the exit status
// (design §5, "Completed, even after a non-zero exit").
#[test]
fn crash_after_result_counts_as_success() {
    let fake = FakeAgent::new();
    let output = fake.run(&one_node(
        r#"timeout="30s", prompt="scenario=crash_after_result""#,
    ));

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fake.attempts("crash_after_result"), 1);
}

#[test]
fn slow_stream_completes() {
    let fake = FakeAgent::new();
    let output = fake.run(&one_node(r#"timeout="30s", prompt="scenario=slow""#));

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fake.attempts("slow"), 1);
}
