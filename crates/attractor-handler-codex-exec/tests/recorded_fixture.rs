#![cfg(unix)]
//! The recorded Codex CLI 0.151.0 `exec --json` output, through the
//! `codex-exec` classifier.

use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::Duration;

use attractor_agent_handler::{AgentResult, AgentStatus, Usage};
use attractor_handler_codex_exec::{classify, Exited};

const CODEX_FIXTURE: &str =
    include_str!("../../attractor-pipeline/tests/fixtures/providers/codex-0.151.0.jsonl");

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

/// Add unknown fields to every JSON line (top level and inside `usage`) and
/// an unknown event type before the last line.
fn with_unknown_fields(stdout: &str) -> String {
    let unknown = serde_json::json!({"nested": [1, {"deep": null}], "flag": true});
    let mut lines: Vec<String> = stdout
        .lines()
        .map(|line| {
            let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
            if let Some(object) = value.as_object_mut() {
                object.insert("pas_unknown_field".into(), unknown.clone());
                if let Some(inner) = object.get_mut("usage").and_then(|v| v.as_object_mut()) {
                    inner.insert("pas_unknown_field".into(), unknown.clone());
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

// Codex reports neither model nor cost.
#[test]
fn recorded_fixture_yields_text_and_tokens() {
    let result = classify_ok(CODEX_FIXTURE);
    assert_eq!(result.status, AgentStatus::Completed);
    assert_eq!(result.text, "OK");
    assert_eq!(
        result.usage,
        Usage {
            input_tokens: Some(20_446),
            output_tokens: Some(5),
            ..Usage::default()
        }
    );
}

#[test]
fn unknown_fields_and_event_types_do_not_change_the_result() {
    let widened = with_unknown_fields(CODEX_FIXTURE);
    assert!(widened.contains("pas_unknown_field"));
    let expected = classify_ok(CODEX_FIXTURE);
    let parsed = classify_ok(&widened);
    assert_eq!(parsed.status, expected.status);
    assert_eq!(parsed.text, expected.text);
    assert_eq!(parsed.usage, expected.usage);
    assert_ne!(parsed.usage, Usage::default());
}
