//! Seam 1 for the registry itself: `Agents::new` and `Agents::run` with a
//! recording fake handler.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use attractor_agent_handler::{
    AgentHandler, AgentObserver, AgentRequest, AgentResult, AgentStatus, Agents, CancellationToken,
    ConfigError, FailureClass, Invocation, Limits, Profile, ProfileEnv, RateLimited, Record,
    Resume, Selection, Spawned, Started, Usage,
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
            agent_session_id: None,
            continued: false,
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
    rate_limited: Mutex<Vec<RateLimited>>,
}

impl AgentObserver for Counter {
    fn started(&self, started: &Started) {
        self.started.lock().unwrap().push(started.clone());
    }

    fn finished(&self, result: &AgentResult) {
        self.finished.lock().unwrap().push(result.clone());
    }

    fn rate_limited(&self, rate_limited: &RateLimited) {
        self.rate_limited.lock().unwrap().push(rate_limited.clone());
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
        rate_limit_window: Duration::ZERO,
        env: ProfileEnv {
            remove: vec!["ANTHROPIC_API_KEY".into()],
            set: BTreeMap::new(),
        },
        test_only: false,
        session_args: vec![],
        resume: None,
        provider: None,
        base_url: None,
        api_key_env: None,
        limits: None,
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
        prompt_file: None,
        session: attractor_agent_handler::Session::New("sess-1".into()),
        state_dir: None,
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
            session_id: Some("sess-1".into()),
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
    assert_eq!(
        agents.stop_grace_of(["p", "nope"]),
        Duration::from_millis(1050)
    );
    assert_eq!(agents.stop_grace_of(["p", "slow"]), Duration::from_secs(4));
    assert_eq!(agents.stop_grace_of([]), Duration::from_secs(1));
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

/// Records `command_len`; says it is "Named" and reports no cost.
struct Named(Mutex<Vec<usize>>);

#[async_trait]
impl AgentHandler for Named {
    fn mechanism(&self) -> &'static str {
        "named"
    }

    fn display_name(&self) -> &'static str {
        "Named Agent"
    }

    fn reports_cost(&self) -> bool {
        false
    }

    async fn run(&self, inv: Invocation<'_>) -> AgentResult {
        self.0.lock().unwrap().push(inv.command_len);
        AgentResult::failed(inv.invocation_id, FailureClass::Reported, "")
    }

    fn transcript_usage(&self, _transcript: &str) -> Usage {
        Usage::default()
    }
}

#[tokio::test]
async fn handlers_name_themselves_and_see_the_commands_length() {
    let named = Arc::new(Named(Mutex::new(Vec::new())));
    let mut multi = profile("multi", "named");
    multi.command = vec!["npx".into(), "@google/gemini-cli".into()];
    let agents = Agents::new(
        vec![named.clone(), Recorder::new("fake")],
        vec![multi, profile("p", "fake")],
        BTreeMap::new(),
        false,
    )
    .unwrap();

    agents.run(request("multi", None)).await;

    assert_eq!(*named.0.lock().unwrap(), vec![2]);
    assert_eq!(agents.display_name("multi"), Some("Named Agent"));
    assert_eq!(agents.reports_cost("multi"), Some(false));
    // The defaults: the mechanism, and a cost.
    assert_eq!(agents.display_name("p"), Some("fake"));
    assert_eq!(agents.reports_cost("p"), Some(true));
    assert_eq!(agents.display_name("nope"), None);
}

/// What a keyed invocation was given beyond argv and env.
#[derive(Debug, Clone)]
struct KeyedSeen {
    api_key: Option<String>,
    state_dir: Option<PathBuf>,
    env: BTreeMap<String, String>,
    debug: String,
}

/// A handler that takes the Pi fields, resumes natively and records the
/// key and state dir it is handed.
struct Keyed {
    seen: Mutex<Vec<KeyedSeen>>,
}

impl Keyed {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl AgentHandler for Keyed {
    fn mechanism(&self) -> &'static str {
        "keyed"
    }

    fn check(&self, profile: &Profile) -> Result<(), ConfigError> {
        match profile.provider {
            Some(_) => Ok(()),
            None => Err(ConfigError::HandlerField {
                profile: profile.name.clone(),
                field: "provider",
                problem: "is required".into(),
            }),
        }
    }

    fn resumes_natively(&self) -> bool {
        true
    }

    async fn run(&self, inv: Invocation<'_>) -> AgentResult {
        self.seen.lock().unwrap().push(KeyedSeen {
            api_key: inv.api_key.map(str::to_string),
            state_dir: inv.state_dir.map(PathBuf::from),
            env: inv.env.clone(),
            debug: format!("{inv:?}"),
        });
        AgentResult::failed(inv.invocation_id, FailureClass::Launch, "recorded")
    }

    fn transcript_usage(&self, _transcript: &str) -> Usage {
        Usage::default()
    }
}

