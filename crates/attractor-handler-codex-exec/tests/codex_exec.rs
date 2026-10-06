#![cfg(unix)]
//! Seam 1: `Agents::run` with the `codex-exec` handler and a profile whose
//! command is the shell fake (`tests/agents/fake-codex`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use attractor_agent_handler::{
    AgentRequest, AgentResult, AgentStatus, Agents, CancellationToken, FailureClass, Profile,
    ProfileEnv, Record, Selection,
};
use attractor_handler_codex_exec::CodexExec;

fn fake_codex() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/agents/fake-codex")
        .canonicalize()
        .unwrap()
}

/// The built-in `codex` profile's shape, run with the fake.
fn profile() -> Profile {
    Profile {
        name: "codex".into(),
        mechanism: "codex-exec".into(),
        command: vec![fake_codex().to_string_lossy().into_owned()],
        args: [
            "exec",
            "--json",
            "--yolo",
            "--skip-git-repo-check",
            "--ephemeral",
        ]
        .map(String::from)
        .to_vec(),
        model: None,
        model_args: vec!["--model".into(), "{model}".into()],
        reasoning: None,
        reasoning_args: vec![],
        timeout: Duration::from_secs(600),
        kill_grace: Duration::from_secs(1),
        env: ProfileEnv {
            remove: vec!["OPENAI_API_KEY".into()],
            set: BTreeMap::new(),
        },
        test_only: false,
    }
}

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("scenarios")).unwrap();
        fs::create_dir(dir.path().join("work")).unwrap();
        Self { dir }
    }

    fn scenarios(&self) -> PathBuf {
        self.dir.path().join("scenarios")
    }

    fn workdir(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.scenarios().join(name)).unwrap()
    }

    fn agents(&self) -> Agents {
        let env = BTreeMap::from([
            ("PATH".to_string(), std::env::var("PATH").unwrap()),
            (
                "FAKE_AGENT_SCENARIOS".to_string(),
                self.scenarios().to_string_lossy().into_owned(),
            ),
            ("OPENAI_API_KEY".to_string(), "sk-test".to_string()),
        ]);
        Agents::new(vec![Arc::new(CodexExec)], vec![profile()], env, false).unwrap()
    }

    async fn run(&self, scenario: &str, model: Option<&str>, timeout: Duration) -> AgentResult {
        fs::write(self.scenarios().join("work"), format!("{scenario}\n")).unwrap();
        self.agents()
            .run(AgentRequest {
                selection: Selection {
                    profile: "codex".into(),
                    model: model.map(String::from),
                    reasoning: None,
                },
                prompt: "line one\nTask (work): do it".into(),
                extra_args: vec![],
                workdir: self.workdir(),
                timeout: Some(timeout),
                record: Record {
                    run_id: Some("run-1".into()),
                    node_id: "work".into(),
                    attempt: 1,
                    invocation_id: "inv-1".into(),
                },
                transcript: Some(self.dir.path().join("t.jsonl")),
                stderr: Some(self.dir.path().join("t.stderr.log")),
                prompt_file: Some(self.dir.path().join("t.prompt.txt")),
                observer: None,
                cancel: CancellationToken::new(),
            })
            .await
    }
}

const LONG: Duration = Duration::from_secs(20);

#[tokio::test]
async fn success_completes_with_the_last_message() {
    let fx = Fixture::new();
    let result = fx.run("scenario=success", None, LONG).await;
    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    assert_eq!(result.text, "fake-codex: success");
    assert_eq!(result.usage.input_tokens, Some(3));
    assert_eq!(result.usage.output_tokens, Some(2));
}

#[tokio::test]
async fn a_failed_turn_is_reported() {
    let fx = Fixture::new();
    let result = fx.run("scenario=turn_failed", None, LONG).await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Reported));
    assert_eq!(result.text, "fake-codex: partial");
}

#[tokio::test]
async fn a_crash_is_a_crash_with_its_stderr() {
    let fx = Fixture::new();
    let result = fx.run("scenario=crash", None, LONG).await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Crash));
    assert_eq!(result.detail, "exited with exit status: 3");
    assert_eq!(result.stderr_tail, "fake-codex: crashed");
}

#[tokio::test]
async fn no_output_is_no_result() {
    let fx = Fixture::new();
    let result = fx.run("scenario=silent", None, LONG).await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::NoResult));
    assert!(result.detail.contains("Codex CLI produced no output"));
}

#[tokio::test]
async fn garbage_completes_without_a_message_as_before() {
    let fx = Fixture::new();
    let result = fx.run("scenario=garbage", None, LONG).await;
    assert_eq!(result.status, AgentStatus::Completed);
    assert_eq!(result.text, "No agent message found in Codex output");
}

#[tokio::test]
async fn a_hang_times_out() {
    let fx = Fixture::new();
    let result = fx
        .run("scenario=hang", None, Duration::from_millis(500))
        .await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Timeout));
}

#[tokio::test]
async fn argv_is_todays_with_cd_and_the_prompt_last() {
    let fx = Fixture::new();
    fx.run("scenario=success", Some("o3"), LONG).await;
    let argv: Vec<String> = fx
        .read("invocations.log")
        .lines()
        .skip(1)
        .map(String::from)
        .collect();
    let workdir = fx.workdir().to_string_lossy().into_owned();
    assert_eq!(
        argv,
        [
            "exec",
            "--json",
            "--yolo",
            "--skip-git-repo-check",
            "--ephemeral",
            "--model",
            "o3",
            "--cd",
            workdir.as_str(),
            "<prompt>",
        ]
    );
    assert_eq!(
        fx.read("prompts.log"),
        "--- start\nline one\nTask (work): do it\n"
    );
}

#[tokio::test]
async fn the_api_key_is_stripped_and_the_pas_ids_are_set() {
    let fx = Fixture::new();
    fx.run("scenario=success", None, LONG).await;
    let env = fx.read("env.log");
    assert!(!env.contains("OPENAI_API_KEY"), "{env}");
    assert!(env.contains("PAS_NODE_ID=work"), "{env}");
    assert!(env.contains("PAS_RUN_ID=run-1"), "{env}");
}

#[tokio::test]
async fn the_prompt_file_records_the_spawned_argv() {
    let fx = Fixture::new();
    fx.run("scenario=success", None, LONG).await;
    let text = fs::read_to_string(fx.dir.path().join("t.prompt.txt")).unwrap();
    let start = text.find("argv:\n").unwrap() + "argv:\n".len();
    let end = text[start..].find("\n\n").unwrap() + start;
    let argv: Vec<&str> = text[start..end].lines().collect();
    let workdir = fx.workdir().to_string_lossy().into_owned();
    let fake = fake_codex().to_string_lossy().into_owned();
    assert_eq!(
        argv,
        [
            fake.as_str(),
            "exec",
            "--json",
            "--yolo",
            "--skip-git-repo-check",
            "--ephemeral",
            "--cd",
            workdir.as_str(),
            "<prompt>",
        ]
    );
}
