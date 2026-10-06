//! Agent profiles from config: the embedded defaults (`agents.toml`) and a
//! project's `pas.toml` `[agents.<name>]`, layered and resolved into
//! [`Profile`]s. Pure: parsing and merging, no I/O.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::Deserialize;

use crate::profile::{Profile, ProfileEnv, Resume};
use crate::registry::ConfigError;

/// The built-in profiles and defaults, compiled into the binary.
const EMBEDDED: &str = include_str!("agents.toml");

/// One profile as written in config. Every field is optional: a profile
/// takes what it leaves unset from its `inherit_from` parent, then from
/// `[defaults]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    pub inherit_from: Option<String>,
    pub mechanism: Option<String>,
    /// A program, or a program and its first arguments.
    pub command: Option<CommandLine>,
    pub args: Option<Vec<String>>,
    pub model: Option<String>,
    pub model_args: Option<Vec<String>>,
    pub reasoning: Option<String>,
    pub reasoning_args: Option<Vec<String>>,
    /// A duration such as `"10m"`, `"30s"` or `"500ms"`.
    pub timeout: Option<String>,
    pub kill_grace: Option<String>,
    pub env: Option<EnvConfig>,
    pub test_only: Option<bool>,
    /// Added when starting a new session; `{session_id}` is the minted id.
    pub session_args: Option<Vec<String>>,
    /// Continues a session; replaces only `session_args`.
    pub resume_args: Option<Vec<String>>,
    /// Continues a session; replaces `command` and `args` (`{command}`
    /// expands to the command). At most one of the two resume forms.
    pub resume_command: Option<Vec<String>>,
}

/// `command = "claude"` or `command = ["claude", "--sub"]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum CommandLine {
    Program(String),
    Argv(Vec<String>),
}

impl CommandLine {
    fn into_argv(self) -> Vec<String> {
        match self {
            Self::Program(program) => vec![program],
            Self::Argv(argv) => argv,
        }
    }
}

/// `env.remove` and `env.set`; each is inherited on its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvConfig {
    pub remove: Option<Vec<String>>,
    pub set: Option<BTreeMap<String, String>>,
}

/// The shape of the embedded `agents.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentsConfig {
    #[serde(default)]
    pub defaults: ProfileConfig,
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileConfig>,
    /// Built-in profiles an override replaced, so a missing field can say so.
    #[serde(skip)]
    replaced: BTreeSet<String>,
}

impl AgentsConfig {
    /// The built-in profiles and defaults.
    pub fn builtin() -> Result<Self, ConfigError> {
        Self::parse(EMBEDDED)
    }

    /// Parse a file shaped like `agents.toml`.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// These profiles with `overrides` layered on top: a profile named in
    /// `overrides` replaces the one of the same name whole.
    pub fn with_overrides(mut self, overrides: &BTreeMap<String, ProfileConfig>) -> Self {
        for (name, profile) in overrides {
            if self
                .profiles
                .insert(name.clone(), profile.clone())
                .is_some()
            {
                self.replaced.insert(name.clone());
            }
        }
        self
    }

    /// Every profile, resolved: `inherit_from` chains followed, then
    /// `[defaults]`, then checked.
    pub fn resolve(&self) -> Result<Vec<Profile>, ConfigError> {
        self.profiles
            .keys()
            .map(|name| {
                resolve_one(name, &self.profiles, &self.defaults).map_err(|error| match error {
                    ConfigError::MissingField { profile, field, .. }
                        if self.replaced.contains(&profile) =>
                    {
                        ConfigError::MissingField {
                            profile,
                            field,
                            replaced_builtin: true,
                        }
                    }
                    other => other,
                })
            })
            .collect()
    }
}

/// The profiles PAS ships with.
pub fn builtin_profiles() -> Result<Vec<Profile>, ConfigError> {
    AgentsConfig::builtin()?.resolve()
}

