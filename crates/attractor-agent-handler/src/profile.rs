//! Agent profiles and the argv they produce: pure.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::types::AgentRequest;

/// A named way to run an agent: which handler (`mechanism`), which command
/// line, and its limits and environment. Built from config by
/// [`crate::AgentsConfig::resolve`]. Templates in `model_args` use
/// `{model}`; in `reasoning_args`, `{reasoning}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub mechanism: String,
    pub command: Vec<String>,
    pub args: Vec<String>,
    /// The model when the node names none.
    pub model: Option<String>,
    /// Added only when a model is selected.
    pub model_args: Vec<String>,
    /// The reasoning level when the node names none.
    pub reasoning: Option<String>,
    /// Added only when a reasoning level is selected. Empty: the profile
    /// takes no reasoning level.
    pub reasoning_args: Vec<String>,
    /// How long an invocation may run when the node sets no timeout.
    pub timeout: Duration,
    /// How long to wait after TERM before KILL, on timeout or cancel.
    pub kill_grace: Duration,
    pub env: ProfileEnv,
    /// Refused unless the run allows test agents.
    pub test_only: bool,
}

/// The child environment's changes: `remove` from the parent's, then `set`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileEnv {
    pub remove: Vec<String>,
    pub set: BTreeMap<String, String>,
}

/// `template` with every `{name}` replaced by `value`.
pub fn fill(template: &str, name: &str, value: &str) -> String {
    template.replace(&format!("{{{name}}}"), value)
}

/// `templates` filled with `value`, or nothing when there is no value.
fn filled<'a>(
    templates: &'a [String],
    name: &'a str,
    value: Option<&'a str>,
) -> impl Iterator<Item = String> + 'a {
    value
        .into_iter()
        .flat_map(move |value| templates.iter().map(move |t| fill(t, name, value)))
}

/// The model an invocation uses: the request's, else the profile's.
pub fn selected_model<'a>(profile: &'a Profile, req: &'a AgentRequest<'_>) -> Option<&'a str> {
    req.selection.model.as_deref().or(profile.model.as_deref())
}

/// The reasoning level an invocation uses: the request's, else the
/// profile's.
pub fn selected_reasoning<'a>(profile: &'a Profile, req: &'a AgentRequest<'_>) -> Option<&'a str> {
    req.selection
        .reasoning
        .as_deref()
        .or(profile.reasoning.as_deref())
}

/// The profile's command, its args, the request's extra args, then the
/// filled model args and reasoning args when a model and a reasoning level
/// are selected. The handler appends its own flags after these.
pub fn argv(profile: &Profile, req: &AgentRequest<'_>) -> Vec<String> {
    profile
        .command
        .iter()
        .chain(&profile.args)
        .chain(&req.extra_args)
        .cloned()
        .chain(filled(
            &profile.model_args,
            "model",
            selected_model(profile, req),
        ))
        .chain(filled(
            &profile.reasoning_args,
            "reasoning",
            selected_reasoning(profile, req),
        ))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Record, Selection};
    use std::path::PathBuf;

    fn request(
        model: Option<&str>,
        reasoning: Option<&str>,
        extra: &[&str],
    ) -> AgentRequest<'static> {
        AgentRequest {
            selection: Selection {
                profile: "claude".into(),
                model: model.map(String::from),
                reasoning: reasoning.map(String::from),
            },
            prompt: "hi".into(),
            extra_args: extra.iter().map(|s| s.to_string()).collect(),
            workdir: PathBuf::from("/w"),
            timeout: None,
            record: Record {
                run_id: None,
                node_id: "n".into(),
                attempt: 1,
                invocation_id: "i".into(),
            },
            transcript: None,
            stderr: None,
            observer: None,
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    fn profile() -> Profile {
        Profile {
            name: "claude".into(),
            mechanism: "claude-p".into(),
            command: vec!["bin/claude".into(), "--sub".into()],
            args: vec!["--a".into()],
            model: None,
            model_args: vec!["--model".into(), "m={model}".into()],
            reasoning: None,
            reasoning_args: vec!["--effort".into(), "{reasoning}".into()],
            timeout: Duration::from_secs(600),
            kill_grace: Duration::from_secs(10),
            env: ProfileEnv::default(),
            test_only: false,
        }
    }

    #[test]
    fn argv_is_command_args_extra_then_filled_model_and_reasoning_args() {
        let got = argv(
            &profile(),
            &request(Some("opus"), Some("high"), &["--x", "1"]),
        );
        assert_eq!(
            got,
            [
                "bin/claude",
                "--sub",
                "--a",
                "--x",
                "1",
                "--model",
                "m=opus",
                "--effort",
                "high"
            ]
        );
    }

    #[test]
    fn argv_leaves_model_and_reasoning_args_out_without_values() {
        let got = argv(&profile(), &request(None, None, &[]));
        assert_eq!(got, ["bin/claude", "--sub", "--a"]);
    }

    #[test]
    fn the_request_beats_the_profiles_model_and_reasoning() {
        let profile = Profile {
            model: Some("m0".into()),
            reasoning: Some("low".into()),
            ..profile()
        };
        let got = argv(&profile, &request(None, None, &[]));
        assert_eq!(got[3..], ["--model", "m=m0", "--effort", "low"]);
        let got = argv(&profile, &request(Some("m1"), Some("high"), &[]));
        assert_eq!(got[3..], ["--model", "m=m1", "--effort", "high"]);
    }

    #[test]
    fn fill_replaces_every_named_placeholder() {
        assert_eq!(fill("{model}/{model}", "model", "x"), "x/x");
        assert_eq!(fill("{reasoning}", "model", "x"), "{reasoning}");
        assert_eq!(fill("--plain", "model", "x"), "--plain");
    }
}
