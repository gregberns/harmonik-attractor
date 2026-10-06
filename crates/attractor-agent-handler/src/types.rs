//! What the engine asks for and what it gets back.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
    /// Told once when the invocation ends, with the value `run` returns.
    pub observer: Option<&'a dyn AgentObserver>,
}

/// What [`crate::Agents::run`] hands a handler: the request, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub transcript: Option<&'a Path>,
}

/// How an invocation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Completed,
    Failed(FailureClass),
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
            usage: Usage::default(),
            exit: None,
            duration: Duration::ZERO,
            launch_error: None,
        }
    }
}