/// `name`'s config with its `inherit_from` chain and the defaults merged
/// in, as a [`Profile`].
fn resolve_one(
    name: &str,
    profiles: &BTreeMap<String, ProfileConfig>,
    defaults: &ProfileConfig,
) -> Result<Profile, ConfigError> {
    let mut merged = profiles
        .get(name)
        .cloned()
        .ok_or_else(|| ConfigError::UnknownProfile(name.to_string()))?;
    let mut chain = vec![name.to_string()];
    let mut next = merged.inherit_from.clone();
    while let Some(parent_name) = next {
        if chain.contains(&parent_name) {
            return Err(ConfigError::InheritCycle {
                profile: name.to_string(),
            });
        }
        let parent = profiles
            .get(&parent_name)
            .ok_or_else(|| ConfigError::UnknownParent {
                profile: chain.last().cloned().unwrap_or_default(),
                parent: parent_name.clone(),
            })?;
        merged = overlay(merged, parent);
        next = parent.inherit_from.clone();
        chain.push(parent_name);
    }
    into_profile(name, overlay(merged, defaults))
}

/// `child` with each field it leaves unset taken from `parent`. A set
/// field replaces the parent's whole value; lists are not merged.
fn overlay(child: ProfileConfig, parent: &ProfileConfig) -> ProfileConfig {
    let child_resumes = child.resume_args.is_some() || child.resume_command.is_some();
    let env = match (child.env, &parent.env) {
        (Some(c), Some(p)) => Some(EnvConfig {
            remove: c.remove.or_else(|| p.remove.clone()),
            set: c.set.or_else(|| p.set.clone()),
        }),
        (c, p) => c.or_else(|| p.clone()),
    };
    ProfileConfig {
        inherit_from: child.inherit_from,
        mechanism: child.mechanism.or_else(|| parent.mechanism.clone()),
        command: child.command.or_else(|| parent.command.clone()),
        args: child.args.or_else(|| parent.args.clone()),
        model: child.model.or_else(|| parent.model.clone()),
        model_args: child.model_args.or_else(|| parent.model_args.clone()),
        reasoning: child.reasoning.or_else(|| parent.reasoning.clone()),
        reasoning_args: child
            .reasoning_args
            .or_else(|| parent.reasoning_args.clone()),
        timeout: child.timeout.or_else(|| parent.timeout.clone()),
        kill_grace: child.kill_grace.or_else(|| parent.kill_grace.clone()),
        env,
        test_only: child.test_only.or(parent.test_only),
        session_args: child.session_args.or_else(|| parent.session_args.clone()),
        // The resume form is inherited as one field: a child that sets
        // either form replaces the parent's.
        resume_args: if child_resumes {
            child.resume_args
        } else {
            parent.resume_args.clone()
        },
        resume_command: if child_resumes {
            child.resume_command
        } else {
            parent.resume_command.clone()
        },
    }
}

fn into_profile(name: &str, config: ProfileConfig) -> Result<Profile, ConfigError> {
    let missing = |field: &'static str| ConfigError::MissingField {
        profile: name.to_string(),
        field,
        replaced_builtin: false,
    };
    let duration = |field: &'static str, value: Option<String>| {
        let value = value.ok_or_else(|| missing(field))?;
        parse_duration(&value).ok_or_else(|| ConfigError::BadDuration {
            profile: name.to_string(),
            field,
            value,
        })
    };
    let command = config
        .command
        .map(CommandLine::into_argv)
        .filter(|argv| argv.first().is_some_and(|program| !program.is_empty()))
        .ok_or_else(|| missing("command"))?;
    let env = config.env.unwrap_or_default();
    let resume = match (config.resume_args, config.resume_command) {
        (Some(_), Some(_)) => {
            return Err(ConfigError::TwoResumeForms {
                profile: name.to_string(),
            })
        }
        (Some(args), None) => Some(Resume::Args(args)),
        (None, Some(command)) => Some(Resume::Command(command)),
        (None, None) => None,
    };
    Ok(Profile {
        name: name.to_string(),
        mechanism: config.mechanism.ok_or_else(|| missing("mechanism"))?,
        command,
        args: config.args.unwrap_or_default(),
        model: config.model,
        model_args: config.model_args.unwrap_or_default(),
        reasoning: config.reasoning,
        reasoning_args: config.reasoning_args.unwrap_or_default(),
        timeout: duration("timeout", config.timeout)?,
        kill_grace: duration("kill_grace", config.kill_grace)?,
        env: ProfileEnv {
            remove: env.remove.unwrap_or_default(),
            set: env.set.unwrap_or_default(),
        },
        test_only: config.test_only.unwrap_or(false),
        session_args: config.session_args.unwrap_or_default(),
        resume,
    })
}

