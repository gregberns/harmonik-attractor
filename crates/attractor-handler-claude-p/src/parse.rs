//! Claude `stream-json` output: the final result line, usage, and the
//! `claude-p` failure table (design §1). Pure functions over the output.

use std::collections::BTreeMap;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::Duration;

use attractor_agent_handler::{AgentResult, AgentStatus, ExitInfo, FailureClass, Usage};
use serde::de::DeserializeOwned;
use serde::Deserialize;

/// Result shape from `claude -p --output-format json`, which is also the final
/// `{"type":"result",...}` line of `--output-format stream-json`.
#[derive(Deserialize)]
struct ClaudeOutput {
    #[serde(default)]
    result: String,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    subtype: String,
    #[serde(default)]
    num_turns: u32,
}

/// The parts of a Claude `stream-json` line read for [`Usage`].
#[derive(Deserialize)]
struct ClaudeUsageLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    subtype: Option<String>,
    model: Option<String>,
    message: Option<ClaudeMessageModel>,
    total_cost_usd: Option<f64>,
    usage: Option<ClaudeUsage>,
    #[serde(rename = "modelUsage")]
    model_usage: Option<BTreeMap<String, ClaudeModelUsage>>,
}

#[derive(Deserialize)]
struct ClaudeMessageModel {
    model: Option<String>,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeModelUsage {
    input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

/// What a finished process left: its exit status and output.
pub struct Exited<'a> {
    pub status: ExitStatus,
    pub stdout: &'a str,
    pub stderr: &'a str,
}

/// Borrow the first `max` characters of `s`. Unlike byte slicing (`&s[..500]`),
/// this never panics on a multi-byte UTF-8 boundary — CLI output is arbitrary
/// text and may contain non-ASCII bytes exactly at the cutoff.
fn head(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// The last `{"type":"result",...}` line of a Claude `stream-json` stdout.
pub fn claude_result_line(stdout: &str) -> Option<&str> {
    stdout.lines().rev().map(str::trim).find(|line| {
        line.starts_with('{')
            && serde_json::from_str::<serde_json::Value>(line)
                .is_ok_and(|value| value.get("type").and_then(|t| t.as_str()) == Some("result"))
    })
}

/// The `claude-p` failure table, first match wins (the timeout and launch
/// rows are decided before there is any output):
/// - a final result line that doesn't deserialize: `NoResult`, whatever the exit;
/// - a final result line with `is_error` or a `subtype` starting `error`: `Reported`;
/// - any other final result line: `Completed`, even after a non-zero exit;
/// - no result line and a non-zero exit or a signal: `Crash`;
/// - no result line and exit 0: `NoResult`.
///
/// Without a result line, stdout as a whole is tried as one result object,
/// as older CLIs and `json` mode print it.
pub fn classify(invocation_id: &str, out: &Exited<'_>, duration: Duration) -> AgentResult {
    let usage = summarize(out.stdout);
    let result_line = claude_result_line(out.stdout);
    let base = AgentResult {
        exit: Some(ExitInfo {
            code: out.status.code(),
            signal: out.status.signal(),
        }),
        duration,
        usage,
        ..AgentResult::failed(invocation_id, FailureClass::NoResult, "")
    };
    if result_line.is_none() && !out.status.success() {
        return AgentResult {
            status: AgentStatus::Failed(FailureClass::Crash),
            detail: format!("exited with {}: {}", out.status, out.stderr.trim()),
            ..base
        };
    }
    if out.stdout.trim().is_empty() {
        return AgentResult {
            detail: format!(
                "Claude Code produced no output. stderr: {}",
                head(out.stderr, 500)
            ),
            ..base
        };
    }
    match serde_json::from_str::<ClaudeOutput>(result_line.unwrap_or(out.stdout)) {
        Err(e) => AgentResult {
            detail: format!(
                "Failed to parse Claude output: {} — raw: {}",
                e,
                head(out.stdout, 500)
            ),
            ..base
        },
        Ok(parsed) => {
            let reported = parsed.is_error || parsed.subtype.starts_with("error");
            AgentResult {
                status: if reported {
                    AgentStatus::Failed(FailureClass::Reported)
                } else {
                    AgentStatus::Completed
                },
                detail: if reported {
                    parsed.result.clone()
                } else {
                    String::new()
                },
                text: parsed.result,
                usage: Usage {
                    turns: Some(parsed.num_turns),
                    ..base.usage.clone()
                },
                ..base
            }
        }
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

/// The model of the `system/init` line (the main loop), else of the last
/// assistant message, else the only `modelUsage` entry. Tokens and cost come
/// from the final `result` line; `modelUsage` also counts subagents. Never
/// fails: missing or unreadable data is `None`. `turns` is left `None`.
pub fn summarize(stdout: &str) -> Usage {
    let mut init_model = None;
    let mut message_model = None;
    for line in json_lines::<ClaudeUsageLine>(stdout) {
        match (line.kind.as_deref(), line.subtype.as_deref()) {
            (Some("system"), Some("init")) if init_model.is_none() => init_model = line.model,
            (Some("assistant"), _) => {
                if let Some(model) = line.message.and_then(|message| message.model) {
                    message_model = Some(model);
                }
            }
            _ => {}
        }
    }
    let result: Option<ClaudeUsageLine> =
        serde_json::from_str(claude_result_line(stdout).unwrap_or(stdout.trim())).ok();
    let Some(result) = result else {
        return Usage {
            model_actual: init_model.or(message_model),
            ..Usage::default()
        };
    };

    let (input_tokens, output_tokens, only_model) = match &result.model_usage {
        Some(models) if !models.is_empty() => (
            sum_present(models.values().map(|m| {
                sum_present([
                    m.input_tokens,
                    m.cache_read_input_tokens,
                    m.cache_creation_input_tokens,
                ])
            })),
            sum_present(models.values().map(|m| m.output_tokens)),
            (models.len() == 1)
                .then(|| models.keys().next().cloned())
                .flatten(),
        ),
        _ => match &result.usage {
            Some(usage) => (
                sum_present([
                    usage.input_tokens,
                    usage.cache_read_input_tokens,
                    usage.cache_creation_input_tokens,
                ]),
                usage.output_tokens,
                None,
            ),
            None => (None, None, None),
        },
    };
    Usage {
        model_actual: init_model.or(message_model).or(only_model),
        input_tokens,
        output_tokens,
        cost_usd: result.total_cost_usd,
        turns: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_RESULT_LINE: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"done","total_cost_usd":0.01,"num_turns":2}"#;

    fn exit(code: i32) -> ExitStatus {
        ExitStatus::from_raw(code << 8)
    }

    fn run(stdout: &str, stderr: &str, code: i32) -> AgentResult {
        classify(
            "inv",
            &Exited {
                status: exit(code),
                stdout,
                stderr,
            },
            Duration::ZERO,
        )
    }

    fn usage(
        model: Option<&str>,
        input: Option<u64>,
        output: Option<u64>,
        cost: Option<f64>,
    ) -> Usage {
        Usage {
            model_actual: model.map(String::from),
            input_tokens: input,
            output_tokens: output,
            cost_usd: cost,
            turns: None,
        }
    }

    #[test]
    fn success_line_completes_with_text_turns_and_cost() {
        let json = r#"{"result":"Hello world","is_error":false,"subtype":"","total_cost_usd":0.05,"num_turns":3}"#;
        let result = run(json, "", 0);
        assert_eq!(result.status, AgentStatus::Completed);
        assert_eq!(result.text, "Hello world");
        assert_eq!(result.usage.cost_usd, Some(0.05));
        assert_eq!(result.usage.turns, Some(3));
        assert_eq!(result.invocation_id, "inv");
        assert_eq!(
            result.exit,
            Some(ExitInfo {
                code: Some(0),
                signal: None
            })
        );
    }

    #[test]
    fn is_error_is_reported_with_the_text_as_detail() {
        let json = r#"{"type":"result","result":"Something failed","is_error":true,"subtype":"error","total_cost_usd":0.01,"num_turns":1}"#;
        let result = run(json, "", 1);
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::Reported));
        assert_eq!(result.text, "Something failed");
        assert_eq!(result.detail, "Something failed");
    }

    #[test]
    fn a_subtype_starting_error_is_reported_even_without_is_error() {
        for subtype in ["error_max_turns", "error_during_execution", "error"] {
            let json = format!(
                r#"{{"type":"result","subtype":"{subtype}","is_error":false,"result":"x"}}"#
            );
            assert_eq!(
                run(&json, "", 0).status,
                AgentStatus::Failed(FailureClass::Reported),
                "{subtype}"
            );
        }
    }

    #[test]
    fn a_result_line_completes_even_after_a_non_zero_exit() {
        let result = run(CLAUDE_RESULT_LINE, "late crash", 3);
        assert_eq!(result.status, AgentStatus::Completed);
        assert_eq!(result.text, "done");
    }

    #[test]
    fn no_result_line_and_non_zero_exit_is_a_crash_with_exit_status_text() {
        let result = run("{\"type\":\"system\"}\n", "  crashed\n", 3);
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::Crash));
        assert_eq!(result.detail, "exited with exit status: 3: crashed");
        assert_eq!(result.exit.and_then(|e| e.code), Some(3));
    }

