#![cfg(unix)]
//! `pas run` end to end with the fake `claude` (`tests/agents/fake-claude`):
//! each test runs a pipeline whose agent nodes name a fake scenario in their
//! prompt, and checks what a caller sees: exit status, stderr, journal, the
//! fake's own logs and the workdir's git history.

mod fake_agent;

use fake_agent::{stderr, FakeAgent};

#[test]
fn success_completes_with_fake_result_text() {
    let fake = FakeAgent::new();
    // `next` sees `work`'s result in its prompt, the only place a caller can
    // read a node's result text after a successful Run.
    let output = fake.run(
        r#"digraph G {
            start [shape="Mdiamond"]
            work [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            next [shape="box", llm_provider="claude", timeout="30s", prompt="scenario=success"]
            done [shape="Msquare"]
            start -> work -> next -> done
        }"#,
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fake.attempts("success"), 2);
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[1].contains("- work.result: fake-claude: success\n"),
        "{}",
        prompts[1]
    );
    let invocations = fake.invocations();
    let argv = &invocations[0];
    assert!(argv.contains(&"-p".to_string()), "{argv:?}");
    let format = argv.iter().position(|a| a == "--output-format").unwrap();
    assert_eq!(argv[format + 1], "stream-json");
}
