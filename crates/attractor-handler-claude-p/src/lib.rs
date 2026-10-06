//! The `claude-p` agent handler: runs Claude Code with `-p <prompt>
//! --output-format stream-json --verbose` as a local process and classifies
//! its end with the failure table in [`parse::classify`].

mod parse;

use std::time::Instant;

use async_trait::async_trait;

use attractor_agent_handler::{AgentHandler, AgentResult, FailureClass, Invocation};
use attractor_agent_process::{run_local, LocalRun};

pub use parse::{classify, claude_result_line, summarize, Exited};

/// The handler for profiles with mechanism `claude-p`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeP;

impl ClaudeP {
    pub const MECHANISM: &'static str = "claude-p";
}

/// The invocation's argv followed by the flags the parser depends on.
fn full_argv(inv: &Invocation<'_>) -> Vec<String> {
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

#[async_trait]
impl AgentHandler for ClaudeP {
    fn mechanism(&self) -> &'static str {
        Self::MECHANISM
    }

    async fn run(&self, inv: Invocation<'_>) -> AgentResult {
        let started = Instant::now();
        let local = run_local(
            &full_argv(&inv),
            &inv.env,
            inv.workdir,
            inv.transcript,
            inv.timeout,
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
            LocalRun::TimedOut => AgentResult::failed(
                id,
                FailureClass::Timeout,
                format!("timed out after {}ms", inv.timeout.as_millis()),
            ),
            LocalRun::WaitFailed(error) => AgentResult::failed(
                id,
                FailureClass::Crash,
                format!("execution failed: {error}"),
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
}
