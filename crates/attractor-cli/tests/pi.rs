#![cfg(unix)]
//! Pi through `pas run` (ticket 09): the `pi` handler with
//! `tests/agents/fake-pi`, selected by the harness's `fake-pi` profile. `pas`
//! gets the dummy key `FAKE_PI_KEY=sk-secret` and the harness's temp `HOME`;
//! no test runs a real `pi`, calls an endpoint or uses a real key.

// This crate uses a subset of the shared harness.
#[allow(dead_code)]
mod fake_agent;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use fake_agent::{stderr, FakeAgent, FAKE_PI_KEY};

fn run(fake: &FakeAgent, dot: &str) -> Output {
    fake.command(dot)
        .env("FAKE_PI_KEY", FAKE_PI_KEY)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", stderr(output));
}

fn node_file(fake: &FakeAgent, name: &str, line: &str) {
    fs::write(fake.scenarios().join(name), format!("{line}\n")).unwrap();
}

/// start -> work -> next -> done on `fake-pi`; `next` sees `work`'s result.
fn work_then_next(work: &str) -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            work [shape="box", agent="fake-pi", timeout="30s", prompt="scenario={work}"]
            next [shape="box", agent="fake-pi", timeout="30s", prompt="scenario=stop"]
            done [shape="Msquare"]
            start -> work -> next -> done
        }}"#
    )
}

/// start -> work, then `on_ok` or `on_fail` by outcome.
fn routed(work: &str) -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            work [shape="box", agent="fake-pi", timeout="30s", prompt="scenario={work}"]
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
            work [shape="box", agent="fake-pi", prompt="do it", {attrs}]
            done [shape="Msquare"]
            start -> work -> done
        }}"#
    )
}

/// The value after `flag` in `argv`.
fn flag(argv: &[String], flag: &str) -> Option<String> {
    let at = argv.iter().position(|a| a == flag)?;
    argv.get(at + 1).cloned()
}

/// Every file under `dir`, skipping `skip`.
fn files(dir: &Path, skip: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path == skip {
            continue;
        }
        let kind = fs::symlink_metadata(&path).unwrap().file_type();
        if kind.is_dir() {
            files(&path, skip, out);
        } else if kind.is_file() {
            out.push(path);
        }
    }
}

/// The key is nowhere but the fake's own copies: no `models.json` is left
/// in the logs, no file in the test's folder (repo, worktree, logs, state,
/// home) holds it, and no commit on any branch does.
fn assert_no_key_left(fake: &FakeAgent) {
    let scenarios = fake.scenarios();
    // The fake's copies of models.json are the only place the key may be;
    // that folder is outside the repo and its worktrees.
    assert!(!scenarios.starts_with(fake.repo()));
    for run in fake.run_dirs() {
        assert!(!scenarios.starts_with(&run));
    }
    let mut all = vec![];
    files(fake.root(), &scenarios, &mut all);
    assert!(!all.is_empty());
    let models: Vec<&PathBuf> = all
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "models.json"))
        .collect();
    assert!(models.is_empty(), "{models:?}");
    for path in &all {
        let bytes = fs::read(path).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(FAKE_PI_KEY), "key in {}", path.display());
    }
    let history = fake.git(&["log", "-p", "--all"]);
    assert!(!history.is_empty());
    assert!(!history.contains(FAKE_PI_KEY), "key in a commit");
}

#[test]
fn stop_completes_with_the_fakes_text() {
    let fake = FakeAgent::new();
    assert_success(&run(&fake, &work_then_next("stop")));
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    assert!(
        prompts[1].contains("- work.result: fake-pi: done\n"),
        "{}",
        prompts[1]
    );
    assert_eq!(fake.events_of("LlmInvoked")[0]["provider"], "fake-pi");
}

#[test]
fn error_routes_on_the_fail_edge() {
    let fake = FakeAgent::new();
    assert_success(&run(&fake, &routed("error")));
    let stages = fake.stages_started();
    assert!(stages.contains(&"on_fail".to_string()), "{stages:?}");
    assert!(!stages.contains(&"on_ok".to_string()), "{stages:?}");
}

