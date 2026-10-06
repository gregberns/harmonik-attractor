use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use attractor_agent_handler::{
    AgentObserver, AgentRequest, AgentResult, AgentStatus, Agents, CancellationToken, FailureClass,
    Record, Selection, Session, Started,
};
use attractor_dot::AttributeValue;
use attractor_quality::{
    ClaudeCodergenConfig, ClaudeSettingSource, ClaudeSettingsMode, ResolutionError,
};
use attractor_types::{AttractorError, Context, Outcome, Result, StageStatus};

use crate::events::PipelineEvent;
use crate::execution_plan::{
    Fidelity, HandlerIdentity, ProviderAlias, ResolvedNode, ResolvedNodeKind,
};
use crate::graph::{PipelineGraph, PipelineNode};
use crate::handler::{
    EventSink, HandlerExecutionContext, NodeHandler, ProviderNodeHandler, SessionContext,
};

#[path = "codergen_claude.rs"]
mod claude;
use claude::{
    agent_outcome, claude_node_args, claude_settings_args, invocation_usage, AgentAttempt,
    ClaudeCliConfig, InvocationUsage,
};
pub use claude::{agent_profiles, CLAUDE_PROFILE};

// ---------------------------------------------------------------------------
// CodergenHandler — LLM task handler (box shape)
//
// Shells out to a CLI tool (Claude Code, Codex CLI, or Gemini CLI) for each
// node, passing the node's prompt. The provider is supplied by the canonical
// ExecutionPlan after strict validation.
//
// Supported node attributes:
//   - prompt: Optional task prompt sent to the CLI
//   - llm_provider: "claude", "codex", or "gemini" (required)
//   - llm_model: Override the model (e.g. "sonnet", "o3", "gemini-2.5-pro")
//   - allowed_tools: Comma-separated tool list (Claude only)
//   - max_budget_usd: Spending cap for this node (Claude only)
//   - timeout: Duration before the CLI invocation is killed (default: 10m)
//
// The pipeline context key "workdir" controls the working directory.
//
// When the executor has a Run folder, each provider process (Model Invocation)
// gets a new Invocation ID and its raw stdout is streamed, line by line, to
// `transcripts/<invocation-id>.jsonl` while it runs. When the engine also
// passes its Event path, each Model Invocation emits exactly one `LlmInvoked`
// once the provider exits, fails, or times out.
// ---------------------------------------------------------------------------

pub struct CodergenHandler {
    /// Runs Claude nodes (the `claude` profile); Codex and Gemini nodes
    /// still run through this module's own process code.
    agents: Arc<Agents>,
}

impl CodergenHandler {
    pub fn new(agents: Arc<Agents>) -> Self {
        Self { agents }
    }
}

struct CodergenExecutionControls<'a> {
    dry_run: bool,
    workdir: Option<String>,
    /// The `[codergen.claude]` settings to pass as extra args, on the
    /// compatibility path only; a configured run has them in the `claude`
    /// profile (see [`agent_profiles`]).
    claude: Option<ClaudeCliConfig>,
    /// Run folder that receives Transcripts; `None` writes no Transcript.
    run_dir: Option<PathBuf>,
    /// Receives `LlmInvoked`; `None` emits nothing.
    events: Option<&'a dyn EventSink>,
    /// The Run's id, for `PAS_RUN_ID`; `None` outside a Run.
    run_id: Option<&'a str>,
    /// This attempt at the node, 1-based, for `PAS_ATTEMPT`.
    attempt: u32,
    /// Cancelled when the Run is stopped; stops a Claude agent gracefully.
    cancel: CancellationToken,
    /// Set for the attempt after an interrupted one; added to the prompt.
    resume_note: Option<String>,
    /// The attempt's session context from the engine; `None` outside it.
    session: Option<SessionContext<'a>>,
}

/// `LlmInvoked.status` values (spec C3).
const INVOKED_SUCCESS: &str = "success";
const INVOKED_FAILED: &str = "failed";
const INVOKED_TIMEOUT: &str = "timeout";

