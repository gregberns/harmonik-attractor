//! `Agents`: the profiles and handlers the engine runs agents through.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::env::child_env;
use crate::profile::{argv, selected_model, Profile};
use crate::prompt_file::{prompt_file_text, write_prompt_file};
use crate::types::{
    AgentRequest, AgentResult, FailureClass, Invocation, Selection, Spawned, Started, Usage,
};

/// How long past `timeout + kill_grace` `Agents` waits for a handler that
/// ignores its timeout before giving up on it.
pub const HARD_DEADLINE_MARGIN: Duration = Duration::from_secs(5);

/// One way of driving an agent, named by its mechanism (e.g. `claude-p`).
#[async_trait]
pub trait AgentHandler: Send + Sync {
    /// The `mechanism` name profiles use.
    fn mechanism(&self) -> &'static str;
    /// The complete argv the handler starts the agent with: `inv.argv` plus
    /// the handler's own flags. The prompt file records it, so a handler
    /// must spawn with exactly this.
    fn argv(&self, inv: &Invocation<'_>) -> Vec<String> {
        inv.argv.clone()
    }
    /// Run one invocation to its end. Never returns an error.
    async fn run(&self, inv: Invocation<'_>) -> AgentResult;
    /// Usage read from a transcript (the agent's stdout so far). Never
    /// fails: missing or unreadable data is `None`. For an invocation whose
    /// future was dropped before `run` returned.
    fn transcript_usage(&self, transcript: &str) -> Usage;
}

/// Told about each invocation's processes and its end (the engine journals
/// them).
pub trait AgentObserver: Send + Sync {
    /// Called once per process, synchronously, after the process exists and
    /// its transcript and stderr files exist, and before any of its stdout is
    /// written to the transcript. It must stay synchronous: that is what
    /// makes `LlmStarted` come before the agent's first output.
    fn started(&self, started: &Started);
    /// Called once per invocation, with the value `Agents::run` returns.
    fn finished(&self, result: &AgentResult);
}

/// Agent configuration PAS refuses: a profile set or handler set, or a
/// selection that cannot run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    DuplicateMechanism(String),
    DuplicateProfile(String),
    NoHandler {
        profile: String,
        mechanism: String,
    },
    /// An agents file that is not valid TOML of the expected shape.
    Parse(String),
    UnknownProfile(String),
    UnknownParent {
        profile: String,
        parent: String,
    },
    InheritCycle {
        profile: String,
    },
    MissingField {
        profile: String,
        field: &'static str,
        /// A `pas.toml` profile replaced the built-in one of this name.
        replaced_builtin: bool,
    },
    BadDuration {
        profile: String,
        field: &'static str,
        value: String,
    },
    /// A reasoning level on a profile with no `reasoning_args`.
    NoReasoningArgs {
        profile: String,
    },
    /// A `test_only` profile in a run that does not allow test agents.
    TestOnly {
        profile: String,
    },
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
            Self::Parse(message) => write!(f, "invalid agent profiles: {message}"),
            Self::UnknownProfile(p) => write!(f, "unknown agent profile {p}"),
            Self::UnknownParent { profile, parent } => write!(
                f,
                "agent profile {profile} inherits from {parent}, which is not defined"
            ),
            Self::InheritCycle { profile } => {
                write!(f, "agent profile {profile} inherits from itself")
            }
            Self::MissingField {
                profile,
                field,
                replaced_builtin,
            } => {
                write!(f, "agent profile {profile} has no {field}")?;
                if *replaced_builtin {
                    write!(
                        f,
                        " (a pas.toml profile replaces the built-in {profile} profile whole: \
                         copy its fields, or define a new profile with \
                         inherit_from = \"{profile}\")"
                    )?;
                }
                Ok(())
            }
            Self::BadDuration {
                profile,
                field,
                value,
            } => write!(
                f,
                "agent profile {profile}: {field} {value:?} is not a duration such as \"30s\" or \"10m\""
            ),
            Self::NoReasoningArgs { profile } => write!(
                f,
                "agent profile {profile} has no reasoning_args, so it takes no reasoning level"
            ),
            Self::TestOnly { profile } => write!(
                f,
                "agent profile {profile} is test_only; pass --allow-test-agents to use it"
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
    allow_test_agents: bool,
    hard_deadline_margin: Duration,
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
    /// or value is not UTF-8 is not passed to agents. A `test_only`
    /// profile runs only when `allow_test_agents` is set.
    pub fn new(
        handlers: Vec<Arc<dyn AgentHandler>>,
        profiles: Vec<Profile>,
        parent_env: BTreeMap<String, String>,
        allow_test_agents: bool,
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
            allow_test_agents,
            hard_deadline_margin: HARD_DEADLINE_MARGIN,
        })
    }

    /// The same registry with another hard-deadline margin (tests).
    pub fn with_hard_deadline_margin(self, margin: Duration) -> Self {
        Self {
            hard_deadline_margin: margin,
            ..self
        }
    }

    /// The longest `kill_grace` of any profile: how long a stopped agent
    /// may take to exit.
    pub fn max_kill_grace(&self) -> Duration {
        self.profiles
            .values()
            .map(|p| p.kill_grace)
            .max()
            .unwrap_or_default()
    }

    /// How long a stopped Run should wait for its agents before giving up:
    /// the longest `kill_grace` plus the hard-deadline margin.
    pub fn stop_grace(&self) -> Duration {
        self.max_kill_grace()
            .saturating_add(self.hard_deadline_margin)
    }

    /// [`Agents::stop_grace`] over only the named profiles (the ones a
    /// pipeline runs); unknown names are skipped, and no profile leaves the
    /// margin alone.
    pub fn stop_grace_of<'a>(&self, profiles: impl IntoIterator<Item = &'a str>) -> Duration {
        profiles
            .into_iter()
            .filter_map(|name| self.profiles.get(name))
            .map(|p| p.kill_grace)
            .max()
            .unwrap_or_default()
            .saturating_add(self.hard_deadline_margin)
    }

    /// No handlers and no profiles: every `run` fails as `Launch`.
    pub fn empty() -> Self {
        Self {
            handlers: BTreeMap::new(),
            profiles: BTreeMap::new(),
            parent_env: BTreeMap::new(),
            allow_test_agents: false,
            hard_deadline_margin: HARD_DEADLINE_MARGIN,
        }
    }

    /// The profile named `name`, if any.
    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }

    /// Whether `selection` can run: its profile exists, takes a reasoning
    /// level if one is selected (by the node or the profile), and is not
    /// `test_only` unless test agents are allowed.
    pub fn check(&self, selection: &Selection) -> Result<(), ConfigError> {
        let profile = self
            .profiles
            .get(&selection.profile)
            .ok_or_else(|| ConfigError::UnknownProfile(selection.profile.clone()))?;
        let reasoning = selection.reasoning.is_some() || profile.reasoning.is_some();
        if reasoning && profile.reasoning_args.is_empty() {
            return Err(ConfigError::NoReasoningArgs {
                profile: profile.name.clone(),
            });
        }
        if profile.test_only && !self.allow_test_agents {
            return Err(ConfigError::TestOnly {
                profile: profile.name.clone(),
            });
        }
        Ok(())
    }

    /// Run one invocation through the selected profile's handler. Never an
    /// error: a selection [`Agents::check`] refuses is `Failed(Launch)`, and
    /// starts nothing. The request's observer,
    /// if any, is told each process start and the returned value exactly
    /// once.
    ///
    /// The timeout is the request's, else the profile's. A handler that has
    /// not returned by `timeout + kill_grace` plus the
    /// hard-deadline margin is dropped (its process guard kills any child)
    /// and the result is `Failed(Timeout)`.
    pub async fn run(&self, req: AgentRequest<'_>) -> AgentResult {
        let resolved = self.check(&req.selection).and_then(|()| {
            self.resolve(&req.selection.profile)
                .ok_or_else(|| ConfigError::UnknownProfile(req.selection.profile.clone()))
        });
        let result = match resolved {
            Ok((profile, handler)) => {
                let timeout = req.timeout.unwrap_or(profile.timeout);
                let spawns = AtomicU32::new(0);
                let spawned = |spawn: Spawned| {
                    let index = spawns.fetch_add(1, Ordering::SeqCst).saturating_add(1);
                    if let Some(observer) = req.observer {
                        observer.started(&started(&req, profile, index, spawn));
                    }
                };
                let inv = Invocation {
                    invocation_id: &req.record.invocation_id,
                    argv: argv(profile, &req),
                    env: child_env(&self.parent_env, &profile.env, &req.record),
                    prompt: &req.prompt,
                    workdir: &req.workdir,
                    timeout,
                    kill_grace: profile.kill_grace,
                    transcript: req.transcript.as_deref(),
                    stderr: req.stderr.as_deref(),
                    cancel: req.cancel.clone(),
                    spawned: &spawned,
                };
                if let Some(path) = &req.prompt_file {
                    let text = prompt_file_text(
                        &req.record.invocation_id,
                        &req.record.node_id,
                        req.record.attempt,
                        &handler.argv(&inv),
                        &inv.env,
                        &req.prompt,
                    );
                    // Observability: a failed write never stops the agent.
                    if let Err(error) = write_prompt_file(path, &text) {
                        tracing::warn!(
                            path = %path.display(),
                            %error,
                            "cannot write the prompt file"
                        );
                    }
                }
                let deadline = timeout
                    .saturating_add(profile.kill_grace)
                    .saturating_add(self.hard_deadline_margin);
                match tokio::time::timeout(deadline, handler.run(inv)).await {
                    Ok(result) => result,
                    Err(_elapsed) => AgentResult::failed(
                        req.record.invocation_id.clone(),
                        FailureClass::Timeout,
                        format!("handler did not return within {}ms", deadline.as_millis()),
                    ),
                }
            }
            Err(error) => AgentResult::failed(
                req.record.invocation_id.clone(),
                FailureClass::Launch,
                error.to_string(),
            ),
        };
        if let Some(observer) = req.observer {
            observer.finished(&result);
        }
        result
    }

    /// [`AgentHandler::transcript_usage`] of the profile's handler; empty for
    /// an unknown profile.
    pub fn transcript_usage(&self, profile: &str, transcript: &str) -> Usage {
        self.resolve(profile)
            .map(|(_, handler)| handler.transcript_usage(transcript))
            .unwrap_or_default()
    }

    fn resolve(&self, name: &str) -> Option<(&Profile, &Arc<dyn AgentHandler>)> {
        let profile = self.profiles.get(name)?;
        let handler = self.handlers.get(profile.mechanism.as_str())?;
        Some((profile, handler))
    }
}

/// The `Started` report for one process of `req`.
fn started(req: &AgentRequest<'_>, profile: &Profile, spawn: u32, process: Spawned) -> Started {
    Started {
        invocation_id: req.record.invocation_id.clone(),
        spawn,
        node_id: req.record.node_id.clone(),
        attempt: req.record.attempt,
        profile: profile.name.clone(),
        model: selected_model(profile, req).map(str::to_owned),
        pid: process.pid,
        pgid: process.pgid,
        host: process.host,
        transcript: req.transcript.clone(),
        stderr: req.stderr.clone(),
    }
}
