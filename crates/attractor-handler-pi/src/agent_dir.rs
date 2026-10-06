//! The per-invocation `PI_CODING_AGENT_DIR`: `models.json` (with the key)
//! and `settings.json`, written by the engine, removed when the invocation
//! ends.

use std::fs::{DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use attractor_agent_handler::{Limits, Profile};
use serde_json::{json, Map, Value};

/// The provider and model a Pi invocation uses, both required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target<'a> {
    pub provider: &'a str,
    pub model: &'a str,
}

/// Pi's `models.json` for one invocation: the profile's provider with its
/// `baseUrl` (if set), the key as `apiKey` (if any) and, only with
/// `limits`, the model's entry. Without `limits` no model list is written,
/// so a provider Pi knows keeps its own catalog.
pub fn models_json(target: Target<'_>, profile: &Profile, api_key: Option<&str>) -> Value {
    let mut provider = Map::new();
    if let Some(url) = &profile.base_url {
        provider.insert("baseUrl".into(), json!(url));
    }
    provider.insert("api".into(), json!("openai-completions"));
    if let Some(key) = api_key {
        provider.insert("apiKey".into(), json!(key));
    }
    if let Some(Limits {
        context,
        max_output,
    }) = profile.limits
    {
        provider.insert(
            "models".into(),
            json!([{
                "id": target.model,
                "contextWindow": context,
                "maxTokens": max_output,
            }]),
        );
    }
    json!({ "providers": { target.provider: provider } })
}

/// Pi's first retry delay; it doubles on each retry (Pi's docs, 0.80.2).
const PI_BASE_DELAY_MS: u64 = 2000;

/// Pi's `settings.json`: its own retry set to cover the profile's
/// `rate_limit_window` (Pi retries inside one process, so PAS doesn't
/// re-spawn it). `maxRetries` is the smallest count whose exponential
/// backoff (2 s, 4 s, 8 s, ...) adds up to the window, and a delay the
/// server asks for is waited up to the window (`provider.maxRetryDelayMs`).
/// A zero window turns Pi's retry off. The keys are from Pi's docs
/// (`docs/settings.md`, "Retry", Pi 0.80.2); no real Pi run checks them.
pub fn settings_json(window: Duration) -> Value {
    let window_ms = u64::try_from(window.as_millis()).unwrap_or(u64::MAX);
    if window_ms == 0 {
        return json!({ "retry": { "enabled": false } });
    }
    let mut max_retries = 0u32;
    let mut total = 0u64;
    while total < window_ms && max_retries < 32 {
        total = total.saturating_add(PI_BASE_DELAY_MS.saturating_mul(1 << max_retries));
        max_retries += 1;
    }
    json!({
        "retry": {
            "enabled": true,
            "baseDelayMs": PI_BASE_DELAY_MS,
            "maxRetries": max_retries,
            "provider": { "maxRetryDelayMs": window_ms },
        }
    })
}

/// A directory that is removed (with what it holds) when dropped, so every
/// way an invocation ends removes it, a dropped future included.
#[derive(Debug)]
pub struct AgentDir {
    path: PathBuf,
}

