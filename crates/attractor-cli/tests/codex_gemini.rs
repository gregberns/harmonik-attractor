#![cfg(unix)]
//! Codex and Gemini through `pas run` (ticket 04): the `codex-exec` and
//! `gemini` handlers with `tests/agents/fake-codex` and `fake-gemini`,
//! selected by the harness's `fake-codex`/`fake-gemini` profiles, and the
//! `llm_provider` aliases with the fakes shimmed on `PATH`.

// This crate uses a subset of the shared harness.
#[allow(dead_code)]
mod fake_agent;

use std::fs;

use fake_agent::{stderr, FakeAgent};

/// start -> work -> next -> done, both nodes on `profile`; `next` sees
/// `work`'s result in its prompt.
fn work_then_next(profile: &str, work: &str) -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            work [shape="box", agent="{profile}", timeout="30s", prompt="scenario={work}"]
            next [shape="box", agent="{profile}", timeout="30s", prompt="scenario=success"]
            done [shape="Msquare"]
            start -> work -> next -> done
        }}"#
    )
}

/// start -> work, then `on_ok` or `on_fail` by outcome.
fn routed(profile: &str, work: &str) -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            work [shape="box", agent="{profile}", timeout="30s", prompt="scenario={work}"]
            on_ok [shape="parallelogram", tool_command="true"]
            on_fail [shape="parallelogram", tool_command="true"]
            done [shape="Msquare"]
            start -> work
            work -> on_ok [condition="outcome=success"]
            work -> on_fail [condition="outcome=fail"]
            on_ok -> done
            on_fail -> done
        }}"#
    )
}

/// start -> work -> done with `attrs` on `work`.
fn one_node(attrs: &str) -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            work [shape="box", timeout="30s", {attrs}]
            done [shape="Msquare"]
            start -> work -> done
        }}"#
    )
}

fn assert_success(output: &std::process::Output) {
    assert!(output.status.success(), "{}", stderr(output));
}

fn assert_routed_to(fake: &FakeAgent, node: &str, not: &str) {
    let stages = fake.stages_started();
    assert!(stages.contains(&node.to_string()), "{stages:?}");
    assert!(!stages.contains(&not.to_string()), "{stages:?}");
}

/// `next`'s prompt carries `work`'s result text.
fn assert_next_saw(fake: &FakeAgent, text: &str) {
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    assert!(
        prompts[1].contains(&format!("- work.result: {text}\n")),
        "{}",
        prompts[1]
    );
}

// --- Codex ---

#[test]
fn codex_success_completes_with_the_fakes_text() {
    let fake = FakeAgent::new();
    assert_success(&fake.run(&work_then_next("fake-codex", "success")));
    assert_next_saw(&fake, "fake-codex: success");
    assert_eq!(fake.events_of("LlmInvoked")[0]["provider"], "fake-codex");
}

#[test]
fn codex_turn_failed_routes_on_the_fail_edge() {
    let fake = FakeAgent::new();
    assert_success(&fake.run(&routed("fake-codex", "turn_failed")));
    assert_routed_to(&fake, "on_fail", "on_ok");
}

#[test]
fn codex_crash_stops_the_run_with_the_crash_message() {
    let fake = FakeAgent::new();
    let output = fake.run(&one_node(r#"agent="fake-codex", prompt="scenario=crash""#));
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(
        err.contains("attempt 1: Codex CLI exited with exit status: 3; last stderr lines:\nfake-codex: crashed\ntranscript "),
        "{err}"
    );
}

// --- Gemini ---

#[test]
fn gemini_success_completes_with_stream_json_by_probe() {
    let fake = FakeAgent::new();
    assert_success(&fake.run(&work_then_next("fake-gemini", "success")));
    assert_next_saw(&fake, "fake-gemini: success");
    // Probed once for the two invocations; it chose stream-json.
    let probes = fs::read_to_string(fake.scenarios().join("probes.log")).unwrap();
    assert_eq!(probes.lines().count(), 1, "{probes}");
    assert_eq!(
        fake.invocations()[0][..2],
        ["--output-format", "stream-json"]
    );
}

#[test]
fn gemini_failure_routes_on_the_fail_edge_in_both_formats() {
    for help in ["stream", "json"] {
        let fake = FakeAgent::new();
        fs::write(fake.scenarios().join("gemini-help"), format!("{help}\n")).unwrap();
        assert_success(&fake.run(&routed("fake-gemini", "failure")));
        assert_routed_to(&fake, "on_fail", "on_ok");
    }
}

// --- The llm_provider aliases: the built-in profiles, today's argv ---

#[test]
fn llm_provider_codex_runs_the_built_in_profile_with_todays_argv() {
    let fake = FakeAgent::new();
    fake.shim_codex_on_path();

    let output = fake.run(&one_node(
        r#"llm_provider="openai", llm_model="o3", prompt="scenario=success""#,
    ));

    assert_success(&output);
    let worktree = fake.run_meta()["worktree"].as_str().unwrap().to_string();
    let expected = [
        "exec",
        "--json",
        "--yolo",
        "--skip-git-repo-check",
        "--model",
        "o3",
        "--cd",
        worktree.as_str(),
        "<prompt>",
    ];
    assert_eq!(
        fake.invocations(),
        vec![expected.map(String::from).to_vec()]
    );
    assert_eq!(fake.events_of("LlmInvoked")[0]["provider"], "codex");
}

#[test]
fn llm_provider_gemini_runs_the_built_in_profile_with_todays_argv() {
    let fake = FakeAgent::new();
    fake.shim_gemini_on_path();

    let output = fake.run(&one_node(
        r#"llm_provider="gemini", llm_model="gemini-2.5-pro", prompt="scenario=success""#,
    ));

    assert_success(&output);
    let expected = [
        "--output-format",
        "stream-json",
        "--approval-mode",
        "yolo",
        "--model",
        "gemini-2.5-pro",
        "<prompt>",
    ];
    assert_eq!(
        fake.invocations(),
        vec![expected.map(String::from).to_vec()]
    );
    assert_eq!(fake.events_of("LlmInvoked")[0]["provider"], "gemini");
}

#[test]
fn the_codex_api_key_is_stripped_from_the_agent() {
    let fake = FakeAgent::new();
    let output = fake
        .command(&one_node(
            r#"agent="fake-codex", prompt="scenario=success""#,
        ))
        .env("OPENAI_API_KEY", "sk-test")
        .output()
        .unwrap();
    assert_success(&output);
    assert!(!fake.env_logs()[0].contains_key("OPENAI_API_KEY"));
}
