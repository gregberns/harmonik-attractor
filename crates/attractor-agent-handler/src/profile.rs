//! Agent profiles and the argv they produce: pure.

use std::time::Duration;

use crate::types::AgentRequest;

/// How long the built-in profiles wait after TERM before KILL.
pub const DEFAULT_KILL_GRACE: Duration = Duration::from_secs(10);

/// A named way to run an agent: which handler (`mechanism`) and which
/// command line. Templates in `model_args` use `{model}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub mechanism: String,
    pub command: Vec<String>,
    pub args: Vec<String>,
    /// Added only when the request names a model.
    pub model_args: Vec<String>,
    /// How long to wait after TERM before KILL, on timeout or cancel.
    pub kill_grace: Duration,
}

/// The profiles PAS ships with: `claude`, run by the `claude-p` handler.
pub fn builtin_profiles() -> Vec<Profile> {
    vec![Profile {
        name: "claude".into(),
        mechanism: "claude-p".into(),
        command: vec!["claude".into()],
        args: [
            "--no-session-persistence",
            "--dangerously-skip-permissions",
            "--strict-mcp-config",
            "--disable-slash-commands",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
        model_args: vec!["--model".into(), "{model}".into()],
        kill_grace: DEFAULT_KILL_GRACE,
    }]
}

/// `template` with every `{model}` replaced by `model`.
pub fn fill(template: &str, model: &str) -> String {
    template.replace("{model}", model)
}

/// The profile's command, its args, the request's extra args, then the
/// filled model args when the request names a model. The handler appends
/// its own flags after these.
pub fn argv(profile: &Profile, req: &AgentRequest<'_>) -> Vec<String> {
    let model_args = req.selection.model.as_deref().map(|model| {
        profile
            .model_args
            .iter()
            .map(|t| fill(t, model))
            .collect::<Vec<_>>()
    });
    profile
        .command
        .iter()
        .chain(&profile.args)
        .chain(&req.extra_args)
        .cloned()
        .chain(model_args.into_iter().flatten())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Record, Selection};
    use std::path::PathBuf;
    use std::time::Duration;

    fn request(model: Option<&str>, extra: &[&str]) -> AgentRequest<'static> {
        AgentRequest {
            selection: Selection {
                profile: "claude".into(),
                model: model.map(String::from),
            },
            prompt: "hi".into(),
            extra_args: extra.iter().map(|s| s.to_string()).collect(),
            workdir: PathBuf::from("/w"),
            timeout: Duration::from_secs(1),
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
            model_args: vec!["--model".into(), "m={model}".into()],
            kill_grace: DEFAULT_KILL_GRACE,
        }
    }

    #[test]
    fn argv_is_command_args_extra_then_filled_model_args() {
        let got = argv(&profile(), &request(Some("opus"), &["--x", "1"]));
        assert_eq!(
            got,
            [
                "bin/claude",
                "--sub",
                "--a",
                "--x",
                "1",
                "--model",
                "m=opus"
            ]
        );
    }

    #[test]
    fn argv_leaves_model_args_out_without_a_model() {
        let got = argv(&profile(), &request(None, &[]));
        assert_eq!(got, ["bin/claude", "--sub", "--a"]);
    }

    #[test]
    fn fill_replaces_every_model_placeholder() {
        assert_eq!(fill("{model}/{model}", "x"), "x/x");
        assert_eq!(fill("--plain", "x"), "--plain");
    }

    #[test]
    fn builtin_claude_profile_matches_todays_flags() {
        let profiles = builtin_profiles();
        assert_eq!(profiles.len(), 1);
        let claude = &profiles[0];
        assert_eq!(claude.name, "claude");
        assert_eq!(claude.mechanism, "claude-p");
        assert_eq!(claude.command, ["claude"]);
        assert_eq!(
            claude.args,
            [
                "--no-session-persistence",
                "--dangerously-skip-permissions",
                "--strict-mcp-config",
                "--disable-slash-commands",
            ]
        );
        assert_eq!(claude.model_args, ["--model", "{model}"]);
        assert_eq!(claude.kill_grace, Duration::from_secs(10));
    }
}
