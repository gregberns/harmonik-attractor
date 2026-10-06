//! The `claude-p` agent handler: runs Claude Code with `-p <prompt>
//! --output-format stream-json --verbose` as a local process and classifies
//! its end with the failure table in [`parse::classify`].

mod parse;

use std::time::Instant;

use async_trait::async_trait;

use attractor_agent_handler::{
    AgentHandler, AgentResult, AgentStatus, FailureClass, Invocation, Spawned, Usage,
};
use attractor_agent_process::{run_local, LocalRun, Spawn};

pub use parse::{classify, claude_result_line, claude_session_id, summarize, Exited};

/// The handler for profiles with mechanism `claude-p`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeP;

impl ClaudeP {
    pub const MECHANISM: &'static str = "claude-p";
}

#[async_trait]
impl AgentHandler for ClaudeP {
    fn mechanism(&self) -> &'static str {
        Self::MECHANISM
    }

    /// The invocation's argv followed by the flags the parser depends on.
    fn argv(&self, inv: &Invocation<'_>) -> Vec<String> {
        let mut argv = inv.argv.clone();
        argv.extend([
            "-p".to_string(),
            inv.prompt.to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
        ]);
        argv
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    async fn run(&self, inv: Invocation<'_>) -> AgentResult {
        let started = Instant::now();
        let spawned = |spawn: Spawn| {
            (inv.spawned)(Spawned {
                pid: spawn.pid,
                pgid: spawn.pgid,
                host: spawn.host,
            })
        };
        let local = run_local(
            &self.argv(&inv),
            &inv.env,
            inv.workdir,
            inv.transcript,
            inv.stderr,
            inv.timeout,
            inv.kill_grace,
            &inv.cancel,
            &spawned,
        )
        .await;
        let id = inv.invocation_id;
        let result = match local {
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
            LocalRun::TimedOut => with_partial_output(
                &inv,
                AgentResult::failed(
                    id,
                    FailureClass::Timeout,
                    format!("timed out after {}ms", inv.timeout.as_millis()),
                ),
            ),
            LocalRun::Cancelled => with_partial_output(
                &inv,
                AgentResult {
                    status: AgentStatus::Cancelled,
                    ..AgentResult::failed(id, FailureClass::Crash, "cancelled")
                },
            ),
            LocalRun::WaitFailed(error) => with_partial_output(
                &inv,
                AgentResult::failed(
                    id,
                    FailureClass::Crash,
                    format!("execution failed: {error}"),
                ),
            ),
            LocalRun::LaunchFailed(error) => AgentResult {
                launch_error: Some(error.kind()),
                ..AgentResult::failed(id, FailureClass::Launch, error.to_string())
            },
        };
        AgentResult {
            duration: started.elapsed(),
            ..result
        }
    }

    fn transcript_usage(&self, transcript: &str) -> Usage {
        summarize(transcript)
    }
}

/// `result` with the usage and session id read from the transcript written
/// so far, for a run that ended without its full output (timeout, cancel,
/// or a failed wait).
fn with_partial_output(inv: &Invocation<'_>, result: AgentResult) -> AgentResult {
    let partial = inv
        .transcript
        .and_then(|path| std::fs::read(path).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    AgentResult {
        usage: summarize(&partial),
        agent_session_id: claude_session_id(&partial),
        ..result
    }
}
