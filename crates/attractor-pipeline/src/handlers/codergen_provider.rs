use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use attractor_types::{AttractorError, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::execution_plan::LlmProvider;
#[cfg(test)]
pub(super) use crate::execution_plan::LlmProvider as LlmCliProvider;
use crate::graph::PipelineGraph;

/// The providers this module still starts itself. Claude runs through
/// `Agents` (the `claude-p` handler); Codex and Gemini move there in ticket 04.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CliProvider {
    Codex,
    Gemini,
}

impl CliProvider {
    pub(super) fn provider(self) -> LlmProvider {
        match self {
            Self::Codex => LlmProvider::Codex,
            Self::Gemini => LlmProvider::Gemini,
        }
    }

    pub(super) fn display_name(self) -> &'static str {
        self.provider().display_name()
    }
}

// ---------------------------------------------------------------------------
// CLI output structs
// ---------------------------------------------------------------------------

/// Codex JSONL event (tagged enum for streaming deserializer).
/// Source: codex-rs/exec/src/exec_events.rs — ThreadEvent has 8 variants.
#[derive(Deserialize)]
#[serde(tag = "type")]
pub(super) enum CodexEvent {
    #[serde(rename = "item.completed")]
    ItemCompleted { item: CodexItem },
    #[serde(rename = "turn.completed")]
    TurnCompleted {
        #[allow(dead_code)]
        usage: Option<CodexUsage>,
    },
    #[serde(rename = "turn.failed")]
    TurnFailed { error: Option<CodexError> },
    /// Top-level fatal stream error — distinct from turn.failed.
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(other)]
    Other, // Absorbs thread.started, turn.started, item.started, item.updated
}

#[derive(Deserialize)]
pub(super) struct CodexItem {
    #[serde(rename = "type")]
    pub(super) item_type: String,
    #[serde(default)]
    pub(super) text: Option<String>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
pub(super) struct CodexUsage {
    pub(super) input_tokens: i64,
    pub(super) output_tokens: i64,
    #[serde(default)]
    pub(super) cached_input_tokens: i64,
}

#[derive(Deserialize)]
pub(super) struct CodexError {
    pub(super) message: String,
}

/// Gemini JSON output (single object).
/// Source: packages/core/src/output/types.ts — JsonOutput interface.
#[derive(Deserialize)]
pub(super) struct GeminiOutput {
    #[serde(default)]
    #[allow(dead_code)]
    pub(super) session_id: Option<String>,
    #[serde(default)]
    pub(super) response: Option<String>,
    #[serde(default)]
    pub(super) error: Option<GeminiError>,
}

#[derive(Deserialize)]
pub(super) struct GeminiError {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub(super) error_type: String,
    pub(super) message: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub(super) code: Option<serde_json::Value>,
}

/// One line of a Gemini `--output-format stream-json` stream.
/// Source: packages/core/src/output/types.ts — JsonStreamEvent.
#[derive(Deserialize)]
struct GeminiStreamLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    model: Option<String>,
    role: Option<String>,
    content: Option<String>,
    status: Option<String>,
    error: Option<GeminiStreamError>,
    stats: Option<GeminiStreamStats>,
}

#[derive(Deserialize)]
struct GeminiStreamError {
    message: Option<String>,
}

#[derive(Deserialize)]
struct GeminiStreamStats {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    models: Option<BTreeMap<String, GeminiStreamModelStats>>,
}

#[derive(Deserialize)]
struct GeminiStreamModelStats {
    output_tokens: Option<u64>,
}

/// The `stats` part of Gemini `--output-format json` output.
#[derive(Deserialize)]
struct GeminiJsonStats {
    stats: Option<GeminiJsonStatsBody>,
}

#[derive(Deserialize)]
struct GeminiJsonStatsBody {
    models: Option<BTreeMap<String, GeminiJsonModelStats>>,
}

#[derive(Deserialize)]
struct GeminiJsonModelStats {
    tokens: Option<GeminiJsonTokens>,
}

#[derive(Deserialize)]
struct GeminiJsonTokens {
    prompt: Option<u64>,
    candidates: Option<u64>,
}

