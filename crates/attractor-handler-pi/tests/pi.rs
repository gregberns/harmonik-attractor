#![cfg(unix)]
//! Seam 1: `Agents::run` with the `pi` handler and a profile whose command
//! is the shell fake (`tests/agents/fake-pi`). No real `pi`, endpoint or
//! key: the key is the dummy `sk-secret`, in a variable only this test sets.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use attractor_agent_handler::{
    AgentRequest, AgentResult, AgentStatus, Agents, CancellationToken, FailureClass, Limits,
    Profile, ProfileEnv, Record, Selection, Session,
};
use attractor_handler_pi::Pi;

const KEY: &str = "sk-secret";

fn fake_pi() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/agents/fake-pi")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// A Pi profile as design §2 has it, run by the fake, for a custom
/// endpoint (so `base_url` and `limits` are set).
fn profile() -> Profile {
    Profile {
        name: "pi".into(),
        mechanism: "pi".into(),
        command: vec![fake_pi()],
        args: vec!["--mode".into(), "json".into()],
        model: Some("fake-model".into()),
        model_args: vec![],
        reasoning: Some("high".into()),
        reasoning_args: vec!["--thinking".into(), "{reasoning}".into()],
        timeout: Duration::from_secs(20),
        kill_grace: Duration::from_millis(200),
        env: ProfileEnv {
            remove: vec!["OPENAI_API_KEY".into(), "ANTHROPIC_API_KEY".into()],
            set: BTreeMap::new(),
        },
        test_only: false,
        session_args: vec![],
        resume: None,
        provider: Some("fakeprov".into()),
        base_url: Some("http://127.0.0.1:9/v1".into()),
        api_key_env: Some("FAKE_PI_KEY".into()),
        limits: Some(Limits {
            context: 1000,
            max_output: 100,
        }),
    }
}

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["scenarios", "work", "run", "home"] {
            fs::create_dir(dir.path().join(sub)).unwrap();
        }
        Self { dir }
    }

    fn scenarios(&self) -> PathBuf {
        self.dir.path().join("scenarios")
    }

    fn state_dir(&self) -> PathBuf {
        self.dir.path().join("run")
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.scenarios().join(name)).unwrap_or_default()
    }

    fn parent_env(&self, extra: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut env = BTreeMap::from([
            ("PATH".to_string(), std::env::var("PATH").unwrap()),
            (
                "HOME".to_string(),
                self.dir.path().join("home").to_string_lossy().into_owned(),
            ),
            (
                "FAKE_AGENT_SCENARIOS".to_string(),
                self.scenarios().to_string_lossy().into_owned(),
            ),
        ]);
        env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        env
    }

    fn agents_with(&self, profile: Profile, extra: &[(&str, &str)]) -> Agents {
        Agents::new(
            vec![Arc::new(Pi)],
            vec![profile],
            self.parent_env(extra),
            false,
        )
        .unwrap()
    }

    fn agents(&self) -> Agents {
        self.agents_with(profile(), &[("FAKE_PI_KEY", KEY)])
    }

    fn request(&self, scenario: &str) -> AgentRequest<'static> {
        fs::write(
            self.scenarios().join("work"),
            format!("scenario={scenario}\n"),
        )
        .unwrap();
        AgentRequest {
            selection: Selection {
                profile: "pi".into(),
                model: None,
                reasoning: None,
            },
            prompt: "Task (work): do it".into(),
            extra_args: vec![],
            workdir: self.dir.path().join("work"),
            timeout: None,
            record: Record {
                run_id: Some("run-1".into()),
                node_id: "work".into(),
                attempt: 1,
                invocation_id: "inv-1".into(),
            },
            transcript: Some(self.dir.path().join("t.jsonl")),
            stderr: Some(self.dir.path().join("t.stderr.log")),
            prompt_file: Some(self.dir.path().join("t.prompt.txt")),
            session: Session::New("sess-1".into()),
            state_dir: Some(self.state_dir()),
            observer: None,
            cancel: CancellationToken::new(),
        }
    }

    async fn run(&self, scenario: &str) -> AgentResult {
        self.agents().run(self.request(scenario)).await
    }

    /// The per-invocation agent dir the fake was given on its first start.
    fn agent_dir(&self) -> PathBuf {
        PathBuf::from(self.read("pi-agent-dir.work@1").trim())
    }

    fn models(&self) -> serde_json::Value {
        serde_json::from_str(&self.read("models.work@1.json")).unwrap()
    }
}

