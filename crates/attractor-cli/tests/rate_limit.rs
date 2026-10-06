#![cfg(unix)]
//! A rate-limited Claude attempt waits and re-spawns within the profile's
//! `rate_limit_window` (ticket 10, design §5), through `pas run` with the
//! fake (`fake-claude` profile: the built-in `claude` profile, which can
//! resume). The fake's `resetsAt` is the current second, so each wait is
//! the 1 s minimum: the product waiting, not a sleep in this test.

// This crate uses a subset of the shared harness.
#[allow(dead_code)]
mod fake_agent;

use std::fs;

use fake_agent::{stderr, FakeAgent};
use serde_json::Value;

fn assert_success(output: &std::process::Output) {
    assert!(output.status.success(), "{}", stderr(output));
}

/// The value after `flag` in `argv`.
fn flag(argv: &[String], flag: &str) -> Option<String> {
    let at = argv.iter().position(|a| a == flag)?;
    argv.get(at + 1).cloned()
}

fn files_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn rate_limited_twice_then_success_journals_each_wait_and_spawn() {
    let fake = FakeAgent::new();
    fs::write(
        fake.scenarios().join("work"),
        "scenario=rate_limited times=2\n",
    )
    .unwrap();

    assert_success(&fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="box", agent="fake-claude", timeout="30s", prompt="do it"]
            done [shape="Msquare"]
            start -> work -> done
        }"#,
    ));

    let started = fake.events_of("LlmStarted");
    assert_eq!(
        started
            .iter()
            .map(|s| s["spawn"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    let inv = started[0]["invocation_id"].as_str().unwrap().to_string();
    assert!(started.iter().all(|s| s["invocation_id"] == inv.as_str()));
    let mut pids: Vec<u64> = started.iter().map(|s| s["pid"].as_u64().unwrap()).collect();
    pids.sort_unstable();
    pids.dedup();
    assert_eq!(pids.len(), 3, "{pids:?}");
    let names: Vec<String> = started
        .iter()
        .map(|s| s["transcript"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        [
            format!("transcripts/{inv}.jsonl"),
            format!("transcripts/{inv}.2.jsonl"),
            format!("transcripts/{inv}.3.jsonl"),
        ]
    );
    assert_eq!(
        started[1]["stderr"].as_str().unwrap(),
        format!("transcripts/{inv}.2.stderr.log")
    );

    let waits: Vec<Value> = fake.events_of("LlmRateLimited");
    assert_eq!(waits.len(), 2, "{waits:?}");
    for (wait, spawn) in waits.iter().zip([1, 2]) {
        assert_eq!(wait["invocation_id"], inv.as_str());
        assert_eq!(wait["spawn"], spawn);
        assert_eq!(wait["wait_s"], 1);
    }

    let invoked = fake.events_of("LlmInvoked");
    assert_eq!(invoked.len(), 1, "{invoked:?}");
    assert_eq!(
        invoked[0]["transcript"],
        format!("transcripts/{inv}.3.jsonl")
    );
    assert_eq!(invoked[0]["status"], "success");

    let transcripts = fake.run_dirs()[0].join("transcripts");
    let files = files_in(&transcripts);
    for spawn in ["", ".2", ".3"] {
        for ext in ["jsonl", "stderr.log"] {
            let name = format!("{inv}{spawn}.{ext}");
            assert!(files.contains(&name), "{name} in {files:?}");
        }
    }

    let argvs = fake.invocations();
    assert_eq!(argvs.len(), 3);
    let id = flag(&argvs[0], "--session-id").unwrap();
    for argv in &argvs[1..] {
        assert_eq!(
            flag(argv, "--resume").as_deref(),
            Some(id.as_str()),
            "{argv:?}"
        );
    }
}

#[test]
fn always_rate_limited_with_no_window_fails_at_once_saying_rate_limited() {
    let fake = FakeAgent::new();
    fake.commit_pas_toml(
        "\n[agents.fake-claude-nowait]\ninherit_from = \"fake-claude\"\nrate_limit_window = \"0s\"",
    );
    fs::write(
        fake.scenarios().join("work"),
        "scenario=always_rate_limited\n",
    )
    .unwrap();
    fs::write(fake.scenarios().join("report"), "scenario=success\n").unwrap();

    // `report` runs on the fail edge and sees work's result in its prompt.
    assert_success(&fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="box", agent="fake-claude-nowait", timeout="30s", prompt="do it"]
            report [shape="box", agent="fake", timeout="30s", prompt="report"]
            done [shape="Msquare"]
            start -> work
            work -> done [condition="outcome=success"]
            work -> report [condition="outcome=fail"]
            report -> done
        }"#,
    ));

    let report = fs::read_to_string(fake.scenarios().join("prompt.report@1")).unwrap();
    assert!(
        report.contains(
            "- work.result: rate limited: API Error: Rate limit reached; waited 0 s over 1 spawn"
        ),
        "{report}"
    );
    assert!(fake.events_of("LlmRateLimited").is_empty());
    assert_eq!(
        fake.events_of("LlmStarted")
            .iter()
            .filter(|s| s["node_id"] == "work")
            .count(),
        1
    );
}
