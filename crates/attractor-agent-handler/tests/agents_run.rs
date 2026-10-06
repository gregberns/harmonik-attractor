//! Seam 1 for the registry itself: `Agents::new` and `Agents::run` with a
//! recording fake handler.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use attractor_agent_handler::{
    AgentHandler, AgentObserver, AgentRequest, AgentResult, AgentStatus, Agents, CancellationToken,
    ConfigError, FailureClass, Invocation, Profile, ProfileEnv, Record, Selection, Spawned,
    Started, Usage,
};

/// The argv, env and prompt one invocation was given.
type Seen = (Vec<String>, BTreeMap<String, String>, String);

/// Records each invocation it is given and returns Completed with "done".
struct Recorder {
    mechanism: &'static str,
    seen: Mutex<Vec<Seen>>,
}

impl Recorder {
    fn new(mechanism: &'static str) -> Arc<Self> {
        Arc::new(Self {
            mechanism,
            seen: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl AgentHandler for Recorder {
    fn mechanism(&self) -> &'static str {
        self.mechanism
    }

    async fn run(&self, inv: Invocation<'_>) -> AgentResult {
        self.seen
            .lock()
            .unwrap()
            .push((inv.argv.clone(), inv.env.clone(), inv.prompt.to_string()));
        (inv.spawned)(Spawned {
            pid: 4242,
            pgid: 4242,
            host: Some("host-1".into()),
        });
        AgentResult {
            invocation_id: inv.invocation_id.to_string(),
            status: AgentStatus::Completed,
            text: "done".into(),
            detail: String::new(),
            stderr_tail: String::new(),
            usage: Usage::default(),
            exit: None,
            duration: Duration::ZERO,
            launch_error: None,
        }
    }

    fn transcript_usage(&self, transcript: &str) -> Usage {
        Usage {
            model_actual: Some(transcript.to_string()),
            ..Usage::default()
        }
    }
}

/// Never returns: ignores its timeout and its cancel token.
struct Stuck;

#[async_trait]
impl AgentHandler for Stuck {
    fn mechanism(&self) -> &'static str {
        "stuck"
    }

    async fn run(&self, _inv: Invocation<'_>) -> AgentResult {
        std::future::pending().await
    }

    fn transcript_usage(&self, _transcript: &str) -> Usage {
        Usage::default()
    }
}

/// Records what the observer is told, in order.
#[derive(Default)]
struct Counter {
    finished: Mutex<Vec<AgentResult>>,
    started: Mutex<Vec<Started>>,
}

impl AgentObserver for Counter {
    fn started(&self, started: &Started) {
        self.started.lock().unwrap().push(started.clone());
    }

    fn finished(&self, result: &AgentResult) {
        self.finished.lock().unwrap().push(result.clone());
    }
}

fn profile(name: &str, mechanism: &str) -> Profile {
    Profile {
        name: name.into(),
        mechanism: mechanism.into(),
        command: vec!["agent".into()],
        args: vec!["--quiet".into()],
        model: None,
        model_args: vec!["--model".into(), "{model}".into()],
        reasoning: None,
        reasoning_args: vec!["--effort".into(), "{reasoning}".into()],
        timeout: Duration::from_secs(600),
        kill_grace: Duration::from_millis(50),
        env: ProfileEnv {
            remove: vec!["ANTHROPIC_API_KEY".into()],
            set: BTreeMap::new(),
        },
        test_only: false,
    }
}

fn request<'a>(profile: &str, observer: Option<&'a dyn AgentObserver>) -> AgentRequest<'a> {
    AgentRequest {
        selection: Selection {
            profile: profile.into(),
            model: Some("m1".into()),
            reasoning: None,
        },
        prompt: "do it".into(),
        extra_args: vec!["--extra".into()],
        workdir: PathBuf::from("/work"),
        timeout: Some(Duration::from_secs(5)),
        record: Record {
            run_id: Some("run-1".into()),
            node_id: "work".into(),
            attempt: 1,
            invocation_id: "inv-1".into(),
        },
        transcript: Some(PathBuf::from("/run/transcripts/inv-1.jsonl")),
        stderr: Some(PathBuf::from("/run/transcripts/inv-1.stderr.log")),
        observer,
        cancel: CancellationToken::new(),
    }
}

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[tokio::test]
async fn run_hands_the_handler_argv_env_and_prompt() {
    let recorder = Recorder::new("fake");
    let agents = Agents::new(
        vec![recorder.clone()],
        vec![profile("p", "fake")],
        env(&[("PATH", "/bin"), ("ANTHROPIC_API_KEY", "sk-test")]),
        false,
    )
    .unwrap();

    let result = agents.run(request("p", None)).await;

    assert_eq!(result.status, AgentStatus::Completed);
    assert_eq!(result.invocation_id, "inv-1");
    let seen = recorder.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let (argv, child_env, prompt) = &seen[0];
    assert_eq!(argv, &["agent", "--quiet", "--extra", "--model", "m1"]);
    assert_eq!(prompt, "do it");
    assert_eq!(child_env.get("PATH").map(String::as_str), Some("/bin"));
    assert!(!child_env.contains_key("ANTHROPIC_API_KEY"));
    assert_eq!(
        child_env.get("PAS_RUN_ID").map(String::as_str),
        Some("run-1")
    );
    assert_eq!(
        child_env.get("PAS_INVOCATION_ID").map(String::as_str),
        Some("inv-1")
    );
}

