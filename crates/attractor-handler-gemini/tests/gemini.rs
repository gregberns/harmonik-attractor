#![cfg(unix)]
//! Seam 1: `Agents::run` with the `gemini` handler and a profile whose
//! command is the shell fake (`tests/agents/fake-gemini`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use attractor_agent_handler::{
    AgentRequest, AgentResult, AgentStatus, Agents, CancellationToken, FailureClass, Profile,
    ProfileEnv, Record, Selection,
};
use attractor_handler_gemini::Gemini;

fn fake_gemini() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/agents/fake-gemini")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// The built-in `gemini` profile's shape with `command`.
fn profile(command: Vec<String>) -> Profile {
    Profile {
        name: "gemini".into(),
        mechanism: "gemini".into(),
        command,
        args: vec!["--approval-mode".into(), "yolo".into()],
        model: None,
        model_args: vec!["--model".into(), "{model}".into()],
        reasoning: None,
        reasoning_args: vec![],
        timeout: Duration::from_secs(600),
        kill_grace: Duration::from_secs(1),
        rate_limit_window: Duration::ZERO,
        env: ProfileEnv::default(),
        test_only: false,
        session_args: vec![],
        resume: None,
        provider: None,
        base_url: None,
        api_key_env: None,
        limits: None,
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

    fn prompt_file(&self) -> PathBuf {
        self.dir.path().join("t.prompt.txt")
    }

    /// The argv section of the prompt file.
    fn prompt_file_argv(&self) -> Vec<String> {
        let text = fs::read_to_string(self.prompt_file()).unwrap();
        let start = text.find("argv:\n").unwrap() + "argv:\n".len();
        let end = text[start..].find("\n\n").unwrap() + start;
        text[start..end].lines().map(String::from).collect()
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.scenarios().join(name)).unwrap_or_default()
    }

    /// How the fake answers `--help`: `json`, `fail`, `hang`, or (default)
    /// with `stream-json`.
    fn help(&self, mode: &str) {
        fs::write(self.scenarios().join("gemini-help"), format!("{mode}\n")).unwrap();
    }

    fn agents(&self, handler: Gemini, command: Vec<String>) -> Agents {
        let env = BTreeMap::from([
            ("PATH".to_string(), std::env::var("PATH").unwrap()),
            (
                "FAKE_AGENT_SCENARIOS".to_string(),
                self.scenarios().to_string_lossy().into_owned(),
            ),
        ]);
        Agents::new(vec![Arc::new(handler)], vec![profile(command)], env, false).unwrap()
    }

    async fn run_with(&self, agents: &Agents, scenario: &str, model: Option<&str>) -> AgentResult {
        fs::write(self.scenarios().join("work"), format!("{scenario}\n")).unwrap();
        agents
            .run(AgentRequest {
                selection: Selection {
                    profile: "gemini".into(),
                    model: model.map(String::from),
                    reasoning: None,
                },
                prompt: "Task (work): do it".into(),
                extra_args: vec![],
                workdir: self.dir.path().join("work"),
                timeout: Some(Duration::from_secs(20)),
                record: Record {
                    run_id: None,
                    node_id: "work".into(),
                    attempt: 1,
                    invocation_id: "inv-1".into(),
                },
                transcript: Some(self.dir.path().join("t.jsonl")),
                stderr: Some(self.dir.path().join("t.stderr.log")),
                prompt_file: Some(self.prompt_file()),
                session: attractor_agent_handler::Session::New("sess-1".into()),
                state_dir: None,
                observer: None,
                cancel: CancellationToken::new(),
            })
            .await
    }

    async fn run(&self, scenario: &str) -> AgentResult {
        let agents = self.agents(Gemini::default(), vec![fake_gemini()]);
        self.run_with(&agents, scenario, None).await
    }

    /// The fake's argv per start.
    fn invocations(&self) -> Vec<Vec<String>> {
        self.read("invocations.log")
            .split("--- start\n")
            .skip(1)
            .map(|block| block.lines().map(String::from).collect())
            .collect()
    }
}

#[tokio::test]
async fn stream_json_success_when_help_lists_it() {
    let fx = Fixture::new();
    let result = fx.run("scenario=success").await;
    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    assert_eq!(result.text, "fake-gemini: success");
    assert_eq!(fx.invocations()[0][..2], ["--output-format", "stream-json"]);
    assert_eq!(result.usage.model_actual.as_deref(), Some("fake-model"));
}

#[tokio::test]
async fn json_success_when_help_does_not_list_stream_json() {
    let fx = Fixture::new();
    fx.help("json");
    let result = fx.run("scenario=success").await;
    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    assert_eq!(result.text, "fake-gemini: success");
    assert_eq!(fx.invocations()[0][..2], ["--output-format", "json"]);
}

#[tokio::test]
async fn a_failing_probe_falls_back_to_json() {
    let fx = Fixture::new();
    fx.help("fail");
    let result = fx.run("scenario=success").await;
    assert_eq!(result.status, AgentStatus::Completed);
    assert_eq!(fx.invocations()[0][..2], ["--output-format", "json"]);
}

