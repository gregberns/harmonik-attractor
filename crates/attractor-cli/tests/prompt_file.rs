#![cfg(unix)]
//! `pas run` end to end with the fake `claude`: every invocation leaves a
//! `transcripts/<inv>.prompt.txt` with the prompt and argv and no
//! environment values (ticket 12).

#[allow(dead_code)]
mod fake_agent;

use std::fs;
use std::path::Path;

use fake_agent::{stderr, FakeAgent};

/// Every file under `dir`, recursively.
fn files_under(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

#[test]
fn each_invocation_has_a_prompt_file_without_env_values() {
    let fake = FakeAgent::new();
    fs::write(fake.scenarios().join("work.1"), "scenario=hang\n").unwrap();
    fs::write(fake.scenarios().join("work"), "scenario=success\n").unwrap();
    let output = fake
        .command(
            r#"digraph G {
                start [shape="Mdiamond"]
                work [shape="box", llm_provider="claude", timeout="1s", max_retries=1, prompt="do the work"]
                done [shape="Msquare"]
                start -> work -> done
            }"#,
        )
        .env("PAS_TEST_SECRET", "s3cr3t-value")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));

    let run = fake.run_dirs()[0].clone();
    let started = fake.events_of("LlmStarted");
    assert_eq!(started.len(), 2, "{started:?}");
    for (attempt, event) in started.iter().enumerate() {
        let id = event["invocation_id"].as_str().unwrap();
        let text = fs::read_to_string(run.join(format!("transcripts/{id}.prompt.txt"))).unwrap();
        assert!(
            text.starts_with(&format!(
                "pas prompt file v1\ninvocation: {id}\nnode: work\nattempt: {}\n",
                attempt + 1
            )),
            "{text}"
        );
        assert!(text.contains("Task (work): do the work"), "{text}");
        assert!(
            text.contains("\n-p\n<prompt>\n--output-format\nstream-json\n"),
            "{text}"
        );
        assert!(text.contains("\nPAS_TEST_SECRET\n"), "{text}");
    }
    for file in files_under(&run) {
        let bytes = fs::read(&file).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains("s3cr3t-value"),
            "{} holds the secret",
            file.display()
        );
    }
}