/// The one `LlmInvoked` Event of a Model Invocation, armed once the provider
/// process has started. [`Self::finish`] emits it; if the handler future is
/// dropped first (its own timeout, or the engine's outer deadline) `Drop`
/// emits it with status `timeout`, so either timer yields exactly one Event.
struct LlmInvocation<'a> {
    events: &'a dyn EventSink,
    /// Reads usage from a (partial) Transcript, for the `Drop` path.
    summarize: &'a (dyn Fn(&str) -> InvocationUsage + Sync),
    run_dir: PathBuf,
    invocation_id: String,
    node_id: String,
    /// The agent profile's name (for `llm_provider` nodes: `claude`,
    /// `codex` or `gemini`).
    provider: String,
    model_requested: Option<String>,
    /// Whether the invocation continues an earlier session.
    continued: bool,
    /// The session id the agent reported, once known.
    agent_session_id: Option<String>,
    started: Instant,
    emitted: bool,
}

impl LlmInvocation<'_> {
    fn finish(mut self, status: &str, usage: InvocationUsage) {
        self.emit(status, usage);
    }

    /// No Model Invocation happened (the agent could not start): emit nothing.
    fn disarm(mut self) {
        self.emitted = true;
    }

    /// Usage read back from the Transcript, which holds the provider's stdout
    /// so far. Missing or unreadable → all `None`.
    fn transcript_usage(&self) -> InvocationUsage {
        let path =
            attractor_journal::RunDir::from_path(&self.run_dir).transcript(&self.invocation_id);
        std::fs::read(path)
            .map(|bytes| (self.summarize)(&String::from_utf8_lossy(&bytes)))
            .unwrap_or_default()
    }

    fn emit(&mut self, status: &str, usage: InvocationUsage) {
        if std::mem::replace(&mut self.emitted, true) {
            return;
        }
        self.events.emit(PipelineEvent::LlmInvoked {
            invocation_id: self.invocation_id.clone(),
            node_id: self.node_id.clone(),
            provider: self.provider.clone(),
            model_requested: self.model_requested.clone(),
            model_actual: usage.model_actual,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cost_usd: usage.cost_usd,
            duration_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            transcript: attractor_journal::transcript_rel_path(&self.invocation_id),
            status: status.to_owned(),
            agent_session_id: self.agent_session_id.clone(),
            continued: self.continued,
        });
    }
}

impl Drop for LlmInvocation<'_> {
    fn drop(&mut self) {
        if !self.emitted {
            let usage = self.transcript_usage();
            self.emit(INVOKED_TIMEOUT, usage);
        }
    }
}

/// The session an agent attempt runs in (design §1): `full` continues the
/// thread's recorded session if there is one; otherwise, and always for
/// `fresh`, a new session with a newly minted id.
fn choose_session(
    fidelity: Fidelity,
    prior: Option<&str>,
    mint: impl FnOnce() -> String,
) -> Session {
    match (fidelity, prior) {
        (Fidelity::Full, Some(id)) => Session::Continue(id.to_string()),
        (Fidelity::Full, None) | (Fidelity::Fresh, _) => Session::New(mint()),
    }
}

/// Journals `LlmStarted` for each process of a Claude invocation.
/// `LlmInvoked` stays with the [`LlmInvocation`] guard, which also covers a
/// dropped future.
struct StartedJournal<'a> {
    events: &'a dyn EventSink,
}

impl AgentObserver for StartedJournal<'_> {
    fn started(&self, started: &Started) {
        self.events.emit(PipelineEvent::LlmStarted {
            invocation_id: started.invocation_id.clone(),
            spawn: started.spawn,
            node_id: started.node_id.clone(),
            attempt: started.attempt,
            profile: started.profile.clone(),
            model: started.model.clone(),
            host: started.host.clone(),
            session_id: started.session_id.clone(),
            pid: started.pid,
            pgid: started.pgid,
            transcript: attractor_journal::transcript_rel_path(&started.invocation_id),
            stderr: attractor_journal::stderr_rel_path(&started.invocation_id),
        });
    }

    fn finished(&self, _result: &AgentResult) {}
}

#[async_trait]
impl NodeHandler for CodergenHandler {
    fn handler_type(&self) -> &str {
        "codergen"
    }

    fn provider_handler(&self) -> Option<&dyn ProviderNodeHandler> {
        Some(self)
    }

