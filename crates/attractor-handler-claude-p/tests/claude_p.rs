#![cfg(unix)]
//! Seam 1: `Agents::run` with the `claude-p` handler and a `claude` profile
//! whose command is the shell fake (`tests/agents/fake-claude`). One test
//! per fake scenario, plus the environment, stdin, argv and transcript.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use attractor_agent_handler::{
    builtin_profiles, AgentObserver, AgentRequest, AgentResult, AgentStatus, Agents,
    CancellationToken, FailureClass, Profile, Record, Selection, Started,
};
use attractor_handler_claude_p::{claude_result_line, ClaudeP};

fn fake_claude() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/agents/fake-claude")
}

/// A temp dir holding the fake's scenario folder and a workdir.
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

    fn transcript(&self) -> PathBuf {
        self.dir.path().join("transcript.jsonl")
    }

    fn stderr(&self) -> PathBuf {
        self.dir.path().join("inv-42.stderr.log")
    }

    /// The scenario line the fake reads for node `work`.
    fn scenario(&self, line: &str) {
        fs::write(self.scenarios().join("work"), format!("{line}\n")).unwrap();
    }

    fn workdir(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    /// Runs git in the workdir with the same hermetic settings the fake gets.
    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.workdir())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CEILING_DIRECTORIES", self.dir.path())
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.scenarios().join(name)).unwrap()
    }

    /// The parent env: the test's PATH (the fake needs sh tools), the
    /// fake's scenario folder, and the extra pairs.
    fn parent_env(&self, extra: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), std::env::var("PATH").unwrap());
        env.insert(
            "FAKE_AGENT_SCENARIOS".to_string(),
            self.scenarios().to_string_lossy().into_owned(),
        );
        env.insert("FAKE_HANG_SECS".to_string(), "30".to_string());
        // Git, for edit_commit: ignore the developer's config and any repo
        // above the temp dir.
        env.insert("GIT_CONFIG_GLOBAL".to_string(), "/dev/null".to_string());
        env.insert("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string());
        env.insert(
            "GIT_CEILING_DIRECTORIES".to_string(),
            self.dir.path().to_string_lossy().into_owned(),
        );
        for (k, v) in extra {
            env.insert(k.to_string(), v.to_string());
        }
        env
    }

    fn agents(&self, command: PathBuf, extra_env: &[(&str, &str)]) -> Agents {
        self.agents_with_grace(command, extra_env, Duration::from_secs(10))
    }

    fn agents_with_grace(
        &self,
        command: PathBuf,
        extra_env: &[(&str, &str)],
        kill_grace: Duration,
    ) -> Agents {
        let profile = Profile {
            command: vec![command.to_string_lossy().into_owned()],
            kill_grace,
            ..builtin_profiles().remove(0)
        };
        Agents::new(
            vec![Arc::new(ClaudeP)],
            vec![profile],
            self.parent_env(extra_env),
        )
        .unwrap()
    }

    fn request(&self, timeout: Duration) -> AgentRequest<'static> {
        AgentRequest {
            selection: Selection {
                profile: "claude".into(),
                model: Some("x".into()),
            },
            prompt: "do the work".into(),
            extra_args: vec![
                "--allowedTools".into(),
                "Read".into(),
                "--max-budget-usd".into(),
                "1".into(),
            ],
            workdir: self.dir.path().join("work"),
            timeout,
            record: Record {
                run_id: Some("run-7".into()),
                node_id: "work".into(),
                attempt: 2,
                invocation_id: "inv-42".into(),
            },
            transcript: Some(self.transcript()),
            stderr: Some(self.stderr()),
            observer: None,
            cancel: CancellationToken::new(),
        }
    }

    /// The fake's pid from its last `env.log` start block.
    fn fake_pid(&self) -> u32 {
        self.read("env.log")
            .lines()
            .filter_map(|l| l.strip_prefix("FAKE_PID="))
            .last()
            .unwrap()
            .parse()
            .unwrap()
    }

    async fn run(&self, scenario: &str) -> AgentResult {
        self.scenario(scenario);
        self.agents(fake_claude(), &[])
            .run(self.request(Duration::from_secs(20)))
            .await
    }
}

#[tokio::test]
async fn success_completes_with_the_result_text() {
    let fx = Fixture::new();
    let result = fx.run("scenario=success").await;
    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    assert_eq!(result.text, "fake-claude: success");
    assert_eq!(result.invocation_id, "inv-42");
    assert_eq!(result.exit.and_then(|e| e.code), Some(0));
}

