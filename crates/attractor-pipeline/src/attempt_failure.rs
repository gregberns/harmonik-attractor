//! The note an agent gets when its node runs again after a failed attempt
//! (ticket 11, design §1 "Prompt on a re-run"): the previous attempt's
//! failure class and reason. Pure: the engine records an [`AttemptFailure`]
//! per node in the checkpoint and passes [`failure_note`] to the handler.

use attractor_types::{AttractorError, Outcome, StageStatus};
use serde::{Deserialize, Serialize};

/// How much of a reason the note keeps: its last bytes, so a huge error
/// does not swamp the prompt.
const REASON_BYTES: usize = 2000;

/// How a node's last finished attempt failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttemptFailure {
    /// The attempt's timeout fired.
    TimedOut { timeout_ms: u64 },
    /// The attempt failed. `class` is the `Pas-Failure-Class` name
    /// (`reported`, `crash`, `no_result`, `launch`), when it has one.
    Failed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<String>,
        reason: String,
    },
    /// The node returned a Retry outcome.
    AskedToRetry { reason: String },
}

/// The failure a finished attempt leaves for the node's next attempt;
/// `None` for a success (which clears it) and for a cancelled attempt
/// (which leaves the previous failure as it was).
pub fn attempt_failure(result: &Result<Outcome, AttractorError>) -> Option<AttemptFailure> {
    match result {
        Ok(outcome) => match outcome.status {
            StageStatus::Success | StageStatus::PartialSuccess | StageStatus::Skipped => None,
            StageStatus::Fail => Some(AttemptFailure::Failed {
                class: Some("reported".into()),
                // The agent's own text; else the generic failure reason.
                reason: if outcome.notes.trim().is_empty() {
                    outcome.failure_reason.clone().unwrap_or_default()
                } else {
                    outcome.notes.clone()
                },
            }),
            StageStatus::Retry => Some(AttemptFailure::AskedToRetry {
                reason: outcome.notes.clone(),
            }),
        },
        Err(AttractorError::Cancelled { .. }) => None,
        Err(
            AttractorError::AgentTimeout { timeout_ms, .. }
            | AttractorError::CommandTimeout { timeout_ms },
        ) => Some(AttemptFailure::TimedOut {
            timeout_ms: *timeout_ms,
        }),
        Err(AttractorError::AgentFailed { kind, message, .. }) => Some(AttemptFailure::Failed {
            class: Some(kind.as_str().into()),
            reason: message.clone(),
        }),
        Err(error) => Some(AttemptFailure::Failed {
            class: error.failure_kind().map(|kind| kind.as_str().into()),
            reason: error.to_string(),
        }),
    }
}

/// The note for the next attempt, e.g. "The previous attempt timed out
/// after 600 s." or "The previous attempt failed (reported): tests failed".
pub fn failure_note(failure: &AttemptFailure) -> String {
    let with_reason = |head: String, reason: &str| {
        let reason = bounded(reason);
        if reason.is_empty() {
            format!("{head}.")
        } else {
            format!("{head}: {reason}")
        }
    };
    match failure {
        AttemptFailure::TimedOut { timeout_ms } => {
            format!(
                "The previous attempt timed out after {}.",
                duration(*timeout_ms)
            )
        }
        AttemptFailure::Failed {
            class: Some(class),
            reason,
        } => with_reason(format!("The previous attempt failed ({class})"), reason),
        AttemptFailure::Failed {
            class: None,
            reason,
        } => with_reason("The previous attempt failed".into(), reason),
        AttemptFailure::AskedToRetry { reason } => {
            with_reason("The previous attempt asked to be retried".into(), reason)
        }
    }
}

/// `600 s` for a whole number of seconds, else `1500 ms`.
fn duration(ms: u64) -> String {
    if ms.is_multiple_of(1000) {
        format!("{} s", ms / 1000)
    } else {
        format!("{ms} ms")
    }
}

/// `reason` trimmed, then at most its last [`REASON_BYTES`] bytes, cut on a
/// char boundary (so possibly a little fewer bytes).
fn bounded(reason: &str) -> &str {
    let reason = reason.trim();
    let mut start = reason.len().saturating_sub(REASON_BYTES);
    while !reason.is_char_boundary(start) {
        start += 1;
    }
    reason[start..].trim_start()
}

#[cfg(test)]
mod tests {
    use super::*;
    use attractor_types::FailureKind;

    fn outcome(status: StageStatus, notes: &str, failure_reason: Option<&str>) -> Outcome {
        Outcome {
            status,
            notes: notes.into(),
            failure_reason: failure_reason.map(String::from),
            ..Outcome::success("")
        }
    }

    fn failed(class: Option<&str>, reason: &str) -> Option<AttemptFailure> {
        Some(AttemptFailure::Failed {
            class: class.map(String::from),
            reason: reason.into(),
        })
    }

    #[test]
    fn a_success_leaves_no_failure() {
        for status in [
            StageStatus::Success,
            StageStatus::PartialSuccess,
            StageStatus::Skipped,
        ] {
            assert_eq!(attempt_failure(&Ok(outcome(status, "done", None))), None);
        }
    }

