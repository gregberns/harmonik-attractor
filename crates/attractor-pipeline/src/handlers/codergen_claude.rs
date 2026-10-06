//! Claude nodes: the `[codergen.claude]` flags, folded into the `claude`
//! profile, the node flags the engine still derives, and the mapping from an
//! [`AgentResult`] to today's outcomes and errors. Pure functions, except
//! [`agent_profiles`] reading the built-in profiles compiled into the binary.

use std::path::Path;

use attractor_agent_handler::{
    AgentResult, AgentStatus, AgentsConfig, ConfigError, FailureClass, Profile, Usage,
};
use attractor_dot::AttributeValue;
use attractor_journal::RunDir;
use attractor_quality::ClaudeSettingsMode;
use attractor_types::{AgentFiles, AttractorError, FailureKind, Outcome, Result};

use super::{provider_outcome, ProviderResult};
use crate::execution_plan::ResolvedNode;
use crate::graph::{PipelineGraph, PipelineNode};
use crate::run_configuration::{ResolvedClaudeConfig, ResolvedConfig};

/// The agent profile Claude nodes (`llm_provider="claude"`) run with.
pub const CLAUDE_PROFILE: &str = "claude";

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

impl ClaudeCliConfig {
    /// The `[codergen.claude]` settings a run resolved (CLI over `pas.toml`).
    pub(super) fn from_resolved(claude: &ResolvedClaudeConfig) -> Self {
        Self {
            settings_mode: *claude.settings_mode().value(),
            setting_sources: claude
                .setting_sources()
                .value()
                .iter()
                .map(|source| source.as_str().to_owned())
                .collect(),
            settings: claude.settings().value().clone(),
            tools: claude.tools().value().clone(),
            agents: claude.agents().value().clone(),
            plugin_dirs: claude
                .plugin_dirs()
                .value()
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            mcp_config: claude.mcp_config().value().clone(),
        }
    }
}

/// The agent profiles a run uses: the built-in ones, then `pas.toml`'s
/// `[agents.<name>]` (whole profiles by name), with the run's
/// `[codergen.claude]` flags folded into the `claude` profile's args before
/// `inherit_from` resolves, so a profile inheriting from `claude` keeps them.
pub fn agent_profiles(controls: &ResolvedConfig) -> std::result::Result<Vec<Profile>, ConfigError> {
    let overrides = controls
        .manifest()
        .map(|resolved| resolved.manifest.agents.clone())
        .unwrap_or_default();
    let config = AgentsConfig::builtin()?.with_overrides(&overrides);
    let flags = claude_settings_args(&ClaudeCliConfig::from_resolved(controls.claude()));
    fold_codergen_claude(config, flags).resolve()
}

/// `config` with `flags` appended to the `claude` profile's args. When that
/// profile sets no args of its own, they go after the args it inherits.
pub(super) fn fold_codergen_claude(mut config: AgentsConfig, flags: Vec<String>) -> AgentsConfig {
    let inherited = inherited_args(&config, CLAUDE_PROFILE);
    if let Some(claude) = config.profiles.get_mut(CLAUDE_PROFILE) {
        let args = claude.args.get_or_insert(inherited);
        args.extend(flags);
    }
    config
}

/// The args `name` gets from its `inherit_from` chain; empty when none sets
/// any (or the chain is broken, which resolving reports).
fn inherited_args(config: &AgentsConfig, name: &str) -> Vec<String> {
    let mut next = config
        .profiles
        .get(name)
        .and_then(|p| p.inherit_from.clone());
    let mut seen = vec![name.to_string()];
    while let Some(parent_name) = next {
        if seen.contains(&parent_name) {
            break;
        }
        let Some(parent) = config.profiles.get(&parent_name) else {
            break;
        };
        if let Some(args) = &parent.args {
            return args.clone();
        }
        next = parent.inherit_from.clone();
        seen.push(parent_name);
    }
    Vec::new()
}

