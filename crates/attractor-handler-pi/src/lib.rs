//! The `pi` agent handler: runs the Pi coding agent (`pi --mode json`)
//! against an OpenAI-compatible provider (DeepSeek, Z.ai's GLM, a hosted
//! Qwen), as a local process. The engine writes Pi's `models.json` (with
//! the API key) and `settings.json` into a per-invocation
//! `PI_CODING_AGENT_DIR` in the run folder and removes it when the
//! invocation ends; the key is never on argv or in Pi's environment.
//! Sessions live in `<run folder>/pi-sessions`: `--session-id` starts a
//! session or continues it, so every invocation runs the same command.

mod agent_dir;
mod parse;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use async_trait::async_trait;

use attractor_agent_handler::{
    AgentHandler, AgentResult, AgentStatus, ConfigError, FailureClass, Invocation, Limits, Profile,
    Spawned, Usage,
};
use attractor_agent_process::{run_local, LocalRun, Spawn};

pub use agent_dir::{models_json, settings_json, AgentDir, Target};
pub use parse::{classify, last_assistant, summarize, Assistant, Exited};

/// The handler for profiles with mechanism `pi`.
#[derive(Debug, Default, Clone, Copy)]
pub struct Pi;

impl Pi {
    pub const MECHANISM: &'static str = "pi";
    pub const DISPLAY_NAME: &'static str = "Pi";
}

/// Where Pi keeps its sessions: `<run folder>/pi-sessions`.
pub fn sessions_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("pi-sessions")
}

/// Where an invocation's `PI_CODING_AGENT_DIR`s are made:
/// `<run folder>/pi-agent`.
pub fn agent_dirs(state_dir: &Path) -> PathBuf {
    state_dir.join("pi-agent")
}

/// A `pi` profile names its provider and model, takes the model through
/// `--model <provider>/<model>` rather than `model_args`, and has positive
/// limits.
pub fn check_profile(profile: &Profile) -> Result<(), ConfigError> {
    let required = |field: &'static str, value: &Option<String>| match value.as_deref() {
        Some(value) if !value.trim().is_empty() => Ok(()),
        _ => Err(ConfigError::HandlerField {
            profile: profile.name.clone(),
            field,
            problem: "is required for mechanism pi".into(),
        }),
    };
    required("provider", &profile.provider)?;
    required("model", &profile.model)?;
    if !profile.model_args.is_empty() {
        return Err(ConfigError::FieldNotForMechanism {
            profile: profile.name.clone(),
            field: "model_args",
            mechanism: Pi::MECHANISM.into(),
        });
    }
    match profile.limits {
        Some(Limits {
            context,
            max_output,
        }) if context == 0 || max_output == 0 => Err(ConfigError::HandlerField {
            profile: profile.name.clone(),
            field: "limits",
            problem: "must be positive".into(),
        }),
        _ => Ok(()),
    }
}

/// The provider and model an invocation uses, when both are known.
fn target<'a>(inv: &'a Invocation<'_>) -> Option<Target<'a>> {
    Some(Target {
        provider: inv.profile.provider.as_deref()?,
        model: inv.model?,
    })
}

/// The child environment: the invocation's, with Pi pointed at `agent_dir`
/// and kept off the network for anything but the model.
fn pi_env(env: &BTreeMap<String, String>, agent_dir: &Path) -> BTreeMap<String, String> {
    let mut env = env.clone();
    env.insert(
        "PI_CODING_AGENT_DIR".into(),
        agent_dir.to_string_lossy().into_owned(),
    );
    env.insert("PI_TELEMETRY".into(), "0".into());
    env.insert("PI_OFFLINE".into(), "1".into());
    env.insert("PI_SKIP_VERSION_CHECK".into(), "1".into());
    env
}

