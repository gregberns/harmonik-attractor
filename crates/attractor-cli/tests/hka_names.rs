#![cfg(unix)]
//! The `hka` names (ticket 27): agents get `HKA_*` variables and no `PAS_*`
//! ones, run commits say `hka(<run>)` with `Hka-*` trailers on an
//! `hka/run/` branch, and the Run Index lives under `HKA_STATE_DIR`.

mod blocked_agents;
#[allow(dead_code)]
mod fake_agent;

use fake_agent::{stderr, FakeAgent};

const ONE_NODE: &str = r#"digraph G {
    start [shape="Mdiamond"]
    work [shape="box", agent="fake", timeout="30s", prompt="scenario=success"]
    done [shape="Msquare"]
    start -> work -> done
}"#;

fn assert_success(output: &std::process::Output) {
    assert!(output.status.success(), "{}", stderr(output));
}

#[test]
fn the_agent_sees_hka_ids_and_no_pas_variables() {
    let fake = FakeAgent::new();
    assert_success(&fake.run(ONE_NODE));

    let envs = fake.env_logs();
    assert_eq!(envs.len(), 1, "{envs:?}");
    let env = &envs[0];
    let run_id = fake.run_meta()["run_id"].as_str().unwrap().to_string();
    assert_eq!(env.get("HKA_RUN_ID"), Some(&run_id), "{env:?}");
    assert_eq!(env.get("HKA_NODE_ID").map(String::as_str), Some("work"));
    assert_eq!(env.get("HKA_ATTEMPT").map(String::as_str), Some("1"));
    assert!(env.contains_key("HKA_INVOCATION_ID"), "{env:?}");
    assert!(env.contains_key("HKA_SESSION_ID"), "{env:?}");
    assert!(!env.keys().any(|k| k.starts_with("PAS_")), "{env:?}");
}

#[test]
fn run_commits_say_hka_on_an_hka_branch() {
    let fake = FakeAgent::new();
    assert_success(&fake.run(ONE_NODE));

    let meta = fake.run_meta();
    let run_id = meta["run_id"].as_str().unwrap();
    let branch = meta["branch"].as_str().unwrap();
    assert_eq!(branch, format!("hka/run/{run_id}"));
    assert!(meta.get("hka_version").is_some(), "{meta}");
    assert!(meta.get("pas_version").is_none(), "{meta}");

    let body = fake.git(&["log", "-1", "--format=%B", branch]);
    assert!(body.starts_with(&format!("hka({run_id}): work")), "{body}");
    for trailer in ["Hka-Run:", "Hka-Node:", "Hka-Attempt:", "Hka-Status:"] {
        assert!(body.contains(trailer), "{trailer} missing:\n{body}");
    }
    assert!(!body.contains("Pas-"), "{body}");
}

#[test]
fn the_run_index_lives_in_hka_state_dir() {
    let fake = FakeAgent::new();
    let state = fake.root().join("hka-state");
    let output = fake
        .command(ONE_NODE)
        .env("HKA_STATE_DIR", &state)
        .output()
        .unwrap();
    assert_success(&output);

    assert!(state.join("runs.jsonl").is_file());
}

#[test]
fn the_run_index_defaults_to_xdg_state_home_harmonik_attractor() {
    let fake = FakeAgent::new();
    let xdg = fake.root().join("xdg-state");
    let output = fake
        .command(ONE_NODE)
        .env_remove("HKA_STATE_DIR")
        .env_remove("PAS_STATE_DIR")
        .env("XDG_STATE_HOME", &xdg)
        .output()
        .unwrap();
    assert_success(&output);

    assert!(xdg.join("harmonik-attractor/runs.jsonl").is_file());
}
