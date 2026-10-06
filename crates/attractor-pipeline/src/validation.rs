//! Pipeline validation: lint rules and diagnostics.
//!
//! Performs canonical semantic compilation followed by nine structural checks
//! for a [`PipelineGraph`]. Call [`validate`] for advisory diagnostics or
//! [`validate_or_raise`] to fail on the first `Error`-severity issue.

use std::collections::{HashSet, VecDeque};

use attractor_dot::AttributeValue;

use crate::graph::PipelineGraph;
use crate::parse_condition;
use crate::{ExecutionPlan, Fidelity, ProviderAlias, SemanticDiagnostic, SemanticDiagnosticKind};

// ---------------------------------------------------------------------------
// Diagnostic types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub rule: String,
    pub severity: Severity,
    pub message: String,
    pub node_id: Option<String>,
    pub edge: Option<(String, String)>,
    pub fix: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

// ---------------------------------------------------------------------------
// LintRule trait
// ---------------------------------------------------------------------------

pub trait LintRule: Send + Sync {
    fn name(&self) -> &str;
    fn apply(&self, graph: &PipelineGraph) -> Vec<Diagnostic>;
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

struct EdgeTargetExistsRule;
impl LintRule for EdgeTargetExistsRule {
    fn name(&self) -> &str {
        "edge_target_exists"
    }
    fn apply(&self, graph: &PipelineGraph) -> Vec<Diagnostic> {
        graph
            .all_edges()
            .iter()
            .filter(|e| graph.node(&e.to).is_none())
            .map(|e| Diagnostic {
                rule: self.name().into(),
                severity: Severity::Error,
                message: format!(
                    "Edge {} -> {} references non-existent target '{}'",
                    e.from, e.to, e.to
                ),
                node_id: None,
                edge: Some((e.from.clone(), e.to.clone())),
                fix: Some(format!("Add node '{}' or fix the edge target", e.to)),
            })
            .collect()
    }
}

struct ConditionSyntaxRule;
impl LintRule for ConditionSyntaxRule {
    fn name(&self) -> &str {
        "condition_syntax"
    }
    fn apply(&self, graph: &PipelineGraph) -> Vec<Diagnostic> {
        graph
            .all_edges()
            .iter()
            .filter_map(|e| {
                let cond = e.condition.as_deref()?;
                match parse_condition(cond) {
                    Ok(_) => None,
                    Err(err) => Some(Diagnostic {
                        rule: self.name().into(),
                        severity: Severity::Error,
                        message: format!(
                            "Edge {} -> {} has invalid condition '{}': {}",
                            e.from, e.to, cond, err
                        ),
                        node_id: None,
                        edge: Some((e.from.clone(), e.to.clone())),
                        fix: Some("Fix the condition expression syntax".into()),
                    }),
                }
            })
            .collect()
    }
}

struct RetryTargetExistsRule;
impl LintRule for RetryTargetExistsRule {
    fn name(&self) -> &str {
        "retry_target_exists"
    }
    fn apply(&self, graph: &PipelineGraph) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        for node in graph.all_nodes() {
            if let Some(ref target) = node.retry_target {
                if graph.node(target).is_none() {
                    diags.push(Diagnostic {
                        rule: self.name().into(),
                        severity: Severity::Warning,
                        message: format!(
                            "Node '{}' has retry_target '{}' which does not exist",
                            node.id, target
                        ),
                        node_id: Some(node.id.clone()),
                        edge: None,
                        fix: Some(format!("Add node '{target}' or fix retry_target")),
                    });
                }
            }
            if let Some(ref target) = node.fallback_retry_target {
                if graph.node(target).is_none() {
                    diags.push(Diagnostic {
                        rule: self.name().into(),
                        severity: Severity::Warning,
                        message: format!(
                            "Node '{}' has fallback_retry_target '{}' which does not exist",
                            node.id, target
                        ),
                        node_id: Some(node.id.clone()),
                        edge: None,
                        fix: Some(format!("Add node '{target}' or fix fallback_retry_target")),
                    });
                }
            }
        }
        diags
    }
}