    async fn execute(
        &self,
        node: &PipelineNode,
        context: &Context,
        graph: &PipelineGraph,
    ) -> Result<Outcome> {
        // Compatibility path for consumers that still dispatch raw nodes (the
        // web executor). Preserve their historical fallback behavior until
        // they are explicitly migrated to ExecutionPlan.
        let provider = node
            .llm_provider
            .as_deref()
            .and_then(ProviderAlias::parse)
            .unwrap_or(ProviderAlias::CLAUDE);
        let resolved = ResolvedNode {
            node_id: node.id.clone(),
            kind: if node.shape == "diamond" || node.node_type.as_deref() == Some("conditional") {
                ResolvedNodeKind::Conditional { llm_backed: true }
            } else {
                ResolvedNodeKind::Task
            },
            handler: HandlerIdentity::Codergen,
            agent: Some(Selection {
                profile: provider.as_str().to_string(),
                model: node.llm_model.clone(),
                reasoning: None,
            }),
            invocation: Default::default(),
            fidelity: None,
            thread_id: None,
        };
        ProviderNodeHandler::execute_resolved(self, node, &resolved, context, graph).await
    }
}

impl CodergenHandler {
    async fn execute_with_controls(
        &self,
        node: &PipelineNode,
        resolved: &ResolvedNode,
        context: &Context,
        graph: &PipelineGraph,
        controls: CodergenExecutionControls<'_>,
    ) -> Result<Outcome> {
        let prompt = node.prompt.as_deref().unwrap_or("No prompt specified");
        let label = node.label.clone();
        let Some(agent) = &resolved.agent else {
            return Err(AttractorError::HandlerError {
                handler: "codergen".into(),
                node: node.id.clone(),
                message: "compiled codergen node has no agent profile".into(),
            });
        };
        // The handler's name ("Claude Code", "Codex CLI", ...), else the
        // profile's.
        let display_name = self
            .agents
            .display_name(&agent.profile)
            .map_or_else(|| agent.profile.clone(), str::to_owned);

        tracing::info!(
            node = %node.id,
            label = %label,
            provider = %display_name,
            "Executing codergen handler"
        );

        if controls.dry_run {
            tracing::info!(node = %node.id, provider = %display_name, "Dry run — skipping CLI execution");
            return Ok(Outcome {
                status: StageStatus::Success,
                preferred_label: None,
                suggested_next_ids: vec![],
                context_updates: {
                    let mut m = HashMap::new();
                    m.insert(
                        format!("{}.result", node.id),
                        serde_json::Value::String(format!("Dry run — prompt not sent: {}", prompt)),
                    );
                    m.insert(
                        format!("{}.completed", node.id),
                        serde_json::Value::Bool(true),
                    );
                    m.insert(
                        format!("{}.dry_run", node.id),
                        serde_json::Value::Bool(true),
                    );
                    m.insert(
                        format!("{}.provider", node.id),
                        serde_json::Value::String(display_name.clone()),
                    );
                    m
                },
                notes: format!("Dry run — {display_name} not invoked for: {label}"),
                failure_reason: None,
            });
        }

        // Build the full prompt with pipeline context
        let goal = &graph.goal;
        let mut full_prompt = String::new();

        if !goal.is_empty() {
            full_prompt.push_str(&format!("Pipeline goal: {}\n\n", goal));
        }

        // Inject relevant context from prior nodes
        let snapshot = context.snapshot().await;
        let context_keys: Vec<_> = snapshot
            .iter()
            .filter(|(k, _)| k.ends_with(".result") || k.ends_with(".output"))
            .collect();
        if !context_keys.is_empty() {
            full_prompt.push_str("Context from prior pipeline steps:\n");
            for (k, v) in &context_keys {
                if let serde_json::Value::String(s) = v {
                    full_prompt.push_str(&format!("- {}: {}\n", k, s));
                } else {
                    full_prompt.push_str(&format!("- {}: {}\n", k, v));
                }
            }
            full_prompt.push('\n');
        }

        full_prompt.push_str(&format!("Task ({}): {}", label, prompt));
        // The previous attempt was interrupted: tell the agent where its work is.
        if let Some(note) = &controls.resume_note {
            full_prompt.push_str(&format!("\n\n{note}"));
        }

        // If this is a conditional node, instruct the LLM to output a label
        if matches!(
            resolved.kind,
            ResolvedNodeKind::Conditional { llm_backed: true }
        ) {
            let edges = graph.outgoing_edges(&node.id);
            let labels: Vec<_> = edges.iter().filter_map(|e| e.label.as_deref()).collect();
            if !labels.is_empty() {
                full_prompt.push_str(&format!(
                    "\n\nYou MUST end your response with exactly one of these labels on its own line: {}",
                    labels.join(", ")
                ));
            }
        }

        let graph_model = match graph.attrs.get("model") {
            Some(AttributeValue::String(m)) => Some(m.as_str()),
            _ => None,
        };
        let profile_model = self
            .agents
            .profile(&agent.profile)
            .and_then(|p| p.model.as_deref());
        let selection = Selection {
            profile: agent.profile.clone(),
            model: select_model(agent.model.as_deref(), profile_model, graph_model),
            reasoning: agent.reasoning.clone(),
        };
        self.run_agent(node, resolved, graph, full_prompt, selection, controls)
            .await
    }