#[async_trait]
impl AgentHandler for Pi {
    fn mechanism(&self) -> &'static str {
        Self::MECHANISM
    }

    fn check(&self, profile: &Profile) -> Result<(), ConfigError> {
        check_profile(profile)
    }

    /// `--session-id` starts the session or continues it.
    fn resumes_natively(&self) -> bool {
        true
    }

    /// The invocation's argv, then `--model <provider>/<model>`,
    /// `--session-dir`, `--session-id` and the prompt, last. Never the key.
    fn argv(&self, inv: &Invocation<'_>) -> Vec<String> {
        let mut argv = inv.argv.clone();
        if let Some(target) = target(inv) {
            argv.extend([
                "--model".to_string(),
                format!("{}/{}", target.provider, target.model),
            ]);
        }
        if let Some(state_dir) = inv.state_dir {
            argv.extend([
                "--session-dir".to_string(),
                sessions_dir(state_dir).to_string_lossy().into_owned(),
            ]);
        }
        argv.extend([
            "--session-id".to_string(),
            inv.session.id().to_string(),
            inv.prompt.to_string(),
        ]);
        argv
    }

    fn display_name(&self) -> &'static str {
        Self::DISPLAY_NAME
    }

    fn reports_cost(&self) -> bool {
        true
    }

    /// Every result carries the session id passed: Pi keeps it, so a retry
    /// after a timeout continues even if Pi printed nothing.
    async fn run(&self, inv: Invocation<'_>) -> AgentResult {
        let started = Instant::now();
        let result = run_pi(self, &inv, started).await;
        AgentResult {
            duration: started.elapsed(),
            agent_session_id: Some(inv.session.id().to_string()),
            ..result
        }
    }

    fn transcript_usage(&self, transcript: &str) -> Usage {
        summarize(transcript)
    }
}

/// One Pi process with its agent dir, which is removed when this returns
/// or is dropped.
async fn run_pi(pi: &Pi, inv: &Invocation<'_>, started: Instant) -> AgentResult {
    let id = inv.invocation_id;
    let launch = |detail: String| AgentResult::failed(id, FailureClass::Launch, detail);
    let Some(state_dir) = inv.state_dir else {
        return launch("the pi handler needs a run folder".into());
    };
    let Some(target) = target(inv) else {
        return launch("the pi handler needs a provider and a model".into());
    };
    if let Err(error) = agent_dir::private_dir(true).create(sessions_dir(state_dir)) {
        return launch(format!("could not create pi's session folder: {error}"));
    }
    let agent_dir = match AgentDir::create(
        &agent_dirs(state_dir),
        id,
        &models_json(target, inv.profile, inv.api_key),
        &settings_json(),
    ) {
        Ok(dir) => dir,
        Err(error) => return launch(format!("could not write pi's agent folder: {error}")),
    };
    let spawned = |spawn: Spawn| {
        (inv.spawned)(Spawned {
            pid: spawn.pid,
            pgid: spawn.pgid,
            host: spawn.host,
        })
    };
    let local = run_local(
        &pi.argv(inv),
        &pi_env(&inv.env, agent_dir.path()),
        inv.workdir,
        inv.transcript,
        inv.stderr,
        inv.timeout,
        inv.kill_grace,
        &inv.cancel,
        &spawned,
    )
    .await;
    drop(agent_dir);
    match local {
        LocalRun::Exited(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            classify(
                id,
                &Exited {
                    status: out.status,
                    stdout: &stdout,
                    stderr: &stderr,
                },
                started.elapsed(),
            )
        }
        LocalRun::TimedOut => with_partial_usage(
            inv,
            AgentResult::failed(
                id,
                FailureClass::Timeout,
                format!("timed out after {}ms", inv.timeout.as_millis()),
            ),
        ),
        LocalRun::Cancelled => with_partial_usage(
            inv,
            AgentResult {
                status: AgentStatus::Cancelled,
                ..AgentResult::failed(id, FailureClass::Crash, "cancelled")
            },
        ),
        LocalRun::WaitFailed(error) => with_partial_usage(
            inv,
            AgentResult::failed(
                id,
                FailureClass::Crash,
                format!("execution failed: {error}"),
            ),
        ),
        LocalRun::LaunchFailed(error) => AgentResult {
            launch_error: Some(error.kind()),
            ..launch(error.to_string())
        },
    }
}