#[tokio::test]
async fn unknown_profile_is_a_launch_failure_naming_it() {
    let agents = Agents::new(vec![Recorder::new("fake")], vec![], BTreeMap::new(), false).unwrap();

    let result = agents.run(request("nope", None)).await;

    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Launch));
    assert!(result.detail.contains("nope"), "{}", result.detail);
    assert_eq!(result.invocation_id, "inv-1");
}

#[tokio::test]
async fn observer_is_told_once_with_the_returned_value() {
    let agents = Agents::new(
        vec![Recorder::new("fake")],
        vec![profile("p", "fake")],
        BTreeMap::new(),
        false,
    )
    .unwrap();
    let counter = Counter::default();

    let ok = agents.run(request("p", Some(&counter))).await;
    let failed = agents.run(request("nope", Some(&counter))).await;

    assert_eq!(*counter.finished.lock().unwrap(), vec![ok, failed]);
}

#[test]
fn new_refuses_a_duplicate_mechanism() {
    let err = Agents::new(
        vec![Recorder::new("fake"), Recorder::new("fake")],
        vec![],
        BTreeMap::new(),
        false,
    )
    .unwrap_err();
    assert_eq!(err, ConfigError::DuplicateMechanism("fake".into()));
}

#[test]
fn new_refuses_a_duplicate_profile() {
    let err = Agents::new(
        vec![Recorder::new("fake")],
        vec![profile("p", "fake"), profile("p", "fake")],
        BTreeMap::new(),
        false,
    )
    .unwrap_err();
    assert_eq!(err, ConfigError::DuplicateProfile("p".into()));
}

#[test]
fn new_refuses_a_profile_without_a_handler() {
    let err = Agents::new(
        vec![Recorder::new("fake")],
        vec![profile("p", "other")],
        BTreeMap::new(),
        false,
    )
    .unwrap_err();
    assert_eq!(
        err,
        ConfigError::NoHandler {
            profile: "p".into(),
            mechanism: "other".into()
        }
    );
}

#[test]
fn transcript_usage_asks_the_profiles_handler() {
    let agents = Agents::new(
        vec![Recorder::new("fake")],
        vec![profile("p", "fake")],
        BTreeMap::new(),
        false,
    )
    .unwrap();
    assert_eq!(
        agents
            .transcript_usage("p", "stream")
            .model_actual
            .as_deref(),
        Some("stream")
    );
    assert_eq!(agents.transcript_usage("nope", "stream"), Usage::default());
}

