//! The agent handlers and profiles `pas` runs agents with, and the caller's
//! environment they start from: read here, at the edge, once per run.

use std::collections::BTreeMap;
use std::sync::Arc;

use attractor_agent_handler::{AgentHandler, Agents, ConfigError, Profile};
use attractor_handler_claude_p::ClaudeP;
use attractor_handler_codex_exec::CodexExec;
use attractor_handler_gemini::Gemini;

/// The handlers `pas` ships with.
fn handlers() -> Vec<Arc<dyn AgentHandler>> {
    vec![
        Arc::new(ClaudeP),
        Arc::new(CodexExec),
        Arc::new(Gemini::default()),
    ]
}

/// This process's environment, keeping only variables whose name and value
/// are UTF-8 (`std::env::vars` would panic on the others).
fn parent_env() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

/// The registry agent nodes run through: the run's `profiles` (see
/// [`attractor_pipeline::agent_profiles`]), the shipped handlers and this
/// process's environment. `test_only` profiles run only when
/// `allow_test_agents` is set.
pub fn agents(profiles: Vec<Profile>, allow_test_agents: bool) -> Result<Arc<Agents>, ConfigError> {
    Agents::new(handlers(), profiles, parent_env(), allow_test_agents).map(Arc::new)
}