fn keyed_profile(name: &str) -> Profile {
    let mut p = profile(name, "keyed");
    p.provider = Some("deepseek".into());
    p.api_key_env = Some("FAKE_PI_KEY".into());
    p
}

#[test]
fn the_default_check_refuses_pi_fields_on_another_mechanism() {
    for (field, set) in [
        (
            "provider",
            (|p: &mut Profile| p.provider = Some("x".into())) as fn(&mut Profile),
        ),
        ("base_url", |p| p.base_url = Some("https://x".into())),
        ("api_key_env", |p| p.api_key_env = Some("K".into())),
        ("limits", |p| {
            p.limits = Some(Limits {
                context: 1,
                max_output: 1,
            })
        }),
    ] {
        let mut p = profile("p", "claude-p");
        set(&mut p);
        let err = Agents::new(
            vec![Recorder::new("claude-p")],
            vec![p],
            BTreeMap::new(),
            false,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ConfigError::FieldNotForMechanism {
                profile: "p".into(),
                field,
                mechanism: "claude-p".into(),
            }
        );
        assert!(err.to_string().contains(field), "{err}");
    }
}

#[test]
fn new_asks_each_profiles_handler_to_check_it() {
    let mut p = keyed_profile("p");
    p.provider = None;
    let err = Agents::new(vec![Keyed::new()], vec![p], BTreeMap::new(), false).unwrap_err();
    assert_eq!(err.to_string(), "agent profile p: provider is required");
}

#[test]
fn a_natively_resuming_handler_can_resume_without_a_resume_form() {
    let agents = Agents::new(
        vec![Keyed::new(), Recorder::new("fake")],
        vec![keyed_profile("pi"), profile("plain", "fake")],
        BTreeMap::new(),
        false,
    )
    .unwrap();
    assert_eq!(agents.can_resume("pi"), Some(true));
    assert_eq!(agents.can_resume("plain"), Some(false));
    assert_eq!(agents.can_resume("nope"), None);
}

#[tokio::test]
async fn the_api_key_reaches_the_handler_but_not_the_childs_env() {
    let keyed = Keyed::new();
    let mut p = keyed_profile("p");
    p.env.remove.push("FAKE_PI_KEY".into());
    let agents = Agents::new(
        vec![keyed.clone()],
        vec![p],
        env(&[("PATH", "/bin"), ("FAKE_PI_KEY", "sk-secret")]),
        false,
    )
    .unwrap();
    let mut req = request("p", None);
    req.state_dir = Some(PathBuf::from("/run/dir"));

    agents.run(req).await;

    let seen = keyed.seen.lock().unwrap();
    assert_eq!(seen[0].api_key.as_deref(), Some("sk-secret"));
    assert_eq!(seen[0].state_dir, Some(PathBuf::from("/run/dir")));
    assert!(
        !seen[0].env.contains_key("FAKE_PI_KEY"),
        "{:?}",
        seen[0].env
    );
    assert_eq!(seen[0].env.get("PATH").map(String::as_str), Some("/bin"));
    assert!(!seen[0].debug.contains("sk-secret"), "{}", seen[0].debug);
}

#[tokio::test]
async fn the_profiles_env_set_key_wins_and_is_still_kept_from_the_child() {
    let keyed = Keyed::new();
    let mut p = keyed_profile("p");
    p.env
        .set
        .insert("FAKE_PI_KEY".into(), "sk-from-profile".into());
    let agents = Agents::new(
        vec![keyed.clone()],
        vec![p],
        env(&[("FAKE_PI_KEY", "sk-inherited")]),
        false,
    )
    .unwrap();

    agents.run(request("p", None)).await;

    let seen = keyed.seen.lock().unwrap();
    assert_eq!(seen[0].api_key.as_deref(), Some("sk-from-profile"));
    assert!(!seen[0].env.contains_key("FAKE_PI_KEY"));
}

#[tokio::test]
async fn a_missing_or_empty_api_key_is_a_launch_failure_naming_the_variable() {
    for parent in [env(&[]), env(&[("FAKE_PI_KEY", "")])] {
        let keyed = Keyed::new();
        let agents =
            Agents::new(vec![keyed.clone()], vec![keyed_profile("p")], parent, false).unwrap();

        let result = agents.run(request("p", None)).await;

        assert_eq!(result.status, AgentStatus::Failed(FailureClass::Launch));
        assert!(
            result
                .detail
                .contains("API key variable FAKE_PI_KEY is not set"),
            "{}",
            result.detail
        );
        assert!(
            keyed.seen.lock().unwrap().is_empty(),
            "the handler never ran"
        );
    }
}

#[tokio::test]
async fn a_profile_without_api_key_env_hands_no_key() {
    let keyed = Keyed::new();
    let mut p = keyed_profile("p");
    p.api_key_env = None;
    let agents = Agents::new(
        vec![keyed.clone()],
        vec![p],
        env(&[("FAKE_PI_KEY", "sk-secret")]),
        false,
    )
    .unwrap();

    agents.run(request("p", None)).await;

    let seen = keyed.seen.lock().unwrap();
    assert_eq!(seen[0].api_key, None);
    assert_eq!(seen[0].state_dir, None);
}

