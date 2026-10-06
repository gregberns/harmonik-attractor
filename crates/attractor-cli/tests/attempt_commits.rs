#![cfg(unix)]
//! `pas run` end to end with the fake `claude`: the engine records every
//! attempt as one commit on the Run's branch (ticket 07).

#[allow(dead_code)]
mod fake_agent;

use std::collections::BTreeMap;
use std::fs;

use fake_agent::{stderr, FakeAgent};

/// One engine commit: its subject and its `Pas-*` trailers.
#[derive(Debug)]
struct EngineCommit {
    subject: String,
    trailers: BTreeMap<String, String>,
    author: String,
}

/// The engine's commits on the Run's branch since its base, oldest first.
fn engine_commits(fake: &FakeAgent) -> Vec<EngineCommit> {
    let meta = fake.run_meta();
    let range = format!(
        "{}..{}",
        meta["base_sha"].as_str().unwrap(),
        meta["branch"].as_str().unwrap()
    );
    fake.git(&["log", "--reverse", "--format=%an <%ae>%x1f%B%x1e", &range])
        .split('\x1e')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let (author, body) = entry.split_once('\x1f')?;
            let subject = body.lines().next()?.to_string();
            subject.starts_with("pas(").then(|| EngineCommit {
                subject,
                author: author.to_string(),
                trailers: body
                    .lines()
                    .filter_map(|line| line.split_once(": "))
                    .filter(|(key, _)| key.starts_with("Pas-"))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            })
        })
        .collect()
}

fn trailer<'a>(commit: &'a EngineCommit, key: &str) -> Option<&'a str> {
    commit.trailers.get(key).map(String::as_str)
}

/// start -> work -> done, where `work` is a claude node with `attrs`.
fn one_node(attrs: &str) -> String {
    format!(
        r#"digraph G {{
            start [shape="Mdiamond"]
            work [shape="box", agent="fake", {attrs}]
            done [shape="Msquare"]
            start -> work -> done
        }}"#
    )
}

fn assert_success(fake: &FakeAgent, dot: &str) {
    let output = fake.run(dot);
    assert!(output.status.success(), "{}", stderr(&output));
}

#[test]
fn three_agent_nodes_leave_three_commits_with_trailers() {
    let fake = FakeAgent::new();
    assert_success(
        &fake,
        r#"digraph G {
            start [shape="Mdiamond"]
            a [shape="box", agent="fake", timeout="30s", prompt="scenario=success"]
            b [shape="box", agent="fake", timeout="30s", prompt="scenario=success"]
            c [shape="box", agent="fake", timeout="30s", prompt="scenario=success"]
            done [shape="Msquare"]
            start -> a -> b -> c -> done
        }"#,
    );

    let run_id = fake.run_meta()["run_id"].as_str().unwrap().to_string();
    let commits = engine_commits(&fake);
    assert_eq!(commits.len(), 3, "{commits:#?}");
    for (commit, node) in commits.iter().zip(["a", "b", "c"]) {
        assert_eq!(
            commit.subject,
            format!("pas({run_id}): {node} attempt 1 (success)")
        );
        assert_eq!(trailer(commit, "Pas-Run"), Some(run_id.as_str()));
        assert_eq!(trailer(commit, "Pas-Node"), Some(node));
        assert_eq!(trailer(commit, "Pas-Attempt"), Some("1"));
        assert_eq!(trailer(commit, "Pas-Status"), Some("success"));
        assert_eq!(trailer(commit, "Pas-Failure-Class"), None);
    }
}

#[test]
fn a_retried_node_leaves_one_commit_per_attempt() {
    let fake = FakeAgent::new();
    fs::write(fake.scenarios().join("work.1"), "scenario=hang\n").unwrap();
    fs::write(fake.scenarios().join("work"), "scenario=success\n").unwrap();
    assert_success(
        &fake,
        &one_node(r#"timeout="1s", max_retries=1, prompt="do it""#),
    );

    let commits = engine_commits(&fake);
    assert_eq!(commits.len(), 2, "{commits:#?}");
    assert_eq!(trailer(&commits[0], "Pas-Attempt"), Some("1"));
    assert_eq!(trailer(&commits[0], "Pas-Status"), Some("fail"));
    assert_eq!(trailer(&commits[0], "Pas-Failure-Class"), Some("timeout"));
    assert_eq!(trailer(&commits[1], "Pas-Attempt"), Some("2"));
    assert_eq!(trailer(&commits[1], "Pas-Status"), Some("success"));
}

#[test]
fn a_reported_failure_and_a_crash_record_their_class() {
    let fake = FakeAgent::new();
    let output = fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="box", agent="fake", timeout="30s", prompt="scenario=fail"]
            fixup [shape="box", agent="fake", timeout="30s", prompt="scenario=crash"]
            done [shape="Msquare"]
            start -> work
            work -> fixup [condition="outcome=fail"]
            work -> done [condition="outcome=success"]
            fixup -> done
        }"#,
    );
    assert!(!output.status.success(), "the crash ends the run");

    let commits = engine_commits(&fake);
    assert_eq!(commits.len(), 2, "{commits:#?}");
    assert_eq!(trailer(&commits[0], "Pas-Node"), Some("work"));
    assert_eq!(trailer(&commits[0], "Pas-Status"), Some("fail"));
    assert_eq!(trailer(&commits[0], "Pas-Failure-Class"), Some("reported"));
    assert_eq!(trailer(&commits[1], "Pas-Node"), Some("fixup"));
    assert_eq!(trailer(&commits[1], "Pas-Status"), Some("fail"));
    assert_eq!(trailer(&commits[1], "Pas-Failure-Class"), Some("crash"));
}

