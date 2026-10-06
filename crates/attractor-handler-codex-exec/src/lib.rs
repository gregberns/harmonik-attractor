//! The `codex-exec` agent handler: runs the Codex CLI's `exec --json` (the
//! profile holds `exec --json ...`) with `--cd <workdir> <prompt>` appended,
//! as a local process, and classifies its end with [`parse::classify`].

mod parse;

use std::time::Instant;

use async_trait::async_trait;

use attractor_agent_handler::{
    AgentHandler, AgentResult, AgentStatus, FailureClass, Invocation, Spawned, Usage,
};
use attractor_agent_process::{run_local, LocalRun, Spawn};

pub use parse::{classify, has_final_result, parse, summarize, Exited, Parsed};

/// The handler for profiles with mechanism `codex-exec`.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodexExec;

impl CodexExec {
    pub const MECHANISM: &'static str = "codex-exec";
    pub const DISPLAY_NAME: &'static str = "Codex CLI";
}

#[async_trait]
impl AgentHandler for CodexExec {
    fn mechanism(&self) -> &'static str {
        Self::MECHANISM
    }

    /// The invocation's argv, then `--cd <workdir>` and the prompt, which
    /// Codex takes as its last, positional argument (`-p` is `--profile`).
    fn argv(&self, inv: &Invocation<'_>) -> Vec<String> {
        let mut argv = inv.argv.clone();
        argv.extend([
            "--cd".to_string(),
            inv.workdir.to_string_lossy().into_owned(),
            inv.prompt.to_string(),
        ]);
        argv
    }

    fn display_name(&self) -> &'static str {
        Self::DISPLAY_NAME
    }

    /// Codex's stream carries no cost.
    fn reports_cost(&self) -> bool {
        false
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
            LocalRun::TimedOut => AgentResult {
                usage: partial_usage(&inv),
                ..AgentResult::failed(
                    id,
                    FailureClass::Timeout,
                    format!("timed out after {}ms", inv.timeout.as_millis()),
                )
            },
            LocalRun::Cancelled => AgentResult {
                status: AgentStatus::Cancelled,
                usage: partial_usage(&inv),
                ..AgentResult::failed(id, FailureClass::Crash, "cancelled")
            },
            LocalRun::WaitFailed(error) => AgentResult {
                usage: partial_usage(&inv),
                ..AgentResult::failed(
                    id,
                    FailureClass::Crash,
                    format!("execution failed: {error}"),
                )
            },
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

/// Usage from the transcript written so far, for a run that ended without
/// its full output (timeout, cancel, or a failed wait).
fn partial_usage(inv: &Invocation<'_>) -> Usage {
    inv.transcript
        .and_then(|path| std::fs::read(path).ok())
        .map(|bytes| summarize(&String::from_utf8_lossy(&bytes)))
        .unwrap_or_default()
}
