//! The `claude-p` agent handler: runs Claude Code with `-p <prompt>
//! --output-format stream-json --verbose` as a local process and classifies
//! its end with the failure table in [`parse::classify`]. A rate-limited
//! attempt waits and re-spawns, continuing the session, within the
//! profile's `rate_limit_window` (design §5).

mod parse;
mod rate_limit;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;

use attractor_agent_handler::{
    spawn_path, AgentHandler, AgentResult, AgentStatus, FailureClass, Invocation, Spawned, Usage,
};
use attractor_agent_process::{run_local, LocalRun, Spawn};

pub use parse::{
    classify, claude_result_line, claude_session_id, rate_limited, summarize, Exited, RateLimit,
};
pub use rate_limit::{unix_seconds, wait_for, Clock, SystemClock, BACKOFF, MIN_WAIT};

/// The handler for profiles with mechanism `claude-p`.
#[derive(Clone)]
pub struct ClaudeP {
    clock: Arc<dyn Clock>,
}

impl Default for ClaudeP {
    fn default() -> Self {
        Self::with_clock(Arc::new(SystemClock))
    }
}

impl std::fmt::Debug for ClaudeP {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeP").finish_non_exhaustive()
    }
}

impl ClaudeP {
    pub const MECHANISM: &'static str = "claude-p";

    /// The handler waiting for rate limits on `clock` (tests inject one
    /// that never waits).
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self { clock }
    }

    /// `argv` followed by the flags the parser depends on.
    fn with_flags(argv: &[String], prompt: &str) -> Vec<String> {
        let mut argv = argv.to_vec();
        argv.extend([
            "-p".to_string(),
            prompt.to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
        ]);
        argv
    }
}

/// What one spawn left.
struct SpawnEnd {
    result: AgentResult,
    /// Set when the spawn ended with a rate-limited result.
    rate_limit: Option<RateLimit>,
}

#[async_trait]
impl AgentHandler for ClaudeP {
    fn mechanism(&self) -> &'static str {
        Self::MECHANISM
    }

    /// The invocation's argv followed by the flags the parser depends on.
    fn argv(&self, inv: &Invocation<'_>) -> Vec<String> {
        Self::with_flags(&inv.argv, inv.prompt)
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    async fn run(&self, inv: Invocation<'_>) -> AgentResult {
        let started = Instant::now();
        let start = self.clock.now();
        // The invocation can't outlast its timeout plus the window: the
        // window bounds the waiting, the timeout each spawn.
        let deadline = start + inv.timeout.saturating_add(inv.rate_limit_window);
        let mut window_start: Option<SystemTime> = None;
        let mut usage = Usage::default();
        let mut session_reported = None;
        let mut waited = Duration::ZERO;
        let mut spawn = 1u32;
        loop {
            // A re-spawn resumes only a session the agent reported (it
            // printed its init line); otherwise it starts the same minted
            // session again.
            let argv = match (&inv.continue_argv, &session_reported) {
                (Some(continue_argv), Some(_)) if spawn > 1 => {
                    Self::with_flags(continue_argv, inv.prompt)
                }
                _ => self.argv(&inv),
            };
            let timeout = remaining(deadline, self.clock.now()).min(inv.timeout);
            let end = self.spawn_once(&inv, &argv, spawn, timeout).await;
            usage = add_usage(usage, &end.result.usage);
            session_reported = end.result.agent_session_id.clone().or(session_reported);
            let finish = |result: AgentResult| AgentResult {
                usage: usage.clone(),
                agent_session_id: session_reported.clone(),
                duration: started.elapsed(),
                ..result
            };
            let Some(rate_limit) = end.rate_limit else {
                return finish(end.result);
            };
            let now = self.clock.now();
            let window_start = *window_start.get_or_insert(now);
            let used = now.duration_since(window_start).unwrap_or_default();
            let wait = inv.continue_argv.as_ref().and_then(|_| {
                wait_for(
                    &rate_limit,
                    unix_seconds(now),
                    spawn,
                    inv.rate_limit_window.saturating_sub(used),
                )
            });
            let Some(wait) = wait else {
                return finish(rate_limited_failure(
                    &end.result,
                    &rate_limit,
                    waited,
                    spawn,
                ));
            };
            (inv.rate_limited)(wait);
            tokio::select! {
                () = self.clock.sleep(wait) => {}
                () = inv.cancel.cancelled() => {
                    return finish(AgentResult {
                        status: AgentStatus::Cancelled,
                        ..AgentResult::failed(inv.invocation_id, FailureClass::Crash, "cancelled")
                    });
                }
            }
            waited = waited.saturating_add(wait);
            spawn = spawn.saturating_add(1);
        }
    }

    fn transcript_usage(&self, transcript: &str) -> Usage {
        summarize(transcript)
    }
}

