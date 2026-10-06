//! The `gemini` agent handler: runs the Gemini CLI with `--output-format
//! <json|stream-json>` after the profile's command and the prompt last, as a
//! local process, and classifies its end with [`parse::classify`]. The
//! format comes from a `--help` probe ([`probe::probe`]), once per command.

mod parse;
mod probe;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use attractor_agent_handler::{
    AgentHandler, AgentResult, AgentStatus, FailureClass, Invocation, Spawned, Usage,
};
use attractor_agent_process::{run_local, LocalRun, Spawn};

pub use parse::{classify, has_final_result, is_stream, parse, summarize, Exited, Parsed};
pub use probe::{probe, OutputFormat, PROBE_TIMEOUT};

/// The handler for profiles with mechanism `gemini`. It remembers each
/// command's probed output format, so one `Gemini` probes a command once.
#[derive(Debug)]
pub struct Gemini {
    formats: Mutex<HashMap<Vec<String>, OutputFormat>>,
    probe_timeout: Duration,
}

impl Default for Gemini {
    fn default() -> Self {
        Self {
            formats: Mutex::default(),
            probe_timeout: PROBE_TIMEOUT,
        }
    }
}

impl Gemini {
    pub const MECHANISM: &'static str = "gemini";
    pub const DISPLAY_NAME: &'static str = "Gemini CLI";

    /// The same handler with another probe timeout (tests).
    pub fn with_probe_timeout(self, probe_timeout: Duration) -> Self {
        Self {
            probe_timeout,
            ..self
        }
    }

    /// The invocation's argv with `--output-format <format>` right after
    /// the profile's (possibly multi-word) command, and the prompt last
    /// (positional; `-p` is deprecated). Gemini has no `--cwd`: the process
    /// runs in the workdir.
    pub fn argv_with(inv: &Invocation<'_>, format: OutputFormat) -> Vec<String> {
        let at = inv.command_len.min(inv.argv.len());
        let (command, rest) = inv.argv.split_at(at);
        command
            .iter()
            .cloned()
            .chain(["--output-format".to_string(), format.as_arg().to_string()])
            .chain(rest.iter().cloned())
            .chain([inv.prompt.to_string()])
            .collect()
    }

    /// The profile's command in `inv`'s argv.
    fn command(inv: &Invocation<'_>) -> Vec<String> {
        inv.argv[..inv.command_len.min(inv.argv.len())].to_vec()
    }

    /// The format already probed for `inv`'s command, if any.
    fn known_format(&self, inv: &Invocation<'_>) -> Option<OutputFormat> {
        let command = Self::command(inv);
        self.formats
            .lock()
            .ok()
            .and_then(|formats| formats.get(&command).copied())
    }

    /// The output format for `inv`'s command, probed on first use.
    async fn format(&self, inv: &Invocation<'_>) -> OutputFormat {
        if let Some(format) = self.known_format(inv) {
            return format;
        }
        let command = Self::command(inv);
        let format = probe(&command, &inv.env, inv.workdir, self.probe_timeout).await;
        if let Ok(mut formats) = self.formats.lock() {
            formats.insert(command, format);
        }
        format
    }
}

#[async_trait]
impl AgentHandler for Gemini {
    fn mechanism(&self) -> &'static str {
        Self::MECHANISM
    }

    /// Probes the command's output format (once per command).
    async fn prepare(&self, inv: &Invocation<'_>) {
        self.format(inv).await;
    }

    /// The argv with the probed `--output-format` (`json` if no probe result
    /// is known, as after any probe failure).
    fn argv(&self, inv: &Invocation<'_>) -> Vec<String> {
        Self::argv_with(inv, self.known_format(inv).unwrap_or(OutputFormat::Json))
    }

    fn display_name(&self) -> &'static str {
        Self::DISPLAY_NAME
    }

    /// Gemini reports no cost.
    fn reports_cost(&self) -> bool {
        false
    }

    async fn run(&self, inv: Invocation<'_>) -> AgentResult {
        let started = Instant::now();
        // `Agents::run` already called `prepare`; this is the cache.
        self.format(&inv).await;
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