    /// An agent node: one invocation of its profile through `Agents`, with
    /// the model already resolved (see [`select_model`]).
    async fn run_agent(
        &self,
        node: &PipelineNode,
        resolved: &ResolvedNode,
        graph: &PipelineGraph,
        prompt: String,
        selection: Selection,
        controls: CodergenExecutionControls<'_>,
    ) -> Result<Outcome> {
        let profile_name = selection.profile.clone();
        let profile = self.agents.profile(&profile_name);
        let can_resume = profile.is_some_and(|p| p.can_resume());
        let session = choose_session(
            Fidelity::effective(resolved.fidelity, can_resume),
            controls.session.and_then(|s| s.prior),
            || uuid::Uuid::new_v4().to_string(),
        );
        let continued = session.is_continue();
        // One id names the Model Invocation everywhere: `LlmInvoked`, the
        // Transcript file and `PAS_INVOCATION_ID`.
        let invocation_id = attractor_journal::new_invocation_id();
        let run_dir = controls
            .run_dir
            .as_ref()
            .map(attractor_journal::RunDir::from_path);
        let transcript = run_dir.as_ref().map(|dir| dir.transcript(&invocation_id));
        let stderr = run_dir.as_ref().map(|dir| dir.stderr(&invocation_id));
        let prompt_file = run_dir.as_ref().map(|dir| dir.prompt(&invocation_id));
        // `LlmStarted` needs a Run folder: its paths are relative to it.
        let journal_starts = match (controls.events, &controls.run_dir) {
            (Some(events), Some(_)) => Some(StartedJournal { events }),
            _ => None,
        };
        let summarize =
            |stdout: &str| invocation_usage(&self.agents.transcript_usage(&profile_name, stdout));
        // Armed before the agent starts: if the engine's outer deadline drops
        // this future, `Drop` still emits `LlmInvoked` with status `timeout`.
        let invocation = match (controls.events, &controls.run_dir) {
            (Some(events), Some(run_dir)) => Some(LlmInvocation {
                events,
                summarize: &summarize,
                run_dir: run_dir.clone(),
                invocation_id: invocation_id.clone(),
                node_id: node.id.clone(),
                provider: profile_name.clone(),
                model_requested: selection.model.clone(),
                continued,
                agent_session_id: None,
                started: Instant::now(),
                emitted: false,
            }),
            _ => None,
        };
        let failure_invocation_id = invocation_id.clone();
        let result = self
            .agents
            .run(AgentRequest {
                selection,
                prompt,
                extra_args: controls
                    .claude
                    .as_ref()
                    .map(claude_settings_args)
                    .unwrap_or_default()
                    .into_iter()
                    .chain(claude_node_args(node))
                    .collect(),
                workdir: PathBuf::from(controls.workdir.as_deref().unwrap_or(".")),
                // None: the profile's timeout.
                timeout: node.timeout,
                record: Record {
                    run_id: controls.run_id.map(str::to_owned),
                    node_id: node.id.clone(),
                    attempt: controls.attempt,
                    invocation_id,
                },
                transcript,
                stderr,
                prompt_file,
                session,
                observer: journal_starts
                    .as_ref()
                    .map(|observer| observer as &dyn AgentObserver),
                cancel: controls.cancel.clone(),
            })
            .await;
        // The engine records the reported id for the node's thread.
        if let (Some(context), Some(id)) = (controls.session, &result.agent_session_id) {
            let _ = context.reported.set(id.clone());
        }
        if let Some(mut invocation) = invocation {
            invocation.agent_session_id = result.agent_session_id.clone();
            match result.status {
                AgentStatus::Failed(FailureClass::Launch) => invocation.disarm(),
                AgentStatus::Completed => {
                    invocation.finish(INVOKED_SUCCESS, invocation_usage(&result.usage))
                }
                // A stopped agent is reported as a dropped one always was.
                AgentStatus::Failed(FailureClass::Timeout) | AgentStatus::Cancelled => {
                    invocation.finish(INVOKED_TIMEOUT, invocation_usage(&result.usage))
                }
                AgentStatus::Failed(
                    FailureClass::Reported | FailureClass::Crash | FailureClass::NoResult,
                ) => invocation.finish(INVOKED_FAILED, invocation_usage(&result.usage)),
            }
        }
        tracing::info!(
            node = %node.id,
            status = ?result.status,
            model_actual = result.usage.model_actual.as_deref(),
            input_tokens = result.usage.input_tokens,
            output_tokens = result.usage.output_tokens,
            cost_usd = result.usage.cost_usd,
            "{} finished",
            self.agents
                .display_name(&profile_name)
                .unwrap_or(profile_name.as_str())
        );
        // The timeout the agent ran under: the node's, else the profile's.
        let timeout = node
            .timeout
            .or_else(|| profile.map(|p| p.timeout))
            .unwrap_or_default();
        agent_outcome(
            &result,
            node,
            resolved,
            graph,
            &AgentAttempt {
                agent: self
                    .agents
                    .display_name(&profile_name)
                    .unwrap_or(profile_name.as_str()),
                program: profile
                    .and_then(|p| p.command.first())
                    .map_or(profile_name.as_str(), String::as_str),
                reports_cost: self.agents.reports_cost(&profile_name).unwrap_or(true),
                attempt: controls.attempt,
                timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                run_dir: controls.run_dir.as_deref(),
                invocation_id: &failure_invocation_id,
            },
        )
    }
}