/// `result` with the usage read from the transcript written so far, for a
/// run that ended without its full output (timeout, cancel, failed wait).
fn with_partial_usage(inv: &Invocation<'_>, result: AgentResult) -> AgentResult {
    let partial = inv
        .transcript
        .and_then(|path| std::fs::read(path).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    AgentResult {
        usage: summarize(&partial),
        ..result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attractor_agent_handler::{CancellationToken, ProfileEnv, Session};
    use std::time::Duration;

    fn profile() -> Profile {
        Profile {
            name: "deepseek".into(),
            mechanism: "pi".into(),
            command: vec!["pi".into()],
            args: vec!["--mode".into(), "json".into()],
            model: Some("deepseek-v4-pro".into()),
            model_args: vec![],
            reasoning: None,
            reasoning_args: vec!["--thinking".into(), "{reasoning}".into()],
            timeout: Duration::from_secs(1),
            kill_grace: Duration::from_secs(1),
            rate_limit_window: Duration::ZERO,
            env: ProfileEnv::default(),
            test_only: false,
            session_args: vec![],
            resume: None,
            provider: Some("deepseek".into()),
            base_url: None,
            api_key_env: Some("DEEPSEEK_API_KEY".into()),
            limits: None,
        }
    }

    fn argv_for(session: Session) -> Vec<String> {
        let p = profile();
        Pi.argv(&Invocation {
            invocation_id: "inv-1",
            argv: vec![
                "pi".into(),
                "--mode".into(),
                "json".into(),
                "--thinking".into(),
                "high".into(),
            ],
            command_len: 1,
            env: BTreeMap::new(),
            api_key: Some("sk-secret"),
            state_dir: Some(Path::new("/run")),
            profile: &p,
            model: Some("deepseek-v4-flash"),
            session,
            continue_argv: None,
            rate_limit_window: Duration::ZERO,
            prompt: "do it",
            workdir: Path::new("/work"),
            timeout: Duration::from_secs(1),
            kill_grace: Duration::from_secs(1),
            transcript: None,
            stderr: None,
            cancel: CancellationToken::new(),
            spawned: &|_| {},
            rate_limited: &|_| {},
        })
    }

    #[test]
    fn new_and_continue_run_the_same_command_without_the_key() {
        let new = argv_for(Session::New("s-1".into()));
        assert_eq!(
            new,
            [
                "pi",
                "--mode",
                "json",
                "--thinking",
                "high",
                "--model",
                "deepseek/deepseek-v4-flash",
                "--session-dir",
                "/run/pi-sessions",
                "--session-id",
                "s-1",
                "do it",
            ]
        );
        assert_eq!(argv_for(Session::Continue("s-1".into())), new);
        assert!(!new
            .iter()
            .any(|a| a.contains("sk-secret") || a == "--api-key"));
    }

    #[test]
    fn check_accepts_a_pi_profile() {
        assert_eq!(check_profile(&profile()), Ok(()));
    }

    #[test]
    fn check_requires_provider_and_model() {
        for (field, p) in [
            (
                "provider",
                Profile {
                    provider: None,
                    ..profile()
                },
            ),
            (
                "provider",
                Profile {
                    provider: Some(" ".into()),
                    ..profile()
                },
            ),
            (
                "model",
                Profile {
                    model: None,
                    ..profile()
                },
            ),
        ] {
            assert_eq!(
                check_profile(&p).map_err(|e| e.to_string()),
                Err(format!(
                    "agent profile deepseek: {field} is required for mechanism pi"
                ))
            );
        }
    }

    #[test]
    fn check_refuses_model_args_and_zero_limits() {
        let with_model_args = Profile {
            model_args: vec!["--model".into(), "{model}".into()],
            ..profile()
        };
        assert_eq!(
            check_profile(&with_model_args).map_err(|e| e.to_string()),
            Err("agent profile deepseek: model_args is not used by mechanism pi".into())
        );
        let zero = Profile {
            limits: Some(Limits {
                context: 0,
                max_output: 1,
            }),
            ..profile()
        };
        assert_eq!(
            check_profile(&zero).map_err(|e| e.to_string()),
            Err("agent profile deepseek: limits must be positive".into())
        );
    }
}
