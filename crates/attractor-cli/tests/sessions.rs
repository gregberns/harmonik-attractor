#![cfg(unix)]
//! `pas run` end to end with the fakes: a node that runs again continues its
//! agent's session (ticket 08). Claude nodes use the harness's `fake-claude`
//! profile (the built-in `claude` with the fake as command), so they get the
//! built-in `--session-id`/`--resume` forms.

#[allow(dead_code)]
mod fake_agent;

use std::fs;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use fake_agent::{stderr, FakeAgent};

/// One agent start: its node, its session id env, and its argv.
#[derive(Debug)]
struct Start {
    node: String,
    session_env: String,
    argv: Vec<String>,
}

/// Every agent start, in order: `env.log` and `invocations.log` have one
/// block per start, written in the same order.
fn starts(fake: &FakeAgent) -> Vec<Start> {
    let envs = fake.env_logs();
    let argvs = fake.invocations();
    assert_eq!(envs.len(), argvs.len(), "{envs:?} {argvs:?}");
    envs.into_iter()
        .zip(argvs)
        .map(|(env, argv)| Start {
            node: env.get("PAS_NODE_ID").cloned().unwrap_or_default(),
            session_env: env.get("PAS_SESSION_ID").cloned().unwrap_or_default(),
            argv,
        })
        .collect()
}

/// The value after `flag` in `argv`.
fn flag(argv: &[String], flag: &str) -> Option<String> {
    let at = argv.iter().position(|a| a == flag)?;
    argv.get(at + 1).cloned()
}

fn node_file(fake: &FakeAgent, name: &str, line: &str) {
    fs::write(fake.scenarios().join(name), format!("{line}\n")).unwrap();
}

fn assert_success(output: &std::process::Output) {
    assert!(output.status.success(), "{}", stderr(output));
}

/// start -> work -> done; `work`'s scenario comes from node files.
fn one_node(attrs: &str) -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            work [shape="box", prompt="do it", {attrs}]
            done [shape="Msquare"]
            start -> work -> done
        }}"#
    )
}

/// implement -> review, and review routes back to implement once.
const LOOP: &str = r#"digraph G {
    start [shape="Mdiamond"]
    implement [shape="box", agent="fake-claude", timeout="30s", prompt="implement"]
    review [shape="diamond", agent="fake-claude", timeout="30s", prompt="review"]
    done [shape="Msquare"]
    start -> implement -> review
    review -> implement [label="again", condition="preferred_label=again"]
    review -> done [label="done", condition="preferred_label=done"]
}"#;

#[test]
fn a_retry_resumes_the_same_session_and_keeps_the_profile_args() {
    let fake = FakeAgent::new();
    // The hang prints its init line (with the session id) before hanging.
    node_file(&fake, "work.1", "scenario=hang");
    node_file(&fake, "work", "scenario=success");
    assert_success(&fake.run(&one_node(
        r#"agent="fake-claude", timeout="1s", max_retries=1"#,
    )));

    let starts = starts(&fake);
    assert_eq!(starts.len(), 2, "{starts:#?}");
    let id = flag(&starts[0].argv, "--session-id").expect("a new session first");
    assert!(uuid_like(&id), "{id}");
    assert_eq!(
        flag(&starts[1].argv, "--resume").as_deref(),
        Some(id.as_str())
    );
    assert_eq!(flag(&starts[1].argv, "--session-id"), None);
    for start in &starts {
        assert_eq!(start.session_env, id);
        for kept in [
            "--safe-mode",
            "--dangerously-skip-permissions",
            "--strict-mcp-config",
        ] {
            assert!(
                start.argv.iter().any(|a| a == kept),
                "{kept}: {:?}",
                start.argv
            );
        }
    }
    // The attempt commits name the session.
    let branch = fake.run_meta()["branch"].as_str().unwrap().to_string();
    let trailers = fake.git(&[
        "log",
        "--format=%(trailers:key=Pas-Session,valueonly)",
        &branch,
    ]);
    assert_eq!(
        trailers.lines().filter(|l| *l == id).count(),
        2,
        "{trailers}"
    );
}

#[test]
fn a_loop_back_continues_the_nodes_own_session() {
    let fake = FakeAgent::new();
    node_file(&fake, "implement", "scenario=success");
    node_file(&fake, "review@1", "scenario=label label=again");
    node_file(&fake, "review@2", "scenario=label label=done");
    assert_success(&fake.run(LOOP));

    let starts = starts(&fake);
    let nodes: Vec<&str> = starts.iter().map(|s| s.node.as_str()).collect();
    assert_eq!(nodes, ["implement", "review", "implement", "review"]);
    let implement = flag(&starts[0].argv, "--session-id").unwrap();
    let review = flag(&starts[1].argv, "--session-id").unwrap();
    assert_ne!(implement, review);
    assert_eq!(flag(&starts[2].argv, "--resume"), Some(implement));
    assert_eq!(flag(&starts[3].argv, "--resume"), Some(review));
}