/// The agent selection the compiler gives a node with `provider`: the
/// built-in profile of that name (tests building `ResolvedNode`s).
#[cfg(test)]
pub(crate) fn test_agent<P: Into<Option<ProviderAlias>>>(
    provider: P,
    model: Option<String>,
) -> Option<Selection> {
    provider.into().map(|provider| Selection {
        profile: provider.as_str().to_string(),
        model,
        reasoning: None,
    })
}

/// The model an agent node asks for: the node's `llm_model`, else the
/// profile's `model`, else the graph's `model`. A graph-wide default never
/// overrides a profile's own model.
fn select_model(node: Option<&str>, profile: Option<&str>, graph: Option<&str>) -> Option<String> {
    node.or(profile).or(graph).map(str::to_owned)
}

/// What a provider answered, as the outcome needs it.
struct ProviderResult<'a> {
    text: &'a str,
    is_error: bool,
    cost_usd: Option<f64>,
    turns: Option<u32>,
}

/// The outcome of a provider that answered: Success, or Fail when it
/// reported an error, with the node's context updates and, for an
/// LLM-backed conditional, the label found in the text.
fn provider_outcome(
    node: &PipelineNode,
    resolved: &ResolvedNode,
    graph: &PipelineGraph,
    agent: &str,
    answer: ProviderResult<'_>,
) -> Outcome {
    // Determine status
    let status = if answer.is_error {
        StageStatus::Fail
    } else {
        StageStatus::Success
    };

    // Extract preferred_label from the response for conditional routing
    let preferred_label = if matches!(
        resolved.kind,
        ResolvedNodeKind::Conditional { llm_backed: true }
    ) {
        let edges = graph.outgoing_edges(&node.id);
        let labels: Vec<String> = edges.iter().filter_map(|e| e.label.clone()).collect();
        extract_label(answer.text, &labels)
    } else {
        None
    };

    // Build context updates
    let mut updates = HashMap::new();
    updates.insert(
        format!("{}.completed", node.id),
        serde_json::Value::Bool(true),
    );
    updates.insert(
        format!("{}.result", node.id),
        serde_json::Value::String(answer.text.to_string()),
    );
    updates.insert(
        format!("{}.provider", node.id),
        serde_json::Value::String(agent.to_string()),
    );
    if let Some(cost) = answer.cost_usd {
        updates.insert(format!("{}.cost_usd", node.id), serde_json::json!(cost));
    }
    if let Some(turns) = answer.turns {
        updates.insert(format!("{}.turns", node.id), serde_json::json!(turns));
    }
    if let Some(ref lbl) = preferred_label {
        updates.insert(
            format!("{}.label", node.id),
            serde_json::Value::String(lbl.clone()),
        );
    }

    Outcome {
        status,
        preferred_label,
        suggested_next_ids: vec![],
        context_updates: updates,
        notes: answer.text.to_string(),
        failure_reason: if status == StageStatus::Fail {
            Some(format!("{agent} returned an error"))
        } else {
            None
        },
    }
}

