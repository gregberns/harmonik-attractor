//! Pi's `--mode json` output: the assistant messages, usage, and the `pi`
//! failure table (design §1). Pure functions over the output.

use std::process::ExitStatus;
use std::time::Duration;

use attractor_agent_handler::{AgentResult, AgentStatus, ExitInfo, FailureClass, Usage};
use attractor_agent_process::stderr_tail;
use serde::Deserialize;
use serde_json::Value;

/// One JSONL line, as far as it is read: its type and, for `message_end`,
/// the message.
#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Message {
    role: Option<String>,
    /// A list of parts for assistant messages; anything else is ignored.
    content: Option<Value>,
    stop_reason: Option<String>,
    error_message: Option<String>,
    model: Option<String>,
    usage: Option<PiUsage>,
}

#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
struct PiUsage {
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    cost: Option<PiCost>,
}

#[derive(Deserialize, Clone, Copy, Default)]
struct PiCost {
    total: Option<f64>,
}

/// One assistant `message_end`: how it stopped and what it said.
#[derive(Debug, Clone, PartialEq)]
pub struct Assistant {
    pub stop_reason: Option<String>,
    pub error_message: Option<String>,
    /// The text parts of its content, joined.
    pub text: String,
}

/// What a finished process left: its exit status and output.
pub struct Exited<'a> {
    pub status: ExitStatus,
    pub stdout: &'a str,
    pub stderr: &'a str,
}

/// Each line of `stdout` that is a `message_end` of an assistant message;
/// unknown types and malformed lines are skipped.
fn assistant_messages(stdout: &str) -> impl Iterator<Item = Message> + '_ {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .filter_map(|line| serde_json::from_str::<Line>(line).ok())
        .filter(|line| line.kind.as_deref() == Some("message_end"))
        .filter_map(|line| line.message)
        .filter(|message| message.role.as_deref() == Some("assistant"))
}