#[test]
fn the_agents_own_commit_is_kept_under_the_engines() {
    let fake = FakeAgent::new();
    assert_success(
        &fake,
        &one_node(r#"timeout="30s", prompt="scenario=edit_commit""#),
    );

    let branch = fake.run_meta()["branch"].as_str().unwrap().to_string();
    assert_eq!(
        fake.git(&["log", "-1", "--format=%s", &format!("{branch}~1")]),
        "fake-claude edit"
    );
    assert_eq!(engine_commits(&fake).len(), 1);
    assert!(fake
        .git(&["log", "-1", "--format=%s", &branch])
        .ends_with(": work attempt 1 (success)"));
}

#[test]
fn a_pas_folder_in_the_worktree_is_never_committed() {
    let fake = FakeAgent::new();
    assert_success(
        &fake,
        &one_node(r#"timeout="30s", prompt="scenario=pas_dir""#),
    );

    let meta = fake.run_meta();
    let range = format!(
        "{}..{}",
        meta["base_sha"].as_str().unwrap(),
        meta["branch"].as_str().unwrap()
    );
    let names = fake.git(&["log", "--name-only", "--format=", &range]);
    assert!(names.lines().any(|l| l == "visible.txt"), "{names}");
    assert!(!names.lines().any(|l| l.starts_with(".pas")), "{names}");
}

#[test]
fn a_repo_without_an_identity_still_gets_attempt_commits() {
    // The helper's repo sets no user.name/user.email (its own commits pass
    // `-c`), and pas runs with the global and system config ignored.
    let fake = FakeAgent::new();
    assert_success(
        &fake,
        &one_node(r#"timeout="30s", prompt="scenario=success""#),
    );

    let commits = engine_commits(&fake);
    assert_eq!(commits.len(), 1, "{commits:#?}");
    assert_eq!(commits[0].author, "PAS <pas@localhost>");
}

#[test]
fn a_repo_with_an_identity_keeps_it() {
    let fake = FakeAgent::new();
    fake.git(&["config", "user.name", "Repo Owner"]);
    fake.git(&["config", "user.email", "owner@example.invalid"]);
    assert_success(
        &fake,
        &one_node(r#"timeout="30s", prompt="scenario=success""#),
    );

    let commits = engine_commits(&fake);
    assert_eq!(commits.len(), 1, "{commits:#?}");
    assert_eq!(commits[0].author, "Repo Owner <owner@example.invalid>");
}

#[test]
fn a_failing_pre_commit_hook_does_not_block_attempt_commits() {
    use std::os::unix::fs::PermissionsExt;
    let fake = FakeAgent::new();
    // Hooks in the source repo apply to its linked worktrees too.
    let hook = fake.repo().join(".git/hooks/pre-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    assert_success(
        &fake,
        &one_node(r#"timeout="30s", prompt="scenario=success""#),
    );

    assert_eq!(engine_commits(&fake).len(), 1);
}

#[test]
fn a_non_git_workdir_gets_no_commits() {
    let fake = FakeAgent::new();
    let plain = fake.root().join("plain");
    fs::create_dir(&plain).unwrap();
    // The fake profile comes from pas.toml, which the plain folder needs too.
    fs::copy(fake.repo().join("pas.toml"), plain.join("pas.toml")).unwrap();
    let output = fake
        .command_in(
            &one_node(r#"timeout="30s", prompt="scenario=success""#),
            &plain,
            &[],
        )
        .output()
        .unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(fake.run_meta()["worktree"].is_null());
    assert!(!plain.join(".git").exists());
}

#[test]
fn signing_and_the_repos_other_hooks_do_not_affect_attempt_commits() {
    use std::os::unix::fs::PermissionsExt;
    let fake = FakeAgent::new();
    // Signing on with a signer that always fails, and a failing post-commit
    // hook: neither may block or change an attempt commit.
    fake.git(&["config", "commit.gpgsign", "true"]);
    fake.git(&["config", "gpg.program", "false"]);
    let hook = fake.repo().join(".git/hooks/post-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    let ran = fake.scenarios().join("post-commit-ran");
    fs::write(
        &hook,
        format!("#!/bin/sh\necho ran > '{}'\nexit 1\n", ran.display()),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    assert_success(
        &fake,
        &one_node(r#"timeout="30s", prompt="scenario=success""#),
    );

    let commits = engine_commits(&fake);
    assert_eq!(commits.len(), 1, "{commits:#?}");
    let branch = fake.run_meta()["branch"].as_str().unwrap().to_string();
    assert_eq!(fake.git(&["log", "-1", "--format=%G?", &branch]), "N");
    assert!(!ran.exists(), "the repo's post-commit hook ran");
}