#[async_trait]
impl ProviderNodeHandler for CodergenHandler {
    async fn execute_resolved(
        &self,
        node: &PipelineNode,
        resolved: &ResolvedNode,
        context: &Context,
        graph: &PipelineGraph,
    ) -> Result<Outcome> {
        let snapshot = context.snapshot().await;
        let dry_run = snapshot
            .get("dry_run")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let workdir = snapshot
            .get("workdir")
            .and_then(|value| value.as_str())
            .map(str::to_owned);
        let claude = resolve_claude_cli_config(&snapshot, workdir.as_deref(), &node.id)?;
        self.execute_with_controls(
            node,
            resolved,
            context,
            graph,
            CodergenExecutionControls {
                dry_run,
                workdir,
                claude: Some(claude),
                run_dir: None,
                events: None,
                run_id: None,
                attempt: 1,
                cancel: CancellationToken::new(),
                resume_note: None,
                session: None,
            },
        )
        .await
    }

    async fn execute_configured(
        &self,
        node: &PipelineNode,
        resolved: &ResolvedNode,
        execution: HandlerExecutionContext<'_>,
        graph: &PipelineGraph,
    ) -> Result<Outcome> {
        let config = execution.config();
        self.execute_with_controls(
            node,
            resolved,
            execution.workflow(),
            graph,
            CodergenExecutionControls {
                dry_run: *config.dry_run().value(),
                workdir: Some(config.workdir().value().to_string_lossy().into_owned()),
                claude: None,
                run_dir: execution.run_dir().map(Path::to_path_buf),
                events: execution.events(),
                run_id: execution.run_id(),
                attempt: execution.attempt(),
                cancel: execution.cancel().clone(),
                resume_note: execution.resume_note().map(str::to_owned),
                session: execution.session(),
            },
        )
        .await
    }
}

const CLAUDE_MODE_KEY: &str = "codergen.claude.settings_mode";
const CLAUDE_SOURCES_KEY: &str = "codergen.claude.setting_sources";
const CLAUDE_SETTINGS_KEY: &str = "codergen.claude.settings";
const CLAUDE_TOOLS_KEY: &str = "codergen.claude.tools";
const CLAUDE_AGENTS_KEY: &str = "codergen.claude.agents";
const CLAUDE_PLUGIN_DIRS_KEY: &str = "codergen.claude.plugin_dirs";
const CLAUDE_MCP_CONFIG_KEY: &str = "codergen.claude.mcp_config";

fn resolve_claude_cli_config(
    snapshot: &HashMap<String, serde_json::Value>,
    workdir: Option<&str>,
    node_id: &str,
) -> Result<ClaudeCliConfig> {
    let mut cfg = ClaudeCliConfig::default();

    if let Some(dir) = workdir {
        match attractor_quality::resolve(Path::new(dir)) {
            Ok(resolved) => {
                if let Some(claude) = resolved.manifest.codergen.and_then(|c| c.claude) {
                    apply_manifest_claude_config(&mut cfg, claude, resolved.path.parent());
                }
            }
            Err(ResolutionError::NotFound) => {}
            Err(err) => {
                return Err(AttractorError::HandlerError {
                    handler: "codergen".into(),
                    node: node_id.into(),
                    message: format!(
                        "Failed to resolve pas.toml for Claude codergen config: {err}"
                    ),
                });
            }
        }
    }

    apply_context_claude_overrides(&mut cfg, snapshot, node_id)?;

    if cfg.settings_mode == ClaudeSettingsMode::Inherit && cfg.setting_sources.is_empty() {
        return Err(AttractorError::HandlerError {
            handler: "codergen".into(),
            node: node_id.into(),
            message: "Claude settings_mode=inherit requires explicit setting_sources".into(),
        });
    }
    if cfg.settings_mode == ClaudeSettingsMode::Inherit {
        tracing::warn!(
            node = %node_id,
            setting_sources = %cfg.setting_sources.join(","),
            "Claude codergen inherit mode may load personal hooks/settings"
        );
    }

    Ok(cfg)
}

