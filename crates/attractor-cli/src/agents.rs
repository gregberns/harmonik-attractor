//! The agent handlers and profiles `pas` runs agents with, and the caller's
//! environment they start from: read here, at the edge, once per run.

use std::collections::BTreeMap;
use std::sync::Arc;

use attractor_agent_handler::{builtin_profiles, AgentHandler, Agents, ConfigError};
use attractor_handler_claude_p::ClaudeP;

/// The handlers `pas` ships with.
fn handlers() -> Vec<Arc<dyn AgentHandler>> {
    vec![Arc::new(ClaudeP)]
}

/// This process's environment, keeping only variables whose name and value
/// are UTF-8 (`std::env::vars` would panic on the others).
fn parent_env() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

/// The registry Claude nodes run through: the built-in profiles, the
/// shipped handlers and this process's environment.
pub fn agents() -> Result<Arc<Agents>, ConfigError> {
    Agents::new(handlers(), builtin_profiles()?, parent_env(), false).map(Arc::new)
}