#[tokio::test]
async fn failure_is_reported_in_both_formats() {
    for help in ["stream", "json"] {
        let fx = Fixture::new();
        fx.help(help);
        let result = fx.run("scenario=failure").await;
        assert_eq!(
            result.status,
            AgentStatus::Failed(FailureClass::Reported),
            "{help}"
        );
        assert_eq!(result.text, "fake-gemini: failed", "{help}");
    }
}

#[tokio::test]
async fn a_crash_is_a_crash() {
    let fx = Fixture::new();
    let result = fx.run("scenario=crash").await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Crash));
    assert_eq!(result.stderr_tail, "fake-gemini: crashed");
}

#[tokio::test]
async fn the_probe_runs_once_per_command_and_logs_nothing_else() {
    let fx = Fixture::new();
    let agents = fx.agents(Gemini::default(), vec![fake_gemini()]);
    fx.run_with(&agents, "scenario=success", None).await;
    fx.run_with(&agents, "scenario=success", None).await;
    assert_eq!(
        fx.read("probes.log").lines().count(),
        1,
        "{}",
        fx.read("probes.log")
    );
    assert_eq!(fx.invocations().len(), 2);
    assert_eq!(fx.read("env.log").matches("--- start").count(), 2);
}

#[tokio::test]
async fn argv_is_todays_with_the_prompt_last() {
    let fx = Fixture::new();
    let agents = fx.agents(Gemini::default(), vec![fake_gemini()]);
    fx.run_with(&agents, "scenario=success", Some("gemini-2.5-pro"))
        .await;
    assert_eq!(
        fx.invocations(),
        [[
            "--output-format",
            "stream-json",
            "--approval-mode",
            "yolo",
            "--model",
            "gemini-2.5-pro",
            "<prompt>",
        ]]
    );
}

#[tokio::test]
async fn a_multi_word_command_gets_the_format_after_it_and_is_probed_whole() {
    let fx = Fixture::new();
    let command = vec!["env".to_string(), "FAKE_VIA=env".to_string(), fake_gemini()];
    let agents = fx.agents(Gemini::default(), command);
    let result = fx.run_with(&agents, "scenario=success", None).await;
    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    // The fake logs only its own arguments: --output-format comes first,
    // so it went after the whole command.
    assert_eq!(fx.invocations()[0][..2], ["--output-format", "stream-json"]);
    let probes = fx.read("probes.log");
    assert!(
        probes.contains("--help") && probes.contains("via=env"),
        "{probes}"
    );
}

#[tokio::test]
async fn a_hanging_probe_falls_back_to_json_and_leaves_nothing_behind() {
    let fx = Fixture::new();
    fx.help("hang");
    let agents = fx.agents(
        Gemini::default().with_probe_timeout(Duration::from_millis(500)),
        vec![fake_gemini()],
    );
    let result = fx.run_with(&agents, "scenario=success", None).await;
    assert_eq!(result.status, AgentStatus::Completed);
    assert_eq!(fx.invocations()[0][..2], ["--output-format", "json"]);
    let pid: libc::pid_t = fx.read("probe.pid").trim().parse().unwrap();
    // The probe's group was killed; give the kernel a moment to reap it.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    // SAFETY: signal 0 only checks that the process exists.
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "probe {pid} still alive"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[tokio::test]
async fn the_prompt_file_records_the_spawned_argv_with_the_probed_format() {
    let fx = Fixture::new();
    fx.run("scenario=success").await;
    let mut expected = vec![fake_gemini()];
    expected.extend(fx.invocations()[0].iter().cloned());
    assert_eq!(fx.prompt_file_argv(), expected);
    assert_eq!(
        fx.prompt_file_argv()[1..3],
        ["--output-format", "stream-json"]
    );
}

#[tokio::test]
async fn gemini_reports_no_session_id() {
    let fx = Fixture::new();
    for help in ["stream", "json"] {
        fx.help(help);
        let agents = fx.agents(Gemini::default(), vec![fake_gemini()]);
        let result = fx.run_with(&agents, "scenario=success", None).await;
        assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
        assert_eq!(result.agent_session_id, None, "{help}");
        assert!(!result.continued);
    }
}

#[tokio::test]
async fn the_start_counter_counts_runs_but_not_the_probe() {
    let fx = Fixture::new();
    fs::write(fx.scenarios().join("work@2"), "scenario=crash\n").unwrap();
    let agents = fx.agents(Gemini::default(), vec![fake_gemini()]);
    let first = fx.run_with(&agents, "scenario=success", None).await;
    let second = fx.run_with(&agents, "scenario=success", None).await;
    assert_eq!(first.status, AgentStatus::Completed, "{}", first.detail);
    assert_eq!(second.status, AgentStatus::Failed(FailureClass::Crash));
    assert_eq!(fx.read("probes.log").lines().count(), 1);
    assert_eq!(fx.read("starts.work"), "2\n");
    assert_eq!(fx.read("prompt.work@1"), "Task (work): do it\n");
    assert_eq!(fx.read("prompt.work@2"), "Task (work): do it\n");
}
