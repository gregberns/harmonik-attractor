//! Gemini `json` and `stream-json` output: the answer, usage, and the
//! `gemini` failure table (design §1). Pure functions over the output.

use std::collections::BTreeMap;
use std::process::ExitStatus;
use std::time::Duration;

use attractor_agent_handler::{AgentResult, AgentStatus, ExitInfo, FailureClass, Usage};
use attractor_agent_process::stderr_tail;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::Gemini;

/// Gemini JSON output (single object).
/// Source: packages/core/src/output/types.ts — JsonOutput interface.
#[derive(Deserialize)]
struct GeminiOutput {
    #[serde(default)]
    response: Option<String>,
    #[serde(default)]
    error: Option<GeminiError>,
}

#[derive(Deserialize)]
struct GeminiError {
    message: String,
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

/// What a Gemini answer says: its text, and whether it is an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub text: String,
    pub is_error: bool,
}

/// What a finished process left: its exit status and output.
pub struct Exited<'a> {
    pub status: ExitStatus,
    pub stdout: &'a str,
    pub stderr: &'a str,
}

/// The signal that ended the process, where the platform has signals.
#[cfg(unix)]
fn signal(status: ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn signal(_status: ExitStatus) -> Option<i32> {
    None
}

/// Borrow the first `max` characters of `s`, never splitting a character.
fn head(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Each line of `stdout` that parses as `T`; anything else is skipped.
fn json_lines<T: DeserializeOwned>(stdout: &str) -> impl Iterator<Item = T> + '_ {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .filter_map(|line| serde_json::from_str(line).ok())
}

/// Whether stdout is a `stream-json` stream rather than one `json` object:
/// some line is an object with a `type` field.
pub fn is_stream(stdout: &str) -> bool {
    #[derive(Deserialize)]
    struct Typed {
        #[serde(rename = "type")]
        kind: Option<String>,
    }
    json_lines::<Typed>(stdout).any(|line| line.kind.is_some())
}

/// Whether stdout holds the answer Gemini ends a run with (a stream's
/// `result` event; any `json` output). Without it, a non-zero exit is a
/// crash.
pub fn has_final_result(stdout: &str) -> bool {
    if is_stream(stdout) {
        json_lines::<GeminiStreamLine>(stdout).any(|line| line.kind.as_deref() == Some("result"))
    } else {
        !stdout.is_empty()
    }
}

/// The answer in either format; `Err` holds the parse failure's message.
pub fn parse(stdout: &str) -> Result<Parsed, String> {
    if is_stream(stdout) {
        parse_stream(stdout)
    } else {
        parse_json(stdout)
    }
}

/// `json`: the `response`, or the `error` message as an error.
fn parse_json(stdout: &str) -> Result<Parsed, String> {
    let parsed: GeminiOutput = serde_json::from_str(stdout).map_err(|e| {
        format!(
            "Failed to parse Gemini output: {} — raw: {}",
            e,
            head(stdout, 500)
        )
    })?;
    Ok(match parsed.error {
        Some(error) => Parsed {
            text: error.message,
            is_error: true,
        },
        None => Parsed {
            text: parsed.response.unwrap_or_default(),
            is_error: false,
        },
    })
}

/// `stream-json`: every assistant message joined in order, as `json` mode
/// builds its `response`; a `result` event with `status: "error"` is an
/// error, with its message as text. No `result` event is a parse failure.
fn parse_stream(stdout: &str) -> Result<Parsed, String> {
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
        return Err(format!(
            "Failed to parse Gemini output: no result event — raw: {}",
            head(stdout, 500)
        ));
    };
    let is_error = result.status.as_deref() == Some("error");
    if is_error {
        text = result
            .error
            .and_then(|error| error.message)
            .unwrap_or_else(|| "Gemini reported an error".into());
    }
    Ok(Parsed { text, is_error })
}