    #[test]
    fn a_reported_failure_keeps_the_agents_text_else_the_reason() {
        let result = Ok(outcome(
            StageStatus::Fail,
            "tests failed",
            Some("Claude Code returned an error"),
        ));
        assert_eq!(
            attempt_failure(&result),
            failed(Some("reported"), "tests failed")
        );
        let result = Ok(outcome(
            StageStatus::Fail,
            "  ",
            Some("Claude Code returned an error"),
        ));
        assert_eq!(
            attempt_failure(&result),
            failed(Some("reported"), "Claude Code returned an error")
        );
    }

    #[test]
    fn a_retry_outcome_asks_to_be_retried() {
        assert_eq!(
            attempt_failure(&Ok(outcome(StageStatus::Retry, "flaky", None))),
            Some(AttemptFailure::AskedToRetry {
                reason: "flaky".into()
            })
        );
    }

    #[test]
    fn timeouts_record_their_length() {
        let agent = AttractorError::AgentTimeout {
            node: "n".into(),
            attempt: 1,
            timeout_ms: 600_000,
            files: None,
        };
        let command = AttractorError::CommandTimeout { timeout_ms: 1500 };
        assert_eq!(
            attempt_failure(&Err(agent)),
            Some(AttemptFailure::TimedOut {
                timeout_ms: 600_000
            })
        );
        assert_eq!(
            attempt_failure(&Err(command)),
            Some(AttemptFailure::TimedOut { timeout_ms: 1500 })
        );
    }

    #[test]
    fn agent_failures_record_their_class_and_message() {
        for (kind, name) in [
            (FailureKind::Crash, "crash"),
            (FailureKind::NoResult, "no_result"),
            (FailureKind::Launch, "launch"),
        ] {
            let error = AttractorError::AgentFailed {
                node: "n".into(),
                kind,
                message: "boom".into(),
            };
            assert_eq!(attempt_failure(&Err(error)), failed(Some(name), "boom"));
        }
    }

    #[test]
    fn other_errors_record_their_text_and_class_if_any() {
        let missing = AttractorError::CliNotFound {
            binary: "claude".into(),
        };
        let text = missing.to_string();
        assert_eq!(
            attempt_failure(&Err(missing)),
            failed(Some("launch"), &text)
        );
        let other = AttractorError::Other("disk full".into());
        assert_eq!(attempt_failure(&Err(other)), failed(None, "disk full"));
    }

    #[test]
    fn a_cancelled_attempt_leaves_nothing() {
        let cancelled = AttractorError::Cancelled { node: "n".into() };
        assert_eq!(attempt_failure(&Err(cancelled)), None);
    }

    #[test]
    fn notes_name_the_class_and_reason() {
        assert_eq!(
            failure_note(&AttemptFailure::TimedOut {
                timeout_ms: 600_000
            }),
            "The previous attempt timed out after 600 s."
        );
        assert_eq!(
            failure_note(&AttemptFailure::TimedOut { timeout_ms: 1500 }),
            "The previous attempt timed out after 1500 ms."
        );
        assert_eq!(
            failure_note(&failed(Some("reported"), "tests failed\n").unwrap()),
            "The previous attempt failed (reported): tests failed"
        );
        assert_eq!(
            failure_note(&failed(Some("crash"), "exited with 3").unwrap()),
            "The previous attempt failed (crash): exited with 3"
        );
        assert_eq!(
            failure_note(&failed(None, "disk full").unwrap()),
            "The previous attempt failed: disk full"
        );
        assert_eq!(
            failure_note(&AttemptFailure::AskedToRetry {
                reason: "flaky".into()
            }),
            "The previous attempt asked to be retried: flaky"
        );
        assert_eq!(
            failure_note(&failed(Some("reported"), " ").unwrap()),
            "The previous attempt failed (reported)."
        );
    }

    #[test]
    fn a_long_reason_keeps_its_last_2000_bytes_on_a_char_boundary() {
        // 3-byte chars: 2000 is not a multiple of 3, so the cut lands
        // mid-char and must move forward.
        let reason = format!("start{}", "€".repeat(1000));
        let note = failure_note(&failed(Some("reported"), &reason).unwrap());
        let kept = note
            .strip_prefix("The previous attempt failed (reported): ")
            .unwrap();
        assert!(kept.len() <= 2000, "{}", kept.len());
        assert!(kept.len() >= 1998, "{}", kept.len());
        assert!(kept.chars().all(|c| c == '€'));
        assert!(!note.contains("start"));
    }

    #[test]
    fn failures_round_trip_through_json() {
        for failure in [
            AttemptFailure::TimedOut { timeout_ms: 5 },
            failed(Some("reported"), "x").unwrap(),
            failed(None, "y").unwrap(),
            AttemptFailure::AskedToRetry { reason: "z".into() },
        ] {
            let json = serde_json::to_string(&failure).unwrap();
            assert_eq!(
                serde_json::from_str::<AttemptFailure>(&json).unwrap(),
                failure
            );
        }
    }
}
