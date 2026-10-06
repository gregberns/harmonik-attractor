//! Codex `exec --json` output: the last agent message, usage, and the
//! `codex-exec` failure table (design §1). Pure functions over the output.

use std::process::ExitStatus;
use std::time::Duration;

use attractor_agent_handler::{AgentResult, AgentStatus, ExitInfo, FailureClass, Usage};
use attractor_agent_process::stderr_tail;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::CodexExec;

/// Codex JSONL event (tagged enum for streaming deserializer).
/// Source: codex-rs/exec/src/exec_events.rs — ThreadEvent has 8 variants.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum CodexEvent {
    #[serde(rename = "item.completed")]
    ItemCompleted { item: CodexItem },
    #[serde(rename = "turn.failed")]
    TurnFailed { error: Option<CodexError> },
    /// Top-level fatal stream error — distinct from turn.failed.
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(other)]
    Other, // thread.started, turn.started, turn.completed, item.started, item.updated
}

#[derive(Deserialize)]
struct CodexItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct CodexError {
    message: String,
}

/// The parts of a Codex JSONL event read for [`Usage`].
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

/// What a Codex stream says: its answer, and whether it is an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// The last agent message, else the error message, else a note that
    /// there was none.
    pub text: String,
    /// A `turn.failed` or stream `error` event was seen.
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

/// The last agent message and whether the run failed. Malformed events are
/// skipped; never fails.
pub fn parse(stdout: &str) -> Parsed {
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
            Ok(CodexEvent::Other) | Err(_) => {}
        }
    }

    Parsed {
        text: last_message
            .or(error_message)
            .unwrap_or_else(|| "No agent message found in Codex output".into()),
        is_error,
    }
}

/// Whether stdout holds an answer; without one, a non-zero exit is a crash.
/// For Codex, any stdout counts (as before the handler crates).
pub fn has_final_result(stdout: &str) -> bool {
    !stdout.is_empty()
}

/// The `codex-exec` failure table, first match wins (the cancelled, timeout
/// and launch rows are decided before there is any output):
/// - no stdout and a non-zero exit or a signal: `Crash`, with the exit status
///   as `detail` (the stderr is in `stderr_tail`);
/// - blank stdout: `NoResult`, "Codex CLI produced no output";
/// - a `turn.failed` or stream `error` event: `Reported`, with the last
///   agent message (else the error message) as text;
/// - otherwise `Completed` with the last agent message, even after a
///   non-zero exit (with no message, the text says so).
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
                CodexExec::DISPLAY_NAME,
                head(out.stderr, 500)
            ),
            ..base
        };
    }
    let parsed = parse(out.stdout);
    AgentResult {
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

/// Sum of the values present, or `None` if none is.
fn sum_present(values: impl IntoIterator<Item = Option<u64>>) -> Option<u64> {
    values.into_iter().flatten().fold(None, |sum, value| {
        Some(sum.unwrap_or(0u64).saturating_add(value))
    })
}

/// Tokens summed over `turn.completed` events. Codex's stream carries no
/// model name and no cost (verified on Codex CLI 0.151.0). Never fails.
pub fn summarize(stdout: &str) -> Usage {
    let usages: Vec<CodexTokenUsage> = json_lines::<CodexUsageLine>(stdout)
        .filter(|line| line.kind.as_deref() == Some("turn.completed"))
        .filter_map(|line| line.usage)
        .collect();
    Usage {
        input_tokens: sum_present(usages.iter().map(|u| u.input_tokens)),
        output_tokens: sum_present(usages.iter().map(|u| u.output_tokens)),
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

    const MESSAGE: &str =
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"Done"}}"#;
    const FAILED: &str = r#"{"type":"turn.failed","error":{"message":"Rate limited"}}"#;

    #[test]
    fn parse_extracts_the_last_message() {
        let jsonl = concat!(
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"First message"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"Final answer"}}"#,
        );
        let parsed = parse(jsonl);
        assert_eq!(parsed.text, "Final answer");
        assert!(!parsed.is_error);
    }

    #[test]
    fn parse_handles_turn_failed() {
        let parsed = parse(FAILED);
        assert!(parsed.is_error);
        assert_eq!(parsed.text, "Rate limited");
    }

    #[test]
    fn parse_handles_a_stream_error() {
        let parsed = parse(r#"{"type":"error","message":"Connection lost"}"#);
        assert!(parsed.is_error);
        assert_eq!(parsed.text, "Connection lost");
    }

    #[test]
    fn parse_skips_unknown_events() {
        let jsonl = concat!(
            r#"{"type":"thread.started"}"#,
            "\n",
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"Done"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":100,"output_tokens":50}}"#,
        );
        let parsed = parse(jsonl);
        assert_eq!(parsed.text, "Done");
        assert!(!parsed.is_error);
    }

    #[test]
    fn summary_sums_turns() {
        let stream =
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}\n\
            {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":20,\"output_tokens\":2}}\n";
        let usage = summarize(stream);
        assert_eq!(usage.input_tokens, Some(30));
        assert_eq!(usage.output_tokens, Some(3));
        assert_eq!(usage.model_actual, None);
        assert_eq!(usage.cost_usd, None);
    }

    #[test]
    fn summaries_of_unreadable_streams_are_empty() {
        let torn = "not json\n{\"type\":\"turn.completed\",\"usa";
        let wrong_types = "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":\"many\"}}\n";
        for stdout in ["", "   \n", "not json at all", torn, wrong_types, "[1,2]"] {
            assert_eq!(summarize(stdout), Usage::default(), "{stdout:?}");
        }
    }

    // --- The failure table, row by row ---

    #[test]
    fn no_stdout_and_a_non_zero_exit_is_a_crash() {
        let result = classified(3, "", "boom\n");
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::Crash));
        assert_eq!(result.detail, "exited with exit status: 3");
        assert_eq!(result.stderr_tail, "boom");
        assert_eq!(result.exit.unwrap().code, Some(3));
    }

    #[test]
    fn blank_stdout_with_exit_0_is_no_result() {
        let result = classified(0, "  \n", "warning");
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::NoResult));
        assert_eq!(
            result.detail,
            "Codex CLI produced no output. stderr: warning"
        );
    }

    #[test]
    fn a_failed_turn_is_reported_even_after_a_non_zero_exit() {
        let stdout = format!("{MESSAGE}\n{FAILED}\n");
        let result = classified(1, &stdout, "");
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::Reported));
        assert_eq!(result.text, "Done");
        assert_eq!(result.detail, "Done");
    }

    #[test]
    fn a_message_completes_even_after_a_non_zero_exit() {
        let result = classified(2, MESSAGE, "");
        assert_eq!(result.status, AgentStatus::Completed);
        assert_eq!(result.text, "Done");
    }

    #[test]
    fn output_without_a_message_completes_saying_so() {
        // As before the handler crates: unreadable stdout is not a failure.
        let result = classified(0, "not json", "");
        assert_eq!(result.status, AgentStatus::Completed);
        assert_eq!(result.text, "No agent message found in Codex output");
    }
}
