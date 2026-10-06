#![cfg(unix)]
//! The recorded Claude Code 2.1.282 `stream-json` output, through the
//! `claude-p` classifier.

use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::Duration;

use attractor_agent_handler::{AgentResult, AgentStatus, Usage};
use attractor_handler_claude_p::{classify, Exited};

const CLAUDE_FIXTURE: &str =
    include_str!("../../attractor-pipeline/tests/fixtures/providers/claude-2.1.282.stream.jsonl");

fn classify_ok(stdout: &str) -> AgentResult {
    classify(
        "inv",
        &Exited {
            status: ExitStatus::from_raw(0),
            stdout,
            stderr: "",
        },
        Duration::ZERO,
    )
}

/// Add unknown fields to every JSON line (top level and inside the token
/// objects) and an unknown event type before the last line.
fn with_unknown_fields(stdout: &str) -> String {
    let unknown = serde_json::json!({"nested": [1, {"deep": null}], "flag": true});
    let mut lines: Vec<String> = stdout
        .lines()
        .map(|line| {
            let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
            if let Some(object) = value.as_object_mut() {
                object.insert("pas_unknown_field".into(), unknown.clone());
                for key in ["usage", "message"] {
                    if let Some(inner) = object.get_mut(key).and_then(|v| v.as_object_mut()) {
                        inner.insert("pas_unknown_field".into(), unknown.clone());
                    }
                }
            }
            value.to_string()
        })
        .collect();
    let last = lines.len() - 1;
    lines.insert(
        last,
        r#"{"type":"pas_unknown_event","payload":{"x":1}}"#.into(),
    );
    lines.join("\n") + "\n"
}

#[test]
fn recorded_fixture_yields_text_model_tokens_cost() {
    let result = classify_ok(CLAUDE_FIXTURE);
    assert_eq!(result.status, AgentStatus::Completed);
    assert_eq!(result.text, "OK");
    // Input counts uncached (10) + cache-creation (19555) + cache-read (0).
    assert_eq!(
        result.usage,
        Usage {
            model_actual: Some("claude-haiku-4-5-20251001".into()),
            input_tokens: Some(19_565),
            output_tokens: Some(40),
            cost_usd: Some(0.03932),
            turns: Some(1),
        }
    );
}

#[test]
fn unknown_fields_and_event_types_do_not_change_the_result() {
    let widened = with_unknown_fields(CLAUDE_FIXTURE);
    assert!(widened.contains("pas_unknown_field"));
    let expected = classify_ok(CLAUDE_FIXTURE);
    let parsed = classify_ok(&widened);
    assert_eq!(parsed.status, expected.status);
    assert_eq!(parsed.text, expected.text);
    assert_eq!(parsed.usage, expected.usage);
    assert_ne!(parsed.usage, Usage::default());
}