impl ClaudeP {
    /// Run spawn `spawn` of `inv` with `argv`, writing to that spawn's
    /// transcript and stderr files.
    async fn spawn_once(
        &self,
        inv: &Invocation<'_>,
        argv: &[String],
        spawn: u32,
        timeout: Duration,
    ) -> SpawnEnd {
        let started = Instant::now();
        let transcript = inv.transcript.map(|path| spawn_path(path, spawn));
        let stderr = inv.stderr.map(|path| spawn_path(path, spawn));
        let spawned = |process: Spawn| {
            (inv.spawned)(Spawned {
                pid: process.pid,
                pgid: process.pgid,
                host: process.host,
            })
        };
        let local = run_local(
            argv,
            &inv.env,
            inv.workdir,
            transcript.as_deref(),
            stderr.as_deref(),
            timeout,
            inv.kill_grace,
            &inv.cancel,
            &spawned,
        )
        .await;
        let id = inv.invocation_id;
        let partial = |result: AgentResult| SpawnEnd {
            result: with_partial_output(transcript.as_ref(), result),
            rate_limit: None,
        };
        match local {
            LocalRun::Exited(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                SpawnEnd {
                    result: classify(
                        id,
                        &Exited {
                            status: out.status,
                            stdout: &stdout,
                            stderr: &stderr,
                        },
                        started.elapsed(),
                    ),
                    rate_limit: rate_limited(&stdout),
                }
            }
            LocalRun::TimedOut => partial(AgentResult::failed(
                id,
                FailureClass::Timeout,
                format!("timed out after {}ms", timeout.as_millis()),
            )),
            LocalRun::Cancelled => partial(AgentResult {
                status: AgentStatus::Cancelled,
                ..AgentResult::failed(id, FailureClass::Crash, "cancelled")
            }),
            LocalRun::WaitFailed(error) => partial(AgentResult::failed(
                id,
                FailureClass::Crash,
                format!("execution failed: {error}"),
            )),
            LocalRun::LaunchFailed(error) => SpawnEnd {
                result: AgentResult {
                    launch_error: Some(error.kind()),
                    ..AgentResult::failed(id, FailureClass::Launch, error.to_string())
                },
                rate_limit: None,
            },
        }
    }
}

/// What is left until `deadline` at `now` (zero once past).
fn remaining(deadline: SystemTime, now: SystemTime) -> Duration {
    deadline.duration_since(now).unwrap_or_default()
}

/// The reported failure after the window (or with no way to continue the
/// session): "rate limited: <text>; waited <s> s over <n> spawns".
fn rate_limited_failure(
    last: &AgentResult,
    rate_limit: &RateLimit,
    waited: Duration,
    spawns: u32,
) -> AgentResult {
    let detail = format!(
        "rate limited: {}; waited {} s over {spawns} spawn{}",
        rate_limit.text,
        waited.as_secs(),
        if spawns == 1 { "" } else { "s" }
    );
    AgentResult {
        status: AgentStatus::Failed(FailureClass::Reported),
        text: detail.clone(),
        detail,
        ..last.clone()
    }
}

/// Token and cost figures of two spawns together: counts and cost add up;
/// the model is the later spawn's, if it reported one.
fn add_usage(sum: Usage, spawn: &Usage) -> Usage {
    let add = |a: Option<u64>, b: Option<u64>| match (a, b) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (a, b) => a.or(b),
    };
    Usage {
        model_actual: spawn.model_actual.clone().or(sum.model_actual),
        input_tokens: add(sum.input_tokens, spawn.input_tokens),
        output_tokens: add(sum.output_tokens, spawn.output_tokens),
        cost_usd: match (sum.cost_usd, spawn.cost_usd) {
            (Some(a), Some(b)) => Some(a + b),
            (a, b) => a.or(b),
        },
        turns: match (sum.turns, spawn.turns) {
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
            (a, b) => a.or(b),
        },
    }
}

/// `result` with the usage and session id read from the transcript written
/// so far, for a spawn that ended without its full output (timeout, cancel,
/// or a failed wait).
fn with_partial_output(transcript: Option<&PathBuf>, result: AgentResult) -> AgentResult {
    let partial = transcript
        .and_then(|path| std::fs::read(path).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    AgentResult {
        usage: summarize(&partial),
        agent_session_id: claude_session_id(&partial),
        ..result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawn_gets_what_is_left_until_the_deadline_never_negative() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let deadline = start + Duration::from_secs(30);
        assert_eq!(remaining(deadline, start), Duration::from_secs(30));
        assert_eq!(
            remaining(deadline, start + Duration::from_secs(25)),
            Duration::from_secs(5)
        );
        assert_eq!(
            remaining(deadline, start + Duration::from_secs(40)),
            Duration::ZERO
        );
    }

    #[test]
    fn usage_adds_up_over_spawns() {
        let a = Usage {
            model_actual: Some("m1".into()),
            input_tokens: Some(3),
            output_tokens: None,
            cost_usd: Some(0.5),
            turns: Some(1),
        };
        let b = Usage {
            model_actual: None,
            input_tokens: Some(4),
            output_tokens: Some(2),
            cost_usd: Some(0.25),
            turns: Some(2),
        };
        assert_eq!(
            add_usage(a, &b),
            Usage {
                model_actual: Some("m1".into()),
                input_tokens: Some(7),
                output_tokens: Some(2),
                cost_usd: Some(0.75),
                turns: Some(3),
            }
        );
    }
}