/// The parts of a Codex JSONL event read for [`InvocationUsage`].
#[derive(Deserialize)]
struct CodexUsageLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    usage: Option<CodexTokenUsage>,
}

#[derive(Deserialize)]
struct CodexTokenUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

/// Facts about one Model Invocation read from its provider stream.
/// Every field is optional: data the stream does not carry is `None`.
#[derive(Debug, Default, Clone, PartialEq)]
pub(super) struct InvocationUsage {
    pub(super) model_actual: Option<String>,
    /// Prompt tokens, including cached and cache-creation tokens.
    pub(super) input_tokens: Option<u64>,
    pub(super) output_tokens: Option<u64>,
    pub(super) cost_usd: Option<f64>,
}

/// Normalized result from any CLI provider.
#[derive(Debug)]
pub(super) struct NormalizedCliResult {
    pub(super) text: String,
    pub(super) is_error: bool,
    pub(super) cost_usd: Option<f64>,
    pub(super) turns: Option<u32>,
    pub(super) usage: InvocationUsage,
    #[allow(dead_code)]
    pub(super) raw_output: String,
}

// ---------------------------------------------------------------------------
// CLI command builder
// ---------------------------------------------------------------------------

pub(super) struct CliRunConfig<'a> {
    pub(super) provider: CliProvider,
    pub(super) prompt: &'a str,
    pub(super) model: Option<&'a str>,
    pub(super) workdir: Option<&'a str>,
    #[allow(dead_code)]
    pub(super) graph: &'a PipelineGraph,
}

/// The `--output-format` PAS passes to the Gemini CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GeminiOutputFormat {
    Json,
    /// Available from Gemini CLI 0.11.0.
    StreamJson,
}

impl GeminiOutputFormat {
    fn as_arg(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::StreamJson => "stream-json",
        }
    }
}

/// How long `gemini --help` may take before PAS falls back to `json`.
const GEMINI_HELP_TIMEOUT: Duration = Duration::from_secs(10);

/// The output format to use for the Gemini CLI at `program`: `stream-json`
/// if its `--help` lists it, otherwise `json`. Probed once per program path
/// per process; any probe failure means `json`.
pub(super) async fn gemini_output_format(program: &Path) -> GeminiOutputFormat {
    static FORMATS: OnceLock<tokio::sync::Mutex<HashMap<PathBuf, GeminiOutputFormat>>> =
        OnceLock::new();
    let mut formats = FORMATS.get_or_init(Default::default).lock().await;
    if let Some(format) = formats.get(program) {
        return *format;
    }
    let format = probe_gemini_output_format(program).await;
    formats.insert(program.to_path_buf(), format);
    format
}

async fn probe_gemini_output_format(program: &Path) -> GeminiOutputFormat {
    let mut cmd = tokio::process::Command::new(program);
    cmd.arg("--help")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let supported = match tokio::time::timeout(GEMINI_HELP_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) if output.status.success() => [&output.stdout, &output.stderr]
            .iter()
            .any(|bytes| String::from_utf8_lossy(bytes).contains("stream-json")),
        Ok(Ok(output)) => {
            tracing::debug!(program = %program.display(), status = %output.status, "gemini --help failed");
            false
        }
        Ok(Err(error)) => {
            tracing::debug!(program = %program.display(), %error, "gemini --help could not start");
            false
        }
        Err(_elapsed) => {
            tracing::debug!(program = %program.display(), "gemini --help timed out");
            false
        }
    };
    let format = if supported {
        GeminiOutputFormat::StreamJson
    } else {
        GeminiOutputFormat::Json
    };
    tracing::debug!(program = %program.display(), ?format, "Gemini output format");
    format
}

#[cfg(test)]
pub(super) fn build_cli_command(cfg: &CliRunConfig<'_>) -> tokio::process::Command {
    build_cli_command_with_program(
        cfg,
        cfg.provider.provider().binary_name().as_ref(),
        GeminiOutputFormat::Json,
    )
}

