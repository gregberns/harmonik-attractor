#![cfg(unix)]
//! `docs/run-folder.md` lists every file a Run creates (ticket 12): run a
//! Pipeline with the fake that makes as many files as one Run can, list the
//! logs and state folders, and check each path, with its ids replaced by the
//! doc's placeholders, appears in the doc. A second check covers files and
//! events this Run didn't make: every layout file name and every journal
//! event type.
//!
//! Not in the Run: a stop and resume (for `control/stop`) and a Monitor
//! start (for `console.log`). Both are slow or need the Monitor; the
//! constant check covers those two names.

#[allow(dead_code)]
mod fake_agent;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use attractor_journal::{write_answer, AnswerFile, AnswerSource, EventData, RunDir};
use serde_json::Value;

use fake_agent::FakeAgent;

fn doc() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/run-folder.md"))
        .unwrap()
}

/// Every file under `root`, relative to it, with `/` separators.
fn files_under(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    files.sort();
    files
}

fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The journal so far, skipping a line still being written.
fn journal(run: &Path) -> Vec<Value> {
    fs::read_to_string(run.join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Replace each id in `path` with its placeholder, and a spawn number
/// after `<inv>.` (`<inv>.2.jsonl`) with `<n>`.
fn normalise(path: &str, ids: &[(String, &str)]) -> String {
    let path = ids
        .iter()
        .fold(path.to_string(), |path, (id, placeholder)| {
            path.replace(id.as_str(), placeholder)
        });
    let Some(at) = path.find("<inv>.") else {
        return path;
    };
    let rest = &path[at + "<inv>.".len()..];
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && rest[digits..].starts_with('.') {
        format!("{}<inv>.<n>{}", &path[..at], &rest[digits..])
    } else {
        path
    }
}

#[test]
fn every_file_a_run_creates_is_in_the_doc() {
    let fake = FakeAgent::new();
    // A Human Gate (answers/), a retried agent (two invocations, each with a
    // transcript, stderr log and prompt file), a rate-limited agent (one
    // invocation, two spawns: <inv>.2.jsonl), then a crash, so the Run fails
    // and its checkpoint stays.
    fs::write(fake.scenarios().join("work.1"), "scenario=hang\n").unwrap();
    fs::write(
        fake.scenarios().join("limited"),
        "scenario=rate_limited times=1\n",
    )
    .unwrap();
    fs::write(fake.scenarios().join("work"), "scenario=success\n").unwrap();
    fs::write(fake.scenarios().join("last"), "scenario=crash\n").unwrap();
    let mut pas = fake
        .command(
            r#"digraph G {
                start [shape="Mdiamond"]
                gate [shape="hexagon", prompt="Go on?"]
                work [shape="box", agent="fake", timeout="1s", max_retries=1, prompt="work"]
                limited [shape="box", agent="fake-claude", timeout="30s", prompt="limited"]
                last [shape="box", agent="fake", timeout="30s", prompt="last"]
                done [shape="Msquare"]
                start -> gate
                gate -> work [label="yes"]
                work -> limited -> last -> done
            }"#,
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut question = None;
    wait_for("the Human Gate", || {
        question = fake
            .run_dirs()
            .first()
            .map(|run| journal(run))
            .unwrap_or_default()
            .iter()
            .find(|e| e["type"] == "HumanInputRequested")
            .map(|e| e["data"]["question_id"].as_str().unwrap().to_string());
        question.is_some()
    });
    let question = question.unwrap();
    let run = RunDir::from_path(fake.run_dirs()[0].clone());
    write_answer(&run, &AnswerFile::new(&question, "yes", AnswerSource::Cli)).unwrap();
    let status = pas.wait().unwrap();
    assert!(!status.success(), "the last node crashes");

    let run_id = fake.run_meta()["run_id"].as_str().unwrap().to_string();
    let mut ids: Vec<(String, &str)> = vec![(run_id, "<run-id>"), (question, "<question-id>")];
    for started in fake.events_of("LlmStarted") {
        ids.push((
            started["invocation_id"].as_str().unwrap().to_string(),
            "<inv>",
        ));
    }
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        2 + 4,
        "two invocations of work, one of limited (two spawns), one of last"
    );

    let doc = doc();
    let logs: PathBuf = fake.run_dirs()[0]
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf();
    let state = fake.root().join("state");
    let mut checked = Vec::new();
    for (root, prefix) in [(&logs, "<stem>-<hash>/"), (&state, "")] {
        for file in files_under(root) {
            let name = format!("{prefix}{}", normalise(&file, &ids));
            // A path the placeholders can't map (an unknown id) fails here
            // by name; nothing is skipped.
            assert!(
                doc.contains(&name),
                "{name} (from {}) is not in docs/run-folder.md",
                root.join(&file).display()
            );
            checked.push(name);
        }
    }
    for expected in [
        "<stem>-<hash>/checkpoint.json",
        "<stem>-<hash>/runs/<run-id>/final.json",
        "<stem>-<hash>/runs/<run-id>/transcripts/<inv>.prompt.txt",
        "<stem>-<hash>/runs/<run-id>/transcripts/<inv>.<n>.jsonl",
        "<stem>-<hash>/runs/<run-id>/transcripts/<inv>.<n>.stderr.log",
        "<stem>-<hash>/runs/<run-id>/answers/<question-id>.json",
    ] {
        assert!(
            checked.iter().any(|c| c == expected),
            "{expected}: {checked:#?}"
        );
    }
}

#[test]
fn every_layout_file_and_event_type_is_in_the_doc() {
    let doc = doc();
    for name in [
        attractor_journal::RUN_JSON,
        attractor_journal::EVENTS_FILE,
        attractor_journal::CONSOLE_LOG,
        attractor_journal::FINAL_JSON,
        attractor_journal::TRANSCRIPTS_DIR,
        attractor_journal::ANSWERS_DIR,
        attractor_journal::CONTROL_DIR,
        attractor_journal::RUN_LOCK,
        attractor_journal::CHECKPOINT_FILE,
        attractor_journal::INDEX_FILE,
        "control/stop",
        ".stderr.log",
        ".prompt.txt",
    ] {
        assert!(doc.contains(name), "{name} is not in docs/run-folder.md");
    }
    for event in EventData::KNOWN_TYPES {
        assert!(
            doc.contains(&format!("`{event}`")),
            "event {event} is not in docs/run-folder.md"
        );
    }
}
