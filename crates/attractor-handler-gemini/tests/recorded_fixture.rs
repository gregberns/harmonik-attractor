#![cfg(unix)]
//! The recorded Gemini CLI 0.61.0 `json` and `stream-json` output, through
//! the `gemini` classifier.

use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::Duration;

use attractor_agent_handler::{AgentResult, AgentStatus, Usage};
use attractor_handler_gemini::{classify, Exited};

const STREAM_FIXTURE: &str =
    include_str!("../../attractor-pipeline/tests/fixtures/providers/gemini-0.61.0.stream.jsonl");
const JSON_FIXTURE: &str =
    include_str!("../../attractor-pipeline/tests/fixtures/providers/gemini-0.61.0.json");
const FIXTURE_TEXT: &str = "Checked the file.\nOK";

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

fn expected_usage() -> Usage {
    Usage {
        model_actual: Some("gemini-2.5-pro".into()),
        input_tokens: Some(8_900),
        output_tokens: Some(70),
        ..Usage::default()
    }
}

/// Add unknown fields to every JSON object (top level and inside `stats`
/// and `message`) and, for a stream, an unknown event type before the last
/// line. A `json` fixture is one pretty-printed object.
fn with_unknown_fields(stdout: &str, multi_line_object: bool) -> String {
    fn widen(value: &mut serde_json::Value) {
        let unknown = serde_json::json!({"nested": [1, {"deep": null}], "flag": true});
        if let Some(object) = value.as_object_mut() {
            object.insert("pas_unknown_field".into(), unknown.clone());
            for key in ["usage", "stats", "message"] {
                if let Some(inner) = object.get_mut(key).and_then(|v| v.as_object_mut()) {
                    inner.insert("pas_unknown_field".into(), unknown.clone());
                }
            }
        }
    }
    if multi_line_object {
        let mut value: serde_json::Value = serde_json::from_str(stdout).unwrap();
        widen(&mut value);
        return serde_json::to_string_pretty(&value).unwrap();
    }
    let mut lines: Vec<String> = stdout
        .lines()
        .map(|line| {
            let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
            widen(&mut value);
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

// The model that wrote the most output wins over the configured `init`
// model; Gemini reports no cost.
#[test]
fn stream_fixture_yields_text_model_tokens() {
    let result = classify_ok(STREAM_FIXTURE);
    assert_eq!(result.status, AgentStatus::Completed);
    assert_eq!(result.text, FIXTURE_TEXT);
    assert_eq!(result.usage, expected_usage());
}

#[test]
fn json_fixture_gives_the_same_text_and_summary() {
    let result = classify_ok(JSON_FIXTURE);
    assert_eq!(result.status, AgentStatus::Completed);
    assert_eq!(result.text, FIXTURE_TEXT);
    assert_eq!(result.usage, expected_usage());
}

#[test]
fn unknown_fields_and_event_types_do_not_change_the_result() {
    for (fixture, multi_line_object) in [(STREAM_FIXTURE, false), (JSON_FIXTURE, true)] {
        let widened = with_unknown_fields(fixture, multi_line_object);
        assert!(widened.contains("pas_unknown_field"));
        let expected = classify_ok(fixture);
        let parsed = classify_ok(&widened);
        assert_eq!(parsed.status, expected.status);
        assert_eq!(parsed.text, expected.text);
        assert_eq!(parsed.usage, expected.usage);
        assert_ne!(parsed.usage, Usage::default());
    }
}
