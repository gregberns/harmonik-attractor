//! The agent handler interface (design §1): the [`AgentHandler`] trait a
//! mechanism such as `claude-p` implements, the request and result types,
//! agent profiles, and [`Agents`], the registry the engine calls.
//!
//! This crate depends on no other PAS crate. It decides; it never spawns.
//! Spawning a local process lives in `attractor-agent-process`, and each
//! mechanism lives in its own handler crate.

mod env;
mod profile;
mod registry;
mod types;

pub use env::{child_env, STRIPPED_ENV};
pub use profile::{argv, builtin_profiles, fill, Profile};
pub use registry::{AgentHandler, AgentObserver, Agents, ConfigError};
pub use types::{
    AgentRequest, AgentResult, AgentStatus, ExitInfo, FailureClass, Invocation, Record, Selection,
    Usage,
};
