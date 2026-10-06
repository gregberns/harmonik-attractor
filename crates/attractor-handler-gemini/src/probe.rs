//! The `--output-format` probe: `<command> --help`, once per command.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use attractor_agent_process::process_group::{self, ProcessGroupGuard};

/// The `--output-format` PAS passes to the Gemini CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputFormat {
    Json,
    /// Available from Gemini CLI 0.11.0.
    StreamJson,
}

impl OutputFormat {
    pub fn as_arg(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::StreamJson => "stream-json",
        }
    }
}

/// How long `--help` may take before PAS falls back to `json`.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// `stream-json` if `<command> --help` lists it, else `json`. It runs as the
/// agent would: in `workdir`, with exactly `env`, stdin from `/dev/null`, in
/// its own process group, killed (group and all) after `timeout`. Any
/// failure (cannot start, non-zero exit, timeout) means `json`.
pub async fn probe(
    command: &[String],
    env: &BTreeMap<String, String>,
    workdir: &Path,
    timeout: Duration,
) -> OutputFormat {
    let Some((program, args)) = command.split_first() else {
        return OutputFormat::Json;
    };
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .arg("--help")
        .env_clear()
        .envs(env)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    process_group::configure(&mut cmd);
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            tracing::debug!(?command, %error, "gemini --help could not start");
            return OutputFormat::Json;
        }
    };
    let mut group = ProcessGroupGuard::new(child.id());
    let supported = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => {
            group.disarm();
            [&output.stdout, &output.stderr]
                .iter()
                .any(|bytes| String::from_utf8_lossy(bytes).contains("stream-json"))
        }
        Ok(Ok(output)) => {
            group.disarm();
            tracing::debug!(?command, status = %output.status, "gemini --help failed");
            false
        }
        Ok(Err(error)) => {
            tracing::debug!(?command, %error, "gemini --help failed");
            false
        }
        Err(_elapsed) => {
            tracing::debug!(?command, "gemini --help timed out");
            false
        }
    };
    let format = if supported {
        OutputFormat::StreamJson
    } else {
        OutputFormat::Json
    };
    tracing::debug!(?command, ?format, "Gemini output format");
    format
}