    #[test]
    fn a_signal_without_result_line_is_a_crash() {
        let result = classify(
            "inv",
            &Exited {
                status: ExitStatus::from_raw(9),
                stdout: "",
                stderr: "",
            },
            Duration::ZERO,
        );
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::Crash));
        assert_eq!(result.exit.and_then(|e| e.signal), Some(9));
    }

    #[test]
    fn empty_stdout_and_exit_zero_is_no_result_with_todays_message() {
        let result = run("", "some error", 0);
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::NoResult));
        assert_eq!(
            result.detail,
            "Claude Code produced no output. stderr: some error"
        );
    }

    #[test]
    fn unparsable_stdout_and_exit_zero_is_no_result_with_the_parse_message() {
        let result = run("not json", "", 0);
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::NoResult));
        assert!(
            result.detail.starts_with("Failed to parse Claude output: "),
            "{}",
            result.detail
        );
        assert!(
            result.detail.ends_with("— raw: not json"),
            "{}",
            result.detail
        );
    }

    #[test]
    fn a_result_line_that_does_not_deserialize_is_no_result_whatever_the_exit() {
        let bad = r#"{"type":"result","subtype":"success","is_error":false,"num_turns":"many"}"#;
        for code in [0, 1] {
            let result = run(bad, "", code);
            assert_eq!(
                result.status,
                AgentStatus::Failed(FailureClass::NoResult),
                "exit {code}"
            );
            assert!(
                result.detail.contains("Failed to parse Claude output"),
                "{}",
                result.detail
            );
        }
    }

    #[test]
    fn stream_uses_final_result_line_like_json_mode() {
        let stream = format!(
            "{}\n{}\n{}\n",
            r#"{"type":"system","subtype":"init","model":"claude-haiku"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"working"}]}}"#,
            CLAUDE_RESULT_LINE
        );
        let streamed = run(&stream, "", 0);
        let single = run(CLAUDE_RESULT_LINE, "", 0);
        assert_eq!(streamed.text, "done");
        assert_eq!(streamed.text, single.text);
        assert_eq!(streamed.status, single.status);
        assert_eq!(streamed.usage.cost_usd, single.usage.cost_usd);
        assert_eq!(streamed.usage.turns, single.usage.turns);
    }

    #[test]
    fn stream_last_result_line_wins_and_crlf_is_accepted() {
        let stream = format!(
            "{}\r\n{}\r\n",
            r#"{"type":"result","result":"first","is_error":false}"#,
            r#"{"type":"result","result":"second","is_error":true,"subtype":"error"}"#
        );
        let result = run(&stream, "", 0);
        assert_eq!(result.text, "second");
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::Reported));
    }

    #[test]
    fn stream_without_result_line_and_exit_zero_is_a_parse_error() {
        let stream = "{\"type\":\"system\"}\n{\"type\":\"assistant\"}\n";
        assert_eq!(claude_result_line(stream), None);
        let result = run(stream, "", 0);
        assert_eq!(result.status, AgentStatus::Failed(FailureClass::NoResult));
        assert!(
            result.detail.contains("Failed to parse Claude output"),
            "{}",
            result.detail
        );
    }

    #[test]
    fn claude_result_line_ignores_non_json_and_non_result_lines() {
        let stream =
            format!("{CLAUDE_RESULT_LINE}\nnot json\n{{\"type\":\"rate_limit_event\"}}\nlast");
        assert_eq!(claude_result_line(&stream), Some(CLAUDE_RESULT_LINE));
    }

    #[test]
    fn head_truncates_multibyte_without_panicking() {
        assert_eq!(head("héllo", 2), "hé");
        assert_eq!(head("hi", 5), "hi");
    }

    // A stream without model or cost data gives `None` for those fields.
    #[test]
    fn stream_without_model_or_cost_yields_none() {
        let bare = r#"{"type":"result","result":"done","is_error":false}"#;
        let result = run(bare, "", 0);
        assert_eq!(result.text, "done");
        assert_eq!(
            result.usage,
            Usage {
                turns: Some(0),
                ..Usage::default()
            }
        );

        let tokens_only =
            r#"{"type":"result","result":"done","usage":{"input_tokens":3,"output_tokens":4}}"#;
        assert_eq!(summarize(tokens_only), usage(None, Some(3), Some(4), None));

        let no_result = "{\"type\":\"system\",\"subtype\":\"init\",\"model\":\"m\"}\n";
        assert_eq!(summarize(no_result), usage(Some("m"), None, None, None));
    }

    #[test]
    fn summaries_of_unreadable_streams_are_empty() {
        let torn = "not json\n{\"type\":\"result\",\"total_cost_usd\":0.5,\"usa";
        let wrong_types = "{\"type\":\"result\",\"total_cost_usd\":\"cheap\",\"result\":\"x\"}\n";
        for stdout in ["", "   \n", "not json at all", torn, wrong_types, "[1,2]"] {
            assert_eq!(summarize(stdout), Usage::default(), "{stdout:?}");
        }
    }

    #[test]
    fn summary_sums_subagent_model_usage_and_keeps_init_model() {
        let stream = format!(
            "{}\n{}\n",
            r#"{"type":"system","subtype":"init","model":"claude-opus"}"#,
            r#"{"type":"result","result":"x","total_cost_usd":1.5,"modelUsage":{"claude-opus":{"inputTokens":10,"outputTokens":20,"cacheReadInputTokens":5},"claude-haiku":{"inputTokens":1,"outputTokens":2,"cacheCreationInputTokens":3}},"usage":{"input_tokens":999,"output_tokens":999}}"#
        );
        assert_eq!(
            summarize(&stream),
            usage(Some("claude-opus"), Some(19), Some(22), Some(1.5))
        );

        // Without init, the last assistant message names the model.
        let stream = format!(
            "{}\n{}\n{}\n",
            r#"{"type":"assistant","message":{"model":"first"}}"#,
            r#"{"type":"assistant","message":{"model":"second"}}"#,
            r#"{"type":"result","result":"x"}"#
        );
        assert_eq!(summarize(&stream).model_actual, Some("second".into()));

        // Without init or messages, a single modelUsage entry names the model.
        let single = r#"{"type":"result","result":"x","modelUsage":{"only":{"outputTokens":1}}}"#;
        assert_eq!(summarize(single), usage(Some("only"), None, Some(1), None));
    }
}