#[tokio::test]
async fn started_reports_each_spawn_with_the_request_ids_and_paths() {
    let agents = Agents::new(
        vec![Recorder::new("fake")],
        vec![profile("p", "fake")],
        BTreeMap::new(),
        false,
    )
    .unwrap();
    let counter = Counter::default();

    agents.run(request("p", Some(&counter))).await;

    assert_eq!(
        *counter.started.lock().unwrap(),
        vec![Started {
            invocation_id: "inv-1".into(),
            spawn: 1,
            node_id: "work".into(),
            attempt: 1,
            profile: "p".into(),
            model: Some("m1".into()),
            pid: 4242,
            pgid: 4242,
            host: Some("host-1".into()),
            transcript: Some(PathBuf::from("/run/transcripts/inv-1.jsonl")),
            stderr: Some(PathBuf::from("/run/transcripts/inv-1.stderr.log")),
        }]
    );
    assert_eq!(counter.finished.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_handler_that_ignores_its_timeout_is_cut_off_at_the_hard_deadline() {
    let agents = Agents::new(
        vec![Arc::new(Stuck)],
        vec![profile("p", "stuck")],
        BTreeMap::new(),
        false,
    )
    .unwrap()
    .with_hard_deadline_margin(Duration::from_millis(50));
    let counter = Counter::default();
    let mut req = request("p", Some(&counter));
    req.timeout = Some(Duration::from_millis(100));

    let result = agents.run(req).await;

    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Timeout));
    // timeout 100 + kill_grace 50 + margin 50.
    assert_eq!(result.detail, "handler did not return within 200ms");
    assert_eq!(*counter.finished.lock().unwrap(), vec![result]);
}

#[test]
fn stop_grace_is_the_longest_kill_grace_plus_the_margin() {
    let mut slow = profile("slow", "fake");
    slow.kill_grace = Duration::from_secs(3);
    let agents = Agents::new(
        vec![Recorder::new("fake")],
        vec![profile("p", "fake"), slow],
        BTreeMap::new(),
        false,
    )
    .unwrap()
    .with_hard_deadline_margin(Duration::from_secs(1));
    assert_eq!(agents.max_kill_grace(), Duration::from_secs(3));
    assert_eq!(agents.stop_grace(), Duration::from_secs(4));
}

#[test]
fn stop_grace_saturates_instead_of_overflowing() {
    let mut huge = profile("huge", "fake");
    huge.kill_grace = Duration::MAX;
    let agents = Agents::new(
        vec![Recorder::new("fake")],
        vec![huge],
        BTreeMap::new(),
        false,
    )
    .unwrap();
    assert_eq!(agents.stop_grace(), Duration::MAX);
}

#[tokio::test]
async fn a_huge_timeout_does_not_overflow_the_hard_deadline() {
    let agents = Agents::new(
        vec![Recorder::new("fake")],
        vec![profile("p", "fake")],
        BTreeMap::new(),
        false,
    )
    .unwrap();
    let mut req = request("p", None);
    req.timeout = Some(Duration::MAX);
    assert_eq!(agents.run(req).await.status, AgentStatus::Completed);
}

/// Records the timeout and kill grace each invocation was given.
struct Timeouts(Mutex<Vec<(Duration, Duration)>>);

#[async_trait]
impl AgentHandler for Timeouts {
    fn mechanism(&self) -> &'static str {
        "timeouts"
    }

    async fn run(&self, inv: Invocation<'_>) -> AgentResult {
        self.0.lock().unwrap().push((inv.timeout, inv.kill_grace));
        AgentResult::failed(inv.invocation_id, FailureClass::Reported, "")
    }

    fn transcript_usage(&self, _transcript: &str) -> Usage {
        Usage::default()
    }
}

