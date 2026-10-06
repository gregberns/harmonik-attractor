#![cfg(unix)]
//! Seam 1: `Agents::run` with the `codex-exec` handler and a profile whose
//! command is the shell fake (`tests/agents/fake-codex`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use attractor_agent_handler::{
    builtin_profiles, AgentRequest, AgentResult, AgentStatus, Agents, CancellationToken,
    FailureClass, Profile, Record, Selection, Session,
};
use attractor_handler_codex_exec::CodexExec;

fn fake_codex() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/agents/fake-codex")
        .canonicalize()
        .unwrap()
}

/// The built-in `codex` profile, run with the fake.
fn profile() -> Profile {
    let builtin = builtin_profiles()
        .unwrap()
        .into_iter()
        .find(|p| p.name == "codex")
        .unwrap();
    Profile {
        command: vec![fake_codex().to_string_lossy().into_owned()],
        kill_grace: Duration::from_secs(1),
        ..builtin
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
        self.run_in(scenario, model, timeout, Session::New("sess-1".into()))
            .await
    }

    async fn run_in(
        &self,
        scenario: &str,
        model: Option<&str>,
        timeout: Duration,
        session: Session,
    ) -> AgentResult {
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
                session,
                state_dir: None,
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
            "--cd",
            workdir.as_str(),
            "<prompt>",
        ]
    );
}

/// The argv section of the prompt file: what was spawned, program first.
fn spawned_argv(fx: &Fixture) -> Vec<String> {
    let text = fs::read_to_string(fx.dir.path().join("t.prompt.txt")).unwrap();
    let start = text.find("argv:\n").unwrap() + "argv:\n".len();
    let end = text[start..].find("\n\n").unwrap() + start;
    text[start..end].lines().map(String::from).collect()
}

#[tokio::test]
async fn a_new_session_reports_the_thread_id_codex_printed() {
    let fx = Fixture::new();
    let result = fx.run("scenario=success", None, LONG).await;
    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    let pid: u32 = fx
        .read("env.log")
        .lines()
        .find_map(|l| l.strip_prefix("FAKE_PID="))
        .unwrap()
        .parse()
        .unwrap();
    // The fake makes a fresh id from its pid; PAS can't choose Codex's.
    let expected = format!("00000000-0000-4000-8000-{pid:012}");
    assert_eq!(result.agent_session_id.as_deref(), Some(expected.as_str()));
    assert!(!result.continued);
    assert!(fx.read("env.log").contains("PAS_SESSION_ID=sess-1\n"));
}

#[tokio::test]
async fn continuing_runs_exec_resume_with_the_profiles_program_and_no_cd() {
    let fx = Fixture::new();
    let result = fx
        .run_in(
            "scenario=success",
            Some("o3"),
            LONG,
            Session::Continue("thread-0".into()),
        )
        .await;
    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    let fake = fake_codex().to_string_lossy().into_owned();
    assert_eq!(
        spawned_argv(&fx),
        [
            fake.as_str(),
            "exec",
            "resume",
            "thread-0",
            "--json",
            "--skip-git-repo-check",
            "--dangerously-bypass-approvals-and-sandbox",
            "--model",
            "o3",
            "<prompt>",
        ]
    );
    // The fake itself ran (its log has the start), with that argv.
    let argv: Vec<String> = fx
        .read("invocations.log")
        .lines()
        .skip(1)
        .map(String::from)
        .collect();
    assert_eq!(argv, spawned_argv(&fx)[1..]);
    assert_eq!(result.agent_session_id.as_deref(), Some("thread-0"));
    assert!(result.continued);
}

#[tokio::test]
async fn a_thread_that_is_not_found_is_a_crash_with_the_reason_on_stderr() {
    let fx = Fixture::new();
    let result = fx
        .run_in(
            "scenario=thread_not_found",
            None,
            LONG,
            Session::Continue("thread-0".into()),
        )
        .await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Crash));
    assert_eq!(result.detail, "exited with exit status: 1");
    assert!(
        result
            .stderr_tail
            .contains("no rollout found for thread id thread-0"),
        "{}",
        result.stderr_tail
    );
    assert_eq!(result.agent_session_id, None);
}

#[tokio::test]
async fn a_timed_out_attempt_still_reports_the_thread_id() {
    let fx = Fixture::new();
    let result = fx
        .run_in(
            "scenario=hang",
            None,
            Duration::from_millis(500),
            Session::Continue("thread-0".into()),
        )
        .await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Timeout));
    assert_eq!(result.agent_session_id.as_deref(), Some("thread-0"));
}

#[tokio::test]
async fn node_files_by_start_count_select_the_scenario() {
    let fx = Fixture::new();
    fs::write(fx.scenarios().join("work@2"), "scenario=crash\n").unwrap();
    let first = fx.run("scenario=success", None, LONG).await;
    let second = fx.run("scenario=success", None, LONG).await;
    assert_eq!(first.status, AgentStatus::Completed, "{}", first.detail);
    assert_eq!(second.status, AgentStatus::Failed(FailureClass::Crash));
    assert_eq!(fx.read("starts.work"), "2\n");
    for k in 1..=2 {
        assert_eq!(
            fx.read(&format!("prompt.work@{k}")),
            "line one\nTask (work): do it\n"
        );
    }
    assert_eq!(fx.read("prompt.work.1"), "line one\nTask (work): do it\n");
}