/// The text parts of a message's content, joined.
fn text_of(content: Option<&Value>) -> String {
    content
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// The last assistant message, if Pi printed one.
pub fn last_assistant(stdout: &str) -> Option<Assistant> {
    assistant_messages(stdout).last().map(|message| Assistant {
        text: text_of(message.content.as_ref()),
        stop_reason: message.stop_reason,
        error_message: message.error_message,
    })
}

/// Sum of the values present, or `None` if none is.
fn sum_present(values: impl IntoIterator<Item = Option<u64>>) -> Option<u64> {
    values.into_iter().flatten().fold(None, |sum, value| {
        Some(sum.unwrap_or(0u64).saturating_add(value))
    })
}

/// Tokens and cost summed over the assistant messages; the model is the
/// last message's. Input tokens include cache reads and writes. Never fails.
pub fn summarize(stdout: &str) -> Usage {
    let messages: Vec<Message> = assistant_messages(stdout).collect();
    let usages: Vec<PiUsage> = messages.iter().filter_map(|m| m.usage).collect();
    let costs: Vec<f64> = usages
        .iter()
        .filter_map(|u| u.cost.and_then(|c| c.total))
        .collect();
    Usage {
        model_actual: messages.iter().rev().find_map(|m| m.model.clone()),
        input_tokens: sum_present(
            usages
                .iter()
                .flat_map(|u| [u.input, u.cache_read, u.cache_write]),
        ),
        output_tokens: sum_present(usages.iter().map(|u| u.output)),
        cost_usd: (!costs.is_empty()).then(|| costs.iter().sum()),
        turns: None,
    }
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

/// The `pi` failure table, first match wins (the cancelled, timeout and
/// launch rows are decided before there is any output). By the last
/// assistant `message_end`'s `stopReason`:
/// - `error` or `aborted`: `Reported`, with `errorMessage` (else the
///   reason) as detail and text;
/// - `stop`: `Completed` with the message's text;
/// - any other reason (`toolUse`, `length`, one a later Pi adds) or none:
///   `NoResult` naming it, since Pi is unpinned and an unexpected reason is
///   never a hidden success.
///
/// With no assistant message: `Crash` after a non-zero exit or a signal,
/// else `NoResult`.
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
    match last_assistant(out.stdout) {
        Some(message) => from_message(message, base),
        None if !out.status.success() => AgentResult {
            status: AgentStatus::Failed(FailureClass::Crash),
            detail: format!("exited with {}", out.status),
            ..base
        },
        None => AgentResult {
            detail: "pi printed no assistant message".into(),
            ..base
        },
    }
}

/// The table's rows for a run that printed an assistant message.
fn from_message(message: Assistant, base: AgentResult) -> AgentResult {
    match message.stop_reason.as_deref() {
        Some("stop") => AgentResult {
            status: AgentStatus::Completed,
            text: message.text,
            ..base
        },
        Some(reason @ ("error" | "aborted")) => {
            let detail = message.error_message.unwrap_or_else(|| reason.to_string());
            AgentResult {
                status: AgentStatus::Failed(FailureClass::Reported),
                text: detail.clone(),
                detail,
                ..base
            }
        }
        other => {
            let reason = other.unwrap_or("(none)");
            let detail = match message.text.as_str() {
                "" => format!("pi ended with stopReason {reason}"),
                text => format!("pi ended with stopReason {reason}: {text}"),
            };
            AgentResult {
                status: AgentStatus::Failed(FailureClass::NoResult),
                text: message.text,
                detail,
                ..base
            }
        }
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

    fn classified(code: i32, stdout: &str) -> AgentResult {
        classify(
            "inv",
            &Exited {
                status: status(code),
                stdout,
                stderr: "",
            },
            Duration::ZERO,
        )
    }

    const HEADER: &str = r#"{"type":"session","version":3,"id":"s1","cwd":"/w"}
{"type":"agent_start"}
"#;
    const END: &str = r#"{"type":"agent_end"}
{"type":"agent_settled"}
"#;

    fn assistant(stop: &str, text: &str, extra: &str) -> String {
        format!(
            r#"{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"thinking","thinking":"hm"}},{{"type":"text","text":"{text}"}}],"stopReason":"{stop}"{extra},"provider":"p","model":"m-{stop}","usage":{{"input":10,"output":5,"cacheRead":2,"cacheWrite":1,"totalTokens":18,"cost":{{"total":0.25}}}}}}}}"#
        )
    }

    fn run(lines: &[String]) -> String {
        format!("{HEADER}{}\n{END}", lines.join("\n"))
    }

    #[test]
    fn stop_completes_with_the_text_parts() {
        let result = classified(0, &run(&[assistant("stop", "done", "")]));
        assert_eq!(result.status, AgentStatus::Completed);
        assert_eq!(result.text, "done");
    }

    #[test]
    fn tool_use_messages_before_the_final_stop_count_only_for_usage() {
        let out = run(&[
            assistant("toolUse", "using a tool", ""),
            r#"{"type":"message_end","message":{"role":"toolResult","content":[{"type":"text","text":"ok"}]}}"#.into(),
            assistant("stop", "done after tool", ""),
        ]);
        let result = classified(0, &out);
        assert_eq!(result.status, AgentStatus::Completed);
        assert_eq!(result.text, "done after tool");
        assert_eq!(result.usage.input_tokens, Some(26));
        assert_eq!(result.usage.output_tokens, Some(10));
        assert_eq!(result.usage.cost_usd, Some(0.5));
        assert_eq!(result.usage.model_actual.as_deref(), Some("m-stop"));
    }

    #[test]
    fn error_is_reported_with_its_error_message() {
        let out = run(&[assistant(
            "error",
            "",
            r#","errorMessage":"401: fake auth error""#,
        )]);
        let result = classified(0, &out);
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::Reported));
        assert_eq!(result.detail, "401: fake auth error");
        assert_eq!(result.text, "401: fake auth error");
    }

    #[test]
    fn aborted_without_an_error_message_is_reported_naming_the_reason() {
        let result = classified(0, &run(&[assistant("aborted", "", "")]));
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::Reported));
        assert_eq!(result.detail, "aborted");
    }

    #[test]
    fn any_other_last_stop_reason_is_no_result_naming_it() {
        for reason in ["toolUse", "length", "somethingNew"] {
            let result = classified(0, &run(&[assistant(reason, "partial", "")]));
            assert_eq!(
                result.status,
                AgentStatus::Failed(FailureClass::NoResult),
                "{reason}"
            );
            assert_eq!(
                result.detail,
                format!("pi ended with stopReason {reason}: partial")
            );
            assert_eq!(result.text, "partial");
        }
    }

    #[test]
    fn a_message_without_a_stop_reason_is_no_result() {
        let out =
            run(&[r#"{"type":"message_end","message":{"role":"assistant","content":[]}}"#.into()]);
        let result = classified(0, &out);
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::NoResult));
        assert_eq!(result.detail, "pi ended with stopReason (none)");
    }

    #[test]
    fn no_assistant_message_is_no_result_after_exit_0_and_a_crash_otherwise() {
        let out = run(&[]);
        let quiet = classified(0, &out);
        assert_eq!(quiet.status, AgentStatus::Failed(FailureClass::NoResult));
        assert_eq!(quiet.detail, "pi printed no assistant message");
        let crashed = classified(3, "");
        assert_eq!(crashed.status, AgentStatus::Failed(FailureClass::Crash));
        assert_eq!(crashed.exit.as_ref().and_then(|e| e.code), Some(3));
    }

    #[test]
    fn a_final_message_wins_over_a_non_zero_exit() {
        let result = classified(1, &run(&[assistant("stop", "done", "")]));
        assert_eq!(result.status, AgentStatus::Completed);
    }

    #[test]
    fn malformed_lines_and_unknown_types_are_skipped() {
        let out = format!(
            "not json\n{{\"type\":\"message_end\",\"message\":\n{{\"type\":\"turn_end\"}}\n{}\n{{\"type\":\"message_end\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n",
            assistant("stop", "done", "")
        );
        let result = classified(0, &out);
        assert_eq!(result.status, AgentStatus::Completed);
        assert_eq!(result.text, "done");
    }

    #[test]
    fn summarize_without_messages_is_empty() {
        assert_eq!(summarize(HEADER), Usage::default());
    }
}