#[test]
fn a_retry_after_a_timeout_continues_the_same_pi_session() {
    let fake = FakeAgent::new();
    node_file(&fake, "work.1", "scenario=hang");
    node_file(&fake, "work", "scenario=stop");
    assert_success(&run(&fake, &one_node(r#"timeout="1s", max_retries=1"#)));

    let argvs = fake.invocations();
    let envs = fake.env_logs();
    assert_eq!(argvs.len(), 2, "{argvs:?}");
    let id = flag(&argvs[0], "--session-id").unwrap();
    let sessions = fake.run_dirs()[0]
        .join("pi-sessions")
        .canonicalize()
        .unwrap();
    for (argv, env) in argvs.iter().zip(&envs) {
        assert_eq!(flag(argv, "--session-id").as_deref(), Some(id.as_str()));
        assert_eq!(
            flag(argv, "--session-dir").map(|d| PathBuf::from(d).canonicalize().unwrap()),
            Some(sessions.clone())
        );
        assert_eq!(env.get("PAS_SESSION_ID"), Some(&id));
    }
    assert!(sessions.is_dir());
}

#[test]
fn no_key_is_left_after_a_successful_run() {
    let fake = FakeAgent::new();
    assert_success(&run(&fake, &work_then_next("stop")));
    // The fake saw the key in models.json, so the scan below has a key to miss.
    let copy = fs::read_to_string(fake.scenarios().join("models.work@1.json")).unwrap();
    assert!(copy.contains(FAKE_PI_KEY), "{copy}");
    assert_no_key_left(&fake);
}

#[test]
fn no_key_is_left_after_a_failing_run_that_keeps_its_worktree() {
    let fake = FakeAgent::new();
    let output = run(
        &fake,
        &one_node(r#"timeout="30s", prompt="scenario=crash""#),
    );
    assert!(!output.status.success());
    assert!(fake.worktree().is_dir(), "a failed Run keeps its worktree");
    let copy = fs::read_to_string(fake.scenarios().join("models.work@1.json")).unwrap();
    assert!(copy.contains(FAKE_PI_KEY), "{copy}");
    assert_no_key_left(&fake);
}

#[test]
fn validate_names_a_pi_profile_missing_provider_or_model() {
    for (missing, profile) in [
        ("provider", "model = \"m\"\n"),
        ("model", "provider = \"deepseek\"\n"),
    ] {
        let fake = FakeAgent::new();
        fake.write_pas_toml(&format!(
            "\n[agents.bad]\nmechanism = \"pi\"\ncommand = [\"pi\"]\n{profile}"
        ));
        let output = fake.validate(&one_node(r#"timeout="30s""#), &[]);
        assert!(!output.status.success(), "{missing}");
        let err = stderr(&output);
        assert!(
            err.contains(&format!(
                "agent profile bad: {missing} is required for mechanism pi"
            )),
            "{err}"
        );
    }
}

#[test]
fn the_example_profiles_validate() {
    let fake = FakeAgent::new();
    let examples = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/pi-agents.toml"),
    )
    .unwrap();
    fake.write_pas_toml(&format!("\n{examples}"));
    let dot = r#"digraph G {
        start [shape="Mdiamond"]
        a [shape="box", agent="deepseek", prompt="a"]
        b [shape="box", agent="glm", prompt="b"]
        c [shape="box", agent="qwen", prompt="c"]
        done [shape="Msquare"]
        start -> a -> b -> c -> done
    }"#;
    let output = fake
        .pas_cli_command(&["validate"])
        .arg({
            let path = fake.root().join("examples.dot");
            fs::write(&path, dot).unwrap();
            path
        })
        .current_dir(fake.repo())
        .env("DEEPSEEK_API_KEY", "dummy")
        .env("ZAI_API_KEY", "dummy")
        .env("QWEN_API_KEY", "dummy")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fake.invocations().len(), 0, "validate starts no agent");
}
