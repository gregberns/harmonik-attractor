//! `Agents`: the profiles and handlers the engine runs agents through.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::env::child_env;
use crate::profile::{argv, Profile};
use crate::types::{AgentRequest, AgentResult, FailureClass, Invocation};

/// One way of driving an agent, named by its mechanism (e.g. `claude-p`).
#[async_trait]
pub trait AgentHandler: Send + Sync {
    /// The `mechanism` name profiles use.
    fn mechanism(&self) -> &'static str;
    /// Run one invocation to its end. Never returns an error.
    async fn run(&self, inv: Invocation<'_>) -> AgentResult;
}

/// Told about each invocation's end (the engine journals it).
pub trait AgentObserver: Send + Sync {
    fn finished(&self, result: &AgentResult);
}

/// A profile set or handler set `Agents::new` refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    DuplicateMechanism(String),
    DuplicateProfile(String),
    NoHandler { profile: String, mechanism: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateMechanism(m) => write!(f, "two agent handlers for mechanism {m}"),
            Self::DuplicateProfile(p) => write!(f, "agent profile {p} is defined twice"),
            Self::NoHandler { profile, mechanism } => write!(
                f,
                "agent profile {profile} uses mechanism {mechanism}, which has no handler"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The registry the engine calls: profiles by name, handlers by mechanism,
/// and the caller's environment.
pub struct Agents {
    handlers: BTreeMap<&'static str, Arc<dyn AgentHandler>>,
    profiles: BTreeMap<String, Profile>,
    parent_env: BTreeMap<String, String>,
}

impl fmt::Debug for Agents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Agents")
            .field("handlers", &self.handlers.keys().collect::<Vec<_>>())
            .field("profiles", &self.profiles.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Agents {
    /// `parent_env` is the caller's environment, read once by the caller at
    /// its edge; every child environment is derived from it (see
    /// [`crate::child_env`]). It is a `String` map, so a variable whose name
    /// or value is not UTF-8 is not passed to agents.
    pub fn new(
        handlers: Vec<Arc<dyn AgentHandler>>,
        profiles: Vec<Profile>,
        parent_env: BTreeMap<String, String>,
    ) -> Result<Self, ConfigError> {
        let mut by_mechanism = BTreeMap::new();
        for handler in handlers {
            let mechanism = handler.mechanism();
            if by_mechanism.insert(mechanism, handler).is_some() {
                return Err(ConfigError::DuplicateMechanism(mechanism.into()));
            }
        }
        let mut by_name = BTreeMap::new();
        for profile in profiles {
            if !by_mechanism.contains_key(profile.mechanism.as_str()) {
                return Err(ConfigError::NoHandler {
                    profile: profile.name,
                    mechanism: profile.mechanism,
                });
            }
            let name = profile.name.clone();
            if by_name.insert(name.clone(), profile).is_some() {
                return Err(ConfigError::DuplicateProfile(name));
            }
        }
        Ok(Self {
            handlers: by_mechanism,
            profiles: by_name,
            parent_env,
        })
    }

    /// No handlers and no profiles: every `run` fails as `Launch`.
    pub fn empty() -> Self {
        Self {
            handlers: BTreeMap::new(),
            profiles: BTreeMap::new(),
            parent_env: BTreeMap::new(),
        }
    }

    /// Run one invocation through the selected profile's handler. Never an
    /// error: an unknown profile is `Failed(Launch)`. The request's observer,
    /// if any, is told the returned value exactly once.
    pub async fn run(&self, req: AgentRequest<'_>) -> AgentResult {
        let result = match self.resolve(&req.selection.profile) {
            Some((profile, handler)) => {
                let inv = Invocation {
                    invocation_id: &req.record.invocation_id,
                    argv: argv(profile, &req),
                    env: child_env(&self.parent_env, &req.record),
                    prompt: &req.prompt,
                    workdir: &req.workdir,
                    timeout: req.timeout,
                    transcript: req.transcript.as_deref(),
                };
                handler.run(inv).await
            }
            None => AgentResult::failed(
                req.record.invocation_id.clone(),
                FailureClass::Launch,
                format!("unknown agent profile {}", req.selection.profile),
            ),
        };
        if let Some(observer) = req.observer {
            observer.finished(&result);
        }
        result
    }

    fn resolve(&self, name: &str) -> Option<(&Profile, &Arc<dyn AgentHandler>)> {
        let profile = self.profiles.get(name)?;
        let handler = self.handlers.get(profile.mechanism.as_str())?;
        Some((profile, handler))
    }
}