#[tokio::test]
async fn no_request_timeout_uses_the_profiles() {
    let handler = Arc::new(Timeouts(Mutex::new(Vec::new())));
    let mut p = profile("p", "timeouts");
    p.timeout = Duration::from_secs(7);
    let agents = Agents::new(vec![handler.clone()], vec![p], BTreeMap::new(), false).unwrap();

    let mut req = request("p", None);
    req.timeout = None;
    agents.run(req).await;
    agents.run(request("p", None)).await;

    assert_eq!(
        *handler.0.lock().unwrap(),
        vec![
            (Duration::from_secs(7), Duration::from_millis(50)),
            (Duration::from_secs(5), Duration::from_millis(50)),
        ]
    );
}

#[tokio::test]
async fn reasoning_and_the_profiles_model_reach_the_argv() {
    let recorder = Recorder::new("fake");
    let mut p = profile("p", "fake");
    p.model = Some("m0".into());
    let agents = Agents::new(vec![recorder.clone()], vec![p], BTreeMap::new(), false).unwrap();
    let counter = Counter::default();

    let mut req = request("p", Some(&counter));
    req.selection.model = None;
    req.selection.reasoning = Some("high".into());
    agents.run(req).await;

    let seen = recorder.seen.lock().unwrap();
    assert_eq!(
        seen[0].0,
        ["agent", "--quiet", "--extra", "--model", "m0", "--effort", "high"]
    );
    let started = counter.started.lock().unwrap();
    assert_eq!(started[0].model.as_deref(), Some("m0"));
}

#[tokio::test]
async fn env_set_reaches_the_child_even_when_removed() {
    let recorder = Recorder::new("fake");
    let mut p = profile("p", "fake");
    p.env
        .set
        .insert("ANTHROPIC_API_KEY".into(), "set-by-profile".into());
    let agents = Agents::new(
        vec![recorder.clone()],
        vec![p],
        env(&[("ANTHROPIC_API_KEY", "inherited")]),
        false,
    )
    .unwrap();

    agents.run(request("p", None)).await;

    let seen = recorder.seen.lock().unwrap();
    assert_eq!(
        seen[0].1.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some("set-by-profile")
    );
}

#[tokio::test]
async fn a_test_only_profile_runs_only_when_allowed() {
    let mut p = profile("fake", "fake");
    p.test_only = true;
    let selection = request("fake", None).selection;

    let refused = Recorder::new("fake");
    let agents = Agents::new(
        vec![refused.clone()],
        vec![p.clone()],
        BTreeMap::new(),
        false,
    )
    .unwrap();
    assert_eq!(
        agents.check(&selection),
        Err(ConfigError::TestOnly {
            profile: "fake".into()
        })
    );
    let result = agents.run(request("fake", None)).await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Launch));
    assert!(
        result.detail.contains("--allow-test-agents"),
        "{}",
        result.detail
    );
    assert!(refused.seen.lock().unwrap().is_empty(), "nothing started");

    let allowed = Recorder::new("fake");
    let agents = Agents::new(vec![allowed.clone()], vec![p], BTreeMap::new(), true).unwrap();
    assert_eq!(agents.check(&selection), Ok(()));
    assert_eq!(
        agents.run(request("fake", None)).await.status,
        AgentStatus::Completed
    );
}

#[test]
fn check_refuses_an_unknown_profile_and_reasoning_without_args() {
    let mut plain = profile("plain", "fake");
    plain.reasoning_args.clear();
    let agents = Agents::new(
        vec![Recorder::new("fake")],
        vec![profile("p", "fake"), plain],
        BTreeMap::new(),
        false,
    )
    .unwrap();
    let selection = |profile: &str, reasoning: Option<&str>| Selection {
        profile: profile.into(),
        model: None,
        reasoning: reasoning.map(String::from),
    };

    assert_eq!(
        agents.check(&selection("nope", None)),
        Err(ConfigError::UnknownProfile("nope".into()))
    );
    assert_eq!(agents.check(&selection("p", Some("high"))), Ok(()));
    assert_eq!(agents.check(&selection("plain", None)), Ok(()));
    assert_eq!(
        agents.check(&selection("plain", Some("high"))),
        Err(ConfigError::NoReasoningArgs {
            profile: "plain".into()
        })
    );
}
