//! Agent profiles and the argv they produce: pure.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use crate::types::{AgentRequest, Session};

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
    /// How long a rate-limited agent may keep waiting and retrying inside
    /// one invocation, from its first rate limit, re-spawns included
    /// (`claude-p` re-spawns; `pi` sets its own retry).
    /// Zero: no waiting.
    pub rate_limit_window: Duration,
    pub env: ProfileEnv,
    /// Refused unless the run allows test agents.
    pub test_only: bool,
    /// Added (after `args`) when starting a new session; `{session_id}` is
    /// the minted id.
    pub session_args: Vec<String>,
    /// How to continue a session; `None`: the profile can't resume.
    pub resume: Option<Resume>,
    /// The provider name (`pi` profiles), e.g. `deepseek`, `zai`.
    pub provider: Option<String>,
    /// The provider's API base URL (`pi` profiles).
    pub base_url: Option<String>,
    /// The environment variable holding the API key (`pi` profiles). The
    /// key is resolved by `Agents::run`, passed to the handler only as
    /// `Invocation::api_key` and never put in the child's environment.
    pub api_key_env: Option<String>,
    /// The model's limits (`pi` profiles), for a model the provider doesn't
    /// list itself.
    pub limits: Option<Limits>,
    /// What a Claude agent starts with (`claude-p` profiles); added after
    /// `args`.
    pub setup: ClaudeSetup,
}

/// How much of the operator's own Claude setup an agent loads:
/// `--safe-mode`, `--bare`, or `--setting-sources <list>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    Safe,
    Bare,
    /// Loads the `setting_sources` the profile names.
    Inherit,
}

/// A `claude-p` profile's setup fields, each `None` when no profile in its
/// `inherit_from` chain sets it. `settings` and `mcp_config` are a file or
/// inline JSON (starting with `{`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeSetup {
    pub isolation: Option<Isolation>,
    pub setting_sources: Option<Vec<String>>,
    pub settings: Option<String>,
    pub plugin_dirs: Option<Vec<String>>,
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub mcp_config: Option<String>,
}

impl ClaudeSetup {
    /// The names of the fields that are set, for a mechanism that takes
    /// none of them.
    pub fn set_fields(&self) -> Vec<&'static str> {
        [
            ("isolation", self.isolation.is_some()),
            ("setting_sources", self.setting_sources.is_some()),
            ("settings", self.settings.is_some()),
            ("plugin_dirs", self.plugin_dirs.is_some()),
            ("tools", self.tools.is_some()),
            ("disallowed_tools", self.disallowed_tools.is_some()),
            ("mcp_config", self.mcp_config.is_some()),
        ]
        .into_iter()
        .filter_map(|(field, set)| set.then_some(field))
        .collect()
    }

    /// The same setup with each relative path joined to `root`: the plugin
    /// dirs, and `settings` and `mcp_config` unless they are inline JSON.
    pub fn resolved_against(self, root: &Path) -> Self {
        let path = |value: String| {
            if is_inline_json(&value) || Path::new(&value).is_absolute() {
                value
            } else {
                root.join(value).to_string_lossy().into_owned()
            }
        };
        Self {
            settings: self.settings.map(path),
            mcp_config: self.mcp_config.map(path),
            plugin_dirs: self
                .plugin_dirs
                .map(|dirs| dirs.into_iter().map(path).collect()),
            ..self
        }
    }

    /// The flags: the isolation flag, `--mcp-config`, `--settings`,
    /// `--tools`, `--disallowedTools` (left out when empty) and a
    /// `--plugin-dir` per dir. A tool list is one comma-separated value,
    /// since Claude's tool flags are variadic.
    pub fn args(&self) -> Vec<String> {
        let isolation = match self.isolation {
            Some(Isolation::Safe) => vec!["--safe-mode".to_string()],
            Some(Isolation::Bare) => vec!["--bare".to_string()],
            Some(Isolation::Inherit) => vec![
                "--setting-sources".to_string(),
                self.setting_sources
                    .iter()
                    .flatten()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(","),
            ],
            None => vec![],
        };
        let flag = |name: &str, value: String| [name.to_string(), value];
        let disallowed = self.disallowed_tools.as_ref().filter(|t| !t.is_empty());
        isolation
            .into_iter()
            .chain(
                self.mcp_config
                    .iter()
                    .flat_map(|v| flag("--mcp-config", v.clone())),
            )
            .chain(
                self.settings
                    .iter()
                    .flat_map(|v| flag("--settings", v.clone())),
            )
            .chain(self.tools.iter().flat_map(|v| flag("--tools", v.join(","))))
            .chain(
                disallowed
                    .into_iter()
                    .flat_map(|v| flag("--disallowedTools", v.join(","))),
            )
            .chain(
                self.plugin_dirs
                    .iter()
                    .flatten()
                    .flat_map(|dir| flag("--plugin-dir", dir.clone())),
            )
            .collect()
    }
}

