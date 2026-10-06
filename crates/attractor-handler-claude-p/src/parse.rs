//! Claude `stream-json` output: the final result line, usage, and the
//! `claude-p` failure table (design §1). Pure functions over the output.

use std::collections::BTreeMap;
use std::process::ExitStatus;
use std::time::Duration;

use attractor_agent_handler::{AgentResult, AgentStatus, ExitInfo, FailureClass, Usage};
use attractor_agent_process::stderr_tail;
use serde::de::DeserializeOwned;
use serde::Deserialize;

/// Result shape from `claude -p --output-format json`, which is also the final
/// `{"type":"result",...}` line of `--output-format stream-json`.
#[derive(Deserialize)]
struct ClaudeOutput {
    #[serde(default)]
    result: String,
    // Read only to check their types, as before D7.
    #[serde(default)]
    #[allow(dead_code)]
    is_error: bool,
    #[serde(default)]
    #[allow(dead_code)]
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

/// The `claude-p` failure table, first match wins (the cancelled, timeout
/// and launch rows are decided before there is any output):
/// - a final result line with `is_error: true` or a `subtype` starting
///   `error`: `Reported`, whatever its other fields (design D7: Claude's
///   "No conversation found" line may lack `result` or `num_turns`), with
///   its `errors` joined with "; " as `detail`, else its `result`;
/// - a final result line that doesn't deserialize: `NoResult`, whatever the exit;
/// - any other final result line: `Completed`, even after a non-zero exit;
/// - no result line and a non-zero exit or a signal: `Crash`, with the exit
///   status as `detail` (the stderr is in `stderr_tail`);
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
            signal: signal(out.status),
        }),
        duration,
        usage,
        stderr_tail: stderr_tail(out.stderr),
        agent_session_id: claude_session_id(out.stdout),
        ..AgentResult::failed(invocation_id, FailureClass::NoResult, "")
    };
    if result_line.is_none() && !out.status.success() {
        return AgentResult {
            status: AgentStatus::Failed(FailureClass::Crash),
            detail: format!("exited with {}", out.status),
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
    let line = result_line.unwrap_or(out.stdout);
    if let Some(error) = reported_error(line) {
        return AgentResult {
            status: AgentStatus::Failed(FailureClass::Reported),
            text: error.text,
            detail: error.detail,
            usage: Usage {
                turns: error.turns,
                ..base.usage.clone()
            },
            ..base
        };
    }
    match serde_json::from_str::<ClaudeOutput>(line) {
        Err(e) => AgentResult {
            detail: format!(
                "Failed to parse Claude output: {} — raw: {}",
                e,
                head(out.stdout, 500)
            ),
            ..base
        },
        // `reported_error` has taken every error line: this one succeeded.
        Ok(parsed) => AgentResult {
            status: AgentStatus::Completed,
            detail: String::new(),
            text: parsed.result,
            usage: Usage {
                turns: Some(parsed.num_turns),
                ..base.usage.clone()
            },
            ..base
        },
    }
}

/// An attempt the API rate-limited (design §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimit {
    /// When the limit resets (`rate_limit_event`'s `resetsAt`, Unix
    /// seconds); `None` when only the error text said so.
    pub resets_at: Option<u64>,
    /// The result's error text (its `errors`, else its `result`).
    pub text: String,
}

/// The `rate_limit_info` of a `rate_limit_event` line.
#[derive(Deserialize)]
struct RateLimitLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    rate_limit_info: Option<RateLimitInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitInfo {
    status: Option<String>,
    resets_at: Option<u64>,
}

