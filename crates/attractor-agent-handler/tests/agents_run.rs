//! Seam 1 for the registry itself: `Agents::new` and `Agents::run` with a
//! recording fake handler.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use attractor_agent_handler::{
    AgentHandler, AgentObserver, AgentRequest, AgentResult, AgentStatus, Agents, ConfigError,
    FailureClass, Invocation, Profile, Record, Selection, Usage,
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
        AgentResult {
            invocation_id: inv.invocation_id.to_string(),
            status: AgentStatus::Completed,
            text: "done".into(),
            detail: String::new(),
            usage: Usage::default(),
            exit: None,
            duration: Duration::ZERO,
            launch_error: None,
        }
    }
}

#[derive(Default)]
struct Counter(Mutex<Vec<AgentResult>>);

impl AgentObserver for Counter {
    fn finished(&self, result: &AgentResult) {
        self.0.lock().unwrap().push(result.clone());
    }
}

fn profile(name: &str, mechanism: &str) -> Profile {
    Profile {
        name: name.into(),
        mechanism: mechanism.into(),
        command: vec!["agent".into()],
        args: vec!["--quiet".into()],
        model_args: vec!["--model".into(), "{model}".into()],
    }
}

fn request<'a>(profile: &str, observer: Option<&'a dyn AgentObserver>) -> AgentRequest<'a> {
    AgentRequest {
        selection: Selection {
            profile: profile.into(),
            model: Some("m1".into()),
        },
        prompt: "do it".into(),
        extra_args: vec!["--extra".into()],
        workdir: PathBuf::from("/work"),
        timeout: Duration::from_secs(5),
        record: Record {
            run_id: Some("run-1".into()),
            node_id: "work".into(),
            attempt: 1,
            invocation_id: "inv-1".into(),
        },
        transcript: None,
        observer,
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
    let agents = Agents::new(vec![Recorder::new("fake")], vec![], BTreeMap::new()).unwrap();

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
    )
    .unwrap();
    let counter = Counter::default();

    let ok = agents.run(request("p", Some(&counter))).await;
    let failed = agents.run(request("nope", Some(&counter))).await;

    assert_eq!(*counter.0.lock().unwrap(), vec![ok, failed]);
}

#[test]
fn new_refuses_a_duplicate_mechanism() {
    let err = Agents::new(
        vec![Recorder::new("fake"), Recorder::new("fake")],
        vec![],
        BTreeMap::new(),
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