/// Like [`build_cli_command`], but starts `program` instead of the provider's
/// binary (tests point this at a stub provider script) and passes
/// `gemini_format` to Gemini.
pub(super) fn build_cli_command_with_program(
    cfg: &CliRunConfig<'_>,
    program: &std::ffi::OsStr,
    gemini_format: GeminiOutputFormat,
) -> tokio::process::Command {
    let mut cmd = match cfg.provider {
        CliProvider::Codex => {
            let mut cmd = tokio::process::Command::new(program);
            cmd.arg("exec")
                .arg("--json")
                .arg("--yolo")
                .arg("--skip-git-repo-check")
                .arg("--ephemeral");
            if let Some(model) = cfg.model {
                cmd.arg("--model").arg(model);
            }
            if let Some(dir) = cfg.workdir {
                cmd.arg("--cd").arg(dir);
            }
            // Prompt is POSITIONAL (last arg) — NOT -p (that's --profile in Codex)
            cmd.arg(cfg.prompt);
            cmd
        }
        CliProvider::Gemini => {
            let mut cmd = tokio::process::Command::new(program);
            cmd.arg("--output-format")
                .arg(gemini_format.as_arg())
                .arg("--approval-mode")
                .arg("yolo");
            if let Some(model) = cfg.model {
                cmd.arg("--model").arg(model);
            }
            // Prompt is POSITIONAL (preferred) — -p/--prompt is deprecated
            cmd.arg(cfg.prompt);
            // Gemini has NO --cwd flag — working dir set via cmd.current_dir() only
            cmd
        }
    };

    if let Some(dir) = cfg.workdir {
        cmd.current_dir(dir);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd
}

// ---------------------------------------------------------------------------
// CLI output parsers
// ---------------------------------------------------------------------------

/// Borrow the first `max` characters of `s`. Unlike byte slicing (`&s[..500]`),
/// this never panics on a multi-byte UTF-8 boundary — CLI output is arbitrary
/// text and may contain non-ASCII bytes exactly at the cutoff.
fn head(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

pub(super) fn parse_cli_output(
    provider: CliProvider,
    stdout: &str,
    stderr: &str,
    node_id: &str,
) -> Result<NormalizedCliResult> {
    if stdout.trim().is_empty() {
        return Err(AttractorError::HandlerError {
            handler: "codergen".into(),
            node: node_id.into(),
            message: format!(
                "{} produced no output. stderr: {}",
                provider.display_name(),
                head(stderr, 500)
            ),
        });
    }

    let mut result = match provider {
        CliProvider::Codex => parse_codex_output(stdout, node_id),
        CliProvider::Gemini if is_gemini_stream(stdout) => {
            parse_gemini_stream_output(stdout, node_id)
        }
        CliProvider::Gemini => parse_gemini_output(stdout, node_id),
    }?;
    result.usage = summarize_stream(provider, stdout);
    Ok(result)
}

/// Whether stdout holds the answer the provider ends a run with. Without it,
/// a non-zero exit is reported like an empty stdout.
pub(super) fn has_final_result(provider: CliProvider, stdout: &str) -> bool {
    match provider {
        CliProvider::Gemini if is_gemini_stream(stdout) => json_lines::<GeminiStreamLine>(stdout)
            .any(|line| line.kind.as_deref() == Some("result")),
        CliProvider::Codex | CliProvider::Gemini => !stdout.is_empty(),
    }
}

/// Each line of `stdout` that parses as `T`. Blank lines, non-JSON lines, a
/// torn last line, and lines whose known fields have the wrong type are
/// skipped; unknown fields are ignored.
fn json_lines<T: DeserializeOwned>(stdout: &str) -> impl Iterator<Item = T> + '_ {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .filter_map(|line| serde_json::from_str(line).ok())
}

/// Sum of the values present, or `None` if none is.
fn sum_present(values: impl IntoIterator<Item = Option<u64>>) -> Option<u64> {
    values.into_iter().flatten().fold(None, |sum, value| {
        Some(sum.unwrap_or(0u64).saturating_add(value))
    })
}

/// The model with the most output tokens; ties go to the first name.
fn busiest_model<'a>(
    models: impl IntoIterator<Item = (&'a String, Option<u64>)>,
) -> Option<String> {
    let mut busiest: Option<(&String, u64)> = None;
    for (name, output) in models {
        let output = output.unwrap_or(0);
        if busiest.is_none_or(|(_, most)| output > most) {
            busiest = Some((name, output));
        }
    }
    busiest.map(|(name, _)| name.clone())
}

/// Read the actual model, token counts, and cost of one Model Invocation from
/// its provider stdout. Never fails: missing or unreadable data is `None`.
pub(super) fn summarize_stream(provider: CliProvider, stdout: &str) -> InvocationUsage {
    match provider {
        CliProvider::Codex => summarize_codex(stdout),
        CliProvider::Gemini if is_gemini_stream(stdout) => summarize_gemini_stream(stdout),
        CliProvider::Gemini => summarize_gemini_json(stdout),
    }
}

/// Codex: tokens summed over `turn.completed` events. Its stream carries no
/// model name and no cost (verified on Codex CLI 0.151.0).
fn summarize_codex(stdout: &str) -> InvocationUsage {
    let usages: Vec<CodexTokenUsage> = json_lines::<CodexUsageLine>(stdout)
        .filter(|line| line.kind.as_deref() == Some("turn.completed"))
        .filter_map(|line| line.usage)
        .collect();
    InvocationUsage {
        model_actual: None,
        input_tokens: sum_present(usages.iter().map(|u| u.input_tokens)),
        output_tokens: sum_present(usages.iter().map(|u| u.output_tokens)),
        cost_usd: None,
    }
}

/// Gemini `stream-json`: tokens from the final `result` stats; the model is
/// the one that wrote the most output, else the configured `init` model.
/// Gemini reports no cost.
fn summarize_gemini_stream(stdout: &str) -> InvocationUsage {
    let mut init_model = None;
    let mut stats = None;
    for line in json_lines::<GeminiStreamLine>(stdout) {
        match line.kind.as_deref() {
            Some("init") if init_model.is_none() => init_model = line.model,
            Some("result") => stats = line.stats,
            _ => {}
        }
    }
    let busiest = stats
        .as_ref()
        .and_then(|stats| stats.models.as_ref())
        .and_then(|models| busiest_model(models.iter().map(|(name, m)| (name, m.output_tokens))));
    InvocationUsage {
        model_actual: busiest.or(init_model),
        input_tokens: stats.as_ref().and_then(|stats| stats.input_tokens),
        output_tokens: stats.as_ref().and_then(|stats| stats.output_tokens),
        cost_usd: None,
    }
}

/// Gemini `json`: tokens summed over `stats.models`; the model is the one
/// that wrote the most output. Gemini reports no cost.
fn summarize_gemini_json(stdout: &str) -> InvocationUsage {
    let models = serde_json::from_str::<GeminiJsonStats>(stdout)
        .ok()
        .and_then(|output| output.stats)
        .and_then(|stats| stats.models)
        .unwrap_or_default();
    let tokens = |pick: fn(&GeminiJsonTokens) -> Option<u64>| {
        sum_present(models.values().map(|m| m.tokens.as_ref().and_then(pick)))
    };
    InvocationUsage {
        model_actual: busiest_model(
            models
                .iter()
                .map(|(name, m)| (name, m.tokens.as_ref().and_then(|t| t.candidates))),
        ),
        input_tokens: tokens(|t| t.prompt),
        output_tokens: tokens(|t| t.candidates),
        cost_usd: None,
    }
}

/// Whether Gemini stdout is a `stream-json` stream rather than one `json`
/// object: some line is an object with a `type` field.
fn is_gemini_stream(stdout: &str) -> bool {
    #[derive(Deserialize)]
    struct Typed {
        #[serde(rename = "type")]
        kind: Option<String>,
    }
    json_lines::<Typed>(stdout).any(|line| line.kind.is_some())
}

pub(super) fn parse_codex_output(stdout: &str, node_id: &str) -> Result<NormalizedCliResult> {
    let mut last_message: Option<String> = None;
    let mut is_error = false;
    let mut error_message: Option<String> = None;

    for event in serde_json::Deserializer::from_str(stdout).into_iter::<CodexEvent>() {
        match event {
            Ok(CodexEvent::ItemCompleted { item }) => {
                if item.item_type == "agent_message" {
                    if let Some(text) = item.text {
                        last_message = Some(text);
                    }
                }
            }
            Ok(CodexEvent::TurnFailed { error }) => {
                is_error = true;
                error_message = error.map(|e| e.message);
            }
            Ok(CodexEvent::Error { message }) => {
                is_error = true;
                error_message = Some(message);
            }
            Ok(_) => {}
            Err(e) => {
                tracing::debug!(node = node_id, error = %e, "Skipping malformed Codex JSONL event");
            }
        }
    }

    let text = last_message
        .or(error_message)
        .unwrap_or_else(|| "No agent message found in Codex output".into());

    Ok(NormalizedCliResult {
        text,
        is_error,
        cost_usd: None,
        turns: None,
        usage: InvocationUsage::default(),
        raw_output: stdout.to_string(),
    })
}

pub(super) fn parse_gemini_output(stdout: &str, node_id: &str) -> Result<NormalizedCliResult> {
    let parsed: GeminiOutput =
        serde_json::from_str(stdout).map_err(|e| AttractorError::HandlerError {
            handler: "codergen".into(),
            node: node_id.into(),
            message: format!(
                "Failed to parse Gemini output: {} — raw: {}",
                e,
                head(stdout, 500)
            ),
        })?;

    if let Some(err) = parsed.error {
        return Ok(NormalizedCliResult {
            text: err.message,
            is_error: true,
            cost_usd: None,
            turns: None,
            usage: InvocationUsage::default(),
            raw_output: stdout.to_string(),
        });
    }

    Ok(NormalizedCliResult {
        text: parsed.response.unwrap_or_default(),
        is_error: false,
        cost_usd: None,
        turns: None,
        usage: InvocationUsage::default(),
        raw_output: stdout.to_string(),
    })
}

/// Parse Gemini `stream-json` output. The text is every assistant message
/// joined in order, as `json` mode builds its `response`; a `result` event
/// with `status: "error"` is an error result, like `json` mode's `error`.
pub(super) fn parse_gemini_stream_output(
    stdout: &str,
    node_id: &str,
) -> Result<NormalizedCliResult> {
    let mut text = String::new();
    let mut result = None;
    for line in json_lines::<GeminiStreamLine>(stdout) {
        match line.kind.as_deref() {
            Some("message") if line.role.as_deref() == Some("assistant") => {
                text.push_str(line.content.as_deref().unwrap_or_default());
            }
            Some("result") => result = Some(line),
            _ => {}
        }
    }
    let Some(result) = result else {
        return Err(AttractorError::HandlerError {
            handler: "codergen".into(),
            node: node_id.into(),
            message: format!(
                "Failed to parse Gemini output: no result event — raw: {}",
                head(stdout, 500)
            ),
        });
    };

    let is_error = result.status.as_deref() == Some("error");
    if is_error {
        text = result
            .error
            .and_then(|error| error.message)
            .unwrap_or_else(|| "Gemini reported an error".into());
    }
    Ok(NormalizedCliResult {
        text,
        is_error,
        cost_usd: None,
        turns: None,
        usage: InvocationUsage::default(),
        raw_output: stdout.to_string(),
    })
}

#[cfg(test)]
mod head_tests {
    use super::head;

    #[test]
    fn returns_whole_string_when_shorter_than_max() {
        let s = "€".repeat(400); // 1200 bytes, 400 chars — fewer than 500 chars
        assert_eq!(head(&s, 500), s);
    }

    #[test]
    fn truncates_multibyte_without_panicking() {
        // Byte 500 lands mid-char for a 3-byte character; byte slicing would panic.
        let s = "€".repeat(1000); // 3000 bytes, 1000 chars
        let h = head(&s, 500);
        assert_eq!(h.chars().count(), 500);
        assert!(std::str::from_utf8(h.as_bytes()).is_ok());
    }
}