/// The `[codergen.claude]` flags: settings mode, `--mcp-config`,
/// `--settings`, `--tools`, `--agents`, `--plugin-dir`.
pub(super) fn claude_settings_args(cfg: &ClaudeCliConfig) -> Vec<String> {
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
    args
}

/// The node attributes `allowed_tools` and `max_budget_usd` as flags.
pub(super) fn claude_node_args(node: &PipelineNode) -> Vec<String> {
    let mut args = Vec::new();
    let mut flag = |name: &str, value: &str| {
        args.push(name.to_string());
        args.push(value.to_string());
    };
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

/// Facts about one Model Invocation read from its agent's output, for
/// `LlmInvoked`. Every field is optional: data the output does not carry is
/// `None`.
#[derive(Debug, Default, Clone, PartialEq)]
pub(super) struct InvocationUsage {
    pub(super) model_actual: Option<String>,
    /// Prompt tokens, including cached and cache-creation tokens.
    pub(super) input_tokens: Option<u64>,
    pub(super) output_tokens: Option<u64>,
    pub(super) cost_usd: Option<f64>,
}

/// Which agent, and which attempt of a node, one invocation was, as its
/// outcome and failure messages name them.
#[derive(Debug, Clone, Copy)]
pub(super) struct AgentAttempt<'a> {
    /// The handler's display name, e.g. "Codex CLI".
    pub(super) agent: &'a str,
    /// The profile's program, named when it cannot be found.
    pub(super) program: &'a str,
    /// Whether the agent reports a cost (Claude does; Codex and Gemini
    /// don't, so their `<node>.cost_usd` stays unset).
    pub(super) reports_cost: bool,
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

/// An agent node's outcome, for every handler:
/// - `Completed`: Success; `Failed(Reported)`: Fail, "<agent> returned an error";
/// - `Failed(Timeout)`: `AgentTimeout` (node, attempt, files), the only
///   retryable error;
/// - `Failed(Crash)`: [`crash_message`];
/// - `Failed(NoResult)`: the parse or no-output message;
/// - `Failed(Launch)`: `CliNotFound` naming the program when it is missing,
///   else "Failed to spawn <agent>: ...";
/// - `Cancelled` (the Run was stopped): `AttractorError::Cancelled`.
pub(super) fn agent_outcome(
    result: &AgentResult,
    node: &PipelineNode,
    resolved: &ResolvedNode,
    graph: &PipelineGraph,
    attempt: &AgentAttempt<'_>,
) -> Result<Outcome> {
    // Displays as codergen's `HandlerError` always did; `kind` is the
    // attempt's failure class for its commit.
    let agent_failed = |kind: FailureKind, message: String| AttractorError::AgentFailed {
        node: node.id.clone(),
        kind,
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
            return Err(agent_failed(
                FailureKind::Crash,
                crash_message(
                    attempt.agent,
                    attempt.attempt,
                    &result.detail,
                    &result.stderr_tail,
                    files().as_ref(),
                ),
            ))
        }
        AgentStatus::Failed(FailureClass::NoResult) => {
            return Err(agent_failed(FailureKind::NoResult, result.detail.clone()))
        }
        AgentStatus::Failed(FailureClass::Launch) => {
            return Err(
                if result.launch_error == Some(std::io::ErrorKind::NotFound) {
                    AttractorError::CliNotFound {
                        binary: attempt.program.to_string(),
                    }
                } else {
                    agent_failed(
                        FailureKind::Launch,
                        format!("Failed to spawn {}: {}", attempt.agent, result.detail),
                    )
                },
            )
        }
    };
    Ok(provider_outcome(
        node,
        resolved,
        graph,
        attempt.agent,
        ProviderResult {
            text: &result.text,
            is_error,
            cost_usd: attempt
                .reports_cost
                .then(|| result.usage.cost_usd.unwrap_or(0.0)),
            turns: result.usage.turns,
        },
    ))
}