#[tokio::test]
async fn each_scenario_ends_with_its_status() {
    for (scenario, status, text) in [
        ("stop", AgentStatus::Completed, "fake-pi: done"),
        (
            "tool_use_then_stop",
            AgentStatus::Completed,
            "fake-pi: done after tool",
        ),
        (
            "error",
            AgentStatus::Failed(FailureClass::Reported),
            "401: fake auth error",
        ),
        (
            "aborted",
            AgentStatus::Failed(FailureClass::Reported),
            "aborted",
        ),
        ("no_final", AgentStatus::Failed(FailureClass::NoResult), ""),
        (
            "length",
            AgentStatus::Failed(FailureClass::NoResult),
            "fake-pi: cut",
        ),
        ("crash", AgentStatus::Failed(FailureClass::Crash), ""),
    ] {
        let fx = Fixture::new();
        let result = fx.run(scenario).await;
        assert_eq!(result.status, status, "{scenario}: {}", result.detail);
        assert_eq!(result.text, text, "{scenario}");
        assert_eq!(
            result.agent_session_id.as_deref(),
            Some("sess-1"),
            "{scenario}"
        );
    }
}

#[tokio::test]
async fn reported_and_no_result_details_say_why() {
    let fx = Fixture::new();
    assert_eq!(fx.run("error").await.detail, "401: fake auth error");
    let fx = Fixture::new();
    assert_eq!(
        fx.run("length").await.detail,
        "pi ended with stopReason length: fake-pi: cut"
    );
}

#[tokio::test]
async fn usage_and_cost_come_from_the_assistant_messages() {
    let fx = Fixture::new();
    let result = fx.run("tool_use_then_stop").await;
    assert_eq!(result.usage.input_tokens, Some(20));
    assert_eq!(result.usage.output_tokens, Some(10));
    assert_eq!(result.usage.cost_usd, Some(0.006));
    assert_eq!(result.usage.model_actual.as_deref(), Some("fake-model"));
}

#[tokio::test]
async fn pi_reads_a_private_models_json_holding_the_key_and_nothing_else_does() {
    let fx = Fixture::new();
    let result = fx.run("stop").await;
    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);

    assert_eq!(
        fx.models(),
        serde_json::json!({"providers": {"fakeprov": {
            "baseUrl": "http://127.0.0.1:9/v1",
            "api": "openai-completions",
            "apiKey": KEY,
            "models": [{"id": "fake-model", "contextWindow": 1000, "maxTokens": 100}],
        }}})
    );
    assert_eq!(fx.read("models.work@1.mode").trim(), "0600");
    assert_eq!(fx.read("settings.work@1.json"), "{}");

    let env = fx.read("env.log");
    for line in [
        "PI_TELEMETRY=0",
        "PI_OFFLINE=1",
        "PI_SKIP_VERSION_CHECK=1",
        &format!("PI_CODING_AGENT_DIR={}", fx.agent_dir().display()),
    ] {
        assert!(env.lines().any(|l| l == line), "{line} in {env}");
    }
    assert!(!env.contains("FAKE_PI_KEY"), "{env}");
    assert!(!env.contains(KEY), "{env}");
    let invocations = fx.read("invocations.log");
    assert!(!invocations.contains(KEY), "{invocations}");
    assert!(!invocations.contains("--api-key"), "{invocations}");
    let prompt_file = fs::read_to_string(fx.dir.path().join("t.prompt.txt")).unwrap();
    assert!(!prompt_file.contains(KEY), "{prompt_file}");
}

#[tokio::test]
async fn argv_names_the_model_session_dir_and_id() {
    let fx = Fixture::new();
    fx.run("stop").await;
    let sessions = fx.state_dir().join("pi-sessions");
    let expected = format!(
        "--- start\n--mode\njson\n--thinking\nhigh\n--model\nfakeprov/fake-model\n--session-dir\n{}\n--session-id\nsess-1\n<prompt>\n",
        sessions.display()
    );
    assert_eq!(fx.read("invocations.log"), expected);
    assert!(sessions.is_dir());
    assert!(fx.agent_dir().starts_with(fx.state_dir().join("pi-agent")));
}

