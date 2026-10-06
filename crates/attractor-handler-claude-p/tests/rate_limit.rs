#![cfg(unix)]
//! Seam 1: a rate-limited Claude attempt waits and re-spawns within the
//! profile's `rate_limit_window` (design §5), through `Agents::run` with the
//! `claude-p` handler, the shell fake, and a clock that never really waits.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;

use attractor_agent_handler::{
    builtin_profiles, AgentObserver, AgentRequest, AgentResult, AgentStatus, Agents,
    CancellationToken, FailureClass, Profile, RateLimited, Record, Selection, Session, Started,
};
use attractor_handler_claude_p::{ClaudeP, Clock};

fn fake_claude() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/agents/fake-claude")
}

/// A clock that starts at the real time (the fake's `resetsAt` is the real
/// current second) and moves forward only when something sleeps on it; its
/// sleep returns at once, or never (`blocking`), so a cancel must end it.
struct FakeClock {
    now: Mutex<SystemTime>,
    blocking: bool,
}

impl FakeClock {
    fn new(blocking: bool) -> Arc<Self> {
        Arc::new(Self {
            now: Mutex::new(SystemTime::now()),
            blocking,
        })
    }
}

#[async_trait]
impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        *self.now.lock().unwrap()
    }

    async fn sleep(&self, duration: Duration) {
        if self.blocking {
            std::future::pending::<()>().await;
        }
        *self.now.lock().unwrap() += duration;
    }
}

/// Records starts and rate-limit waits; cancels `cancel` on the first wait
/// when it has one.
#[derive(Default)]
struct Seen {
    started: Mutex<Vec<Started>>,
    waits: Mutex<Vec<RateLimited>>,
    cancel: Option<CancellationToken>,
}

impl AgentObserver for Seen {
    fn started(&self, started: &Started) {
        self.started.lock().unwrap().push(started.clone());
    }

    fn finished(&self, _result: &AgentResult) {}

    fn rate_limited(&self, rate_limited: &RateLimited) {
        self.waits.lock().unwrap().push(rate_limited.clone());
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
    }
}

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(scenario: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("scenarios")).unwrap();
        fs::create_dir(dir.path().join("work")).unwrap();
        fs::create_dir(dir.path().join("transcripts")).unwrap();
        let fx = Self { dir };
        fs::write(fx.scenarios().join("work"), format!("{scenario}\n")).unwrap();
        fx
    }

    fn scenarios(&self) -> PathBuf {
        self.dir.path().join("scenarios")
    }

    fn transcripts(&self) -> PathBuf {
        self.dir.path().join("transcripts")
    }

    fn agents(&self, profile: Profile, clock: Arc<FakeClock>) -> Agents {
        let env = BTreeMap::from([
            ("PATH".to_string(), std::env::var("PATH").unwrap()),
            (
                "FAKE_AGENT_SCENARIOS".to_string(),
                self.scenarios().to_string_lossy().into_owned(),
            ),
        ]);
        Agents::new(
            vec![Arc::new(ClaudeP::with_clock(clock))],
            vec![profile],
            env,
            false,
        )
        .unwrap()
    }

    fn request<'a>(&self, observer: &'a Seen) -> AgentRequest<'a> {
        AgentRequest {
            selection: Selection {
                profile: "claude".into(),
                model: None,
                reasoning: None,
            },
            prompt: "do the work".into(),
            extra_args: vec![],
            workdir: self.dir.path().join("work"),
            timeout: Some(Duration::from_secs(20)),
            record: Record {
                run_id: Some("run-1".into()),
                node_id: "work".into(),
                attempt: 1,
                invocation_id: "inv-1".into(),
            },
            transcript: Some(self.transcripts().join("inv-1.jsonl")),
            stderr: Some(self.transcripts().join("inv-1.stderr.log")),
            prompt_file: None,
            session: Session::New("sess-1".into()),
            state_dir: None,
            observer: Some(observer),
            cancel: observer.cancel.clone().unwrap_or_default(),
        }
    }

    /// The fake's argv per start.
    fn invocations(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.scenarios().join("invocations.log"))
            .unwrap_or_default()
            .split("--- start\n")
            .skip(1)
            .map(|block| block.lines().map(String::from).collect())
            .collect()
    }
}

/// The built-in `claude` profile run by the fake, with `window`.
fn profile(window: Duration) -> Profile {
    Profile {
        command: vec![fake_claude().to_string_lossy().into_owned()],
        rate_limit_window: window,
        ..builtin_profiles()
            .unwrap()
            .into_iter()
            .find(|p| p.name == "claude")
            .unwrap()
    }
}

/// The value after `flag` in `argv`.
fn flag(argv: &[String], flag: &str) -> Option<String> {
    let at = argv.iter().position(|a| a == flag)?;
    argv.get(at + 1).cloned()
}

const WINDOW: Duration = Duration::from_secs(120);

