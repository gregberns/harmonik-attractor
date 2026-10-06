//! Claude nodes: the flags the engine still derives for the `claude` profile
//! and the mapping from an [`AgentResult`] to today's outcomes and errors.
//! Pure functions.

use attractor_agent_handler::{AgentResult, AgentStatus, FailureClass, Usage};
use attractor_dot::AttributeValue;
use attractor_quality::ClaudeSettingsMode;
use attractor_types::{AttractorError, Outcome, Result};

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

/// A Claude node's outcome, with today's messages:
/// - `Completed`: Success; `Failed(Reported)`: Fail, "Claude Code returned an error";
/// - `Failed(Timeout)`: `CommandTimeout`, the only retryable error;
/// - `Failed(Crash)`: "Claude Code exited with <status>: <stderr>";
/// - `Failed(NoResult)`: the parse or no-output message;
/// - `Failed(Launch)`: `CliNotFound` when the command is missing, else
///   "Failed to spawn Claude Code: ...".
pub(super) fn claude_outcome(
    result: &AgentResult,
    node: &PipelineNode,
    resolved: &ResolvedNode,
    graph: &PipelineGraph,
    timeout_ms: u64,
) -> Result<Outcome> {
    let claude = LlmProvider::Claude;
    let handler_error = |message: String| AttractorError::HandlerError {
        handler: "codergen".into(),
        node: node.id.clone(),
        message,
    };
    let is_error = match result.status {
        AgentStatus::Completed => false,
        AgentStatus::Failed(FailureClass::Reported) => true,
        AgentStatus::Failed(FailureClass::Timeout) => {
            return Err(AttractorError::CommandTimeout { timeout_ms })
        }
        AgentStatus::Failed(FailureClass::Crash) => {
            return Err(handler_error(format!(
                "{} {}",
                claude.display_name(),
                result.detail
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
