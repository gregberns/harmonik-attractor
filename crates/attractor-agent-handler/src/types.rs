//! What the engine asks for and what it gets back.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::registry::AgentObserver;

/// Which agent profile runs a node, and with which model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub profile: String,
    /// Fills the profile's `{model}` template; `None` leaves `model_args` out.
    pub model: Option<String>,
}

/// The ids that name one invocation; they become the `PAS_*` variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// `None` outside a run (isolated execution): `PAS_RUN_ID` is then unset.
    pub run_id: Option<String>,
    pub node_id: String,
    /// 1-based.
    pub attempt: u32,
    pub invocation_id: String,
}

/// One request from the engine to [`crate::Agents::run`].
pub struct AgentRequest<'a> {
    pub selection: Selection,
    /// The prompt, assembled by the engine.
    pub prompt: String,
    /// Flags the engine still derives, placed after the profile's args:
    /// 1. the `[codergen.claude]` flags (settings mode, `--mcp-config`,
    ///    `--settings`, `--tools`, `--agents`, `--plugin-dir`), which ticket
    ///    03 folds into the `claude` profile's args and then removes here;
    /// 2. the node attributes `allowed_tools` and `max_budget_usd`, which
    ///    design §1/§2 has no other place for.
    pub extra_args: Vec<String>,
    pub workdir: PathBuf,
    pub timeout: Duration,
    pub record: Record,
    /// Where the handler writes the agent's stdout as it arrives, if anywhere.
    pub transcript: Option<PathBuf>,
    /// Where the handler writes the agent's stderr as it arrives, if anywhere.
    pub stderr: Option<PathBuf>,
    /// Where `Agents::run` writes the prompt file before the handler starts.
    pub prompt_file: Option<PathBuf>,
    /// Told when each process starts and once when the invocation ends.
    pub observer: Option<&'a dyn AgentObserver>,
    /// Cancelled when the Run is stopped: the handler stops the agent
    /// (TERM, grace, KILL) and returns `Cancelled`.
    pub cancel: CancellationToken,
}

/// What [`crate::Agents::run`] hands a handler: the request, resolved.
pub struct Invocation<'a> {
    pub invocation_id: &'a str,
    /// Profile command, profile args, extra args and filled model args.
    /// The handler appends its own flags and must not drop any of these.
    pub argv: Vec<String>,
    /// The complete child environment; the handler uses exactly this map.
    pub env: BTreeMap<String, String>,
    pub prompt: &'a str,
    pub workdir: &'a Path,
    pub timeout: Duration,
    /// How long to wait after TERM before KILL, on timeout or cancel.
    pub kill_grace: Duration,
    pub transcript: Option<&'a Path>,
    pub stderr: Option<&'a Path>,
    pub cancel: CancellationToken,
    /// Call once per process, right after it exists and its transcript and
    /// stderr files exist, and before any of its output is written.
    pub spawned: &'a (dyn Fn(Spawned) + Sync),
}

impl std::fmt::Debug for Invocation<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invocation")
            .field("invocation_id", &self.invocation_id)
            .field("argv", &self.argv)
            .field("env", &self.env)
            .field("prompt", &self.prompt)
            .field("workdir", &self.workdir)
            .field("timeout", &self.timeout)
            .field("kill_grace", &self.kill_grace)
            .field("transcript", &self.transcript)
            .field("stderr", &self.stderr)
            .field("cancel", &self.cancel)
            .finish_non_exhaustive()
    }
}

/// A process a handler started, as it reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawned {
    pub pid: u32,
    pub pgid: u32,
    pub host: Option<String>,
}

/// One process of an invocation has started (the journal's `LlmStarted`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub invocation_id: String,
    /// 1 for the first process of the invocation.
    pub spawn: u32,
    pub node_id: String,
    pub attempt: u32,
    pub profile: String,
    pub model: Option<String>,
    pub pid: u32,
    pub pgid: u32,
    pub host: Option<String>,
    pub transcript: Option<PathBuf>,
    pub stderr: Option<PathBuf>,
}

/// How an invocation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Completed,
    Failed(FailureClass),
    /// The request's cancel token fired and the agent was stopped.
    Cancelled,
}

/// Why an invocation failed (design §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// The agent finished and reported an error.
    Reported,
    /// The timeout fired and the process was killed.
    Timeout,
    /// The process ended without a result: non-zero exit or a signal.
    Crash,
    /// The process exited 0 but gave no usable result.
    NoResult,
    /// The process could not be started.
    Launch,
}

/// Token and cost figures read from the agent's output; `None` when absent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub model_actual: Option<String>,
    /// Prompt tokens, including cached and cache-creation tokens.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub turns: Option<u32>,
}

/// How the process exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// The end of one invocation. Never an error: failures are a status.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentResult {
    pub invocation_id: String,
    pub status: AgentStatus,
    /// The agent's final text (for `Reported`, its error text).
    pub text: String,
    /// For a failure, a human-readable reason.
    pub detail: String,
    /// The last lines of the agent's stderr (bounded), for a failure's
    /// message; empty when there was none or it wasn't read.
    pub stderr_tail: String,
    pub usage: Usage,
    pub exit: Option<ExitInfo>,
    pub duration: Duration,
    /// For `Failed(Launch)` from a spawn error, the error's kind.
    pub launch_error: Option<io::ErrorKind>,
}

impl AgentResult {
    /// A failure with no output, usage or exit.
    pub fn failed(
        invocation_id: impl Into<String>,
        class: FailureClass,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            invocation_id: invocation_id.into(),
            status: AgentStatus::Failed(class),
            text: String::new(),
            detail: detail.into(),
            stderr_tail: String::new(),
            usage: Usage::default(),
            exit: None,
            duration: Duration::ZERO,
            launch_error: None,
        }
    }
}