#[tokio::test]
async fn rate_limited_twice_then_success_resumes_the_session_in_three_spawns() {
    let fx = Fixture::new("scenario=rate_limited times=2");
    let seen = Seen::default();

    let result = fx
        .agents(profile(WINDOW), FakeClock::new(false))
        .run(fx.request(&seen))
        .await;

    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    assert_eq!(result.text, "fake-claude: success");
    let started = seen.started.lock().unwrap();
    assert_eq!(
        started.iter().map(|s| s.spawn).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    let mut pids: Vec<u32> = started.iter().map(|s| s.pid).collect();
    pids.dedup();
    assert_eq!(pids.len(), 3, "{pids:?}");
    for (s, name) in started.iter().zip(["inv-1", "inv-1.2", "inv-1.3"]) {
        let transcript = fx.transcripts().join(format!("{name}.jsonl"));
        let stderr = fx.transcripts().join(format!("{name}.stderr.log"));
        assert_eq!(s.transcript.as_deref(), Some(transcript.as_path()));
        assert_eq!(s.stderr.as_deref(), Some(stderr.as_path()));
        assert!(transcript.is_file() && stderr.is_file(), "{name}");
    }
    let waits = seen.waits.lock().unwrap();
    assert_eq!(
        waits
            .iter()
            .map(|w| (w.spawn, w.wait_s))
            .collect::<Vec<_>>(),
        [(1, 1), (2, 1)]
    );
    let argvs = fx.invocations();
    assert_eq!(flag(&argvs[0], "--session-id").as_deref(), Some("sess-1"));
    for argv in &argvs[1..] {
        assert_eq!(
            flag(argv, "--resume").as_deref(),
            Some("sess-1"),
            "{argv:?}"
        );
        assert_eq!(flag(argv, "--session-id"), None, "{argv:?}");
    }
    // Usage adds up over the spawns (the fake costs 0.001 each).
    assert_eq!(result.usage.cost_usd, Some(0.003));
}

#[tokio::test]
async fn without_an_init_line_the_next_spawn_starts_the_same_session_again() {
    let fx = Fixture::new("scenario=rate_limited times=1 no_init=1");
    let seen = Seen::default();

    let result = fx
        .agents(profile(WINDOW), FakeClock::new(false))
        .run(fx.request(&seen))
        .await;

    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    let argvs = fx.invocations();
    assert_eq!(argvs.len(), 2);
    assert_eq!(flag(&argvs[1], "--session-id").as_deref(), Some("sess-1"));
    assert_eq!(flag(&argvs[1], "--resume"), None);
}

#[tokio::test]
async fn always_rate_limited_fails_reported_once_the_window_is_used() {
    let fx = Fixture::new("scenario=always_rate_limited");
    let seen = Seen::default();
    let window = Duration::from_secs(3);

    let result = fx
        .agents(profile(window), FakeClock::new(false))
        .run(fx.request(&seen))
        .await;

    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Reported));
    assert!(
        result.detail.starts_with("rate limited: "),
        "{}",
        result.detail
    );
    assert!(result.detail.contains("API Error: Rate limit reached"));
    // 1 s waits (the reset is past on the fake clock) until the window is
    // used: 3 waits, 4 spawns, never more than the window.
    let waits = seen.waits.lock().unwrap();
    assert_eq!(waits.len(), 3, "{waits:?}");
    assert!(
        result.detail.ends_with("waited 3 s over 4 spawns"),
        "{}",
        result.detail
    );
}

#[tokio::test]
async fn a_zero_window_fails_at_once() {
    let fx = Fixture::new("scenario=always_rate_limited");
    let seen = Seen::default();

    let result = fx
        .agents(profile(Duration::ZERO), FakeClock::new(false))
        .run(fx.request(&seen))
        .await;

    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Reported));
    assert_eq!(
        result.detail,
        "rate limited: API Error: Rate limit reached; waited 0 s over 1 spawn"
    );
    assert_eq!(fx.invocations().len(), 1);
}

#[tokio::test]
async fn a_profile_that_cant_resume_fails_at_once() {
    let fx = Fixture::new("scenario=rate_limited times=1");
    let seen = Seen::default();
    let mut p = profile(WINDOW);
    p.resume = None;

    let result = fx
        .agents(p, FakeClock::new(false))
        .run(fx.request(&seen))
        .await;

    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Reported));
    assert!(
        result.detail.starts_with("rate limited: "),
        "{}",
        result.detail
    );
    assert_eq!(fx.invocations().len(), 1);
    assert!(seen.waits.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_cancel_during_a_wait_ends_it_cancelled() {
    let fx = Fixture::new("scenario=always_rate_limited");
    let seen = Seen {
        cancel: Some(CancellationToken::new()),
        ..Seen::default()
    };

    let result = fx
        .agents(profile(WINDOW), FakeClock::new(true))
        .run(fx.request(&seen))
        .await;

    assert_eq!(result.status, AgentStatus::Cancelled);
    assert_eq!(fx.invocations().len(), 1);
}