/// `"500ms"`, `"30s"`, `"10m"` or `"1h"`; `None` for anything else.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = text.split_at(split);
    let n: u64 = number.parse().ok()?;
    match unit {
        "ms" => Some(Duration::from_millis(n)),
        "s" => Some(Duration::from_secs(n)),
        "m" => n.checked_mul(60).map(Duration::from_secs),
        "h" => n.checked_mul(3600).map(Duration::from_secs),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> AgentsConfig {
        AgentsConfig::parse(text).unwrap()
    }

    fn overrides(text: &str) -> BTreeMap<String, ProfileConfig> {
        toml::from_str(text).unwrap()
    }

    fn profile<'a>(profiles: &'a [Profile], name: &str) -> &'a Profile {
        profiles.iter().find(|p| p.name == name).unwrap()
    }

    const BASE: &str = r#"
        [defaults]
        timeout = "10m"
        kill_grace = "10s"
        [defaults.env]
        remove = ["SECRET"]

        [profiles.base]
        mechanism = "m"
        command = ["prog", "--sub"]
        args = ["--a"]
        model_args = ["--model", "{model}"]
    "#;

    #[test]
    fn embedded_defaults_give_todays_claude_profile() {
        let profiles = builtin_profiles().unwrap();
        assert_eq!(profiles.len(), 3);
        let claude = profile(&profiles, "claude");
        assert_eq!(claude.name, "claude");
        assert_eq!(claude.mechanism, "claude-p");
        assert_eq!(claude.command, ["claude"]);
        assert_eq!(
            claude.args,
            [
                "--dangerously-skip-permissions",
                "--strict-mcp-config",
                "--disable-slash-commands",
            ]
        );
        // Ticket 08: sessions persist; a retry resumes.
        assert_eq!(claude.session_args, ["--session-id", "{session_id}"]);
        assert_eq!(
            claude.resume,
            Some(Resume::Args(vec!["--resume".into(), "{session_id}".into()]))
        );
        assert_eq!(claude.model, None);
        assert_eq!(claude.model_args, ["--model", "{model}"]);
        assert_eq!(claude.reasoning_args, ["--effort", "{reasoning}"]);
        assert_eq!(claude.timeout, Duration::from_secs(600));
        assert_eq!(claude.kill_grace, Duration::from_secs(10));
        assert_eq!(
            claude.env.remove,
            [
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_BASE_URL",
                "OPENAI_API_KEY",
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
            ]
        );
        assert!(claude.env.set.is_empty());
        assert!(!claude.test_only);
    }

    #[test]
    fn embedded_defaults_give_codex_and_gemini_todays_flags() {
        let profiles = builtin_profiles().unwrap();
        let codex = profile(&profiles, "codex");
        assert_eq!(codex.mechanism, "codex-exec");
        assert_eq!(codex.command, ["codex"]);
        assert_eq!(
            codex.args,
            ["exec", "--json", "--yolo", "--skip-git-repo-check"]
        );
        assert!(codex.session_args.is_empty());
        assert_eq!(
            codex.resume,
            Some(Resume::Command(
                [
                    "{command}",
                    "exec",
                    "resume",
                    "{session_id}",
                    "--json",
                    "--skip-git-repo-check",
                    "--dangerously-bypass-approvals-and-sandbox",
                ]
                .map(String::from)
                .to_vec()
            ))
        );
        assert_eq!(codex.model_args, ["--model", "{model}"]);
        assert!(codex.reasoning_args.is_empty());
        let gemini = profile(&profiles, "gemini");
        assert_eq!(gemini.mechanism, "gemini");
        assert_eq!(gemini.command, ["gemini"]);
        assert_eq!(gemini.args, ["--approval-mode", "yolo"]);
        assert_eq!(gemini.model_args, ["--model", "{model}"]);
        assert!(!gemini.can_resume());
        // Both get the defaults, OPENAI_API_KEY in the strip list included.
        for p in [codex, gemini] {
            assert_eq!(p.timeout, Duration::from_secs(600));
            assert!(p.env.remove.contains(&"OPENAI_API_KEY".to_string()));
        }
    }

    #[test]
    fn a_profile_with_both_resume_forms_is_refused() {
        let error = config(
            r#"
            [defaults]
            timeout = "1m"
            kill_grace = "1s"
            [profiles.x]
            mechanism = "m"
            command = "x"
            resume_args = ["--resume", "{session_id}"]
            resume_command = ["{command}", "resume"]
            "#,
        )
        .resolve()
        .unwrap_err();
        assert_eq!(
            error,
            ConfigError::TwoResumeForms {
                profile: "x".into()
            }
        );
    }

    #[test]
    fn a_child_setting_one_resume_form_replaces_the_parents_other() {
        let profiles = builtin_profiles_with(
            r#"
            [mine]
            inherit_from = "claude"
            resume_command = ["{command}", "--continue-as", "{session_id}"]
            "#,
        );
        let mine = profile(&profiles, "mine");
        assert_eq!(
            mine.resume,
            Some(Resume::Command(
                ["{command}", "--continue-as", "{session_id}"]
                    .map(String::from)
                    .to_vec()
            ))
        );
        // session_args still inherited.
        assert_eq!(mine.session_args, ["--session-id", "{session_id}"]);
    }

    fn builtin_profiles_with(text: &str) -> Vec<Profile> {
        AgentsConfig::builtin()
            .unwrap()
            .with_overrides(&overrides(text))
            .resolve()
            .unwrap()
    }

    #[test]
    fn an_override_replaces_the_whole_profile_by_name() {
        let profiles = config(BASE)
            .with_overrides(&overrides(
                r#"
                [base]
                mechanism = "m"
                command = "other"
                "#,
            ))
            .resolve()
            .unwrap();
        let base = profile(&profiles, "base");
        assert_eq!(base.command, ["other"]);
        assert!(base.args.is_empty(), "args are not kept from the built-in");
        assert!(base.model_args.is_empty());
    }

    #[test]
    fn an_override_adds_a_profile() {
        let profiles = config(BASE)
            .with_overrides(&overrides(
                r#"
                [fake]
                mechanism = "m"
                command = "/abs/fake"
                test_only = true
                "#,
            ))
            .resolve()
            .unwrap();
        assert_eq!(profiles.len(), 2);
        let fake = profile(&profiles, "fake");
        assert!(fake.test_only);
        assert_eq!(fake.timeout, Duration::from_secs(600), "from [defaults]");
        assert_eq!(fake.env.remove, ["SECRET"], "from [defaults]");
    }

    #[test]
    fn inherit_from_takes_each_unset_field_from_the_parent() {
        let profiles = config(BASE)
            .with_overrides(&overrides(
                r#"
                [mid]
                inherit_from = "base"
                model = "opus"
                kill_grace = "3s"

                [leaf]
                inherit_from = "mid"
                args = ["--b"]
                "#,
            ))
            .resolve()
            .unwrap();
        let leaf = profile(&profiles, "leaf");
        assert_eq!(leaf.mechanism, "m");
        assert_eq!(leaf.command, ["prog", "--sub"]);
        assert_eq!(leaf.args, ["--b"], "a set list replaces the parent's");
        assert_eq!(leaf.model.as_deref(), Some("opus"));
        assert_eq!(leaf.model_args, ["--model", "{model}"]);
        assert_eq!(leaf.kill_grace, Duration::from_secs(3));
        assert_eq!(leaf.timeout, Duration::from_secs(600));
    }

    #[test]
    fn env_remove_and_set_are_inherited_separately() {
        let profiles = config(BASE)
            .with_overrides(&overrides(
                r#"
                [parent]
                inherit_from = "base"
                env.set = { A = "1" }

                [child]
                inherit_from = "parent"
                env.remove = ["X"]
                "#,
            ))
            .resolve()
            .unwrap();
        let child = profile(&profiles, "child");
        assert_eq!(child.env.remove, ["X"]);
        assert_eq!(child.env.set.get("A").map(String::as_str), Some("1"));
        let parent = profile(&profiles, "parent");
        assert_eq!(parent.env.remove, ["SECRET"], "from [defaults]");
    }

    #[test]
    fn unknown_parent_names_the_profile_and_the_parent() {
        let err = config(BASE)
            .with_overrides(&overrides("[x]\ninherit_from = \"nope\"\n"))
            .resolve()
            .unwrap_err();
        assert_eq!(
            err,
            ConfigError::UnknownParent {
                profile: "x".into(),
                parent: "nope".into()
            }
        );
    }

    #[test]
    fn an_inherit_cycle_is_an_error() {
        let err = config(BASE)
            .with_overrides(&overrides(
                "[a]\ninherit_from = \"b\"\n[b]\ninherit_from = \"a\"\n",
            ))
            .resolve()
            .unwrap_err();
        assert!(matches!(err, ConfigError::InheritCycle { .. }), "{err}");
    }

    #[test]
    fn a_missing_command_or_mechanism_is_an_error() {
        let err = config(BASE)
            .with_overrides(&overrides("[x]\nmechanism = \"m\"\n"))
            .resolve()
            .unwrap_err();
        assert_eq!(
            err,
            ConfigError::MissingField {
                profile: "x".into(),
                field: "command",
                replaced_builtin: false
            }
        );
        let err = config(BASE)
            .with_overrides(&overrides("[x]\ncommand = \"p\"\n"))
            .resolve()
            .unwrap_err();
        assert_eq!(
            err,
            ConfigError::MissingField {
                profile: "x".into(),
                field: "mechanism",
                replaced_builtin: false
            }
        );
    }

    #[test]
    fn a_replaced_builtin_missing_a_field_says_it_was_replaced_whole() {
        let err = AgentsConfig::builtin()
            .unwrap()
            .with_overrides(&overrides("[claude]\nmodel = \"opus\"\n"))
            .resolve()
            .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("agent profile claude has no "),
            "{message}"
        );
        assert!(
            message.contains("replaces the built-in claude profile whole"),
            "{message}"
        );
        assert!(message.contains("inherit_from = \"claude\""), "{message}");
        // A new profile missing a field gets no such hint.
        let err = config(BASE)
            .with_overrides(&overrides("[x]\nmechanism = \"m\"\n"))
            .resolve()
            .unwrap_err();
        assert!(!err.to_string().contains("built-in"), "{err}");
    }

    #[test]
    fn a_bad_duration_names_the_profile_and_field() {
        let err = config(BASE)
            .with_overrides(&overrides(
                "[x]\ninherit_from = \"base\"\ntimeout = \"ten\"\n",
            ))
            .resolve()
            .unwrap_err();
        assert_eq!(
            err,
            ConfigError::BadDuration {
                profile: "x".into(),
                field: "timeout",
                value: "ten".into()
            }
        );
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let err =
            toml::from_str::<BTreeMap<String, ProfileConfig>>("[x]\ncomand = \"p\"\n").unwrap_err();
        assert!(err.to_string().contains("comand"), "{err}");
        assert!(matches!(
            AgentsConfig::parse("[profiles.x]\nnope = 1\n"),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn durations_parse_with_a_unit() {
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("10m"), Some(Duration::from_secs(600)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration("10"), None);
        assert_eq!(parse_duration("s"), None);
        assert_eq!(parse_duration("1d"), None);
    }
}