fn apply_manifest_claude_config(
    cfg: &mut ClaudeCliConfig,
    claude: ClaudeCodergenConfig,
    manifest_dir: Option<&Path>,
) {
    if let Some(mode) = claude.settings_mode {
        cfg.settings_mode = mode;
    }
    if let Some(sources) = claude.setting_sources {
        cfg.setting_sources = sources
            .into_iter()
            .map(|source| source.as_str().to_string())
            .collect();
    }
    cfg.settings = claude.settings_json.or(cfg.settings.take());
    cfg.tools = claude.tools.or(cfg.tools.take());
    cfg.agents = claude.agents_json.or(cfg.agents.take());
    cfg.mcp_config = claude.mcp_config_json.or(cfg.mcp_config.take());
    if !claude.plugin_dirs.is_empty() {
        cfg.plugin_dirs = claude
            .plugin_dirs
            .into_iter()
            .map(|path| resolve_manifest_relative_path(path, manifest_dir))
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
    }
}

fn resolve_manifest_relative_path(path: PathBuf, manifest_dir: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        path
    } else if let Some(dir) = manifest_dir {
        dir.join(path)
    } else {
        path
    }
}

fn apply_context_claude_overrides(
    cfg: &mut ClaudeCliConfig,
    snapshot: &HashMap<String, serde_json::Value>,
    node_id: &str,
) -> Result<()> {
    if let Some(mode) = snapshot.get(CLAUDE_MODE_KEY).and_then(|v| v.as_str()) {
        cfg.settings_mode =
            ClaudeSettingsMode::from_str(mode).map_err(|err| AttractorError::HandlerError {
                handler: "codergen".into(),
                node: node_id.into(),
                message: err,
            })?;
    }
    if let Some(sources) = string_list_from_value(snapshot.get(CLAUDE_SOURCES_KEY)) {
        cfg.setting_sources = parse_setting_sources(sources, node_id)?;
    }
    if let Some(value) = snapshot.get(CLAUDE_SETTINGS_KEY).and_then(|v| v.as_str()) {
        cfg.settings = Some(value.to_string());
    }
    if let Some(value) = snapshot.get(CLAUDE_TOOLS_KEY).and_then(|v| v.as_str()) {
        cfg.tools = Some(value.to_string());
    }
    if let Some(value) = snapshot.get(CLAUDE_AGENTS_KEY).and_then(|v| v.as_str()) {
        cfg.agents = Some(value.to_string());
    }
    if let Some(plugin_dirs) = string_list_from_value(snapshot.get(CLAUDE_PLUGIN_DIRS_KEY)) {
        cfg.plugin_dirs = plugin_dirs;
    }
    if let Some(value) = snapshot.get(CLAUDE_MCP_CONFIG_KEY).and_then(|v| v.as_str()) {
        cfg.mcp_config = Some(value.to_string());
    }

    Ok(())
}

fn parse_setting_sources(sources: Vec<String>, node_id: &str) -> Result<Vec<String>> {
    sources
        .into_iter()
        .map(|source| {
            ClaudeSettingSource::from_str(&source)
                .map(|parsed| parsed.as_str().to_string())
                .map_err(|err| AttractorError::HandlerError {
                    handler: "codergen".into(),
                    node: node_id.into(),
                    message: err,
                })
        })
        .collect()
}

fn string_list_from_value(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    match value {
        Some(serde_json::Value::String(s)) => Some(
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
        ),
        Some(serde_json::Value::Array(values)) => Some(
            values
                .iter()
                .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                .collect(),
        ),
        _ => None,
    }
}

/// Scan the Claude response for one of the expected edge labels.
/// Checks the last few lines first (where we asked Claude to put it),
/// then falls back to scanning the full text.
fn extract_label(response: &str, labels: &[String]) -> Option<String> {
    let lines: Vec<&str> = response.lines().rev().take(5).collect();
    // Check last lines for an exact match
    for line in &lines {
        let trimmed = line.trim();
        for label in labels {
            if trimmed.eq_ignore_ascii_case(label) {
                return Some(label.clone());
            }
        }
    }
    // Fallback: search full response for label as a standalone word
    let upper = response.to_uppercase();
    for label in labels {
        if upper.contains(&label.to_uppercase()) {
            return Some(label.clone());
        }
    }
    None
}

#[cfg(test)]
#[path = "codergen_handler_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "codergen_regression_tests.rs"]
mod regression_tests;