/// Whether the attempt was rate limited: its final result is an error
/// (`is_error`, or an `error*` subtype) and either the last
/// `rate_limit_event` before it has a `status` other than `allowed` or
/// `allowed_warning` (Claude's "close to the limit"), or the error text
/// says "rate limit", "usage limit" (any case) or `429` as a whole word.
/// `None` for anything else: 05's failure table applies.
pub fn rate_limited(stdout: &str) -> Option<RateLimit> {
    let line = claude_result_line(stdout)?;
    let error = reported_error(line)?;
    let limited_event = json_lines::<RateLimitLine>(stdout)
        .filter(|line| line.kind.as_deref() == Some("rate_limit_event"))
        .filter_map(|line| line.rate_limit_info)
        .last()
        .filter(|info| {
            !matches!(
                info.status.as_deref(),
                Some("allowed" | "allowed_warning") | None
            )
        });
    match limited_event {
        Some(info) => Some(RateLimit {
            resets_at: info.resets_at,
            text: error.detail,
        }),
        None if says_rate_limited(&error.detail) || says_rate_limited(&error.text) => {
            Some(RateLimit {
                resets_at: None,
                text: error.detail,
            })
        }
        None => None,
    }
}

/// "rate limit", "usage limit" (any case), or `429` as a whole word (so
/// "port 4291" doesn't count).
fn says_rate_limited(text: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.contains("rate limit") || lower.contains("usage limit") {
        return true;
    }
    text.match_indices("429").any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + 3..].chars().next();
        !before.is_some_and(|c| c.is_alphanumeric()) && !after.is_some_and(|c| c.is_alphanumeric())
    })
}

/// An error the agent reported on its result line.
struct ReportedError {
    text: String,
    detail: String,
    turns: Option<u32>,
}

/// The error a result line reports, read field by field so a line with
/// missing or odd fields is still the agent's error: `is_error: true` or a
/// `subtype` starting `error`. `None` for any other line (or non-JSON).
fn reported_error(line: &str) -> Option<ReportedError> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let is_error = value.get("is_error").and_then(serde_json::Value::as_bool) == Some(true);
    let error_subtype = value
        .get("subtype")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|subtype| subtype.starts_with("error"));
    if !is_error && !error_subtype {
        return None;
    }
    let result = value
        .get("result")
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    let errors: Vec<&str> = value
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect()
        })
        .unwrap_or_default();
    let detail = if errors.is_empty() {
        result.clone().unwrap_or_default()
    } else {
        errors.join("; ")
    };
    let turns = match value.get("num_turns") {
        None => Some(0),
        Some(turns) => turns.as_u64().and_then(|n| u32::try_from(n).ok()),
    };
    Some(ReportedError {
        text: result.unwrap_or_else(|| detail.clone()),
        detail,
        turns,
    })
}

