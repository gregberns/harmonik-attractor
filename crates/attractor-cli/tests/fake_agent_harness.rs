#![cfg(unix)]
//! `pas run` end to end with the fake `claude` (`tests/agents/fake-claude`):
//! each test runs a pipeline whose agent nodes name a fake scenario in their
//! prompt, and checks what a caller sees: exit status, stderr, journal, the
//! fake's own logs and the workdir's git history.

mod fake_agent;

use std::fs;
use std::io::Write;
use std::process::Stdio;

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

// Flipped in 02a: an `error_*` subtype is a reported failure (design §5 c).
#[test]
fn error_max_turns_is_a_reported_failure() {
    let fake = FakeAgent::new();
    let output = fake.run(&one_node(
        r#"timeout="30s", prompt="scenario=error_max_turns""#,
    ));

    // A Fail with no matching fail edge follows the unconditional edge and
    // the Run completes, as today (ticket 05 changes that).
    assert!(output.status.success(), "{}", stderr(&output));
    let completed = fake.events_of("StageCompleted");
    let work = completed
        .iter()
        .find(|data| data["node_id"] == "work")
        .unwrap();
    assert_eq!(work["status"], "fail");
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

#[test]
fn the_agent_env_has_the_pas_ids_and_no_api_key() {
    let fake = FakeAgent::new();
    let output = fake
        .command(&one_node(r#"timeout="30s", prompt="scenario=success""#))
        .env("ANTHROPIC_API_KEY", "sk-must-not-reach-the-agent")
        .output()
        .unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    let envs = fake.env_logs();
    assert_eq!(envs.len(), 1, "{envs:?}");
    let env = &envs[0];
    assert_eq!(env["PAS_NODE_ID"], "work");
    assert_eq!(env["PAS_ATTEMPT"], "1");
    assert_eq!(
        env["PAS_RUN_ID"],
        fake.run_json()["run_id"].as_str().unwrap()
    );
    let invoked = fake.events_of("LlmInvoked");
    assert_eq!(invoked.len(), 1, "{invoked:?}");
    assert_eq!(
        env["PAS_INVOCATION_ID"],
        invoked[0]["invocation_id"].as_str().unwrap()
    );
    assert!(!env.contains_key("ANTHROPIC_API_KEY"), "{env:?}");
}

#[test]
fn node_files_pick_a_scenario_per_attempt() {
    let fake = FakeAgent::new();
    fs::write(fake.scenarios().join("work.1"), "scenario=hang\n").unwrap();
    fs::write(fake.scenarios().join("work"), "scenario=success\n").unwrap();
    let output = fake.run(&one_node(r#"timeout="1s", max_retries=1, prompt="do it""#));

    assert!(output.status.success(), "{}", stderr(&output));
    let attempts: Vec<String> = fake
        .env_logs()
        .iter()
        .map(|env| env["PAS_ATTEMPT"].clone())
        .collect();
    assert_eq!(attempts, ["1", "2"]);
    assert_eq!(fake.attempts("hang"), 1);
    assert_eq!(fake.attempts("success"), 1);
}

#[test]
fn the_agent_reads_dev_null_even_when_pas_stdin_is_an_open_pipe() {
    let fake = FakeAgent::new();
    // An inherited stdin would block the agent until the node timeout, and
    // max_retries defaults to 0, so no retry could hide it.
    let mut child = fake
        .command(&one_node(r#"timeout="5s", prompt="scenario=stdin""#))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Write something and keep the pipe open until pas has finished.
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"never read\n").unwrap();
    let output = child.wait_with_output().unwrap();
    drop(stdin);

    assert!(output.status.success(), "{}", stderr(&output));
    let completed = fake.events_of("PipelineCompleted");
    assert_eq!(completed.len(), 1, "the Run did not complete");
    assert!(fake.events_of("PipelineFailed").is_empty());
    assert_eq!(
        fs::read_to_string(fake.scenarios().join("stdin.bytes"))
            .unwrap()
            .trim(),
        "0"
    );
}