#[tokio::test]
async fn fail_is_reported() {
    let fx = Fixture::new();
    let result = fx.run("scenario=fail").await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Reported));
}

#[tokio::test]
async fn error_max_turns_is_reported() {
    let fx = Fixture::new();
    let result = fx.run("scenario=error_max_turns").await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Reported));
}

#[tokio::test]
async fn crash_is_a_crash_with_exit_three_and_stderr() {
    let fx = Fixture::new();
    let result = fx.run("scenario=crash").await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Crash));
    assert_eq!(result.exit.and_then(|e| e.code), Some(3));
    assert_eq!(
        result.detail,
        "exited with exit status: 3: fake-claude: crashed"
    );
}

#[tokio::test]
async fn crash_after_result_completes() {
    let fx = Fixture::new();
    let result = fx.run("scenario=crash_after_result").await;
    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    assert_eq!(result.exit.and_then(|e| e.code), Some(3));
}

#[tokio::test]
async fn garbage_is_no_result_with_the_parse_message() {
    let fx = Fixture::new();
    let result = fx.run("scenario=garbage").await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::NoResult));
    assert!(
        result.detail.contains("Failed to parse Claude output"),
        "{}",
        result.detail
    );
}

#[tokio::test]
async fn silent_is_no_result() {
    let fx = Fixture::new();
    let result = fx.run("scenario=silent").await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::NoResult));
    assert!(
        result.detail.contains("Claude Code produced no output"),
        "{}",
        result.detail
    );
}

#[tokio::test]
async fn a_result_line_that_does_not_deserialize_is_no_result_whatever_the_exit() {
    for exit in [0, 1] {
        let fx = Fixture::new();
        let result = fx.run(&format!("scenario=bad_result exit={exit}")).await;
        assert_eq!(
            result.status,
            AgentStatus::Failed(FailureClass::NoResult),
            "exit {exit}"
        );
        assert_eq!(result.exit.and_then(|e| e.code), Some(exit));
        assert!(
            result.detail.contains("Failed to parse Claude output"),
            "{}",
            result.detail
        );
    }
}

#[tokio::test]
async fn hang_past_the_timeout_is_a_timeout() {
    let fx = Fixture::new();
    fx.scenario("scenario=hang");
    let result = fx
        .agents(fake_claude(), &[])
        .run(fx.request(Duration::from_secs(1)))
        .await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Timeout));
    assert_eq!(result.detail, "timed out after 1000ms");
}

#[tokio::test]
async fn a_missing_command_is_a_launch_failure_and_leaves_no_transcript() {
    let fx = Fixture::new();
    let result = fx
        .agents(fx.dir.path().join("no-such-claude"), &[])
        .run(fx.request(Duration::from_secs(5)))
        .await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Launch));
    assert_eq!(result.launch_error, Some(std::io::ErrorKind::NotFound));
    assert!(!fx.transcript().exists());
    assert!(!fx.stderr().exists());
}