struct GoalGateHasRetryRule;
impl LintRule for GoalGateHasRetryRule {
    fn name(&self) -> &str {
        "goal_gate_has_retry"
    }
    fn apply(&self, graph: &PipelineGraph) -> Vec<Diagnostic> {
        graph
            .all_nodes()
            .filter(|n| n.goal_gate && n.retry_target.is_none())
            .map(|n| Diagnostic {
                rule: self.name().into(),
                severity: Severity::Warning,
                message: format!("Node '{}' has goal_gate=true but no retry_target", n.id),
                node_id: Some(n.id.clone()),
                edge: None,
                fix: Some("Add a retry_target attribute so the goal gate can retry".into()),
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run all built-in lint rules and return collected diagnostics.
///
/// Unlike [`validate_plan`], this also checks the environment the default
/// handlers need (see [`validate_beads_available`]).
pub fn validate(graph: &PipelineGraph) -> Vec<Diagnostic> {
    match ExecutionPlan::compile(graph.clone()) {
        Ok(plan) => {
            let mut diagnostics = validate_plan(&plan);
            diagnostics.extend(validate_beads_available(&plan));
            diagnostics
        }
        Err(error) => {
            let mut diagnostics = validate_nonsemantic_structure(graph);
            if let Ok(compilation) =
                ExecutionPlan::compile_for_generation(graph.clone(), ProviderAlias::CLAUDE)
            {
                diagnostics.extend(validate_plan_structure(&compilation.plan));
                diagnostics.extend(validate_beads_available(&compilation.plan));
            }
            diagnostics.extend(error.diagnostics.into_iter().map(semantic_diagnostic));
            diagnostics
        }
    }
}

/// Run structural lint rules against an already compiled semantic plan.
pub fn validate_plan(plan: &ExecutionPlan) -> Vec<Diagnostic> {
    let mut diagnostics = validate_nonsemantic_structure(plan.graph());
    diagnostics.extend(validate_plan_structure(plan));
    diagnostics
}

/// One `agent_profile` error per agent node whose selection `agents` cannot
/// run: an unknown profile, a reasoning level on a profile without
/// `reasoning_args`, or a `test_only` profile when test agents are not
/// allowed. Pure; `pas run` and `pas validate` call it after compiling.
pub fn check_agents(
    plan: &ExecutionPlan,
    agents: &attractor_agent_handler::Agents,
) -> Vec<Diagnostic> {
    use attractor_agent_handler::ConfigError;
    let mut nodes = plan
        .all_nodes()
        .filter_map(|node| Some((node.node_id.as_str(), node.agent.as_ref()?)))
        .collect::<Vec<_>>();
    nodes.sort_unstable_by_key(|(id, _)| *id);
    let profile_errors = nodes.iter().filter_map(|(node_id, selection)| {
        let error = agents.check(selection).err()?;
        Some((*node_id, selection, error))
    });
    let mut diagnostics: Vec<Diagnostic> = profile_errors
        .map(|(node_id, selection, error)| {
            let fix = match &error {
                ConfigError::UnknownProfile(_) => format!(
                    "Use a defined profile, or define [agents.{}] in pas.toml",
                    selection.profile
                ),
                ConfigError::NoReasoningArgs { .. } => {
                    "Remove reasoning_effort, or give the profile reasoning_args".into()
                }
                ConfigError::TestOnly { .. } => {
                    "Pass --allow-test-agents (test profiles only)".into()
                }
                _ => "Fix the agent profile in pas.toml".into(),
            };
            Diagnostic {
                rule: "agent_profile".into(),
                severity: Severity::Error,
                message: format!("Node '{node_id}': {error}"),
                node_id: Some(node_id.to_string()),
                edge: None,
                fix: Some(fix),
            }
        })
        .collect();
    // `fidelity="full"` needs a profile that can continue a session.
    for session in agent_sessions(plan, agents) {
        if session.explicit && session.fidelity == Fidelity::Full && !session.can_resume {
            diagnostics.push(Diagnostic {
                rule: "agent_session".into(),
                severity: Severity::Error,
                message: format!(
                    "Node '{}': fidelity=\"full\" needs a profile that can resume, and profile '{}' can't",
                    session.node_id, session.profile
                ),
                node_id: Some(session.node_id.clone()),
                edge: None,
                fix: Some(
                    "Use fidelity=\"fresh\", or give the profile resume_args or resume_command"
                        .into(),
                ),
            });
        }
    }
    // A thread's session belongs to one agent: nodes with different profiles
    // can't share a thread key, or one would resume the other's session.
    let mut threads: std::collections::BTreeMap<&str, Vec<&AgentSession>> =
        std::collections::BTreeMap::new();
    let sessions = agent_sessions(plan, agents);
    for session in &sessions {
        threads
            .entry(session.thread_key.as_str())
            .or_default()
            .push(session);
    }
    for (thread, members) in threads {
        let mut profiles: Vec<&str> = members.iter().map(|s| s.profile.as_str()).collect();
        profiles.sort_unstable();
        profiles.dedup();
        if profiles.len() < 2 {
            continue;
        }
        let nodes: Vec<String> = members.iter().map(|s| format!("'{}'", s.node_id)).collect();
        diagnostics.push(Diagnostic {
            rule: "agent_session".into(),
            severity: Severity::Error,
            message: format!(
                "thread '{thread}' is shared by nodes {} with different agent profiles ({})",
                nodes.join(", "),
                profiles.join(", ")
            ),
            node_id: members.first().map(|s| s.node_id.clone()),
            edge: None,
            fix: Some("Use one profile per thread_id, or separate thread_ids".into()),
        });
    }
    // `allowed_tools` and `max_budget_usd` reach the agent as `claude-p`
    // flags, so only a `claude-p` profile takes them.
    for (node_id, selection) in &nodes {
        let mechanism = agents
            .profile(&selection.profile)
            .map(|profile| profile.mechanism.as_str());
        if mechanism.is_none_or(|m| m == CLAUDE_P_MECHANISM) {
            continue;
        }
        let Some(source) = plan.source_node(node_id) else {
            continue;
        };
        for attribute in ["allowed_tools", "max_budget_usd"] {
            if source.raw_attrs.contains_key(attribute) {
                diagnostics.push(Diagnostic {
                    rule: "unsupported_execution_capability".into(),
                    severity: Severity::Error,
                    message: format!(
                        "Node '{node_id}' uses unsupported execution capability '{attribute}'"
                    ),
                    node_id: Some(node_id.to_string()),
                    edge: None,
                    fix: Some(format!(
                        "Remove '{attribute}' or use it on a Claude-backed codergen node"
                    )),
                });
            }
        }
    }
    diagnostics
}

/// How an agent node uses sessions, as `pas validate` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSession {
    pub node_id: String,
    pub profile: String,
    /// The fidelity the node runs with.
    pub fidelity: Fidelity,
    /// Whether the node sets `fidelity` itself (else it is the default).
    pub explicit: bool,
    pub can_resume: bool,
    /// Its thread key: `thread_id`, else the node id.
    pub thread_key: String,
}

/// Every agent node's session use, by node id. Pure. An unknown profile
/// counts as one that can't resume (`check_agents` reports it).
pub fn agent_sessions(
    plan: &ExecutionPlan,
    agents: &attractor_agent_handler::Agents,
) -> Vec<AgentSession> {
    let mut sessions: Vec<AgentSession> = plan
        .all_nodes()
        .filter_map(|node| {
            let profile = node.profile()?.to_string();
            let can_resume = agents.can_resume(&profile).unwrap_or(false);
            Some(AgentSession {
                node_id: node.node_id.clone(),
                fidelity: Fidelity::effective(node.fidelity, can_resume),
                explicit: node.fidelity.is_some(),
                can_resume,
                thread_key: node.thread_key().to_string(),
                profile,
            })
        })
        .collect();
    sessions.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    sessions
}

/// The handler whose argv takes the node flags `allowed_tools` and
/// `max_budget_usd` (`extra_args`).
const CLAUDE_P_MECHANISM: &str = "claude-p";

/// One `beads_available` error per `beads.select` / `beads.close` node when
/// `bd` is not on `PATH`, so a Run fails before it starts rather than at the
/// first Beads stage. Pipelines without Beads nodes never look at `PATH`.
///
/// Kept out of [`validate_plan`] because an engine may register Beads
/// handlers that run a `bd` from somewhere other than `PATH`.
pub fn validate_beads_available(plan: &ExecutionPlan) -> Vec<Diagnostic> {
    if beads_nodes(plan).is_empty() {
        return Vec::new();
    }
    let found = crate::beads_adapter::bd_on_path(std::env::var_os("PATH").as_deref());
    beads_unavailable(plan, found)
}

fn beads_nodes(plan: &ExecutionPlan) -> Vec<(&str, &str)> {
    let mut nodes = plan
        .all_nodes()
        .map(|node| (node.node_id.as_str(), node.handler.as_str()))
        .filter(|(_, handler)| {
            [
                crate::handlers::beads::SELECT_HANDLER,
                crate::handlers::beads::CLOSE_HANDLER,
            ]
            .contains(handler)
        })
        .collect::<Vec<_>>();
    nodes.sort_unstable();
    nodes
}

fn beads_unavailable(plan: &ExecutionPlan, bd_found: bool) -> Vec<Diagnostic> {
    if bd_found {
        return Vec::new();
    }
    beads_nodes(plan)
        .into_iter()
        .map(|(node_id, handler)| Diagnostic {
            rule: "beads_available".into(),
            severity: Severity::Error,
            message: format!("Node '{node_id}' uses handler '{handler}' but bd is not on PATH"),
            node_id: Some(node_id.to_string()),
            edge: None,
            fix: Some("Install beads (bd) and make sure it is on PATH".into()),
        })
        .collect()
}

fn validate_nonsemantic_structure(graph: &PipelineGraph) -> Vec<Diagnostic> {
    let rules: Vec<Box<dyn LintRule>> = vec![
        Box::new(EdgeTargetExistsRule),
        Box::new(ConditionSyntaxRule),
        Box::new(RetryTargetExistsRule),
        Box::new(GoalGateHasRetryRule),
    ];

    let mut diagnostics = Vec::new();
    for rule in &rules {
        diagnostics.extend(rule.apply(graph));
    }
    diagnostics
}

fn validate_plan_structure(plan: &ExecutionPlan) -> Vec<Diagnostic> {
    let graph = plan.graph();
    let mut diagnostics = Vec::new();

    let mut retry_targets = graph
        .all_nodes()
        .flat_map(|node| {
            [
                ("retry_target", node.retry_target.as_deref()),
                (
                    "fallback_retry_target",
                    node.fallback_retry_target.as_deref(),
                ),
            ]
            .into_iter()
            .filter_map(move |(attribute, target)| {
                target.map(|target| {
                    (
                        node.id.clone(),
                        Some(node.id.clone()),
                        attribute,
                        target.to_string(),
                    )
                })
            })
        })
        .collect::<Vec<_>>();
    for attribute in ["retry_target", "fallback_retry_target"] {
        if let Some(AttributeValue::String(target)) = graph.attrs.get(attribute) {
            retry_targets.push(("graph".to_string(), None, attribute, target.clone()));
        }
    }
    retry_targets
        .sort_by(|left, right| (&left.0, left.2, &left.3).cmp(&(&right.0, right.2, &right.3)));
    diagnostics.extend(
        retry_targets
            .into_iter()
            .filter(|(_, _, _, target)| plan.is_exit(target))
            .map(|(owner, node_id, attribute, target)| Diagnostic {
                rule: "retry_target_not_terminal".into(),
                severity: Severity::Error,
                message: format!(
                    "{owner} {attribute} '{target}' resolves to a terminal node and cannot change an unsatisfied goal gate"
                ),
                node_id,
                edge: None,
                fix: Some(format!(
                    "Point {attribute} at a non-terminal node that can change the goal-gate outcome"
                )),
            }),
    );

    if graph
        .all_edges()
        .iter()
        .any(|edge| edge.to == plan.start_id())
    {
        diagnostics.push(Diagnostic {
            rule: "start_no_incoming".into(),
            severity: Severity::Error,
            message: format!("Start node '{}' has incoming edges", plan.start_id()),
            node_id: Some(plan.start_id().to_string()),
            edge: None,
            fix: Some("Remove edges pointing to the start node".into()),
        });
    }

    for exit_id in plan.exit_ids() {
        if !plan.outgoing_edges(exit_id).is_empty() {
            diagnostics.push(Diagnostic {
                rule: "exit_no_outgoing".into(),
                severity: Severity::Error,
                message: format!("Terminal node '{exit_id}' has outgoing edges"),
                node_id: Some(exit_id.clone()),
                edge: None,
                fix: Some(format!("Remove outgoing edges from '{exit_id}'")),
            });
        }
    }

    let mut visited = HashSet::new();
    let mut queue = VecDeque::from([plan.start_id().to_string()]);
    while let Some(current) = queue.pop_front() {
        if !visited.insert(current.clone()) {
            continue;
        }
        queue.extend(
            plan.outgoing_edges(&current)
                .iter()
                .map(|edge| edge.to.clone()),
        );
    }
    let mut unreachable = plan
        .all_nodes()
        .map(|node| node.node_id.as_str())
        .filter(|node_id| !visited.contains(*node_id))
        .collect::<Vec<_>>();
    unreachable.sort_unstable();
    diagnostics.extend(unreachable.into_iter().map(|node_id| Diagnostic {
        rule: "reachability".into(),
        severity: Severity::Error,
        message: format!("Node '{node_id}' is not reachable from the start node"),
        node_id: Some(node_id.to_string()),
        edge: None,
        fix: Some(format!("Add an edge leading to '{node_id}' or remove it")),
    }));

    let mut llm_nodes = plan
        .all_nodes()
        .filter(|node| node.handler == crate::HandlerIdentity::Codergen)
        .filter_map(|node| plan.source_node(&node.node_id))
        .filter(|node| node.prompt.is_none() && node.label == node.id)
        .collect::<Vec<_>>();
    llm_nodes.sort_by(|left, right| left.id.cmp(&right.id));
    diagnostics.extend(llm_nodes.into_iter().map(|node| Diagnostic {
        rule: "prompt_on_llm_nodes".into(),
        severity: Severity::Warning,
        message: format!(
            "Node '{}' (handler=codergen) has no prompt and label matches id",
            node.id
        ),
        node_id: Some(node.id.clone()),
        edge: None,
        fix: Some("Add a prompt or a descriptive label attribute".into()),
    }));

    diagnostics
}

fn semantic_diagnostic(diagnostic: SemanticDiagnostic) -> Diagnostic {
    let rule = match diagnostic.kind {
        SemanticDiagnosticKind::InvalidAttributeType => "attribute_type",
        SemanticDiagnosticKind::InvalidAttributeValue => "attribute_value",
        SemanticDiagnosticKind::MissingProvider => "provider_required",
        SemanticDiagnosticKind::MissingAttribute => "attribute_required",
        SemanticDiagnosticKind::UnknownProvider => "provider_valid",
        SemanticDiagnosticKind::MissingStart | SemanticDiagnosticKind::MultipleStarts => {
            "start_node"
        }
        SemanticDiagnosticKind::MissingExit | SemanticDiagnosticKind::MultipleExits => {
            "terminal_node"
        }
        SemanticDiagnosticKind::ConflictingRoleSignals => "semantic_conflict",
        SemanticDiagnosticKind::ConflictingAttributeAliases => "attribute_alias_conflict",
        SemanticDiagnosticKind::UnknownShape | SemanticDiagnosticKind::UnknownHandler => {
            "semantic_unknown"
        }
        SemanticDiagnosticKind::HandlerCapabilityMismatch => "handler_registry",
        SemanticDiagnosticKind::UnsupportedExecutionTopology => "unsupported_execution_topology",
        SemanticDiagnosticKind::UnsupportedExecutionCapability => {
            "unsupported_execution_capability"
        }
        SemanticDiagnosticKind::TransformError => "transform",
    };
    Diagnostic {
        rule: rule.into(),
        severity: Severity::Error,
        message: diagnostic.message,
        node_id: diagnostic.node_id,
        edge: None,
        fix: Some(diagnostic.fix),
    }
}

/// Run all lint rules; return `Err` if any `Error`-severity diagnostic found.
pub fn validate_or_raise(graph: &PipelineGraph) -> attractor_types::Result<Vec<Diagnostic>> {
    let diagnostics = validate(graph);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    if !errors.is_empty() {
        let messages: Vec<_> = errors.iter().map(|d| d.message.clone()).collect();
        return Err(attractor_types::AttractorError::ValidationError(
            messages.join("; "),
        ));
    }
    Ok(diagnostics)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "validation_tests.rs"]
mod tests;