impl AgentDir {
    /// Create `<parent>/<name>` (mode 0700; `parent` too, if missing) with
    /// `models.json` and `settings.json` (mode 0600 from creation). A
    /// directory already there is an error, never reused.
    pub fn create(parent: &Path, name: &str, models: &Value, settings: &Value) -> io::Result<Self> {
        private_dir(true).create(parent)?;
        let path = parent.join(name);
        private_dir(false).create(&path)?;
        let dir = Self { path };
        write_private(&dir.path.join("models.json"), models)?;
        write_private(&dir.path.join("settings.json"), settings)?;
        Ok(dir)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for AgentDir {
    fn drop(&mut self) {
        // Nothing to report to: the invocation has ended. A failure leaves
        // the directory in the run folder, as a SIGKILL of pas would.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
pub(crate) fn private_dir(recursive: bool) -> DirBuilder {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = DirBuilder::new();
    builder.recursive(recursive).mode(0o700);
    builder
}

#[cfg(not(unix))]
pub(crate) fn private_dir(recursive: bool) -> DirBuilder {
    let mut builder = DirBuilder::new();
    builder.recursive(recursive);
    builder
}

/// Write `value` as JSON to a new file at `path`, mode 0600 from the start.
fn write_private(path: &Path, value: &Value) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(value.to_string().as_bytes())?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use attractor_agent_handler::ProfileEnv;
    use std::time::Duration;

    fn profile() -> Profile {
        Profile {
            name: "pi".into(),
            mechanism: "pi".into(),
            command: vec!["pi".into()],
            args: vec![],
            model: Some("m".into()),
            model_args: vec![],
            reasoning: None,
            reasoning_args: vec![],
            timeout: Duration::from_secs(1),
            kill_grace: Duration::from_secs(1),
            rate_limit_window: Duration::ZERO,
            env: ProfileEnv::default(),
            test_only: false,
            session_args: vec![],
            resume: None,
            provider: Some("deepseek".into()),
            base_url: None,
            api_key_env: None,
            limits: None,
        }
    }

    const TARGET: Target<'static> = Target {
        provider: "deepseek",
        model: "deepseek-v4-pro",
    };

    #[test]
    fn a_known_provider_without_url_or_limits_gets_only_the_api_and_key() {
        assert_eq!(
            models_json(TARGET, &profile(), Some("sk-1")),
            json!({"providers": {"deepseek": {"api": "openai-completions", "apiKey": "sk-1"}}})
        );
    }

    #[test]
    fn base_url_and_limits_add_the_url_and_the_models_entry() {
        let p = Profile {
            base_url: Some("http://host:1/v1".into()),
            limits: Some(Limits {
                context: 262144,
                max_output: 32768,
            }),
            ..profile()
        };
        let target = Target {
            provider: "qwen-lan",
            model: "qwen3.8",
        };
        assert_eq!(
            models_json(target, &p, Some("sk-q")),
            json!({"providers": {"qwen-lan": {
                "baseUrl": "http://host:1/v1",
                "api": "openai-completions",
                "apiKey": "sk-q",
                "models": [{"id": "qwen3.8", "contextWindow": 262144, "maxTokens": 32768}],
            }}})
        );
    }

    #[test]
    fn no_key_writes_no_api_key() {
        let p = Profile {
            base_url: Some("http://host:1/v1".into()),
            ..profile()
        };
        assert_eq!(
            models_json(TARGET, &p, None),
            json!({"providers": {"deepseek": {"baseUrl": "http://host:1/v1", "api": "openai-completions"}}})
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_dir_is_private_and_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("pi-agent");
        let dir = AgentDir::create(
            &parent,
            "inv-1",
            &json!({"a": 1}),
            &settings_json(Duration::ZERO),
        )
        .unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.path()), 0o700);
        assert_eq!(mode(&dir.path().join("models.json")), 0o600);
        assert_eq!(mode(&dir.path().join("settings.json")), 0o600);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("models.json")).unwrap(),
            r#"{"a":1}"#
        );
        let path = dir.path().to_path_buf();
        drop(dir);
        assert!(!path.exists());
        assert!(parent.exists(), "only the invocation's dir is removed");
    }

    #[test]
    fn an_existing_dir_is_not_reused() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("inv-1")).unwrap();
        assert!(AgentDir::create(tmp.path(), "inv-1", &json!({}), &json!({})).is_err());
        assert!(tmp.path().join("inv-1").exists(), "not ours to remove");
    }

    #[test]
    fn settings_cover_the_rate_limit_window() {
        assert_eq!(
            settings_json(Duration::from_secs(120)),
            json!({"retry": {
                "enabled": true,
                "baseDelayMs": 2000,
                "maxRetries": 6,
                "provider": {"maxRetryDelayMs": 120000},
            }})
        );
        // 2 s covers one retry; 6 s two (2 + 4).
        assert_eq!(
            settings_json(Duration::from_secs(2))["retry"]["maxRetries"],
            1
        );
        assert_eq!(
            settings_json(Duration::from_secs(6))["retry"]["maxRetries"],
            2
        );
        assert_eq!(
            settings_json(Duration::from_secs(7))["retry"]["maxRetries"],
            3
        );
        assert_eq!(
            settings_json(Duration::ZERO),
            json!({"retry": {"enabled": false}})
        );
    }
}