/// The `gemini` failure table, first match wins (the cancelled, timeout and
/// launch rows are decided before there is any output):
/// - no final result and a non-zero exit or a signal: `Crash`, with the exit
///   status as `detail` (the stderr is in `stderr_tail`);
/// - blank stdout: `NoResult`, "Gemini CLI produced no output";
/// - output that doesn't parse (or a stream without a `result`): `NoResult`;
/// - an `error` (json) or a `result` with status `error` (stream): `Reported`;
/// - otherwise `Completed`, even after a non-zero exit.
pub fn classify(invocation_id: &str, out: &Exited<'_>, duration: Duration) -> AgentResult {
    let base = AgentResult {
        exit: Some(ExitInfo {
            code: out.status.code(),
            signal: signal(out.status),
        }),
        duration,
        usage: summarize(out.stdout),
        stderr_tail: stderr_tail(out.stderr),
        ..AgentResult::failed(invocation_id, FailureClass::NoResult, "")
    };
    if !out.status.success() && !has_final_result(out.stdout) {
        return AgentResult {
            status: AgentStatus::Failed(FailureClass::Crash),
            detail: format!("exited with {}", out.status),
            ..base
        };
    }
    if out.stdout.trim().is_empty() {
        return AgentResult {
            detail: format!(
                "{} produced no output. stderr: {}",
                Gemini::DISPLAY_NAME,
                head(out.stderr, 500)
            ),
            ..base
        };
    }
    match parse(out.stdout) {
        Err(detail) => AgentResult { detail, ..base },
        Ok(parsed) => AgentResult {
            status: if parsed.is_error {
                AgentStatus::Failed(FailureClass::Reported)
            } else {
                AgentStatus::Completed
            },
            detail: if parsed.is_error {
                parsed.text.clone()
            } else {
                String::new()
            },
            text: parsed.text,
            ..base
        },
    }
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

/// The actual model and token counts of one invocation, from either format.
/// Gemini reports no cost. Never fails: missing or unreadable data is `None`.
pub fn summarize(stdout: &str) -> Usage {
    if is_stream(stdout) {
        summarize_stream(stdout)
    } else {
        summarize_json(stdout)
    }
}

/// `stream-json`: tokens from the final `result` stats; the model is the one
/// that wrote the most output, else the configured `init` model.
fn summarize_stream(stdout: &str) -> Usage {
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
    Usage {
        model_actual: busiest.or(init_model),
        input_tokens: stats.as_ref().and_then(|stats| stats.input_tokens),
        output_tokens: stats.as_ref().and_then(|stats| stats.output_tokens),
        ..Usage::default()
    }
}

/// `json`: tokens summed over `stats.models`; the model is the one that
/// wrote the most output.
fn summarize_json(stdout: &str) -> Usage {
    let models = serde_json::from_str::<GeminiJsonStats>(stdout)
        .ok()
        .and_then(|output| output.stats)
        .and_then(|stats| stats.models)
        .unwrap_or_default();
    let tokens = |pick: fn(&GeminiJsonTokens) -> Option<u64>| {
        sum_present(models.values().map(|m| m.tokens.as_ref().and_then(pick)))
    };
    Usage {
        model_actual: busiest_model(
            models
                .iter()
                .map(|(name, m)| (name, m.tokens.as_ref().and_then(|t| t.candidates))),
        ),
        input_tokens: tokens(|t| t.prompt),
        output_tokens: tokens(|t| t.candidates),
        ..Usage::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn status(code: i32) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(code << 8)
    }

    fn classified(code: i32, stdout: &str, stderr: &str) -> AgentResult {
        classify(
            "inv",
            &Exited {
                status: status(code),
                stdout,
                stderr,
            },
            Duration::ZERO,
        )
    }

    #[test]
    fn json_success() {
        let parsed = parse(r#"{"session_id":"abc","response":"Gemini says hi"}"#).unwrap();
        assert_eq!(parsed.text, "Gemini says hi");
        assert!(!parsed.is_error);
    }

    #[test]
    fn json_error() {
        let parsed =
            parse(r#"{"error":{"type":"api_error","message":"Model not found","code":404}}"#)
                .unwrap();
        assert!(parsed.is_error);
        assert_eq!(parsed.text, "Model not found");
    }

    #[test]
    fn invalid_json_is_a_parse_failure() {
        let error = parse("not json").unwrap_err();
        assert!(
            error.starts_with("Failed to parse Gemini output"),
            "{error}"
        );
    }

    #[test]
    fn a_stream_error_result_is_an_error() {
        let stream = "{\"type\":\"init\",\"model\":\"gemini-2.5-pro\"}\n\
            {\"type\":\"message\",\"role\":\"assistant\",\"content\":\"partial\",\"delta\":true}\n\
            {\"type\":\"result\",\"status\":\"error\",\"error\":{\"type\":\"FatalTurnLimitedError\",\"message\":\"turn limit\"},\"stats\":{\"input_tokens\":5,\"output_tokens\":1}}\n";
        let parsed = parse(stream).unwrap();
        assert!(parsed.is_error);
        assert_eq!(parsed.text, "turn limit");
        let usage = summarize(stream);
        assert_eq!(usage.model_actual.as_deref(), Some("gemini-2.5-pro"));
        assert_eq!(usage.input_tokens, Some(5));
        assert_eq!(usage.output_tokens, Some(1));
    }

    #[test]
    fn a_stream_without_a_result_is_a_parse_failure() {
        let stream = "{\"type\":\"init\",\"model\":\"m\"}\n\
            {\"type\":\"message\",\"role\":\"assistant\",\"content\":\"partial\"}\n";
        let error = parse(stream).unwrap_err();
        assert!(error.contains("Failed to parse Gemini output"), "{error}");
    }

    #[test]
    fn streams_without_model_or_stats_give_none() {
        let stream =
            "{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"hi\",\"delta\":true}\n\
            {\"type\":\"result\",\"status\":\"success\"}\n";
        assert_eq!(parse(stream).unwrap().text, "hi");
        assert_eq!(summarize(stream), Usage::default());
        assert_eq!(parse(r#"{"response":"hi"}"#).unwrap().text, "hi");
        assert_eq!(summarize(r#"{"response":"hi"}"#), Usage::default());
    }

    #[test]
    fn summaries_of_unreadable_streams_are_empty() {
        let torn = "not json\n{\"type\":\"result\",\"stats\":{\"input_tok";
        let wrong_types = "{\"type\":\"result\",\"stats\":{\"input_tokens\":\"many\"}}\n";
        for stdout in ["", "   \n", "not json at all", torn, wrong_types, "[1,2]"] {
            assert_eq!(summarize(stdout), Usage::default(), "{stdout:?}");
        }
    }

    // --- The failure table, row by row ---

    #[test]
    fn no_final_result_and_a_non_zero_exit_is_a_crash() {
        let result = classified(3, "", "boom\n");
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::Crash));
        assert_eq!(result.detail, "exited with exit status: 3");
        assert_eq!(result.stderr_tail, "boom");
        // A stream that ends without its result line is a crash too.
        let torn = "{\"type\":\"init\",\"model\":\"m\"}\n";
        let result = classified(1, torn, "");
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::Crash));
    }

    #[test]
    fn blank_stdout_with_exit_0_is_no_result() {
        let result = classified(0, "\n", "warn");
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::NoResult));
        assert_eq!(result.detail, "Gemini CLI produced no output. stderr: warn");
    }

    #[test]
    fn unparsable_output_is_no_result() {
        let result = classified(0, "not json", "");
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::NoResult));
        assert!(result.detail.starts_with("Failed to parse Gemini output"));
    }

    #[test]
    fn an_error_is_reported_even_after_a_non_zero_exit() {
        let result = classified(1, r#"{"error":{"type":"x","message":"nope"}}"#, "");
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::Reported));
        assert_eq!(result.text, "nope");
    }

    #[test]
    fn an_answer_completes_even_after_a_non_zero_exit() {
        let result = classified(2, r#"{"response":"done"}"#, "");
        assert_eq!(result.status, AgentStatus::Completed);
        assert_eq!(result.text, "done");
    }
}