/// What [`TwoSpawns`] was given: the continue argv, window and timeout.
type GivenLimits = (Option<Vec<String>>, Duration, Duration);

/// Spawns twice, reporting a 1.2 s rate-limit wait in between.
struct TwoSpawns(Mutex<Vec<GivenLimits>>);

#[async_trait]
impl AgentHandler for TwoSpawns {
    fn mechanism(&self) -> &'static str {
        "two"
    }

    async fn run(&self, inv: Invocation<'_>) -> AgentResult {
        self.0.lock().unwrap().push((
            inv.continue_argv.clone(),
            inv.rate_limit_window,
            inv.timeout,
        ));
        let spawn = |pid| Spawned {
            pid,
            pgid: pid,
            host: None,
        };
        (inv.spawned)(spawn(11));
        (inv.rate_limited)(Duration::from_millis(1200));
        (inv.spawned)(spawn(12));
        AgentResult::failed(inv.invocation_id, FailureClass::Reported, "")
    }

    fn transcript_usage(&self, _transcript: &str) -> Usage {
        Usage::default()
    }
}

#[tokio::test]
async fn each_spawn_is_started_with_its_own_files_and_a_wait_is_reported() {
    let handler = Arc::new(TwoSpawns(Mutex::new(Vec::new())));
    let mut p = profile("p", "two");
    p.session_args = vec!["--session-id".into(), "{session_id}".into()];
    p.resume = Some(Resume::Args(vec!["--resume".into(), "{session_id}".into()]));
    p.rate_limit_window = Duration::from_secs(120);
    let agents = Agents::new(vec![handler.clone()], vec![p], BTreeMap::new(), false).unwrap();
    let counter = Counter::default();

    agents.run(request("p", Some(&counter))).await;

    let started = counter.started.lock().unwrap();
    let files: Vec<_> = started
        .iter()
        .map(|s| (s.spawn, s.pid, s.transcript.clone(), s.stderr.clone()))
        .collect();
    assert_eq!(
        files,
        [
            (
                1,
                11,
                Some(PathBuf::from("/run/transcripts/inv-1.jsonl")),
                Some(PathBuf::from("/run/transcripts/inv-1.stderr.log"))
            ),
            (
                2,
                12,
                Some(PathBuf::from("/run/transcripts/inv-1.2.jsonl")),
                Some(PathBuf::from("/run/transcripts/inv-1.2.stderr.log"))
            ),
        ]
    );
    assert_eq!(
        *counter.rate_limited.lock().unwrap(),
        [RateLimited {
            invocation_id: "inv-1".into(),
            spawn: 1,
            wait_s: 2,
        }]
    );
    let (continue_argv, window, _) = handler.0.lock().unwrap()[0].clone();
    assert_eq!(
        continue_argv.unwrap(),
        ["agent", "--quiet", "--resume", "sess-1", "--extra", "--model", "m1"]
    );
    assert_eq!(window, Duration::from_secs(120));
}

#[tokio::test]
async fn a_profile_that_cant_resume_gets_no_continue_argv() {
    let handler = Arc::new(TwoSpawns(Mutex::new(Vec::new())));
    let agents = Agents::new(
        vec![handler.clone()],
        vec![profile("p", "two")],
        BTreeMap::new(),
        false,
    )
    .unwrap();
    agents.run(request("p", None)).await;
    assert_eq!(handler.0.lock().unwrap()[0].0, None);
}

#[test]
fn spawn_paths_insert_the_spawn_number_after_the_invocation_id() {
    use attractor_agent_handler::spawn_path;
    use std::path::Path;
    let jsonl = Path::new("/r/transcripts/0192-ab.jsonl");
    let stderr = Path::new("/r/transcripts/0192-ab.stderr.log");
    assert_eq!(spawn_path(jsonl, 1), jsonl);
    assert_eq!(
        spawn_path(jsonl, 2),
        Path::new("/r/transcripts/0192-ab.2.jsonl")
    );
    assert_eq!(
        spawn_path(stderr, 3),
        Path::new("/r/transcripts/0192-ab.3.stderr.log")
    );
}

#[tokio::test]
async fn the_hard_deadline_leaves_room_for_the_rate_limit_window() {
    let mut p = profile("p", "stuck");
    p.rate_limit_window = Duration::from_millis(200);
    let agents = Agents::new(vec![Arc::new(Stuck)], vec![p], BTreeMap::new(), false)
        .unwrap()
        .with_hard_deadline_margin(Duration::from_millis(50));
    let mut req = request("p", None);
    req.timeout = Some(Duration::from_millis(100));
    let result = agents.run(req).await;
    // timeout 100 + window 200 + kill_grace 50 + margin 50.
    assert_eq!(result.detail, "handler did not return within 400ms");
}
