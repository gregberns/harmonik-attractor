#![cfg(unix)]
//! `pas run` end to end with the fake `claude` (`tests/agents/fake-claude`):
//! each test runs a pipeline whose agent nodes name a fake scenario in their
//! prompt, and checks what a caller sees: exit status, stderr, journal, the
//! fake's own logs and the workdir's git history.

mod common;
mod fake_agent;

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

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
    // The Run works in its own worktree (ticket 06).
    let wt = fake.worktree();
    assert!(wt.join("fake-edit.txt").is_file());
    assert_eq!(
        fake.git_in(&wt, &["log", "-1", "--format=%s"]),
        "fake-claude edit"
    );
    let head = fake.git_in(&wt, &["rev-parse", "HEAD"]);
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

// --- 02b: LlmStarted, live stderr, TERM before KILL, graceful stop ---

/// Polls `ready` until it holds, failing after `within`.
fn wait_for(what: &str, within: Duration, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
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

fn has_event(fake: &FakeAgent, kind: &str) -> bool {
    journal_so_far(fake).iter().any(|e| e["type"] == kind)
}

/// Starts `pas run` in the background.
fn spawn_run(fake: &FakeAgent, dot: &str) -> Child {
    fake.command(dot)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn sigterm(child: &Child) {
    // SAFETY: sending a signal to our own child's pid.
    let sent = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(sent, 0, "kill -TERM failed");
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

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// The fake's pid in its last start.
fn fake_pid(fake: &FakeAgent) -> u32 {
    fake.env_logs().last().unwrap()["FAKE_PID"].parse().unwrap()
}

/// The node's scenario comes from the file `work` in the scenario folder.
fn scenario_file(fake: &FakeAgent, line: &str) {
    fs::write(fake.scenarios().join("work"), format!("{line}\n")).unwrap();
}

#[test]
fn llm_started_is_journalled_with_the_fakes_pid_and_paths() {
    let fake = FakeAgent::new();
    let output = fake.run(&one_node(r#"timeout="30s", prompt="scenario=success""#));
    assert!(output.status.success(), "{}", stderr(&output));

    // The ticket's "before the first transcript line" is proved at seam 1
    // (claude_p.rs: the transcript is empty when `started` runs); file
    // timing can't show it from here.
    let events = fake.events();
    let started_at = events.iter().position(|e| e["type"] == "LlmStarted");
    let invoked_at = events.iter().position(|e| e["type"] == "LlmInvoked");
    let completed_at = events
        .iter()
        .position(|e| e["type"] == "StageCompleted" && e["data"]["node_id"] == "work");
    assert!(started_at < invoked_at && invoked_at < completed_at);
    let started = &events[started_at.unwrap()]["data"];
    let invoked = &events[invoked_at.unwrap()]["data"];
    assert_eq!(started["pid"], fake_pid(&fake));
    assert_eq!(started["pgid"], started["pid"]);
    assert_eq!(started["node_id"], "work");
    assert_eq!(started["attempt"], 1);
    assert_eq!(started["spawn"], 1);
    assert_eq!(started["profile"], "claude");
    assert_eq!(started["invocation_id"], invoked["invocation_id"]);
    let run = &fake.run_dirs()[0];
    for key in ["transcript", "stderr"] {
        let path = run.join(started[key].as_str().unwrap());
        assert!(path.exists(), "{key}: {}", path.display());
    }
}

#[test]
fn each_attempt_has_its_own_llm_started() {
    let fake = FakeAgent::new();
    fs::write(fake.scenarios().join("work.1"), "scenario=hang\n").unwrap();
    scenario_file(&fake, "scenario=success");
    let output = fake.run(&one_node(r#"timeout="1s", max_retries=1, prompt="do it""#));
    assert!(output.status.success(), "{}", stderr(&output));

    let started = fake.events_of("LlmStarted");
    let attempts: Vec<&Value> = started.iter().map(|s| &s["attempt"]).collect();
    assert_eq!(attempts, [1, 2]);
    assert_ne!(started[0]["invocation_id"], started[1]["invocation_id"]);
}

#[test]
fn a_crash_leaves_its_stderr_log() {
    let fake = FakeAgent::new();
    let output = fake.run(&one_node(r#"timeout="30s", prompt="scenario=crash""#));
    assert!(!output.status.success());

    let started = fake.events_of("LlmStarted");
    let log = fake.run_dirs()[0].join(started[0]["stderr"].as_str().unwrap());
    let text = fs::read_to_string(&log).unwrap();
    assert!(text.contains("fake-claude: crashed"), "{text}");
}

#[test]
fn a_timeout_sends_term_first() {
    let fake = FakeAgent::new();
    // If the engine's own deadline still raced the agent's, the agent would
    // be dropped and SIGKILLed: no TERM, no marker.
    let stderr = failed_run(&fake, r#"timeout="1s", prompt="scenario=hang_term""#);
    assert!(
        stderr.contains("Command timed out after 1000ms"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(fake.scenarios().join("term"))
            .unwrap()
            .trim(),
        "term"
    );
}

// Waits the built-in 10 s grace; ticket 03 shortens it with a `fake` profile.
#[test]
fn an_agent_that_ignores_term_is_killed_after_the_grace() {
    let fake = FakeAgent::new();
    let stderr = failed_run(&fake, r#"timeout="1s", prompt="scenario=hang_ignore_term""#);
    assert!(
        stderr.contains("Command timed out after 1000ms"),
        "{stderr}"
    );
    assert!(fs::read_to_string(fake.scenarios().join("term"))
        .unwrap()
        .contains("term"));
    assert!(!alive(fake_pid(&fake)));
}

#[test]
fn a_stop_sends_term_to_the_agent_first() {
    let fake = FakeAgent::new();
    let mut child = spawn_run(
        &fake,
        &one_node(r#"timeout="120s", prompt="scenario=hang_term""#),
    );
    wait_for("LlmStarted", Duration::from_secs(20), || {
        has_event(&fake, "LlmStarted")
    });
    // The trap is set before the init line reaches the transcript.
    let run = fake.run_dirs()[0].clone();
    let transcript: PathBuf = run.join(
        fake.events_of("LlmStarted")[0]["transcript"]
            .as_str()
            .unwrap(),
    );
    wait_for("the init line", Duration::from_secs(20), || {
        fs::read_to_string(&transcript).is_ok_and(|s| s.contains("init"))
    });

    sigterm(&child);
    let status = wait_exit(&mut child, Duration::from_secs(20));

    assert_eq!(status.code(), Some(143), "{status:?}");
    assert_eq!(
        fs::read_to_string(fake.scenarios().join("term"))
            .unwrap()
            .trim(),
        "term"
    );
    let events = fake.events();
    let last = events.last().unwrap();
    assert_eq!(last["type"], "AttemptEnded");
    assert_eq!(last["data"]["reason"], "stopped");
    assert!(fake.events_of("StageFailed").is_empty());
    assert!(fake.events_of("PipelineFailed").is_empty());
}

// Waits the built-in 10 s grace; ticket 03 shortens it with a `fake` profile.
#[test]
fn a_stop_still_kills_an_agent_that_ignores_term() {
    let fake = FakeAgent::new();
    let mut child = spawn_run(
        &fake,
        &one_node(r#"timeout="120s", prompt="scenario=hang_ignore_term""#),
    );
    wait_for("LlmStarted", Duration::from_secs(20), || {
        has_event(&fake, "LlmStarted")
    });
    // From LlmStarted, not env.log: LlmStarted is journalled at spawn, before
    // the fake has written env.log.
    let pid = fake.events_of("LlmStarted")[0]["pid"].as_u64().unwrap() as u32;

    sigterm(&child);
    let status = wait_exit(&mut child, Duration::from_secs(40));

    assert_eq!(status.code(), Some(143), "{status:?}");
    // pas has exited, so init reaps the fake: gone means gone.
    wait_for("the fake to die", Duration::from_secs(5), || !alive(pid));
}

#[test]
fn a_stop_between_nodes_starts_nothing_new() {
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
    let mut child = spawn_run(&fake, &dot);
    wait_for("wait to start", Duration::from_secs(20), || {
        journal_so_far(&fake)
            .iter()
            .any(|e| e["type"] == "StageStarted" && e["data"]["node_id"] == "wait")
    });

    sigterm(&child);
    // Let the tool node finish now: the engine must not move on to `work`.
    fs::write(fake.worktree().join("go"), "").unwrap();
    let status = wait_exit(&mut child, Duration::from_secs(20));

    assert_eq!(status.code(), Some(143), "{status:?}");
    let completed: Vec<Value> = fake.events_of("StageCompleted");
    assert!(
        completed.iter().all(|e| e["node_id"] == "start"),
        "{completed:?}"
    );
    assert!(fake.events_of("LlmStarted").is_empty());
    assert!(!fake.scenarios().join("env.log").exists());
}

#[test]
fn a_resume_after_a_stop_runs_the_stopped_node_again() {
    let fake = FakeAgent::new();
    scenario_file(&fake, "scenario=hang_term");
    let dot = one_node(r#"timeout="120s", prompt="do it""#);
    let mut child = spawn_run(&fake, &dot);
    wait_for("LlmStarted", Duration::from_secs(20), || {
        has_event(&fake, "LlmStarted")
    });
    let run = fake.run_dirs()[0].clone();
    let transcript = run.join(
        fake.events_of("LlmStarted")[0]["transcript"]
            .as_str()
            .unwrap(),
    );
    wait_for("the init line", Duration::from_secs(20), || {
        fs::read_to_string(&transcript).is_ok_and(|s| s.contains("init"))
    });
    sigterm(&child);
    assert_eq!(
        wait_exit(&mut child, Duration::from_secs(20)).code(),
        Some(143)
    );

    scenario_file(&fake, "scenario=success");
    let output = fake.command(&dot).output().unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    let starts: Vec<String> = fake
        .env_logs()
        .iter()
        .map(|env| env["PAS_NODE_ID"].clone())
        .collect();
    assert_eq!(starts, ["work", "work"]);
    assert_eq!(fake.events_of("PipelineCompleted").len(), 1);
}