#[test]
fn nodes_with_one_thread_id_share_a_session() {
    let fake = FakeAgent::new();
    node_file(&fake, "a", "scenario=success");
    node_file(&fake, "b", "scenario=success");
    assert_success(&fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            a [shape="box", agent="fake-claude", timeout="30s", prompt="a", thread_id="t"]
            b [shape="box", agent="fake-claude", timeout="30s", prompt="b", thread_id="t"]
            done [shape="Msquare"]
            start -> a -> b -> done
        }"#,
    ));

    let starts = starts(&fake);
    let first = flag(&starts[0].argv, "--session-id").unwrap();
    assert_eq!(flag(&starts[1].argv, "--resume"), Some(first));
}

#[test]
fn fidelity_fresh_starts_a_new_session_every_time() {
    let fake = FakeAgent::new();
    node_file(&fake, "work.1", "scenario=hang");
    node_file(&fake, "work", "scenario=success");
    assert_success(&fake.run(&one_node(
        r#"agent="fake-claude", timeout="1s", max_retries=1, fidelity="fresh""#,
    )));

    let starts = starts(&fake);
    assert_eq!(starts.len(), 2);
    let ids: Vec<String> = starts
        .iter()
        .map(|s| flag(&s.argv, "--session-id").expect("always new"))
        .collect();
    assert_ne!(ids[0], ids[1]);
    assert!(starts.iter().all(|s| flag(&s.argv, "--resume").is_none()));
}

#[test]
fn the_session_map_survives_a_stop_and_resume() {
    let fake = FakeAgent::new();
    node_file(&fake, "implement", "scenario=success");
    node_file(&fake, "review@1", "scenario=label label=again");
    node_file(&fake, "review@2", "scenario=label label=done");
    // implement -> wait (a tool node held until `go`) -> review -> implement.
    let dot = format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            implement [shape="box", agent="fake-claude", timeout="30s", prompt="implement"]
            wait [shape="parallelogram", timeout="60s", tool_command="{go}"]
            review [shape="diamond", agent="fake-claude", timeout="30s", prompt="review"]
            done [shape="Msquare"]
            start -> implement -> wait -> review
            review -> implement [label="again", condition="preferred_label=again"]
            review -> done [label="done", condition="preferred_label=done"]
        }}"#,
        go = WAIT_FOR_GO
    );
    let mut run = spawn(&fake, &dot);
    wait_until("wait to start", || {
        journal(&fake)
            .iter()
            .any(|e| e["type"] == "StageStarted" && e["data"]["node_id"] == "wait")
    });
    let stop = fake.pas_cli(&["stop", &run_id(&fake)]);
    assert!(stop.status.success(), "{}", stderr(&stop));
    fs::write(fake.worktree().join("go"), "").unwrap();
    assert_eq!(wait_exit(&mut run).code(), Some(0), "a stopped Run exits 0");

    let implement = flag(&starts(&fake)[0].argv, "--session-id").unwrap();
    let checkpoint: Value =
        serde_json::from_str(&fs::read_to_string(fake.logs_dir().join("checkpoint.json")).unwrap())
            .unwrap();
    assert_eq!(
        checkpoint["agent_sessions"]["implement"],
        implement.as_str()
    );

    assert_success(&fake.run(&dot));
    let implement_again = starts(&fake)
        .into_iter()
        .filter(|s| s.node == "implement")
        .nth(1)
        .expect("implement ran again");
    assert_eq!(flag(&implement_again.argv, "--resume"), Some(implement));
}

#[test]
fn a_codex_retry_runs_exec_resume_with_its_thread_id() {
    let fake = FakeAgent::new();
    // fake-codex's hang prints `thread.started` before hanging.
    node_file(&fake, "work.1", "scenario=hang");
    node_file(&fake, "work", "scenario=success");
    assert_success(&fake.run(&one_node(
        r#"agent="fake-codex", timeout="1s", max_retries=1"#,
    )));

    let starts = starts(&fake);
    assert_eq!(starts.len(), 2, "{starts:#?}");
    let thread = fake.events_of("LlmInvoked")[0]["agent_session_id"]
        .as_str()
        .expect("attempt 1 reported its thread")
        .to_string();
    // The fake logs its arguments (not the program): that it logged this
    // start shows the resumed program is the fake, not a `codex` on PATH.
    let argv = &starts[1].argv;
    assert_eq!(&argv[0..3], ["exec", "resume", thread.as_str()]);
    assert!(argv
        .iter()
        .any(|a| a == "--dangerously-bypass-approvals-and-sandbox"));
    assert!(!argv.iter().any(|a| a == "--cd"), "{argv:?}");
}