/// Whether a `settings` or `mcp_config` value is inline JSON, not a file.
fn is_inline_json(value: &str) -> bool {
    value.trim_start().starts_with('{')
}

/// A model's context window and largest reply, in tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub context: u64,
    pub max_output: u64,
}

/// A profile's resume form (design §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    /// Replaces only `session_args`.
    Args(Vec<String>),
    /// Replaces `command` and `args`; a `{command}` element expands to the
    /// profile's command.
    Command(Vec<String>),
}

impl Profile {
    /// Whether the profile can continue a session.
    pub fn can_resume(&self) -> bool {
        self.resume.is_some()
    }

    /// Whether a new session is started with an id PAS chooses (a
    /// `{session_id}` in `session_args`), so the id is known before the
    /// agent runs.
    pub fn takes_session_id(&self) -> bool {
        self.session_args.iter().any(|a| a.contains("{session_id}"))
    }
}

/// An invocation's argv and how many leading words are the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedArgv {
    pub argv: Vec<String>,
    /// For a handler that puts a flag right after the program.
    pub command_len: usize,
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

/// The argv for `req` (design §1, §2), before the handler's own flags:
/// - a new session: command, args, setup flags, filled `session_args`;
/// - continuing with `resume_args`: command, args, setup flags, filled
///   `resume_args`;
/// - continuing with `resume_command`: the filled `resume_command` (its
///   `{command}` element expands to the profile's command), setup flags;
///
/// then the request's extra args, the filled model args and reasoning args.
pub fn resolve_argv(profile: &Profile, req: &AgentRequest<'_>) -> ResolvedArgv {
    resolve_argv_for(profile, req, &req.session)
}

/// [`resolve_argv`] for `session` in place of the request's (a re-spawn
/// continues the request's session).
pub fn resolve_argv_for(
    profile: &Profile,
    req: &AgentRequest<'_>,
    session: &Session,
) -> ResolvedArgv {
    let id = session.id();
    let fill_id = |t: &String| fill(t, "session_id", id);
    let (mut argv, command_len): (Vec<String>, usize) = match (session, &profile.resume) {
        (Session::Continue(_), Some(Resume::Command(template))) => {
            let mut argv = Vec::new();
            let mut command_len = None;
            for element in template {
                if element == "{command}" {
                    argv.extend(profile.command.iter().cloned());
                    command_len = Some(argv.len());
                } else {
                    argv.push(fill_id(element));
                }
            }
            let len = command_len.unwrap_or(argv.len());
            argv.extend(profile.setup.args());
            (argv, len)
        }
        (session, resume) => {
            let session_args = match (session, resume) {
                (Session::Continue(_), Some(Resume::Args(args))) => args.as_slice(),
                _ => profile.session_args.as_slice(),
            };
            let argv = profile
                .command
                .iter()
                .chain(&profile.args)
                .cloned()
                .chain(profile.setup.args())
                .chain(session_args.iter().map(fill_id))
                .collect();
            (argv, profile.command.len())
        }
    };
    argv.extend(req.extra_args.iter().cloned());
    argv.extend(filled(
        &profile.model_args,
        "model",
        selected_model(profile, req),
    ));
    argv.extend(filled(
        &profile.reasoning_args,
        "reasoning",
        selected_reasoning(profile, req),
    ));
    ResolvedArgv { argv, command_len }
}

