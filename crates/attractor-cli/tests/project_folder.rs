#![cfg(unix)]
//! The project folder `.harmonik-attractor/` (ticket 26): the config is
//! `.harmonik-attractor/config.toml`, and the run data next to it stays out
//! of `git status` while `config.toml` stays committable.

mod blocked_agents;
#[allow(dead_code)]
mod fake_agent;

use std::fs;

use fake_agent::{fake_claude, stderr, FakeAgent};

/// A config holding only the `solo` agent profile.
fn solo_config() -> String {
    format!(
        "[project]\nname = \"solo\"\n\n\
         [agents.solo]\nmechanism = \"claude-p\"\ncommand = {:?}\ntest_only = true\n",
        fake_claude().canonicalize().unwrap()
    )
}

/// One node on the `solo` agent.
const SOLO_NODE: &str = r#"digraph G {
    start [shape="Mdiamond"]
    work [shape="box", agent="solo", timeout="30s", prompt="scenario=success"]
    done [shape="Msquare"]
    start -> work -> done
}"#;

/// One tool node.
const TOOL_NODE: &str = r#"digraph G {
    start [shape="Mdiamond"]
    step [shape="parallelogram", tool_command="echo hi > out.txt"]
    done [shape="Msquare"]
    start -> step -> done
}"#;

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "run failed ({:?})\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        stderr(output)
    );
}

#[test]
fn the_config_is_read_from_the_project_folder() {
    let fake = FakeAgent::new();
    let config = fake.repo().join(".harmonik-attractor/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, solo_config()).unwrap();
    fake.git(&["add", ".harmonik-attractor/config.toml"]);
    fake.git(&["commit", "-q", "-m", "config"]);

    assert_success(&fake.run(SOLO_NODE));
}

#[test]
fn a_lone_pas_toml_is_ignored() {
    let fake = FakeAgent::new();
    fs::write(fake.repo().join("pas.toml"), solo_config()).unwrap();
    fake.git(&["add", "pas.toml"]);
    fake.git(&["commit", "-q", "-m", "old config"]);

    let output = fake.run(SOLO_NODE);

    assert!(!output.status.success(), "pas.toml was read");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        stderr(&output)
    );
    assert!(text.contains("solo"), "{text}");
}

#[test]
fn run_data_stays_out_of_git_status_and_the_config_stays_committable() {
    let fake = FakeAgent::new();
    fake.git(&["rm", "-q", ".harmonik-attractor/config.toml"]);
    fake.git(&["commit", "-q", "-m", "no config"]);
    let status = || fake.git(&["status", "--porcelain", "--untracked-files=all"]);
    // Uncommitted at first: it must show in `git status`, not be ignored.
    let config = fake.repo().join(".harmonik-attractor/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "[project]\nname = \"p\"\n").unwrap();
    // `--logs` defaults to the cwd's `.harmonik-attractor/logs`: run in the repo.
    let run = |dot: &str| {
        let mut command = fake.pas_run(dot);
        command
            .arg("--workdir")
            .arg(fake.repo())
            .current_dir(fake.repo());
        command.output().unwrap()
    };

    assert_success(&run(TOOL_NODE));

    assert!(fake.repo().join(".harmonik-attractor/logs").is_dir());
    assert!(fake.repo().join(".harmonik-attractor/worktrees").is_dir());
    assert!(!fake.repo().join(".pas").exists());
    assert_eq!(status(), "?? .harmonik-attractor/config.toml");

    fake.git(&["add", ".harmonik-attractor/config.toml"]);
    fake.git(&["commit", "-q", "-m", "config"]);
    assert_success(&run(TOOL_NODE));
    assert_eq!(status(), "", "no run data in git status");
    assert_eq!(
        fake.git(&["ls-files", ".harmonik-attractor"]),
        ".harmonik-attractor/config.toml"
    );
}

#[test]
fn an_agents_edit_to_the_tracked_config_is_committed_on_the_run_branch() {
    let fake = FakeAgent::new();
    let edit = r#"digraph G {
        start [shape="Mdiamond"]
        step [shape="parallelogram", tool_command="echo '# EDITED' >> .harmonik-attractor/config.toml && echo x > other.txt"]
        done [shape="Msquare"]
        start -> step -> done
    }"#;

    assert_success(&fake.run(edit));

    let branch = fake.run_meta()["branch"].as_str().unwrap().to_string();
    assert_eq!(fake.git(&["show", &format!("{branch}:other.txt")]), "x");
    let config = fake.git(&["show", &format!("{branch}:.harmonik-attractor/config.toml")]);
    assert!(config.contains("# EDITED"), "{config}");
}