#[tokio::test]
async fn the_transcript_holds_the_fakes_stdout() {
    let fx = Fixture::new();
    let result = fx.run("scenario=success").await;
    assert_eq!(result.status, AgentStatus::Completed);
    let transcript = fs::read_to_string(fx.transcript()).unwrap();
    let lines: Vec<&str> = transcript.lines().collect();
    assert_eq!(lines.len(), 3, "{transcript}");
    for line in &lines {
        serde_json::from_str::<serde_json::Value>(line).unwrap();
    }
    assert!(lines[0].contains(r#""subtype":"init""#), "{transcript}");
    assert_eq!(claude_result_line(&transcript), Some(lines[2]));
    assert!(lines[2].contains("fake-claude: success"), "{transcript}");
}

#[tokio::test]
async fn the_child_env_has_the_pas_ids_and_no_api_key() {
    let fx = Fixture::new();
    fx.scenario("scenario=success");
    let result = fx
        .agents(fake_claude(), &[("ANTHROPIC_API_KEY", "sk-must-not-leak")])
        .run(fx.request(Duration::from_secs(20)))
        .await;
    assert_eq!(result.status, AgentStatus::Completed);
    let env_log = fx.read("env.log");
    for line in [
        "PAS_ATTEMPT=2",
        "PAS_INVOCATION_ID=inv-42",
        "PAS_NODE_ID=work",
        "PAS_RUN_ID=run-7",
    ] {
        assert!(
            env_log.lines().any(|l| l == line),
            "{line} missing: {env_log}"
        );
    }
    assert!(!env_log.contains("ANTHROPIC_API_KEY"), "{env_log}");
}

// When the test process's own stdin is already /dev/null (CI, a non-tty
// `cargo test`) this passes even if the runner inherited stdin. The real
// guard is fake_agent_harness.rs
// `the_agent_reads_dev_null_even_when_pas_stdin_is_an_open_pipe`.
#[tokio::test]
async fn stdin_is_dev_null() {
    let fx = Fixture::new();
    let result = fx.run("scenario=stdin").await;
    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    assert_eq!(fx.read("stdin.bytes").trim(), "0");
}

#[tokio::test]
async fn argv_is_profile_args_extra_args_model_then_handler_flags() {
    let fx = Fixture::new();
    let result = fx.run("scenario=success").await;
    assert_eq!(result.status, AgentStatus::Completed);
    let log = fx.read("invocations.log");
    let args: Vec<&str> = log.lines().skip(1).collect();
    assert_eq!(
        args,
        [
            "--no-session-persistence",
            "--dangerously-skip-permissions",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--allowedTools",
            "Read",
            "--max-budget-usd",
            "1",
            "--model",
            "x",
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
        ]
    );
}

#[tokio::test]
async fn label_completes_with_the_label_on_the_last_line() {
    let fx = Fixture::new();
    let result = fx.run("scenario=label label=beta_route").await;
    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    assert_eq!(result.text, "fake-claude: routed\nbeta_route");
}

#[tokio::test]
async fn slow_completes_with_every_line_in_the_transcript() {
    let fx = Fixture::new();
    let result = fx.run("scenario=slow").await;
    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    assert_eq!(result.text, "fake-claude: slow done");
    let transcript = fs::read_to_string(fx.transcript()).unwrap();
    let lines: Vec<&str> = transcript.lines().collect();
    assert_eq!(
        lines.len(),
        5,
        "init, 3 assistant lines, result: {transcript}"
    );
    assert!(lines[0].contains(r#""subtype":"init""#), "{transcript}");
    for line in &lines[1..4] {
        assert!(line.contains(r#""type":"assistant""#), "{transcript}");
    }
    assert_eq!(claude_result_line(&transcript), Some(lines[4]));
}

#[tokio::test]
async fn edit_commit_completes_and_commits_in_the_workdir() {
    let fx = Fixture::new();
    fx.git(&["init", "-q"]);
    fx.git(&[
        "-c",
        "user.name=test",
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "base",
    ]);
    let result = fx.run("scenario=edit_commit").await;
    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    assert!(fx.workdir().join("fake-edit.txt").exists());
    assert_eq!(fx.git(&["log", "-1", "--format=%s"]), "fake-claude edit");
}

#[tokio::test]
async fn flaky_times_out_on_attempt_one_then_completes_on_attempt_two() {
    let fx = Fixture::new();
    fx.scenario("scenario=flaky fails=1");
    let agents = fx.agents(fake_claude(), &[]);

    let first = agents.run(fx.request(Duration::from_secs(1))).await;
    assert_eq!(first.status, AgentStatus::Failed(FailureClass::Timeout));

    let mut retry = fx.request(Duration::from_secs(1));
    retry.record.attempt = 3;
    retry.record.invocation_id = "inv-43".into();
    retry.transcript = Some(fx.dir.path().join("transcript-2.jsonl"));
    let second = agents.run(retry).await;
    assert_eq!(second.status, AgentStatus::Completed, "{}", second.detail);
    assert_eq!(second.text, "fake-claude: success");

    let env_log = fx.read("env.log");
    let attempts: Vec<&str> = env_log
        .lines()
        .filter_map(|l| l.strip_prefix("PAS_ATTEMPT="))
        .collect();
    assert_eq!(attempts, ["2", "3"]);
    assert_eq!(fx.read("attempts.flaky").trim(), "2");
}

/// Records `started` (and how long the transcript was at that moment) and
/// `finished`, in order.
#[derive(Default)]
struct Recording {
    transcript: PathBuf,
    events: Mutex<Vec<String>>,
    started: Mutex<Vec<(Started, u64)>>,
}

impl AgentObserver for Recording {
    fn started(&self, started: &Started) {
        let len = fs::metadata(&self.transcript).map(|m| m.len()).unwrap();
        self.started.lock().unwrap().push((started.clone(), len));
        self.events.lock().unwrap().push("started".into());
    }

    fn finished(&self, _result: &AgentResult) {
        self.events.lock().unwrap().push("finished".into());
    }
}

/// Polls `check` until it holds, failing after 10 s.
async fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Whether process `pid` no longer exists.
fn gone(pid: u32) -> bool {
    // SAFETY: kill with signal 0 only checks that the process exists.
    let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
    !alive
}

#[tokio::test]
async fn started_is_reported_once_before_any_output_and_before_finished() {
    let fx = Fixture::new();
    fx.scenario("scenario=success");
    let recording = Recording {
        transcript: fx.transcript(),
        ..Recording::default()
    };
    let mut req = fx.request(Duration::from_secs(20));
    req.observer = Some(&recording);

    let result = fx.agents(fake_claude(), &[]).run(req).await;

    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    assert_eq!(*recording.events.lock().unwrap(), ["started", "finished"]);
    let started = recording.started.lock().unwrap();
    let (started, transcript_len) = &started[0];
    // The fake prints its init line at once; none of it had been written.
    assert_eq!(*transcript_len, 0);
    assert_eq!(started.invocation_id, "inv-42");
    assert_eq!(started.spawn, 1);
    assert_eq!(started.node_id, "work");
    assert_eq!(started.attempt, 2);
    assert_eq!(started.profile, "claude");
    assert_eq!(started.model.as_deref(), Some("x"));
    assert_eq!(started.pid, fx.fake_pid());
    assert_eq!(started.pgid, started.pid);
    assert_eq!(started.transcript, Some(fx.transcript()));
    assert_eq!(started.stderr, Some(fx.stderr()));
}

#[tokio::test]
async fn stderr_is_written_live_and_kept() {
    let fx = Fixture::new();
    fx.scenario("scenario=stderr_live");
    let agents = fx.agents(fake_claude(), &[]);
    let go = fx.scenarios().join("go");
    let stderr = fx.stderr();

    let (result, ()) = tokio::join!(agents.run(fx.request(Duration::from_secs(20))), async {
        wait_until("the first stderr line", || {
            fs::read_to_string(&stderr).is_ok_and(|s| s.contains("fake-claude: working"))
        })
        .await;
        // The agent is still waiting for `go`, so this line arrived live.
        assert!(!fs::read_to_string(&stderr)
            .unwrap()
            .contains("fake-claude: done"));
        fs::write(&go, "").unwrap();
    });

    assert_eq!(result.status, AgentStatus::Completed, "{}", result.detail);
    let text = fs::read_to_string(fx.stderr()).unwrap();
    assert!(text.contains("fake-claude: working"), "{text}");
    assert!(text.contains("fake-claude: done"), "{text}");
}

#[tokio::test]
async fn a_crash_leaves_its_stderr_in_the_file() {
    let fx = Fixture::new();
    let result = fx.run("scenario=crash").await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Crash));
    let text = fs::read_to_string(fx.stderr()).unwrap();
    assert!(text.contains("fake-claude: crashed"), "{text}");
}

#[tokio::test]
async fn a_timeout_sends_term_first_and_the_agent_exits_on_it() {
    let fx = Fixture::new();
    fx.scenario("scenario=hang_term");
    let result = fx
        .agents(fake_claude(), &[])
        .run(fx.request(Duration::from_secs(1)))
        .await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Timeout));
    assert_eq!(fx.read("term").trim(), "term");
}

#[tokio::test]
async fn an_agent_that_ignores_term_is_killed_after_the_grace() {
    let fx = Fixture::new();
    fx.scenario("scenario=hang_ignore_term");
    let result = fx
        .agents_with_grace(fake_claude(), &[], Duration::from_millis(300))
        .run(fx.request(Duration::from_secs(1)))
        .await;
    assert_eq!(result.status, AgentStatus::Failed(FailureClass::Timeout));
    assert!(fx.read("term").contains("term"));
    // The runner reaped it after KILL.
    assert!(gone(fx.fake_pid()));
}

#[tokio::test]
async fn a_cancel_sends_term_and_returns_cancelled() {
    let fx = Fixture::new();
    fx.scenario("scenario=hang_term");
    let agents = fx.agents(fake_claude(), &[]);
    let mut req = fx.request(Duration::from_secs(20));
    let cancel = CancellationToken::new();
    req.cancel = cancel.clone();
    let transcript = fx.transcript();

    let (result, ()) = tokio::join!(agents.run(req), async {
        // The init line is printed after the trap is set.
        wait_until("the init line", || {
            fs::read_to_string(&transcript).is_ok_and(|s| s.contains("init"))
        })
        .await;
        cancel.cancel();
    });

    assert_eq!(result.status, AgentStatus::Cancelled);
    assert_eq!(fx.read("term").trim(), "term");
}
