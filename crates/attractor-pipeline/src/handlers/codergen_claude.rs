//! Claude nodes: the flags the engine still derives for the `claude` profile
//! and the mapping from an [`AgentResult`] to today's outcomes and errors.
//! Pure functions.

use std::path::Path;

use attractor_agent_handler::{AgentResult, AgentStatus, FailureClass, Usage};
use attractor_dot::AttributeValue;
use attractor_journal::RunDir;
use attractor_quality::ClaudeSettingsMode;
use attractor_types::{AgentFiles, AttractorError, Outcome, Result};

use super::provider::InvocationUsage;
use super::{provider_outcome, ProviderResult};
use crate::execution_plan::{LlmProvider, ResolvedNode};
use crate::graph::{PipelineGraph, PipelineNode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClaudeCliConfig {
    pub(super) settings_mode: ClaudeSettingsMode,
    pub(super) setting_sources: Vec<String>,
    pub(super) settings: Option<String>,
    pub(super) tools: Option<String>,
    pub(super) agents: Option<String>,
    pub(super) plugin_dirs: Vec<String>,
    pub(super) mcp_config: Option<String>,
}

impl Default for ClaudeCliConfig {
    fn default() -> Self {
        Self {
            settings_mode: ClaudeSettingsMode::SubscriptionBare,
            setting_sources: vec![],
            settings: None,
            tools: None,
            agents: None,
            plugin_dirs: vec![],
            mcp_config: None,
        }
    }
}

/// The `AgentRequest.extra_args` of a Claude node: the `[codergen.claude]`
/// flags, then the node's `allowed_tools` and `max_budget_usd`.
pub(super) fn claude_extra_args(cfg: &ClaudeCliConfig, node: &PipelineNode) -> Vec<String> {
    let mut args = Vec::new();
    match cfg.settings_mode {
        ClaudeSettingsMode::SubscriptionBare => args.push("--safe-mode".to_string()),
        ClaudeSettingsMode::StrictBare => args.push("--bare".to_string()),
        ClaudeSettingsMode::Inherit => {
            if !cfg.setting_sources.is_empty() {
                args.push("--setting-sources".to_string());
                args.push(cfg.setting_sources.join(","));
            }
        }
    }
    let mut flag = |name: &str, value: &str| {
        args.push(name.to_string());
        args.push(value.to_string());
    };
    if let Some(mcp_config) = &cfg.mcp_config {
        flag("--mcp-config", mcp_config);
    }
    if let Some(settings) = &cfg.settings {
        flag("--settings", settings);
    }
    if let Some(tools) = &cfg.tools {
        flag("--tools", tools);
    }
    if let Some(agents) = &cfg.agents {
        flag("--agents", agents);
    }
    for plugin_dir in &cfg.plugin_dirs {
        flag("--plugin-dir", plugin_dir);
    }
    if let Some(AttributeValue::String(tools)) = node.raw_attrs.get("allowed_tools") {
        flag("--allowedTools", tools);
    }
    if let Some(AttributeValue::String(budget)) = node.raw_attrs.get("max_budget_usd") {
        flag("--max-budget-usd", budget);
    }
    args
}

/// The `LlmInvoked` usage fields of an agent's usage.
pub(super) fn invocation_usage(usage: &Usage) -> InvocationUsage {
    InvocationUsage {
        model_actual: usage.model_actual.clone(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cost_usd: usage.cost_usd,
    }
}

/// Which attempt of a node one invocation was, as its failure messages
/// name it.
#[derive(Debug, Clone, Copy)]
pub(super) struct AgentAttempt<'a> {
    /// 1-based.
    pub(super) attempt: u32,
    pub(super) timeout_ms: u64,
    /// The Run folder (absolute), when the node runs in a Run.
    pub(super) run_dir: Option<&'a Path>,
    pub(super) invocation_id: &'a str,
}

/// The Transcript and stderr log of `invocation_id` in `run_dir`, when there
/// is a Run folder. Absolute when `run_dir` is.
pub(super) fn agent_failure_files(
    run_dir: Option<&Path>,
    invocation_id: &str,
) -> Option<AgentFiles> {
    run_dir.map(|dir| {
        let dir = RunDir::from_path(dir);
        AgentFiles {
            transcript: dir.transcript(invocation_id),
            stderr: dir.stderr(invocation_id),
        }
    })
}

/// A crashed agent's message: `attempt <n>: <agent> <detail>; last stderr
/// lines:\n<tail>` (`(no stderr)` for an empty tail), then
/// `\ntranscript <path>; stderr <path>` when there are files.
pub(super) fn crash_message(
    agent: &str,
    attempt: u32,
    detail: &str,
    stderr_tail: &str,
    files: Option<&AgentFiles>,
) -> String {
    let tail = if stderr_tail.is_empty() {
        "(no stderr)"
    } else {
        stderr_tail
    };
    let files = files.map(|files| format!("\n{files}")).unwrap_or_default();
    format!("attempt {attempt}: {agent} {detail}; last stderr lines:\n{tail}{files}")
}

/// A Claude node's outcome:
/// - `Completed`: Success; `Failed(Reported)`: Fail, "Claude Code returned an error";
/// - `Failed(Timeout)`: `AgentTimeout` (node, attempt, files), the only
///   retryable error;
/// - `Failed(Crash)`: [`crash_message`];
/// - `Failed(NoResult)`: the parse or no-output message;
/// - `Failed(Launch)`: `CliNotFound` when the command is missing, else
///   "Failed to spawn Claude Code: ...";
/// - `Cancelled` (the Run was stopped): `AttractorError::Cancelled`.
pub(super) fn claude_outcome(
    result: &AgentResult,
    node: &PipelineNode,
    resolved: &ResolvedNode,
    graph: &PipelineGraph,
    attempt: &AgentAttempt<'_>,
) -> Result<Outcome> {
    let claude = LlmProvider::Claude;
    let handler_error = |message: String| AttractorError::HandlerError {
        handler: "codergen".into(),
        node: node.id.clone(),
        message,
    };
    let files = || agent_failure_files(attempt.run_dir, attempt.invocation_id);
    let is_error = match result.status {
        AgentStatus::Cancelled => {
            return Err(AttractorError::Cancelled {
                node: node.id.clone(),
            })
        }
        AgentStatus::Completed => false,
        AgentStatus::Failed(FailureClass::Reported) => true,
        AgentStatus::Failed(FailureClass::Timeout) => {
            return Err(AttractorError::AgentTimeout {
                node: node.id.clone(),
                attempt: attempt.attempt,
                timeout_ms: attempt.timeout_ms,
                files: files(),
            })
        }
        AgentStatus::Failed(FailureClass::Crash) => {
            return Err(handler_error(crash_message(
                claude.display_name(),
                attempt.attempt,
                &result.detail,
                &result.stderr_tail,
                files().as_ref(),
            )))
        }
        AgentStatus::Failed(FailureClass::NoResult) => {
            return Err(handler_error(result.detail.clone()))
        }
        AgentStatus::Failed(FailureClass::Launch) => {
            return Err(
                if result.launch_error == Some(std::io::ErrorKind::NotFound) {
                    AttractorError::CliNotFound {
                        binary: claude.binary_name().to_string(),
                    }
                } else {
                    handler_error(format!(
                        "Failed to spawn {}: {}",
                        claude.display_name(),
                        result.detail
                    ))
                },
            )
        }
    };
    Ok(provider_outcome(
        node,
        resolved,
        graph,
        claude,
        ProviderResult {
            text: &result.text,
            is_error,
            cost_usd: Some(result.usage.cost_usd.unwrap_or(0.0)),
            turns: result.usage.turns,
        },
    ))
}