#[tokio::test]
async fn the_agent_dir_is_gone_after_stop_error_and_timeout() {
    for (scenario, timeout, status) in [
        ("stop", None, AgentStatus::Completed),
        ("error", None, AgentStatus::Failed(FailureClass::Reported)),
        (
            "hang",
            Some(Duration::from_secs(1)),
            AgentStatus::Failed(FailureClass::Timeout),
        ),
    ] {
        let fx = Fixture::new();
        let mut req = fx.request(scenario);
        req.timeout = timeout;
        let result = fx.agents().run(req).await;
        assert_eq!(result.status, status, "{scenario}");
        assert!(fx.read("models.work@1.mode").contains("0600"), "{scenario}");
        assert!(!fx.agent_dir().exists(), "{scenario}");
        assert_eq!(
            fs::read_dir(fx.state_dir().join("pi-agent"))
                .unwrap()
                .count(),
            0,
            "{scenario}"
        );
    }
}

#[tokio::test]
async fn a_timeout_keeps_the_session_id_though_pi_printed_nothing() {
    let fx = Fixture::new();
    let mut req = fx.request("hang");
    req.timeout = Some(Duration::from_secs(1));
    let result = fx.agents().run(req).await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Timeout));
    assert_eq!(result.agent_session_id.as_deref(), Some("sess-1"));
}

#[tokio::test]
async fn a_cancelled_run_removes_the_agent_dir() {
    let fx = Fixture::new();
    let agents = fx.agents();
    let req = fx.request("hang");
    let cancel = req.cancel.clone();
    let mode_file = fx.scenarios().join("models.work@1.mode");
    let run = agents.run(req);
    let cancel_once_started = async {
        // The fake writes the mode file before it hangs.
        while !mode_file.exists() {
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(run, cancel_once_started);
    assert_eq!(result.status, AgentStatus::Cancelled);
    assert_eq!(result.agent_session_id.as_deref(), Some("sess-1"));
    assert!(!fx.agent_dir().exists());
}

#[tokio::test]
async fn a_missing_key_fails_to_launch_without_starting_pi() {
    let fx = Fixture::new();
    let result = fx.agents_with(profile(), &[]).run(fx.request("stop")).await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Launch));
    assert!(
        result
            .detail
            .contains("API key variable FAKE_PI_KEY is not set"),
        "{}",
        result.detail
    );
    assert_eq!(fx.read("invocations.log"), "");
    assert!(!fx.state_dir().join("pi-agent").exists());
}

#[tokio::test]
async fn a_key_in_the_default_remove_list_is_used_but_never_reaches_pi() {
    let named = Profile {
        api_key_env: Some("OPENAI_API_KEY".into()),
        ..profile()
    };
    // From pas's environment, though env.remove lists it.
    let fx = Fixture::new();
    fx.agents_with(named.clone(), &[("OPENAI_API_KEY", "sk-openai")])
        .run(fx.request("stop"))
        .await;
    assert_eq!(
        fx.models()["providers"]["fakeprov"]["apiKey"],
        serde_json::json!("sk-openai")
    );
    assert!(!fx.read("env.log").contains("OPENAI_API_KEY"));

    // From the profile's env.set.
    let mut set = named;
    set.env
        .set
        .insert("OPENAI_API_KEY".into(), "sk-from-profile".into());
    let fx = Fixture::new();
    fx.agents_with(set, &[]).run(fx.request("stop")).await;
    assert_eq!(
        fx.models()["providers"]["fakeprov"]["apiKey"],
        serde_json::json!("sk-from-profile")
    );
    assert!(!fx.read("env.log").contains("OPENAI_API_KEY"));
}

#[tokio::test]
async fn no_run_folder_is_a_launch_failure() {
    let fx = Fixture::new();
    let mut req = fx.request("stop");
    req.state_dir = None;
    let result = fx.agents().run(req).await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Launch));
    assert_eq!(result.detail, "the pi handler needs a run folder");
    assert_eq!(fx.read("invocations.log"), "");
}

#[tokio::test]
async fn continuing_runs_the_same_command() {
    let fx = Fixture::new();
    let agents = fx.agents();
    agents.run(fx.request("stop")).await;
    let mut again = fx.request("stop");
    again.session = Session::Continue("sess-1".into());
    let result = agents.run(again).await;
    assert_eq!(result.status, AgentStatus::Completed);
    let log = fx.read("invocations.log");
    let starts: Vec<&str> = log.split("--- start\n").skip(1).collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0], starts[1]);
}