/// The `session_id` of the first `system`/`init` line: the session the
/// agent runs in (new or continued). `None` when there is no such line.
pub fn claude_session_id(stdout: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct InitLine {
        #[serde(rename = "type")]
        kind: Option<String>,
        subtype: Option<String>,
        session_id: Option<String>,
    }
    json_lines::<InitLine>(stdout)
        .find(|line| {
            line.kind.as_deref() == Some("system") && line.subtype.as_deref() == Some("init")
        })
        .and_then(|line| line.session_id)
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

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::process::ExitStatusExt;

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
        assert_eq!(result.detail, "exited with exit status: 3");
        assert_eq!(result.stderr_tail, "  crashed");
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

    const NOT_FOUND: &str = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["No conversation found with session ID: x"]}"#;

    #[test]
    fn a_minimal_not_found_line_is_reported_with_the_errors_as_detail() {
        for code in [0, 1] {
            let result = run(
                NOT_FOUND,
                "No conversation found with session ID: x\n",
                code,
            );
            assert_eq!(
                result.status,
                AgentStatus::Failed(FailureClass::Reported),
                "exit {code}"
            );
            assert_eq!(result.detail, "No conversation found with session ID: x");
        }
    }

    #[test]
    fn several_errors_are_joined_with_semicolons() {
        let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["one","two"],"result":"ignored"}"#;
        assert_eq!(run(line, "", 1).detail, "one; two");
    }

    #[test]
    fn an_error_line_is_reported_before_its_other_fields_are_checked() {
        // `num_turns` has the wrong type: a success line like this is
        // NoResult, but an error line is still the agent's reported error.
        for line in [
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":"many","result":"boom"}"#,
            r#"{"type":"result","subtype":"error_max_turns","num_turns":"many","result":"boom"}"#,
        ] {
            let result = run(line, "", 1);
            assert_eq!(
                result.status,
                AgentStatus::Failed(FailureClass::Reported),
                "{line}"
            );
            assert_eq!(result.detail, "boom");
            assert_eq!(result.text, "boom");
        }
    }

    #[test]
    fn the_session_id_comes_from_the_init_line_whatever_the_end() {
        let init = r#"{"type":"system","subtype":"init","session_id":"sess-9","model":"m"}"#;
        let cases = [
            (format!("{init}\n{CLAUDE_RESULT_LINE}\n"), 0),
            (format!("{init}\n{NOT_FOUND}\n"), 1),
            (format!("{init}\n"), 3),
            (format!("{init}\n"), 0),
            (format!("{init}\nnot json\n"), 0),
        ];
        for (stdout, code) in cases {
            assert_eq!(
                run(&stdout, "", code).agent_session_id.as_deref(),
                Some("sess-9"),
                "{stdout:?} exit {code}"
            );
        }
    }

    #[test]
    fn session_id_reads_only_the_init_line() {
        assert_eq!(claude_session_id(""), None);
        assert_eq!(claude_session_id(CLAUDE_RESULT_LINE), None);
        let other = r#"{"type":"assistant","session_id":"not-this"}"#;
        assert_eq!(claude_session_id(other), None);
        let no_id = r#"{"type":"system","subtype":"init"}"#;
        assert_eq!(claude_session_id(no_id), None);
        let torn = "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s";
        assert_eq!(claude_session_id(torn), None);
        let two = "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"a\"}\n{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"b\"}";
        assert_eq!(claude_session_id(two).as_deref(), Some("a"));
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

    fn limited(stdout: &str) -> Option<RateLimit> {
        rate_limited(stdout)
    }

    const ERROR_RESULT: &str = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"API Error: Rate limit reached"}"#;

    fn event(status: &str, resets_at: u64) -> String {
        format!(
            r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"{status}","resetsAt":{resets_at},"rateLimitType":"five_hour"}}}}"#
        )
    }

    #[test]
    fn a_rejected_event_before_an_error_result_is_a_rate_limit() {
        let stdout = format!("{}\n{ERROR_RESULT}\n", event("rejected", 1_700_000_000));
        assert_eq!(
            limited(&stdout),
            Some(RateLimit {
                resets_at: Some(1_700_000_000),
                text: "API Error: Rate limit reached".into()
            })
        );
    }

    #[test]
    fn allowed_and_allowed_warning_are_not_rate_limits() {
        let quiet = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"tests failed"}"#;
        for status in ["allowed", "allowed_warning"] {
            let stdout = format!("{}\n{quiet}\n", event(status, 1));
            assert_eq!(limited(&stdout), None, "{status}");
        }
    }

    #[test]
    fn the_error_text_alone_can_say_so() {
        for text in [
            "API Error: Rate limit reached",
            "Claude AI usage limit reached|1700000000",
            "HTTP 429 Too Many Requests",
        ] {
            let stdout = format!(
                r#"{{"type":"result","subtype":"error_during_execution","is_error":true,"result":"{text}"}}"#
            );
            assert_eq!(
                limited(&stdout).map(|rl| rl.resets_at),
                Some(None),
                "{text}"
            );
        }
    }

    #[test]
    fn other_errors_and_429_inside_a_number_are_not_rate_limits() {
        for text in ["tests failed", "listening on port 4291", "id 14290"] {
            let stdout = format!(
                r#"{{"type":"result","subtype":"error_during_execution","is_error":true,"result":"{text}"}}"#
            );
            assert_eq!(limited(&stdout), None, "{text}");
        }
    }

    #[test]
    fn a_successful_result_is_never_a_rate_limit() {
        let stdout = format!(
            "{}\n{}\n",
            event("rejected", 1),
            r#"{"type":"result","subtype":"success","is_error":false,"result":"done"}"#
        );
        assert_eq!(limited(&stdout), None);
    }
}
