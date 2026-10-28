#![cfg(unix)]
//! Agent profiles (ticket 03, design.md §2): `agent=`, `pas.toml`
//! `[agents.*]`, `test_only` and `--allow-test-agents`, model and reasoning
//! overrides, `env.set` over `env.remove`, `pas validate`, and the
//! `llm_provider="claude"` alias, through `pas run` with the fake.

// This crate uses a subset of the shared harness.
mod blocked_agents;
mod common;
#[allow(dead_code)]
mod fake_agent;

use std::fs;
use std::process::{Child, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use fake_agent::{stderr, FakeAgent};
use serde_json::Value;

/// start -> work -> done, `work` an agent node with `attrs`.
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
    assert!(
        output.status.success(),
        "pas failed ({:?})\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        stderr(output)
    );
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The value after `flag` in the fake's only argv.
fn flag_value(fake: &FakeAgent, flag: &str) -> Option<String> {
    let invocations = fake.invocations();
    assert_eq!(invocations.len(), 1, "{invocations:?}");
    let argv = &invocations[0];
    let at = argv.iter().position(|a| a == flag)?;
    argv.get(at + 1).cloned()
}

#[test]
fn the_fake_profile_runs_with_allow_test_agents() {
    let fake = FakeAgent::new();

    let output = fake.run(&one_node(r#"agent="fake", prompt="scenario=success""#));

    assert_success(&output);
    assert_eq!(fake.attempts("success"), 1);
    assert_eq!(fake.events_of("LlmStarted")[0]["profile"], "fake");
}

#[test]
fn a_test_only_profile_is_refused_without_the_flag() {
    let fake = FakeAgent::new();

    let output = fake
        .command_without_test_agents(&one_node(r#"agent="fake", prompt="scenario=success""#))
        .output()
        .unwrap();

    assert!(!output.status.success());
    let out = stdout(&output);
    assert!(out.contains("node: work"), "{out}");
    assert!(out.contains("fake"), "{out}");
    assert!(out.contains("--allow-test-agents"), "{out}");
    assert_eq!(fake.attempts("success"), 0, "the fake never started");
    assert!(fake.run_dirs().is_empty(), "no Run folder");
}

#[test]
fn reasoning_effort_fills_the_profiles_reasoning_args() {
    let fake = FakeAgent::new();
    fake.commit_pas_toml(r#"reasoning_args = ["--effort", "{reasoning}"]"#);

    let output = fake.run(&one_node(
        r#"agent="fake", reasoning_effort="high", prompt="scenario=success""#,
    ));

    assert_success(&output);
    assert_eq!(flag_value(&fake, "--effort").as_deref(), Some("high"));
}

#[test]
fn llm_model_overrides_the_profiles_model() {
    let profile = "model = \"m0\"\nmodel_args = [\"--model\", \"{model}\"]";
    let cases = [
        (r#"agent="fake", llm_model="m1""#, "", "m1"),
        (r#"agent="fake""#, "", "m0"),
        // A graph-wide model never overrides the profile's own.
        (r#"agent="fake""#, r#"model="g""#, "m0"),
    ];
    for (attrs, graph_attrs, expected) in cases {
        let fake = FakeAgent::new();
        fake.commit_pas_toml(profile);
        let dot = format!(
            r#"digraph G {{
                {graph_attrs}
                start [shape="Mdiamond"]
                work [shape="box", timeout="30s", prompt="scenario=success", {attrs}]
                done [shape="Msquare"]
                start -> work -> done
            }}"#
        );

        assert_success(&fake.run(&dot));

        assert_eq!(
            flag_value(&fake, "--model").as_deref(),
            Some(expected),
            "{attrs} {graph_attrs}"
        );
    }
}

/// `pas validate` and `pas run` output (stdout then stderr) for `dot`, with
/// a fake profile that takes `--model`.
fn validate_and_run_output(dot: &str) -> [String; 2] {
    let fake = FakeAgent::new();
    fake.commit_pas_toml("model_args = [\"--model\", \"{model}\"]");
    let both = |output: std::process::Output| {
        assert_success(&output);
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    };
    [
        both(fake.validate(dot, &["--allow-test-agents"])),
        both(fake.run(dot)),
    ]
}

#[test]
fn a_node_that_sets_model_is_warned_to_use_llm_model() {
    let dot = one_node(r#"agent="fake", model="haiku", prompt="scenario=success""#);
    for output in validate_and_run_output(&dot) {
        assert!(
            output.contains("[WARN] node_model (node: work)"),
            "{output}"
        );
        assert!(output.contains("llm_model"), "{output}");
    }
}

#[test]
fn llm_model_on_a_node_and_model_on_the_graph_are_not_warned() {
    let dot = r#"digraph G {
        model="g"
        start [shape="Mdiamond"]
        work [shape="box", timeout="30s", agent="fake", llm_model="m1", prompt="scenario=success"]
        done [shape="Msquare"]
        start -> work -> done
    }"#;
    for output in validate_and_run_output(dot) {
        assert!(!output.contains("node_model"), "{output}");
    }
}

#[test]
fn env_set_reaches_the_agent_even_when_in_the_remove_list() {
    let fake = FakeAgent::new();
    fake.commit_pas_toml(r#"env.set = { ANTHROPIC_API_KEY = "set-by-profile" }"#);

    let output = fake
        .command(&one_node(r#"agent="fake", prompt="scenario=success""#))
        .env("ANTHROPIC_API_KEY", "inherited")
        .env("ANTHROPIC_AUTH_TOKEN", "inherited")
        .output()
        .unwrap();

    assert_success(&output);
    let env = &fake.env_logs()[0];
    assert_eq!(
        env.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some("set-by-profile")
    );
    assert!(!env.contains_key("ANTHROPIC_AUTH_TOKEN"), "{env:?}");
}

/// The Gemini keys `pas` inherits, as the agent sees them.
fn gemini_keys_seen(fake: &FakeAgent) -> (Option<String>, Option<String>) {
    let output = fake
        .command(&one_node(r#"agent="fake", prompt="scenario=success""#))
        .env("GEMINI_API_KEY", "inherited")
        .env("GOOGLE_API_KEY", "inherited")
        .output()
        .unwrap();
    assert_success(&output);
    let env = &fake.env_logs()[0];
    (
        env.get("GEMINI_API_KEY").cloned(),
        env.get("GOOGLE_API_KEY").cloned(),
    )
}

#[test]
fn the_default_env_remove_strips_the_gemini_keys() {
    let fake = FakeAgent::new();
    assert_eq!(gemini_keys_seen(&fake), (None, None));
}

#[test]
fn a_profiles_own_env_remove_still_replaces_the_default_list() {
    let fake = FakeAgent::new();
    fake.commit_pas_toml(r#"env.remove = ["GOOGLE_API_KEY"]"#);
    assert_eq!(gemini_keys_seen(&fake), (Some("inherited".into()), None));
}

#[test]
fn a_node_with_only_agent_validates_and_runs() {
    let fake = FakeAgent::new();
    let dot = one_node(r#"agent="fake", prompt="scenario=success""#);

    let validate = fake.validate(&dot, &["--allow-test-agents"]);
    assert_success(&validate);
    assert!(stdout(&validate).contains("Pipeline is valid"));

    assert_success(&fake.run(&dot));
}

/// `pas validate` fails, and its output names `node` and contains `text`.
fn assert_validate_fails(fake: &FakeAgent, dot: &str, node: &str, text: &str) {
    let output = fake.validate(dot, &["--allow-test-agents"]);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let out = stdout(&output);
    assert!(out.contains(&format!("node: {node}")), "{out}");
    assert!(out.contains(text), "{out}");
}

#[test]
fn validate_names_the_node_with_an_unknown_profile() {
    let fake = FakeAgent::new();
    assert_validate_fails(
        &fake,
        &one_node(r#"agent="nope", prompt="p""#),
        "work",
        "unknown agent profile nope",
    );
}

#[test]
fn validate_names_the_node_with_reasoning_on_a_profile_without_reasoning_args() {
    let fake = FakeAgent::new();
    assert_validate_fails(
        &fake,
        &one_node(r#"agent="fake", reasoning_effort="high", prompt="p""#),
        "work",
        "reasoning_args",
    );
}

#[test]
fn validate_names_the_node_with_reasoning_effort_on_codex() {
    let fake = FakeAgent::new();
    assert_validate_fails(
        &fake,
        &one_node(r#"llm_provider="codex", reasoning_effort="high", prompt="p""#),
        "work",
        "reasoning_effort",
    );
    let styled = r##"digraph G {
        model_stylesheet="#work { reasoning_effort: high; }"
        start [shape="Mdiamond"]
        work [shape="box", llm_provider="codex", prompt="p"]
        done [shape="Msquare"]
        start -> work -> done
    }"##;
    assert_validate_fails(&fake, styled, "work", "reasoning_effort");
}

#[test]
fn validate_refuses_a_test_only_profile_without_the_flag() {
    let fake = FakeAgent::new();
    let output = fake.validate(&one_node(r#"agent="fake", prompt="p""#), &[]);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    assert!(stdout(&output).contains("--allow-test-agents"));
}

/// The `llm_provider="claude"` alias with every `[codergen.claude]` setting
/// gives exactly the argv ticket 02a gave: the `claude` profile's args, the
/// `[codergen.claude]` flags, the node's flags, the model, then the
/// handler's own flags.
#[test]
fn the_claude_alias_with_codergen_claude_settings_keeps_02as_argv() {
    let fake = FakeAgent::new();
    fake.shim_claude_on_path();
    fs::write(
        fake.repo().join("pas.toml"),
        r#"[project]
name = "alias-test"

[codergen.claude]
settings_mode = "inherit"
setting_sources = ["user", "project"]
settings_json = "{}"
tools = "Read,Edit"
agents_json = "{\"r\":{}}"
plugin_dirs = ["plug"]
mcp_config_json = "{\"mcpServers\":{}}"
"#,
    )
    .unwrap();
    fake.git(&["add", "pas.toml"]);
    fake.git(&["commit", "-q", "-m", "codergen.claude"]);

    let output = fake.run(&one_node(
        r#"llm_provider="claude", llm_model="sonnet", allowed_tools="Read", max_budget_usd="1.5", prompt="scenario=success""#,
    ));

    assert_success(&output);
    let plugin = fs::canonicalize(fake.repo()).unwrap().join("plug");
    // The minted session id, as the agent's environment had it.
    let session = fake.env_logs()[0]["PAS_SESSION_ID"].clone();
    let expected = [
        "--dangerously-skip-permissions",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--setting-sources",
        "user,project",
        "--mcp-config",
        "{\"mcpServers\":{}}",
        "--settings",
        "{}",
        "--tools",
        "Read,Edit",
        "--agents",
        "{\"r\":{}}",
        "--plugin-dir",
        plugin.to_str().unwrap(),
        "--session-id",
        session.as_str(),
        "--allowedTools",
        "Read",
        "--max-budget-usd",
        "1.5",
        "--model",
        "sonnet",
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
    ];
    assert_eq!(
        fake.invocations(),
        vec![expected.map(String::from).to_vec()]
    );
}

// --- pas kill's default grace follows the Run's profiles ---

/// Polls `ready` every 20 ms for up to `secs` seconds.
fn wait_until(what: &str, secs: u64, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
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

fn has_event(fake: &FakeAgent, kind: &str) -> bool {
    journal_so_far(fake).iter().any(|e| e["type"] == kind)
}

fn run_id(fake: &FakeAgent) -> String {
    fake.run_meta()["run_id"].as_str().unwrap().to_string()
}

/// A `pas run` in the background, killed if the test fails first.
struct Spawned(Child);

impl Spawned {
    fn wait(&mut self, secs: u64) -> ExitStatus {
        let mut status = None;
        wait_until("pas run to exit", secs, || {
            status = self.0.try_wait().unwrap();
            status.is_some()
        });
        status.unwrap()
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

#[test]
fn attempt_started_records_the_stop_wait_of_the_profiles_used() {
    let fake = FakeAgent::new();
    fake.commit_pas_toml("kill_grace = \"1s\"");
    // `work` uses the fake; `gate` waits for `go` in the worktree; the stop
    // lands before `after`.
    let dot = &format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            work [shape="box", agent="fake", timeout="30s", prompt="scenario=success"]
            gate [shape="parallelogram", tool_command="{go}"]
            after [shape="parallelogram", tool_command="true"]
            done [shape="Msquare"]
            start -> work -> gate -> after -> done
        }}"#,
        go = common::wait_for_go!()
    );
    let mut run = Spawned(
        fake.command(dot)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_until("gate to start", 30, || {
        has_event(&fake, "StageStarted")
            && journal_so_far(&fake)
                .iter()
                .any(|e| e["type"] == "StageStarted" && e["data"]["node_id"] == "gate")
    });
    let id = run_id(&fake);
    let stop = fake.pas_cli(&["stop", &id]);
    assert!(stop.status.success(), "{}", stderr(&stop));
    fs::write(fake.worktree().join("go"), "").unwrap();
    assert_eq!(run.wait(30).code(), Some(0), "a stopped Run exits 0");

    // A resume reloads pas.toml: its Attempt records the new grace.
    fake.commit_pas_toml("kill_grace = \"2s\"");
    let resumed = fake.run(dot);
    assert_success(&resumed);

    let waits: Vec<Value> = fake
        .events_of("AttemptStarted")
        .iter()
        .map(|data| data["stop_wait_ms"].clone())
        .collect();
    assert_eq!(waits, [6000, 7000]);
}

#[test]
fn pas_kill_without_grace_waits_for_a_profile_grace_over_15s() {
    let fake = FakeAgent::new();
    // Over the old fixed 20 s default's limit (15 s grace + 5 s margin).
    fake.commit_pas_toml("kill_grace = \"16s\"");
    let mut run = Spawned(
        fake.command(&one_node(
            r#"agent="fake", timeout="120s", prompt="scenario=hang_ignore_term""#,
        ))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap(),
    );
    wait_until("LlmStarted", 30, || has_event(&fake, "LlmStarted"));
    let id = run_id(&fake);

    // pas kill watches the Run's pid; reap the Run as soon as it exits, or
    // its zombie looks alive and draws a SIGKILL.
    let mut killer = fake
        .pas_cli_command(&["kill", &id, "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ran = None;
    wait_until("pas kill to finish", 60, || {
        if ran.is_none() {
            ran = run.0.try_wait().unwrap();
        }
        killer.try_wait().unwrap().is_some()
    });
    let kill = killer.wait_with_output().unwrap();
    assert!(ran.is_some(), "the Run ended before pas kill escalated");

    let out: Value = serde_json::from_slice(&kill.stdout)
        .unwrap_or_else(|_| panic!("pas kill: {}{}", stdout(&kill), stderr(&kill)));
    assert_eq!(out["signal"], "SIGTERM", "{out}");
    let ended = fake.events_of("AttemptEnded");
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(ended[0]["reason"], "stopped");
}

// --- `pas validate --workdir` reads the workdir's pas.toml, as `run` does ---

/// A git repo beside the fake's whose `pas.toml` has a profile with an
/// unknown field, and the pipeline file `dot`.
fn broken_project(fake: &FakeAgent, dot: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let broken = fake.root().join("broken");
    fs::create_dir(&broken).unwrap();
    fs::write(
        broken.join("pas.toml"),
        "[project]\nname = \"broken\"\n\n[agents.fake]\nmechanism = \"claude-p\"\ncommand = \"x\"\nbogus_field = 1\n",
    )
    .unwrap();
    let pipeline = fake.root().join("workdir.dot");
    fs::write(&pipeline, dot).unwrap();
    (broken, pipeline)
}

fn validate_in(
    fake: &FakeAgent,
    cwd: &std::path::Path,
    pipeline: &std::path::Path,
    workdir: Option<&std::path::Path>,
) -> std::process::Output {
    let mut command = fake.pas_cli_command(&["validate", "--allow-test-agents"]);
    command.arg(pipeline).current_dir(cwd);
    if let Some(workdir) = workdir {
        command.arg("--workdir").arg(workdir);
    }
    command.output().unwrap()
}

/// `pas`'s error for the broken `pas.toml`: from "failed to resolve
/// pas.toml" to the end of its "unknown field" line.
fn bogus_field_error(output: &std::process::Output) -> String {
    let err = stderr(output);
    assert!(!output.status.success(), "{err}");
    let start = err
        .find("failed to resolve pas.toml")
        .unwrap_or_else(|| panic!("{err}"));
    let field = err
        .find("unknown field `bogus_field`")
        .unwrap_or_else(|| panic!("{err}"));
    let end = err[field..].find('\n').map_or(err.len(), |n| field + n);
    err[start..end].to_string()
}

#[test]
fn validate_with_workdir_reads_that_projects_pas_toml_like_run() {
    let fake = FakeAgent::new();
    let dot = one_node(r#"agent="fake", prompt="scenario=success""#);
    let (broken, pipeline) = broken_project(&fake, &dot);

    // From the broken project, --workdir names the good one.
    let validated = validate_in(&fake, &broken, &pipeline, Some(&fake.repo()));
    assert_success(&validated);
    let mut run = fake.command_in(&dot, &fake.repo(), &[]);
    assert_success(&run.current_dir(&broken).output().unwrap());

    // From the good project, --workdir names the broken one: the same error.
    let validated = validate_in(&fake, &fake.repo(), &pipeline, Some(&broken));
    let mut run = fake.command_in(&dot, &broken, &[]);
    let ran = run.current_dir(fake.repo()).output().unwrap();
    assert_eq!(bogus_field_error(&validated), bogus_field_error(&ran));
}

#[test]
fn validate_without_workdir_still_reads_the_current_directorys_pas_toml() {
    let fake = FakeAgent::new();
    let dot = one_node(r#"agent="fake", prompt="scenario=success""#);
    let (broken, pipeline) = broken_project(&fake, &dot);
    bogus_field_error(&validate_in(&fake, &broken, &pipeline, None));
    assert_success(&validate_in(&fake, &fake.repo(), &pipeline, None));
}