/// Where spawn `n` of an invocation writes, given spawn 1's file
/// `<inv>.<rest>`: spawn 1 uses it as is, spawn `n >= 2` uses
/// `<inv>.<n>.<rest>` (`<inv>.2.jsonl`, `<inv>.2.stderr.log`; design §4).
pub fn spawn_path(base: &std::path::Path, n: u32) -> std::path::PathBuf {
    if n <= 1 {
        return base.to_path_buf();
    }
    let Some(name) = base.file_name().and_then(|name| name.to_str()) else {
        return base.to_path_buf();
    };
    let renamed = match name.split_once('.') {
        Some((id, rest)) => format!("{id}.{n}.{rest}"),
        None => format!("{name}.{n}"),
    };
    base.with_file_name(renamed)
}

/// [`resolve_argv`]'s argv.
pub fn argv(profile: &Profile, req: &AgentRequest<'_>) -> Vec<String> {
    resolve_argv(profile, req).argv
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
            prompt_file: None,
            session: Session::New("sess-1".into()),
            state_dir: None,
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
            rate_limit_window: Duration::ZERO,
            env: ProfileEnv::default(),
            test_only: false,
            session_args: vec![],
            resume: None,
            provider: None,
            base_url: None,
            api_key_env: None,
            limits: None,
            setup: ClaudeSetup::default(),
        }
    }

    fn with_session(session: Session) -> AgentRequest<'static> {
        AgentRequest {
            session,
            ..request(Some("opus"), None, &["--x"])
        }
    }

    fn sessioned(resume: Option<Resume>) -> Profile {
        Profile {
            session_args: vec!["--session-id".into(), "{session_id}".into()],
            resume,
            ..profile()
        }
    }

    #[test]
    fn a_new_session_adds_the_filled_session_args_after_args() {
        let got = resolve_argv(
            &sessioned(Some(Resume::Args(vec![
                "--resume".into(),
                "{session_id}".into(),
            ]))),
            &with_session(Session::New("abc".into())),
        );
        assert_eq!(
            got.argv,
            [
                "bin/claude",
                "--sub",
                "--a",
                "--session-id",
                "abc",
                "--x",
                "--model",
                "m=opus"
            ]
        );
        assert_eq!(got.command_len, 2);
    }

    #[test]
    fn continuing_with_resume_args_replaces_only_the_session_args() {
        let got = resolve_argv(
            &sessioned(Some(Resume::Args(vec![
                "--resume".into(),
                "{session_id}".into(),
            ]))),
            &with_session(Session::Continue("abc".into())),
        );
        assert_eq!(
            got.argv,
            [
                "bin/claude",
                "--sub",
                "--a",
                "--resume",
                "abc",
                "--x",
                "--model",
                "m=opus"
            ]
        );
        assert_eq!(got.command_len, 2);
    }

    #[test]
    fn continuing_with_resume_command_replaces_command_and_args() {
        let template = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // `{command}` expands in place to every command element.
        let got = resolve_argv(
            &sessioned(Some(Resume::Command(template(&[
                "{command}",
                "exec",
                "resume",
                "{session_id}",
            ])))),
            &with_session(Session::Continue("t-1".into())),
        );
        assert_eq!(
            got.argv,
            [
                "bin/claude",
                "--sub",
                "exec",
                "resume",
                "t-1",
                "--x",
                "--model",
                "m=opus"
            ]
        );
        assert_eq!(got.command_len, 2);
        // Without `{command}`, the whole resume command counts as the program.
        let got = resolve_argv(
            &sessioned(Some(Resume::Command(template(&[
                "other",
                "resume",
                "{session_id}",
            ])))),
            &with_session(Session::Continue("t-1".into())),
        );
        assert_eq!(&got.argv[..3], ["other", "resume", "t-1"]);
        assert_eq!(got.command_len, 3);
    }

    #[test]
    fn a_profile_that_cant_resume_starts_new_args_even_if_asked_to_continue() {
        let got = resolve_argv(
            &sessioned(None),
            &with_session(Session::Continue("abc".into())),
        );
        assert_eq!(&got.argv[3..5], ["--session-id", "abc"]);
    }

    #[test]
    fn built_in_codex_inherited_with_a_command_resumes_with_that_command() {
        let profiles = crate::AgentsConfig::builtin()
            .unwrap()
            .with_overrides(
                &toml::from_str(
                    r#"
                    [mine]
                    inherit_from = "codex"
                    command = ["X", "--flag"]
                    "#,
                )
                .unwrap(),
            )
            .resolve()
            .unwrap();
        let mine = profiles.iter().find(|p| p.name == "mine").unwrap();
        let got = resolve_argv(mine, &with_session(Session::Continue("th-9".into())));
        assert_eq!(
            got.argv,
            [
                "X",
                "--flag",
                "exec",
                "resume",
                "th-9",
                "--json",
                "--skip-git-repo-check",
                "--dangerously-bypass-approvals-and-sandbox",
                "--x",
                "--model",
                "opus"
            ]
        );
        assert_eq!(got.command_len, 2);
    }

    fn setup() -> ClaudeSetup {
        ClaudeSetup {
            isolation: Some(Isolation::Safe),
            disallowed_tools: Some(vec!["A".into(), "B".into()]),
            ..ClaudeSetup::default()
        }
    }

    #[test]
    fn setup_flags_come_after_args_and_before_the_session_args() {
        let profile = Profile {
            setup: setup(),
            ..sessioned(Some(Resume::Args(vec![
                "--resume".into(),
                "{session_id}".into(),
            ])))
        };
        let got = resolve_argv(&profile, &with_session(Session::Continue("s".into())));
        assert_eq!(
            &got.argv[..8],
            [
                "bin/claude",
                "--sub",
                "--a",
                "--safe-mode",
                "--disallowedTools",
                "A,B",
                "--resume",
                "s"
            ]
        );
    }

    #[test]
    fn a_resume_command_keeps_the_setup_flags() {
        let profile = Profile {
            setup: setup(),
            ..sessioned(Some(Resume::Command(vec!["{command}".into(), "r".into()])))
        };
        let got = resolve_argv(&profile, &with_session(Session::Continue("s".into())));
        assert_eq!(
            &got.argv[..6],
            [
                "bin/claude",
                "--sub",
                "r",
                "--safe-mode",
                "--disallowedTools",
                "A,B"
            ]
        );
    }

    #[test]
    fn setup_args_follow_the_field_order_and_join_tool_lists() {
        let setup = ClaudeSetup {
            isolation: Some(Isolation::Inherit),
            setting_sources: Some(vec!["user".into(), "project".into()]),
            settings: Some("s.json".into()),
            plugin_dirs: Some(vec!["p1".into(), "p2".into()]),
            tools: Some(vec!["Read".into(), "Grep".into()]),
            disallowed_tools: Some(vec!["X".into()]),
            mcp_config: Some("{}".into()),
        };
        assert_eq!(
            setup.args(),
            [
                "--setting-sources",
                "user,project",
                "--mcp-config",
                "{}",
                "--settings",
                "s.json",
                "--tools",
                "Read,Grep",
                "--disallowedTools",
                "X",
                "--plugin-dir",
                "p1",
                "--plugin-dir",
                "p2",
            ]
        );
        let bare = ClaudeSetup {
            isolation: Some(Isolation::Bare),
            tools: Some(vec![]),
            disallowed_tools: Some(vec![]),
            ..ClaudeSetup::default()
        };
        // No tools at all is `--tools ""`; no disallowed tools is no flag.
        assert_eq!(bare.args(), ["--bare", "--tools", ""]);
        assert!(ClaudeSetup::default().args().is_empty());
    }

    #[test]
    fn relative_setup_paths_resolve_against_the_root_inline_json_does_not() {
        let setup = ClaudeSetup {
            settings: Some("agents/r/settings.json".into()),
            mcp_config: Some(" {\"mcpServers\":{}}".into()),
            plugin_dirs: Some(vec!["agents/r".into(), "/abs".into()]),
            ..ClaudeSetup::default()
        }
        .resolved_against(Path::new("/proj"));
        assert_eq!(
            setup.settings.as_deref(),
            Some("/proj/agents/r/settings.json")
        );
        assert_eq!(setup.mcp_config.as_deref(), Some(" {\"mcpServers\":{}}"));
        assert_eq!(
            setup.plugin_dirs,
            Some(vec!["/proj/agents/r".to_string(), "/abs".to_string()])
        );
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