#[test]
fn a_gemini_node_is_fresh_and_full_on_it_fails_validation() {
    let fake = FakeAgent::new();
    node_file(&fake, "work.1", "scenario=hang");
    node_file(&fake, "work", "scenario=success");
    assert_success(&fake.run(&one_node(
        r#"agent="fake-gemini", timeout="1s", max_retries=1"#,
    )));
    let starts = starts(&fake);
    assert_eq!(starts.len(), 2);
    assert_ne!(starts[0].session_env, starts[1].session_env);
    assert!(starts
        .iter()
        .all(|s| !s.argv.iter().any(|a| a.contains("resume"))));

    let pipeline = fake.repo().join("full.dot");
    fs::write(
        &pipeline,
        one_node(r#"agent="fake-gemini", fidelity="full""#),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_pas"))
        .args(["validate", "--allow-test-agents"])
        .arg(&pipeline)
        .current_dir(fake.repo())
        .env("HOME", fake.home())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        stderr(&output)
    );
    assert!(
        text.contains("'work'") && text.contains("fake-gemini"),
        "{text}"
    );
}

#[test]
fn a_session_that_cant_be_continued_is_a_reported_failure_with_no_fallback() {
    let fake = FakeAgent::new();
    node_file(&fake, "work@1", "scenario=success");
    node_file(&fake, "work@2", "scenario=session_not_found");
    node_file(&fake, "review@1", "scenario=label label=again");
    node_file(&fake, "handled", "scenario=success");
    assert_success(&fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="box", agent="fake-claude", timeout="30s", prompt="work"]
            review [shape="diamond", agent="fake-claude", timeout="30s", prompt="review"]
            handled [shape="box", agent="fake-claude", timeout="30s", prompt="handled"]
            done [shape="Msquare"]
            start -> work
            work -> review [condition="outcome=success"]
            work -> handled [condition="outcome=fail"]
            review -> work [label="again", condition="preferred_label=again"]
            handled -> done
        }"#,
    ));

    let starts = starts(&fake);
    let work: Vec<&Start> = starts.iter().filter(|s| s.node == "work").collect();
    assert_eq!(
        work.len(),
        2,
        "no third start: no fallback to a new session"
    );
    let id = flag(&work[0].argv, "--session-id").unwrap();
    assert_eq!(flag(&work[1].argv, "--resume"), Some(id));
    // The failure routed on outcome=fail, carrying the reason.
    let handled = fs::read_to_string(fake.scenarios().join("prompt.handled@1")).unwrap();
    assert!(
        handled.contains("No conversation found with session ID"),
        "{handled}"
    );
}

#[test]
fn the_llm_provider_claude_alias_resumes_the_same_way() {
    let fake = FakeAgent::new();
    fake.shim_claude_on_path();
    node_file(&fake, "work.1", "scenario=hang");
    node_file(&fake, "work", "scenario=success");
    assert_success(&fake.run(&one_node(
        r#"llm_provider="claude", timeout="1s", max_retries=1"#,
    )));
    let starts = starts(&fake);
    let id = flag(&starts[0].argv, "--session-id").unwrap();
    assert_eq!(flag(&starts[1].argv, "--resume"), Some(id));
}

#[test]
fn agents_and_tools_run_with_home_in_the_test_folder() {
    let fake = FakeAgent::new();
    node_file(&fake, "work", "scenario=success");
    assert_success(&fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            home [shape="parallelogram", timeout="30s", tool_command="printf '%s\n%s' \"$HOME\" \"$CODEX_HOME\" > home.txt"]
            work [shape="box", agent="fake-claude", timeout="30s", prompt="work"]
            done [shape="Msquare"]
            start -> home -> work -> done
        }"#,
    ));
    let branch = fake.run_meta()["branch"].as_str().unwrap().to_string();
    let home = fake.git(&["show", &format!("{branch}:home.txt")]);
    let expected_home = fake.home();
    assert_eq!(
        home,
        format!(
            "{}\n{}",
            expected_home.display(),
            expected_home.join(".codex").display()
        )
    );
}

// --- helpers for the stop test ---

/// A tool command that waits for a `go` file in its working directory.
const WAIT_FOR_GO: &str =
    "n=600; while [ ! -f go ]; do [ $n -gt 0 ] || exit 1; n=$((n-1)); sleep 0.05; done";

fn spawn(fake: &FakeAgent, dot: &str) -> Child {
    fake.command(dot)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn journal(fake: &FakeAgent) -> Vec<Value> {
    let Some(run) = fake.run_dirs().into_iter().next() else {
        return vec![];
    };
    fs::read_to_string(run.join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn run_id(fake: &FakeAgent) -> String {
    fake.run_meta()["run_id"].as_str().unwrap().to_string()
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("pas did not exit");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn uuid_like(id: &str) -> bool {
    id.len() == 36 && id.chars().filter(|c| *c == '-').count() == 4
}
