use std::collections::HashMap;

use attractor_types::StageStatus;

use super::claude::fold_codergen_claude;
use super::*;
use crate::handlers::tests::{make_minimal_graph, make_node};

// --- LlmCliProvider ---

#[test]
fn provider_from_str_claude_variants() {
    assert_eq!(
        "claude".parse::<LlmCliProvider>(),
        Ok(LlmCliProvider::Claude)
    );
    assert_eq!(
        "anthropic".parse::<LlmCliProvider>(),
        Ok(LlmCliProvider::Claude)
    );
    assert_eq!(
        "CLAUDE".parse::<LlmCliProvider>(),
        Ok(LlmCliProvider::Claude)
    );
}

#[test]
fn provider_from_str_codex_variants() {
    assert_eq!("codex".parse::<LlmCliProvider>(), Ok(LlmCliProvider::Codex));
    assert_eq!(
        "openai".parse::<LlmCliProvider>(),
        Ok(LlmCliProvider::Codex)
    );
}

#[test]
fn provider_from_str_gemini_variants() {
    assert_eq!(
        "gemini".parse::<LlmCliProvider>(),
        Ok(LlmCliProvider::Gemini)
    );
    assert_eq!(
        "google".parse::<LlmCliProvider>(),
        Ok(LlmCliProvider::Gemini)
    );
}

#[test]
fn provider_parse_unknown_is_rejected() {
    assert!("llama".parse::<LlmCliProvider>().is_err());
}

#[test]
fn provider_binary_names() {
    assert_eq!(LlmCliProvider::Claude.binary_name(), "claude");
    assert_eq!(LlmCliProvider::Codex.binary_name(), "codex");
    assert_eq!(LlmCliProvider::Gemini.binary_name(), "gemini");
}

// --- Output parsers ---

#[test]
fn parse_codex_output_extracts_last_message() {
    let jsonl = concat!(
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"First message"}}"#,
        "\n",
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"Final answer"}}"#,
    );
    let result = parse_codex_output(jsonl, "test_node").unwrap();
    assert_eq!(result.text, "Final answer");
    assert!(!result.is_error);
}

#[test]
fn parse_codex_output_handles_turn_failed() {
    let jsonl = r#"{"type":"turn.failed","error":{"message":"Rate limited"}}"#;
    let result = parse_codex_output(jsonl, "test_node").unwrap();
    assert!(result.is_error);
    assert_eq!(result.text, "Rate limited");
}

#[test]
fn parse_codex_output_handles_stream_error() {
    let jsonl = r#"{"type":"error","message":"Connection lost"}"#;
    let result = parse_codex_output(jsonl, "test_node").unwrap();
    assert!(result.is_error);
    assert_eq!(result.text, "Connection lost");
}

#[test]
fn parse_codex_output_skips_unknown_events() {
    let jsonl = concat!(
        r#"{"type":"thread.started"}"#,
        "\n",
        r#"{"type":"turn.started"}"#,
        "\n",
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"Done"}}"#,
        "\n",
        r#"{"type":"turn.completed","usage":{"input_tokens":100,"output_tokens":50}}"#,
    );
    let result = parse_codex_output(jsonl, "test_node").unwrap();
    assert_eq!(result.text, "Done");
    assert!(!result.is_error);
}

#[test]
fn parse_gemini_output_success() {
    let json = r#"{"session_id":"abc","response":"Gemini says hi"}"#;
    let result = parse_gemini_output(json, "test_node").unwrap();
    assert_eq!(result.text, "Gemini says hi");
    assert!(!result.is_error);
}

#[test]
fn parse_gemini_output_error() {
    let json = r#"{"error":{"type":"api_error","message":"Model not found","code":404}}"#;
    let result = parse_gemini_output(json, "test_node").unwrap();
    assert!(result.is_error);
    assert_eq!(result.text, "Model not found");
}

#[test]
fn parse_gemini_output_invalid_json() {
    let result = parse_gemini_output("not json", "test_node");
    assert!(result.is_err());
}

#[test]
fn parse_cli_output_empty_stdout_errors() {
    let result = parse_cli_output(CliProvider::Codex, "", "some error", "n");
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("produced no output"));
}

// --- build_cli_command ---

/// The argv a Claude node runs with: the `claude` profile's command and
/// args, the node's extra args, the model args, then the `claude-p`
/// handler's own flags. Since ticket 02a the flags come in that order (the
/// settings-mode flag used to come first); the set of flags and values is
/// unchanged.
fn claude_args(cfg: ClaudeCliConfig, model: Option<&str>) -> Vec<String> {
    use attractor_agent_handler::{argv, AgentsConfig, Invocation};
    let node = make_node("n", "box", Some("do work"), HashMap::new());
    let request = AgentRequest {
        selection: Selection {
            profile: CLAUDE_PROFILE.into(),
            model: model.map(str::to_owned),
            reasoning: None,
        },
        prompt: "test prompt".into(),
        extra_args: claude_node_args(&node),
        workdir: PathBuf::from("."),
        timeout: None,
        record: Record {
            run_id: None,
            node_id: node.id.clone(),
            attempt: 1,
            invocation_id: "i".into(),
        },
        transcript: None,
        stderr: None,
        observer: None,
        cancel: CancellationToken::new(),
    };
    let config = fold_codergen_claude(AgentsConfig::builtin().unwrap(), claude_settings_args(&cfg));
    let claude = config.resolve().unwrap().remove(0);
    attractor_handler_claude_p::ClaudeP::argv(&Invocation {
        invocation_id: "i",
        argv: argv(&claude, &request),
        env: Default::default(),
        prompt: "test prompt",
        workdir: Path::new("."),
        timeout: claude.timeout,
        kill_grace: claude.kill_grace,
        transcript: None,
        stderr: None,
        cancel: CancellationToken::new(),
        spawned: &|_| {},
    })
}

#[test]
fn claude_argv_has_todays_flags_with_stream_json_output() {
    let args = claude_args(ClaudeCliConfig::default(), Some("sonnet"));
    assert_eq!(
        args,
        [
            "claude",
            "--no-session-persistence",
            "--dangerously-skip-permissions",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--safe-mode",
            "--model",
            "sonnet",
            "-p",
            "test prompt",
            "--output-format",
            "stream-json",
            "--verbose",
        ]
    );
}

#[test]
fn claude_argv_strict_bare_is_opt_in() {
    let args = claude_args(
        ClaudeCliConfig {
            settings_mode: ClaudeSettingsMode::StrictBare,
            ..ClaudeCliConfig::default()
        },
        None,
    );
    assert!(args.iter().any(|a| a == "--bare"));
    assert!(!args.iter().any(|a| a == "--safe-mode"));
    assert!(!args.iter().any(|a| a == "--setting-sources"));
    assert!(!args.iter().any(|a| a == "--model"));
}

#[test]
fn claude_argv_inherit_uses_setting_sources() {
    let args = claude_args(
        ClaudeCliConfig {
            settings_mode: ClaudeSettingsMode::Inherit,
            setting_sources: vec!["user".into(), "project".into()],
            ..ClaudeCliConfig::default()
        },
        None,
    );
    assert!(!args.iter().any(|a| a == "--bare"));
    assert!(!args.iter().any(|a| a == "--safe-mode"));
    let at = args.iter().position(|a| a == "--setting-sources").unwrap();
    assert_eq!(args[at + 1], "user,project");
}

#[test]
fn claude_argv_emits_explicit_pas_owned_config() {
    let args = claude_args(
        ClaudeCliConfig {
            settings: Some(r#"{"enabledPlugins":{}}"#.into()),
            tools: Some("Read,Edit".into()),
            agents: Some(r#"{"reviewer":{"prompt":"review"}}"#.into()),
            plugin_dirs: vec!["/tmp/pas-plugin".into()],
            mcp_config: Some("{}".into()),
            ..ClaudeCliConfig::default()
        },
        None,
    );
    let value_of = |flag: &str| {
        let at = args.iter().position(|a| a == flag).unwrap();
        args[at + 1].clone()
    };
    assert_eq!(value_of("--settings"), r#"{"enabledPlugins":{}}"#);
    assert_eq!(value_of("--tools"), "Read,Edit");
    assert_eq!(value_of("--agents"), r#"{"reviewer":{"prompt":"review"}}"#);
    assert_eq!(value_of("--plugin-dir"), "/tmp/pas-plugin");
    assert_eq!(value_of("--mcp-config"), "{}");
}

#[test]
fn claude_node_args_carry_allowed_tools_and_budget() {
    let mut attrs = HashMap::new();
    attrs.insert(
        "allowed_tools".to_string(),
        attractor_dot::AttributeValue::String("Read,Grep".into()),
    );
    attrs.insert(
        "max_budget_usd".to_string(),
        attractor_dot::AttributeValue::String("2.5".into()),
    );
    let node = make_node("n", "box", Some("do work"), attrs);
    assert_eq!(
        claude_node_args(&node),
        ["--allowedTools", "Read,Grep", "--max-budget-usd", "2.5"]
    );
}

#[test]
fn codergen_claude_flags_fold_into_the_end_of_the_claude_profiles_args() {
    use attractor_agent_handler::AgentsConfig;
    let cfg = ClaudeCliConfig {
        settings_mode: ClaudeSettingsMode::StrictBare,
        tools: Some("Read".into()),
        ..ClaudeCliConfig::default()
    };
    let overrides = AgentsConfig::parse(
        r#"
        [profiles.claude-opus]
        inherit_from = "claude"
        model = "opus"
        "#,
    )
    .unwrap()
    .profiles;
    let config = AgentsConfig::builtin().unwrap().with_overrides(&overrides);
    let profiles = fold_codergen_claude(config, claude_settings_args(&cfg))
        .resolve()
        .unwrap();
    let expected = [
        "--no-session-persistence",
        "--dangerously-skip-permissions",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--bare",
        "--tools",
        "Read",
    ];
    for name in ["claude", "claude-opus"] {
        let profile = profiles.iter().find(|p| p.name == name).unwrap();
        assert_eq!(profile.args, expected, "{name}");
    }
}

#[test]
fn the_fold_keeps_the_args_an_overridden_claude_inherits() {
    use attractor_agent_handler::AgentsConfig;
    let overrides = AgentsConfig::parse(
        r#"
        [profiles.base]
        mechanism = "claude-p"
        command = "claude"
        args = ["--base"]

        [profiles.claude]
        inherit_from = "base"
        "#,
    )
    .unwrap()
    .profiles;
    let config = AgentsConfig::builtin().unwrap().with_overrides(&overrides);
    let profiles = fold_codergen_claude(config, vec!["--safe-mode".into()])
        .resolve()
        .unwrap();
    let claude = profiles.iter().find(|p| p.name == "claude").unwrap();
    assert_eq!(claude.args, ["--base", "--safe-mode"]);
}

#[test]
fn resolve_claude_cli_config_reads_pas_toml_and_resolves_plugin_dirs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("pas.toml"),
        r#"
[project]
name = "test"

[codergen.claude]
settings_mode = "inherit"
setting_sources = ["user"]
settings_json = "{}"
tools = "Read,Edit"
plugin_dirs = [".pas/plugin"]
"#,
    )
    .unwrap();
    let snapshot = HashMap::new();

    let cfg = resolve_claude_cli_config(&snapshot, dir.path().to_str(), "code").unwrap();

    assert_eq!(cfg.settings_mode, ClaudeSettingsMode::Inherit);
    assert_eq!(cfg.setting_sources, vec!["user"]);
    assert_eq!(cfg.settings.as_deref(), Some("{}"));
    assert_eq!(cfg.tools.as_deref(), Some("Read,Edit"));
    assert_eq!(cfg.plugin_dirs, vec![dir.path().join(".pas/plugin")]);
}

#[test]
fn resolve_claude_cli_config_cli_overrides_pas_toml() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("pas.toml"),
        r#"
[project]
name = "test"

[codergen.claude]
settings_mode = "strict_bare"
"#,
    )
    .unwrap();
    let mut snapshot = HashMap::new();
    snapshot.insert(
        "codergen.claude.settings_mode".into(),
        serde_json::json!("subscription-bare"),
    );

    let cfg = resolve_claude_cli_config(&snapshot, dir.path().to_str(), "code").unwrap();

    assert_eq!(cfg.settings_mode, ClaudeSettingsMode::SubscriptionBare);
}

#[test]
fn resolve_claude_cli_config_requires_sources_for_inherit() {
    let mut snapshot = HashMap::new();
    snapshot.insert(
        "codergen.claude.settings_mode".into(),
        serde_json::json!("inherit"),
    );

    let err = resolve_claude_cli_config(&snapshot, None, "code").unwrap_err();

    assert!(err
        .to_string()
        .contains("requires explicit setting_sources"));
}

#[test]
fn build_cli_command_codex_uses_exec_with_positional_prompt() {
    let graph = make_minimal_graph();
    let cfg = CliRunConfig {
        provider: CliProvider::Codex,
        prompt: "test prompt",
        model: None,
        workdir: Some("/tmp"),
        graph: &graph,
    };
    let cmd = build_cli_command(&cfg);
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_str().unwrap())
        .collect();
    assert_eq!(args.first(), Some(&"exec"));
    assert!(args.contains(&"--json"));
    assert!(args.contains(&"--yolo"));
    // Prompt should be last (positional)
    assert_eq!(args.last(), Some(&"test prompt"));
    // Should NOT contain -p flag
    assert!(!args.contains(&"-p"));
}

#[test]
fn build_cli_command_gemini_matches_documented_invocation() {
    let graph = make_minimal_graph();
    let cfg = CliRunConfig {
        provider: CliProvider::Gemini,
        prompt: "test prompt",
        model: Some("gemini-2.5-pro"),
        workdir: None,
        graph: &graph,
    };
    let cmd = build_cli_command(&cfg);
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_str().unwrap())
        .collect();
    assert_eq!(
        args,
        vec![
            "--output-format",
            "json",
            "--approval-mode",
            "yolo",
            "--model",
            "gemini-2.5-pro",
            "test prompt",
        ]
    );
}

// --- CodergenHandler dry-run with provider ---

#[tokio::test]
async fn codergen_dry_run_includes_provider() {
    use attractor_types::Context;
    let handler = CodergenHandler::new(std::sync::Arc::new(Agents::empty()));
    let mut node = make_node("llm_step", "box", Some("Do the thing"), HashMap::new());
    node.llm_provider = Some("gemini".into());
    let ctx = Context::default();
    ctx.set("dry_run", serde_json::Value::Bool(true)).await;
    let graph = make_minimal_graph();
    let resolved = ResolvedNode {
        node_id: node.id.clone(),
        kind: ResolvedNodeKind::Task,
        handler: crate::HandlerIdentity::Codergen,
        provider: Some(LlmCliProvider::Gemini),
        invocation: Default::default(),
    };

    let outcome = handler
        .execute_resolved(&node, &resolved, &ctx, &graph)
        .await
        .unwrap();
    assert_eq!(outcome.status, StageStatus::Success);
    assert_eq!(
        outcome.context_updates.get("llm_step.provider"),
        Some(&serde_json::Value::String("Gemini CLI".into()))
    );
    assert!(outcome.notes.contains("Gemini CLI"));
}

#[tokio::test]
async fn codergen_rejects_missing_provider_even_in_dry_run() {
    use attractor_types::Context;
    let handler = CodergenHandler::new(std::sync::Arc::new(Agents::empty()));
    let node = make_node("llm_step", "box", Some("Do the thing"), HashMap::new());
    let ctx = Context::default();
    ctx.set("dry_run", serde_json::Value::Bool(true)).await;
    let graph = make_minimal_graph();
    let resolved = ResolvedNode {
        node_id: node.id.clone(),
        kind: ResolvedNodeKind::Task,
        handler: crate::HandlerIdentity::Codergen,
        provider: None,
        invocation: Default::default(),
    };

    let error = handler
        .execute_resolved(&node, &resolved, &ctx, &graph)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no provider"));
}

#[test]
fn extract_label_finds_exact_last_line() {
    let labels = vec!["BUY".into(), "HOLD".into(), "SELL".into()];
    let response = "Based on analysis, I recommend:\n\nBUY";
    assert_eq!(extract_label(response, &labels), Some("BUY".into()));
}

#[test]
fn extract_label_case_insensitive() {
    let labels = vec!["BUY".into(), "HOLD".into(), "SELL".into()];
    let response = "The recommendation is:\n\nhold";
    assert_eq!(extract_label(response, &labels), Some("HOLD".into()));
}

#[test]
fn extract_label_fallback_to_body_scan() {
    let labels = vec!["BUY".into(), "HOLD".into(), "SELL".into()];
    let response = "I recommend a SELL rating because the player is declining.";
    assert_eq!(extract_label(response, &labels), Some("SELL".into()));
}

#[test]
fn extract_label_returns_none_when_no_match() {
    let labels = vec!["BUY".into(), "HOLD".into(), "SELL".into()];
    let response = "This player is interesting but I need more data.";
    assert_eq!(extract_label(response, &labels), None);
}

// --- Claude stream-json parsing ---

const CLAUDE_RESULT_LINE: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"done","total_cost_usd":0.01,"num_turns":2}"#;

// --- Streaming Transcripts with stub providers ---

#[cfg(unix)]
mod transcripts {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use attractor_types::{AttractorError, Context, Outcome, Result};

    use super::*;

    fn stub(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("provider-stub");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn transcripts(run_dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(run_dir.join("transcripts")) else {
            return vec![];
        };
        // Transcripts only: each Claude invocation also has a `.stderr.log`.
        let mut files: Vec<_> = entries
            .map(|e| e.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect();
        files.sort();
        files
    }

    fn only_transcript(run_dir: &Path) -> PathBuf {
        let files = transcripts(run_dir);
        assert_eq!(files.len(), 1, "expected one Transcript: {files:?}");
        files.into_iter().next().unwrap()
    }

    /// `transcript <path>; stderr <path>` of the one invocation in `run_dir`.
    fn only_files(run_dir: &Path) -> String {
        let transcript = only_transcript(run_dir);
        let stem = transcript.file_stem().unwrap().to_str().unwrap().to_owned();
        let stderr = transcript.with_file_name(format!("{stem}.stderr.log"));
        assert!(stderr.is_file(), "{}", stderr.display());
        format!(
            "transcript {}; stderr {}",
            transcript.display(),
            stderr.display()
        )
    }

    async fn run(
        provider: LlmCliProvider,
        program: PathBuf,
        run_dir: Option<&Path>,
        dry_run: bool,
        timeout: Option<Duration>,
    ) -> Result<Outcome> {
        let mut node = make_node("step", "box", Some("do work"), HashMap::new());
        node.timeout = timeout;
        let resolved = ResolvedNode {
            node_id: node.id.clone(),
            kind: ResolvedNodeKind::Task,
            handler: crate::HandlerIdentity::Codergen,
            provider: Some(provider),
            invocation: Default::default(),
        };
        CodergenHandler::new(crate::handlers::tests::claude_agents(&program))
            .execute_with_controls(
                &node,
                &resolved,
                &Context::default(),
                &make_minimal_graph(),
                CodergenExecutionControls {
                    dry_run,
                    workdir: None,
                    claude: Some(ClaudeCliConfig::default()),
                    run_dir: run_dir.map(Path::to_path_buf),
                    program: Some(program),
                    events: None,
                    run_id: None,
                    attempt: 1,
                    cancel: CancellationToken::new(),
                    resume_note: None,
                },
            )
            .await
    }

    async fn run_claude(program: PathBuf, run_dir: Option<&Path>) -> Result<Outcome> {
        run(LlmCliProvider::Claude, program, run_dir, false, None).await
    }

    fn handler_error(message: &str) -> String {
        AttractorError::HandlerError {
            handler: "codergen".into(),
            node: "step".into(),
            message: message.into(),
        }
        .to_string()
    }

    // AC1: one Transcript per Model Invocation, named by Invocation ID.
    #[tokio::test]
    async fn each_invocation_writes_one_transcript_named_by_invocation_id() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let program = stub(tmp.path(), &format!("echo '{CLAUDE_RESULT_LINE}'"));

        for _ in 0..2 {
            let outcome = run_claude(program.clone(), Some(&run_dir)).await.unwrap();
            assert_eq!(outcome.status, StageStatus::Success);
            assert_eq!(outcome.notes, "done");
            assert_eq!(
                outcome.context_updates.get("step.result"),
                Some(&serde_json::json!("done"))
            );
            assert_eq!(
                outcome.context_updates.get("step.turns"),
                Some(&serde_json::json!(2))
            );
        }

        let files = transcripts(&run_dir);
        assert_eq!(files.len(), 2, "{files:?}");
        let mut ids = Vec::new();
        for file in &files {
            assert_eq!(file.extension().unwrap(), "jsonl");
            let id = file.file_stem().unwrap().to_str().unwrap();
            let uuid = uuid::Uuid::parse_str(id).unwrap();
            assert_eq!(uuid.get_version_num(), 7);
            assert_eq!(id, uuid.hyphenated().to_string());
            assert_eq!(
                std::fs::read_to_string(file).unwrap(),
                format!("{CLAUDE_RESULT_LINE}\n")
            );
            ids.push(id.to_string());
        }
        assert_ne!(ids[0], ids[1]);
    }

    #[tokio::test]
    async fn no_transcript_without_run_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let program = stub(tmp.path(), &format!("echo '{CLAUDE_RESULT_LINE}'"));
        let outcome = run_claude(program, None).await.unwrap();
        assert_eq!(outcome.status, StageStatus::Success);
        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("provider-stub")]);
    }

    #[tokio::test]
    async fn no_transcript_in_dry_run() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let program = stub(tmp.path(), &format!("echo '{CLAUDE_RESULT_LINE}'"));
        let outcome = run(LlmCliProvider::Claude, program, Some(&run_dir), true, None)
            .await
            .unwrap();
        assert_eq!(
            outcome.context_updates.get("step.dry_run"),
            Some(&serde_json::json!(true))
        );
        assert!(!run_dir.exists());
    }

    #[tokio::test]
    async fn missing_binary_leaves_no_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let error = run_claude(tmp.path().join("no-such-provider"), Some(&run_dir))
            .await
            .unwrap_err();
        assert!(
            matches!(&error, AttractorError::CliNotFound { binary } if binary == "claude"),
            "{error}"
        );
        assert_eq!(transcripts(&run_dir), Vec::<PathBuf>::new());
        // No stderr log either: nothing at all under transcripts/.
        assert!(!run_dir.join("transcripts").exists());
    }

    // AC2: the Transcript grows while the provider is still running.
    #[tokio::test]
    async fn transcript_grows_while_provider_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let marker = tmp.path().join("provider-exiting");
        let program = stub(
            tmp.path(),
            &format!(
                "echo '{{\"type\":\"system\"}}'; sleep 0.3\n\
                 echo '{{\"type\":\"assistant\",\"n\":1}}'; sleep 0.3\n\
                 echo '{{\"type\":\"assistant\",\"n\":2}}'; sleep 0.3\n\
                 echo '{CLAUDE_RESULT_LINE}'; sleep 0.3\n\
                 touch '{}'",
                marker.display()
            ),
        );

        let poll = async {
            let mut sizes: Vec<u64> = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            loop {
                // Read the size first, then check the marker: a size read
                // before the marker exists was observed before the stub exited.
                let size = transcripts(&run_dir)
                    .first()
                    .and_then(|path| std::fs::metadata(path).ok())
                    .map(|meta| meta.len());
                if marker.exists() || tokio::time::Instant::now() > deadline {
                    break sizes;
                }
                if let Some(size) = size.filter(|size| *size > 0) {
                    if sizes.last() != Some(&size) {
                        sizes.push(size);
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        let (outcome, sizes) = tokio::join!(run_claude(program, Some(&run_dir)), poll);

        assert_eq!(outcome.unwrap().notes, "done");
        assert!(sizes.len() >= 2, "sizes seen before exit: {sizes:?}");
        assert!(sizes.windows(2).all(|w| w[0] < w[1]), "{sizes:?}");
        let final_size = std::fs::metadata(only_transcript(&run_dir)).unwrap().len();
        assert!(*sizes.last().unwrap() <= final_size);
    }

    // AC3: the Transcript is the provider's stdout byte for byte.
    #[tokio::test]
    async fn transcript_matches_provider_stdout_byte_for_byte() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let fixture = tmp.path().join("stdout.bin");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"{\"type\":\"system\",\"note\":\"caf\xc3\xa9 \xe2\x9c\x93\"}\n");
        bytes.extend_from_slice(b"{\"type\":\"assistant\",\"text\":\"a\\tb\"}\t\r\n");
        bytes.extend_from_slice(b"invalid utf-8: \xff\xfe\n");
        bytes.extend_from_slice(b"\n");
        bytes.extend_from_slice(CLAUDE_RESULT_LINE.as_bytes());
        bytes.extend_from_slice(b"\nno trailing newline");
        std::fs::write(&fixture, &bytes).unwrap();
        let program = stub(
            tmp.path(),
            &format!("cat '{}'; echo 'stderr noise' >&2", fixture.display()),
        );

        let outcome = run_claude(program, Some(&run_dir)).await.unwrap();

        assert_eq!(outcome.status, StageStatus::Success);
        assert_eq!(outcome.notes, "done");
        assert_eq!(std::fs::read(only_transcript(&run_dir)).unwrap(), bytes);
    }

    #[tokio::test]
    async fn codex_and_gemini_output_is_streamed_to_transcripts() {
        let tmp = tempfile::tempdir().unwrap();

        let codex_dir = tmp.path().join("codex-run");
        let codex_out = "{\"type\":\"thread.started\"}\n\
            {\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"codex done\"}}\n\
            {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}\n";
        let program = stub(tmp.path(), &format!("printf '%s' '{codex_out}'"));
        let outcome = run(
            LlmCliProvider::Codex,
            program,
            Some(&codex_dir),
            false,
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome.notes, "codex done");
        assert_eq!(
            std::fs::read_to_string(only_transcript(&codex_dir)).unwrap(),
            codex_out
        );

        let gemini_dir = tmp.path().join("gemini-run");
        let gemini_out = "{\n  \"response\": \"gemini done\"\n}";
        let program = stub(tmp.path(), &format!("printf '%s' '{gemini_out}'"));
        let outcome = run(
            LlmCliProvider::Gemini,
            program,
            Some(&gemini_dir),
            false,
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome.notes, "gemini done");
        assert_eq!(
            std::fs::read_to_string(only_transcript(&gemini_dir)).unwrap(),
            gemini_out
        );
    }

    // AC4: a provider that exits non-zero keeps its partial Transcript.
    #[tokio::test]
    async fn nonzero_exit_keeps_partial_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let partial = "{\"type\":\"system\"}\n{\"type\":\"assistant\"}\n";
        let program = stub(
            tmp.path(),
            &format!("printf '%s' '{partial}'; echo 'crashed' >&2; exit 3"),
        );

        let error = run_claude(program, Some(&run_dir)).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            handler_error(&format!(
                "attempt 1: Claude Code exited with exit status: 3; \
                 last stderr lines:\ncrashed\n{}",
                only_files(&run_dir)
            ))
        );
        assert_eq!(
            std::fs::read_to_string(only_transcript(&run_dir)).unwrap(),
            partial
        );
    }

    #[tokio::test]
    async fn nonzero_exit_with_final_result_is_parsed_and_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let result =
            r#"{"type":"result","subtype":"error","is_error":true,"result":"budget exceeded"}"#;
        let program = stub(
            tmp.path(),
            &format!("echo '{{\"type\":\"system\"}}'; echo '{result}'; exit 1"),
        );

        let outcome = run_claude(program, Some(&run_dir)).await.unwrap();

        assert_eq!(outcome.status, StageStatus::Fail);
        assert_eq!(outcome.notes, "budget exceeded");
        assert_eq!(
            outcome.failure_reason.as_deref(),
            Some("Claude Code returned an error")
        );
        assert_eq!(
            std::fs::read_to_string(only_transcript(&run_dir)).unwrap(),
            format!("{{\"type\":\"system\"}}\n{result}\n")
        );
    }

    #[tokio::test]
    async fn timeout_keeps_partial_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let program = stub(tmp.path(), "echo '{\"type\":\"system\"}'; sleep 10");

        let error = run(
            LlmCliProvider::Claude,
            program,
            Some(&run_dir),
            false,
            Some(Duration::from_millis(3000)),
        )
        .await
        .unwrap_err();

        assert!(error.is_retryable(), "{error}");
        assert_eq!(
            error.to_string(),
            format!(
                "node 'step' attempt 1 failed: timeout after 3000ms; {}",
                only_files(&run_dir)
            )
        );
        assert_eq!(
            std::fs::read_to_string(only_transcript(&run_dir)).unwrap(),
            "{\"type\":\"system\"}\n"
        );
    }

    // AC5: no output → empty Transcript and the same error as before.
    #[tokio::test]
    async fn silent_provider_leaves_empty_transcript_and_same_error() {
        // Each stub's expected error, given the Run folder.
        type Expected = fn(&Path) -> String;
        let cases: [(&str, Expected); 2] = [
            ("echo boom >&2; exit 0", |_| {
                handler_error("Claude Code produced no output. stderr: boom\n")
            }),
            ("echo boom >&2; exit 2", |run_dir| {
                handler_error(&format!(
                    "attempt 1: Claude Code exited with exit status: 2; \
                     last stderr lines:\nboom\n{}",
                    only_files(run_dir)
                ))
            }),
        ];
        for (body, expected) in cases {
            let tmp = tempfile::tempdir().unwrap();
            let run_dir = tmp.path().join("run");
            let program = stub(tmp.path(), body);

            let error = run_claude(program, Some(&run_dir)).await.unwrap_err();

            assert_eq!(error.to_string(), expected(&run_dir), "stub: {body}");
            let transcript = only_transcript(&run_dir);
            assert_eq!(std::fs::metadata(transcript).unwrap().len(), 0);
        }
    }

    #[tokio::test]
    async fn transcript_write_failure_does_not_fail_stage() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        std::fs::create_dir_all(&run_dir).unwrap();
        // `transcripts` is a regular file, so no Transcript can be created.
        std::fs::write(run_dir.join("transcripts"), "").unwrap();
        let program = stub(tmp.path(), &format!("echo '{CLAUDE_RESULT_LINE}'"));

        let outcome = run_claude(program, Some(&run_dir)).await.unwrap();

        assert_eq!(outcome.status, StageStatus::Success);
        assert_eq!(outcome.notes, "done");
        assert!(run_dir.join("transcripts").is_file());
    }

    // --- T2-4: one `LlmInvoked` per Model Invocation ---

    const CLAUDE_INIT_LINE: &str =
        r#"{"type":"system","subtype":"init","model":"claude-haiku-4-5"}"#;
    const CLAUDE_USAGE_RESULT_LINE: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"done","total_cost_usd":0.25,"num_turns":1,"usage":{"input_tokens":10,"cache_read_input_tokens":5,"output_tokens":7}}"#;

    #[derive(Default)]
    struct EventLog(std::sync::Mutex<Vec<crate::events::PipelineEvent>>);

    impl crate::handler::EventSink for EventLog {
        fn emit(&self, event: crate::events::PipelineEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    impl EventLog {
        /// The payload of every `kind` Event received; every Event must be
        /// `LlmStarted` or `LlmInvoked`.
        fn of(&self, kind: &str) -> Vec<serde_json::Value> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .map(|event| serde_json::to_value(event).unwrap())
                .inspect(|value| {
                    assert!(
                        value.get("LlmStarted").is_some() || value.get("LlmInvoked").is_some(),
                        "unexpected Event: {value}"
                    )
                })
                .filter_map(|value| value.get(kind).cloned())
                .collect()
        }

        fn llm_invoked(&self) -> Vec<serde_json::Value> {
            self.of("LlmInvoked")
        }

        /// Each Event's kind, in order.
        fn kinds(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .map(|event| serde_json::to_value(event).unwrap())
                .filter_map(|value| value.as_object()?.keys().next().cloned())
                .collect()
        }

        fn only(&self) -> serde_json::Value {
            let events = self.llm_invoked();
            assert_eq!(events.len(), 1, "expected one LlmInvoked: {events:?}");
            events.into_iter().next().unwrap()
        }
    }

    fn step_node(timeout: Option<Duration>) -> PipelineNode {
        let mut node = make_node("step", "box", Some("do work"), HashMap::new());
        node.timeout = timeout;
        node
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_observed(
        provider: LlmCliProvider,
        program: PathBuf,
        run_dir: Option<&Path>,
        node: &PipelineNode,
        graph: &PipelineGraph,
        dry_run: bool,
        events: &EventLog,
    ) -> Result<Outcome> {
        let resolved = ResolvedNode {
            node_id: node.id.clone(),
            kind: ResolvedNodeKind::Task,
            handler: crate::HandlerIdentity::Codergen,
            provider: Some(provider),
            invocation: Default::default(),
        };
        CodergenHandler::new(crate::handlers::tests::claude_agents(&program))
            .execute_with_controls(
                node,
                &resolved,
                &Context::default(),
                graph,
                CodergenExecutionControls {
                    dry_run,
                    workdir: None,
                    claude: Some(ClaudeCliConfig::default()),
                    run_dir: run_dir.map(Path::to_path_buf),
                    program: Some(program),
                    events: Some(events),
                    run_id: None,
                    attempt: 1,
                    cancel: CancellationToken::new(),
                    resume_note: None,
                },
            )
            .await
    }

    async fn run_claude_observed(
        program: PathBuf,
        run_dir: &Path,
        timeout: Option<Duration>,
        events: &EventLog,
    ) -> Result<Outcome> {
        run_observed(
            LlmCliProvider::Claude,
            program,
            Some(run_dir),
            &step_node(timeout),
            &make_minimal_graph(),
            false,
            events,
        )
        .await
    }

    fn transcript_stem(path: &Path) -> String {
        path.file_stem().unwrap().to_str().unwrap().to_owned()
    }

    // AC1: exactly one LlmInvoked per provider process, sharing the
    // Transcript's Invocation ID, with the usage the stream reported.
    #[tokio::test]
    async fn each_invocation_emits_one_llm_invoked_matching_its_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let program = stub(
            tmp.path(),
            &format!("echo '{CLAUDE_INIT_LINE}'; echo '{CLAUDE_USAGE_RESULT_LINE}'"),
        );
        let events = EventLog::default();

        for _ in 0..3 {
            let outcome = run_claude_observed(program.clone(), &run_dir, None, &events)
                .await
                .unwrap();
            assert_eq!(outcome.status, StageStatus::Success);
            assert_eq!(
                outcome.context_updates.get("step.cost_usd"),
                Some(&serde_json::json!(0.25))
            );
        }

        let invoked = events.llm_invoked();
        assert_eq!(invoked.len(), 3, "{invoked:?}");
        // Each invocation: LlmStarted, then LlmInvoked, with one id.
        assert_eq!(
            events.kinds(),
            ["LlmStarted", "LlmInvoked"].repeat(3),
            "event order"
        );
        let started = events.of("LlmStarted");
        for (started, invoked) in started.iter().zip(&invoked) {
            assert_eq!(started["invocation_id"], invoked["invocation_id"]);
            let id = started["invocation_id"].as_str().unwrap();
            assert_eq!(started["transcript"], format!("transcripts/{id}.jsonl"));
            assert_eq!(started["stderr"], format!("transcripts/{id}.stderr.log"));
            assert!(run_dir.join(started["stderr"].as_str().unwrap()).exists());
            assert_eq!(started["pid"], started["pgid"]);
        }
        let mut ids: Vec<String> = invoked
            .iter()
            .map(|event| event["invocation_id"].as_str().unwrap().to_owned())
            .collect();
        ids.sort();
        let mut stems: Vec<String> = transcripts(&run_dir)
            .iter()
            .map(|path| transcript_stem(path))
            .collect();
        stems.sort();
        assert_eq!(ids, stems);
        ids.dedup();
        assert_eq!(ids.len(), 3);
        for event in &invoked {
            assert_eq!(event["node_id"], "step");
            assert_eq!(event["provider"], "claude");
            assert_eq!(event["status"], "success");
            assert_eq!(event["model_actual"], "claude-haiku-4-5");
            assert_eq!(event["input_tokens"], 15);
            assert_eq!(event["output_tokens"], 7);
            assert_eq!(event["cost_usd"], 0.25);
            assert!(event["duration_ms"].is_u64(), "{event}");
        }
    }

    #[tokio::test]
    async fn codex_and_gemini_emit_one_llm_invoked_each() {
        let tmp = tempfile::tempdir().unwrap();
        let codex_out = "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"codex done\"}}\n\
            {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":4,\"output_tokens\":2}}\n";
        let gemini_out = "{\"response\":\"gemini done\",\"stats\":{\"models\":{\"gemini-2.5-pro\":{\"tokens\":{\"prompt\":8,\"candidates\":3}}}}}";
        for (provider, out, name) in [
            (LlmCliProvider::Codex, codex_out, "codex"),
            (LlmCliProvider::Gemini, gemini_out, "gemini"),
        ] {
            let run_dir = tmp.path().join(name);
            let program = stub(tmp.path(), &format!("printf '%s' '{out}'"));
            let events = EventLog::default();

            let outcome = run_observed(
                provider,
                program,
                Some(&run_dir),
                &step_node(None),
                &make_minimal_graph(),
                false,
                &events,
            )
            .await
            .unwrap();

            assert_eq!(outcome.status, StageStatus::Success, "{name}");
            let event = events.only();
            assert_eq!(event["provider"], name);
            assert_eq!(event["status"], "success");
            assert_eq!(
                event["invocation_id"].as_str().unwrap(),
                transcript_stem(&only_transcript(&run_dir))
            );
            assert!(event["input_tokens"].is_u64(), "{name}: {event}");
            assert!(event.get("cost_usd").is_none(), "{name}: {event}");
        }
    }

    // AC1 boundary: no provider process → no Model Invocation → no Event.
    #[tokio::test]
    async fn no_llm_invoked_without_a_model_invocation_or_run_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let program = stub(tmp.path(), &format!("echo '{CLAUDE_RESULT_LINE}'"));
        let graph = make_minimal_graph();
        let node = step_node(None);

        let dry = EventLog::default();
        let provider = LlmCliProvider::Claude;
        run_observed(
            provider,
            program.clone(),
            Some(&run_dir),
            &node,
            &graph,
            true,
            &dry,
        )
        .await
        .unwrap();
        assert!(dry.llm_invoked().is_empty());

        let missing = EventLog::default();
        let error = run_observed(
            provider,
            tmp.path().join("no-such-provider"),
            Some(&run_dir),
            &node,
            &graph,
            false,
            &missing,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, AttractorError::CliNotFound { .. }),
            "{error}"
        );
        assert!(missing.llm_invoked().is_empty());
        assert!(missing.of("LlmStarted").is_empty());

        let no_run_dir = EventLog::default();
        let outcome = run_observed(provider, program, None, &node, &graph, false, &no_run_dir)
            .await
            .unwrap();
        assert_eq!(outcome.status, StageStatus::Success);
        assert!(no_run_dir.llm_invoked().is_empty());
    }

    // AC2: node llm_model, then graph `model`, else the field is absent.
    #[tokio::test]
    async fn model_requested_prefers_node_then_graph_then_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let program = stub(tmp.path(), &format!("echo '{CLAUDE_RESULT_LINE}'"));
        let mut with_graph_model = make_minimal_graph();
        with_graph_model
            .attrs
            .insert("model".into(), AttributeValue::String("opus".into()));
        let mut with_node_model = step_node(None);
        with_node_model.llm_model = Some("sonnet".into());

        let cases = [
            (&with_node_model, &with_graph_model, Some("sonnet")),
            (&with_node_model, &make_minimal_graph(), Some("sonnet")),
            (&step_node(None), &with_graph_model, Some("opus")),
            (&step_node(None), &make_minimal_graph(), None),
        ];
        for (i, (node, graph, expected)) in cases.into_iter().enumerate() {
            let run_dir = tmp.path().join(format!("run-{i}"));
            let events = EventLog::default();
            run_observed(
                LlmCliProvider::Claude,
                program.clone(),
                Some(&run_dir),
                node,
                graph,
                false,
                &events,
            )
            .await
            .unwrap();

            let event = events.only();
            match expected {
                Some(model) => assert_eq!(event["model_requested"], model, "case {i}"),
                None => assert!(
                    event.get("model_requested").is_none(),
                    "case {i}: key must be absent: {event}"
                ),
            }
            // The journal line leaves the key out too.
            let journal = events
                .0
                .lock()
                .unwrap()
                .iter()
                .map(|event| serde_json::to_value(event.to_journal_data()).unwrap())
                .find(|value| value["type"] == "LlmInvoked")
                .unwrap();
            assert_eq!(
                journal.to_string().contains("model_requested"),
                expected.is_some(),
                "case {i}: {journal}"
            );
        }
    }

    // AC3: `transcript` is relative to the Run folder and names the file.
    #[tokio::test]
    async fn llm_invoked_transcript_is_relative_and_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let program = stub(tmp.path(), &format!("echo '{CLAUDE_RESULT_LINE}'"));
        let events = EventLog::default();

        run_claude_observed(program, &run_dir, None, &events)
            .await
            .unwrap();

        let event = events.only();
        let transcript = event["transcript"].as_str().unwrap();
        let id = event["invocation_id"].as_str().unwrap();
        assert_eq!(transcript, format!("transcripts/{id}.jsonl"));
        assert!(Path::new(transcript).is_relative());
        assert!(run_dir.join(transcript).is_file());
        assert_eq!(run_dir.join(transcript), only_transcript(&run_dir));
    }

    // AC3 boundary: a Transcript that cannot be written keeps the stage
    // and the Event; the path is still the relative one.
    #[tokio::test]
    async fn transcript_write_failure_still_emits_llm_invoked() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("transcripts"), "").unwrap();
        let program = stub(
            tmp.path(),
            &format!("echo '{CLAUDE_INIT_LINE}'; echo '{CLAUDE_USAGE_RESULT_LINE}'"),
        );
        let events = EventLog::default();

        let outcome = run_claude_observed(program, &run_dir, None, &events)
            .await
            .unwrap();

        assert_eq!(outcome.status, StageStatus::Success);
        let event = events.only();
        assert_eq!(event["status"], "success");
        assert_eq!(event["model_actual"], "claude-haiku-4-5");
        assert!(event["transcript"]
            .as_str()
            .unwrap()
            .starts_with("transcripts/"));
    }

    // AC4: the handler's own timeout.
    #[tokio::test]
    async fn handler_timeout_emits_timeout_with_measured_duration() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let program = stub(tmp.path(), &format!("echo '{CLAUDE_INIT_LINE}'; sleep 10"));
        let events = EventLog::default();

        let error = run_claude_observed(
            program,
            &run_dir,
            Some(Duration::from_millis(3000)),
            &events,
        )
        .await
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            format!(
                "node 'step' attempt 1 failed: timeout after 3000ms; {}",
                only_files(&run_dir)
            )
        );
        let event = events.only();
        assert_eq!(event["status"], "timeout");
        let duration = event["duration_ms"].as_u64().unwrap();
        assert!((3000..10_000).contains(&duration), "duration_ms {duration}");
        // Usage comes from the partial Transcript.
        assert_eq!(event["model_actual"], "claude-haiku-4-5");
        assert_eq!(
            event["invocation_id"].as_str().unwrap(),
            transcript_stem(&only_transcript(&run_dir))
        );
    }

    // AC4: the engine's outer deadline drops the handler future first.
    #[tokio::test]
    async fn dropped_handler_emits_timeout_with_measured_duration() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let program = stub(tmp.path(), &format!("echo '{CLAUDE_INIT_LINE}'; sleep 10"));
        let events = EventLog::default();

        let before = std::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_millis(3000),
            run_claude_observed(program, &run_dir, Some(Duration::from_secs(60)), &events),
        )
        .await;
        let elapsed = u64::try_from(before.elapsed().as_millis()).unwrap();

        assert!(result.is_err(), "the outer deadline must win");
        let event = events.only();
        assert_eq!(event["status"], "timeout");
        // Measured from spawn, so up to the outer deadline's time, which
        // also covers the work before spawn.
        let duration = event["duration_ms"].as_u64().unwrap();
        assert!(
            (1000..=elapsed).contains(&duration),
            "duration_ms {duration}, outer elapsed {elapsed}"
        );
        assert_eq!(event["model_actual"], "claude-haiku-4-5");
    }

    // AC5: a provider that exits non-zero without a final result.
    #[tokio::test]
    async fn nonzero_exit_without_result_emits_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let program = stub(
            tmp.path(),
            &format!("echo '{CLAUDE_INIT_LINE}'; echo 'crashed' >&2; exit 3"),
        );
        let events = EventLog::default();

        let error = run_claude_observed(program, &run_dir, None, &events)
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            handler_error(&format!(
                "attempt 1: Claude Code exited with exit status: 3; \
                 last stderr lines:\ncrashed\n{}",
                only_files(&run_dir)
            ))
        );
        let event = events.only();
        assert_eq!(event["status"], "failed");
        assert_eq!(event["model_actual"], "claude-haiku-4-5");
        assert!(run_dir
            .join(event["transcript"].as_str().unwrap())
            .is_file());
    }

    // AC5: the provider reports an error result.
    #[tokio::test]
    async fn error_result_emits_failed_with_usage() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let result = r#"{"type":"result","subtype":"error","is_error":true,"result":"budget exceeded","total_cost_usd":0.5,"num_turns":1,"usage":{"input_tokens":3,"output_tokens":1}}"#;
        let program = stub(tmp.path(), &format!("echo '{result}'"));
        let events = EventLog::default();

        let outcome = run_claude_observed(program, &run_dir, None, &events)
            .await
            .unwrap();

        assert_eq!(outcome.status, StageStatus::Fail);
        assert_eq!(
            outcome.failure_reason.as_deref(),
            Some("Claude Code returned an error")
        );
        let event = events.only();
        assert_eq!(event["status"], "failed");
        assert_eq!(event["cost_usd"], 0.5);
        assert_eq!(event["input_tokens"], 3);
        assert_eq!(event["output_tokens"], 1);
    }

    // AC5: output that cannot be parsed, and no output at all.
    #[tokio::test]
    async fn unparseable_or_empty_output_emits_failed() {
        for (body, expected) in [
            ("echo 'not json'", "Failed to parse Claude output"),
            ("echo boom >&2", "Claude Code produced no output"),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let run_dir = tmp.path().join("run");
            let program = stub(tmp.path(), body);
            let events = EventLog::default();

            let error = run_claude_observed(program, &run_dir, None, &events)
                .await
                .unwrap_err();

            assert!(error.to_string().contains(expected), "{body}: {error}");
            assert_eq!(events.only()["status"], "failed", "{body}");
        }
    }

    // A non-zero exit with a final result is used as the result (T2-2), so
    // the invocation succeeded.
    #[tokio::test]
    async fn nonzero_exit_with_successful_result_emits_success() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("run");
        let program = stub(tmp.path(), &format!("echo '{CLAUDE_RESULT_LINE}'; exit 1"));
        let events = EventLog::default();

        let outcome = run_claude_observed(program, &run_dir, None, &events)
            .await
            .unwrap();

        assert_eq!(outcome.status, StageStatus::Success);
        assert_eq!(events.only()["status"], "success");
    }
}

// --- Provider stream summaries (model, tokens, cost) ---

const CODEX_FIXTURE: &str = include_str!("../../tests/fixtures/providers/codex-0.151.0.jsonl");
const GEMINI_STREAM_FIXTURE: &str =
    include_str!("../../tests/fixtures/providers/gemini-0.61.0.stream.jsonl");
const GEMINI_JSON_FIXTURE: &str = include_str!("../../tests/fixtures/providers/gemini-0.61.0.json");
const GEMINI_FIXTURE_TEXT: &str = "Checked the file.\nOK";

fn usage(
    model_actual: Option<&str>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cost_usd: Option<f64>,
) -> InvocationUsage {
    InvocationUsage {
        model_actual: model_actual.map(str::to_owned),
        input_tokens,
        output_tokens,
        cost_usd,
    }
}

// AC1: recorded Codex fixture. Codex reports neither model nor cost.
#[test]
fn codex_fixture_yields_text_and_tokens() {
    let parsed = parse_cli_output(CliProvider::Codex, CODEX_FIXTURE, "", "n").unwrap();
    assert_eq!(parsed.text, "OK");
    assert!(!parsed.is_error);
    assert_eq!(parsed.cost_usd, None);
    assert_eq!(parsed.usage, usage(None, Some(20_446), Some(5), None));
}

// AC1: Gemini stream-json fixture. The model that wrote the most output wins
// over the configured `init` model; Gemini reports no cost.
#[test]
fn gemini_stream_fixture_yields_text_model_tokens() {
    let parsed = parse_cli_output(CliProvider::Gemini, GEMINI_STREAM_FIXTURE, "", "n").unwrap();
    assert_eq!(parsed.text, GEMINI_FIXTURE_TEXT);
    assert!(!parsed.is_error);
    assert_eq!(parsed.cost_usd, None);
    assert_eq!(
        parsed.usage,
        usage(Some("gemini-2.5-pro"), Some(8_900), Some(70), None)
    );
}

// AC1: Gemini json fixture gives the same text and summary as stream-json.
#[test]
fn gemini_json_fixture_yields_text_model_tokens() {
    let parsed = parse_cli_output(CliProvider::Gemini, GEMINI_JSON_FIXTURE, "", "n").unwrap();
    let streamed = parse_cli_output(CliProvider::Gemini, GEMINI_STREAM_FIXTURE, "", "n").unwrap();
    assert_eq!(parsed.text, GEMINI_FIXTURE_TEXT);
    assert!(!parsed.is_error);
    assert_eq!(
        parsed.usage,
        usage(Some("gemini-2.5-pro"), Some(8_900), Some(70), None)
    );
    assert_eq!(parsed.text, streamed.text);
    assert_eq!(parsed.usage, streamed.usage);
}

/// Add unknown fields to every JSON line (top level and inside the token
/// objects) and an unknown event type before the last line.
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

// AC2: unknown fields and unknown event types do not change the result.
#[test]
fn summaries_ignore_unknown_fields_and_event_types() {
    for (provider, fixture, multi_line_object) in [
        (CliProvider::Codex, CODEX_FIXTURE, false),
        (CliProvider::Gemini, GEMINI_STREAM_FIXTURE, false),
        (CliProvider::Gemini, GEMINI_JSON_FIXTURE, true),
    ] {
        let widened = with_unknown_fields(fixture, multi_line_object);
        assert!(widened.contains("pas_unknown_field"));
        let expected = parse_cli_output(provider, fixture, "", "n").unwrap();
        let parsed = parse_cli_output(provider, &widened, "", "n")
            .unwrap_or_else(|e| panic!("{provider:?}: {e}"));
        assert_eq!(parsed.text, expected.text, "{provider:?}");
        assert_eq!(parsed.is_error, expected.is_error, "{provider:?}");
        assert_eq!(parsed.cost_usd, expected.cost_usd, "{provider:?}");
        assert_eq!(parsed.turns, expected.turns, "{provider:?}");
        assert_eq!(parsed.usage, expected.usage, "{provider:?}");
        assert_ne!(parsed.usage, InvocationUsage::default(), "{provider:?}");
    }
}

// AC3: a stream without model or cost data gives `None` for those fields,
// and the Outcome fields are what they were before summaries existed.
#[test]
fn stream_without_model_or_cost_yields_none() {
    // Claude's cases are in attractor-handler-claude-p.

    // Codex: no turn.completed event.
    let codex =
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"hi\"}}\n";
    let parsed = parse_cli_output(CliProvider::Codex, codex, "", "n").unwrap();
    assert_eq!(parsed.text, "hi");
    assert_eq!(parsed.usage, InvocationUsage::default());

    // Gemini stream-json: a result without stats and no init.
    let gemini_stream =
        "{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"hi\",\"delta\":true}\n\
        {\"type\":\"result\",\"status\":\"success\"}\n";
    let parsed = parse_cli_output(CliProvider::Gemini, gemini_stream, "", "n").unwrap();
    assert_eq!(parsed.text, "hi");
    assert!(!parsed.is_error);
    assert_eq!(parsed.usage, InvocationUsage::default());

    // Gemini json: no stats.
    let parsed = parse_cli_output(CliProvider::Gemini, r#"{"response":"hi"}"#, "", "n").unwrap();
    assert_eq!(parsed.text, "hi");
    assert_eq!(parsed.usage, InvocationUsage::default());
}

// AC3 boundary: unreadable input never fails the summary.
#[test]
fn summaries_of_unreadable_streams_are_empty() {
    let torn = "not json\n{\"type\":\"result\",\"total_cost_usd\":0.5,\"usa";
    let wrong_types = "{\"type\":\"result\",\"total_cost_usd\":\"cheap\",\"result\":\"x\"}\n\
        {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":\"many\"}}\n";
    for provider in [CliProvider::Codex, CliProvider::Gemini] {
        for stdout in ["", "   \n", "not json at all", torn, wrong_types, "[1,2]"] {
            assert_eq!(
                summarize_stream(provider, stdout),
                InvocationUsage::default(),
                "{provider:?} {stdout:?}"
            );
        }
    }
}

#[test]
fn codex_summary_sums_turns() {
    let stream =
        "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}\n\
        {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":20,\"output_tokens\":2}}\n";
    assert_eq!(
        summarize_stream(CliProvider::Codex, stream),
        usage(None, Some(30), Some(3), None)
    );
}

#[test]
fn parse_gemini_stream_error_result_is_an_error_result() {
    let stream = "{\"type\":\"init\",\"model\":\"gemini-2.5-pro\"}\n\
        {\"type\":\"message\",\"role\":\"assistant\",\"content\":\"partial\",\"delta\":true}\n\
        {\"type\":\"result\",\"status\":\"error\",\"error\":{\"type\":\"FatalTurnLimitedError\",\"message\":\"turn limit\"},\"stats\":{\"input_tokens\":5,\"output_tokens\":1}}\n";
    let parsed = parse_cli_output(CliProvider::Gemini, stream, "", "n").unwrap();
    assert!(parsed.is_error);
    assert_eq!(parsed.text, "turn limit");
    assert_eq!(
        parsed.usage,
        usage(Some("gemini-2.5-pro"), Some(5), Some(1), None)
    );
}

#[test]
fn parse_gemini_stream_without_result_is_a_parse_error() {
    let stream = "{\"type\":\"init\",\"model\":\"m\"}\n\
        {\"type\":\"message\",\"role\":\"assistant\",\"content\":\"partial\"}\n";
    let error = parse_gemini_stream_output(stream, "n")
        .unwrap_err()
        .to_string();
    assert!(error.contains("Failed to parse Gemini output"), "{error}");
}

// AC5 (command): the chosen Gemini format is what reaches argv.
#[test]
fn build_cli_command_gemini_stream_json_replaces_json() {
    let graph = make_minimal_graph();
    let cfg = CliRunConfig {
        provider: CliProvider::Gemini,
        prompt: "test prompt",
        model: None,
        workdir: None,
        graph: &graph,
    };
    let args = |format| {
        build_cli_command_with_program(&cfg, "gemini".as_ref(), format)
            .as_std()
            .get_args()
            .map(|a| a.to_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        args(GeminiOutputFormat::StreamJson),
        vec![
            "--output-format",
            "stream-json",
            "--approval-mode",
            "yolo",
            "test prompt"
        ]
    );
    assert_eq!(
        args(GeminiOutputFormat::Json),
        vec![
            "--output-format",
            "json",
            "--approval-mode",
            "yolo",
            "test prompt"
        ]
    );
}

// --- Stream summaries and Gemini format selection with stub providers ---

#[cfg(unix)]
mod stream_formats {
    use std::path::{Path, PathBuf};

    use attractor_types::{Context, Outcome, Result};

    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/providers")
            .join(name)
    }

    fn stub(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("provider-stub");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    async fn run(provider: LlmCliProvider, program: PathBuf) -> Result<Outcome> {
        let node = make_node("step", "box", Some("do work"), HashMap::new());
        let resolved = ResolvedNode {
            node_id: node.id.clone(),
            kind: ResolvedNodeKind::Task,
            handler: crate::HandlerIdentity::Codergen,
            provider: Some(provider),
            invocation: Default::default(),
        };
        CodergenHandler::new(crate::handlers::tests::claude_agents(&program))
            .execute_with_controls(
                &node,
                &resolved,
                &Context::default(),
                &make_minimal_graph(),
                CodergenExecutionControls {
                    dry_run: false,
                    workdir: None,
                    claude: Some(ClaudeCliConfig::default()),
                    run_dir: None,
                    program: Some(program),
                    events: None,
                    run_id: None,
                    attempt: 1,
                    cancel: CancellationToken::new(),
                    resume_note: None,
                },
            )
            .await
    }

    /// A Gemini stub: `--help` prints `help` and exits `help_exit`; a run
    /// logs its argv and prints the fixture matching `--output-format`,
    /// rejecting `stream-json` like a pre-0.11.0 CLI unless `stream_ok`.
    fn gemini_stub(dir: &Path, help: &str, help_exit: u8, stream_ok: bool) -> PathBuf {
        let log = dir.join("argv.log");
        let helps = dir.join("help.count");
        let stream = if stream_ok {
            format!("cat '{}'", fixture("gemini-0.61.0.stream.jsonl").display())
        } else {
            "echo 'Invalid values: Argument: output-format, Given: \"stream-json\"' >&2; exit 1"
                .into()
        };
        stub(
            dir,
            &format!(
                "if [ \"$1\" = --help ]; then echo x >> '{helps}'; echo '{help}'; exit {help_exit}; fi\n\
                 echo \"$@\" > '{log}'\n\
                 if [ \"$2\" = stream-json ]; then {stream}; exit 0; fi\n\
                 cat '{json}'",
                helps = helps.display(),
                log = log.display(),
                json = fixture("gemini-0.61.0.json").display(),
            ),
        )
    }

    fn argv(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("argv.log")).unwrap()
    }

    const OLD_HELP: &str = "--output-format  [choices: \"text\", \"json\"]";
    const NEW_HELP: &str = "--output-format  [choices: \"text\", \"json\", \"stream-json\"]";

    // AC5: a Gemini CLI without stream-json gets `--output-format json`.
    #[tokio::test]
    async fn gemini_without_stream_json_falls_back_to_json() {
        let tmp = tempfile::tempdir().unwrap();
        let program = gemini_stub(tmp.path(), OLD_HELP, 0, false);
        let outcome = run(LlmCliProvider::Gemini, program).await.unwrap();
        assert_eq!(outcome.status, StageStatus::Success);
        assert_eq!(outcome.notes, GEMINI_FIXTURE_TEXT);
        assert!(
            argv(tmp.path()).starts_with("--output-format json --approval-mode yolo"),
            "{}",
            argv(tmp.path())
        );
    }

    // AC5 counterpart: a CLI that lists stream-json gets it.
    #[tokio::test]
    async fn gemini_with_stream_json_uses_it() {
        let tmp = tempfile::tempdir().unwrap();
        let program = gemini_stub(tmp.path(), NEW_HELP, 0, true);
        let outcome = run(LlmCliProvider::Gemini, program).await.unwrap();
        assert_eq!(outcome.status, StageStatus::Success);
        assert_eq!(outcome.notes, GEMINI_FIXTURE_TEXT);
        assert!(
            argv(tmp.path()).starts_with("--output-format stream-json --approval-mode yolo"),
            "{}",
            argv(tmp.path())
        );
    }

    // AC5 boundary: a failing `--help` means json, even if it names stream-json.
    #[tokio::test]
    async fn gemini_probe_failure_falls_back_to_json() {
        let tmp = tempfile::tempdir().unwrap();
        let program = gemini_stub(tmp.path(), NEW_HELP, 3, false);
        let outcome = run(LlmCliProvider::Gemini, program).await.unwrap();
        assert_eq!(outcome.status, StageStatus::Success);
        assert!(argv(tmp.path()).starts_with("--output-format json "));
    }

    // The probe runs once per program path, not once per Model Invocation.
    #[tokio::test]
    async fn gemini_probe_runs_once_per_program() {
        let tmp = tempfile::tempdir().unwrap();
        let program = gemini_stub(tmp.path(), NEW_HELP, 0, true);
        for _ in 0..3 {
            let outcome = run(LlmCliProvider::Gemini, program.clone()).await.unwrap();
            assert_eq!(outcome.status, StageStatus::Success);
        }
        let helps = std::fs::read_to_string(tmp.path().join("help.count")).unwrap();
        assert_eq!(helps.lines().count(), 1);
    }

    // A stream-json run that exits non-zero before its result event is
    // reported like an empty stdout, as json mode is.
    #[tokio::test]
    async fn gemini_stream_nonzero_exit_without_result_keeps_exit_error() {
        let tmp = tempfile::tempdir().unwrap();
        let program = stub(
            tmp.path(),
            &format!(
                "if [ \"$1\" = --help ]; then echo '{NEW_HELP}'; exit 0; fi\n\
                 echo '{{\"type\":\"init\",\"model\":\"m\"}}'\n\
                 echo 'quota exceeded' >&2\n\
                 exit 1"
            ),
        );
        let error = run(LlmCliProvider::Gemini, program)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Gemini CLI exited with exit status: 1"),
            "{error}"
        );
        assert!(error.contains("quota exceeded"), "{error}");
    }

    // AC3 at the stage level: no model or cost in the stream, stage succeeds,
    // and the Outcome keeps today's `cost_usd = 0.0` for the budget.
    #[tokio::test]
    async fn stage_succeeds_when_stream_has_no_model_or_cost() {
        let tmp = tempfile::tempdir().unwrap();
        let program = stub(
            tmp.path(),
            "echo '{\"type\":\"result\",\"result\":\"done\",\"is_error\":false}'",
        );
        let outcome = run(LlmCliProvider::Claude, program).await.unwrap();
        assert_eq!(outcome.status, StageStatus::Success);
        assert_eq!(outcome.notes, "done");
        assert_eq!(
            outcome.context_updates.get("step.cost_usd"),
            Some(&serde_json::json!(0.0))
        );
        assert_eq!(
            outcome.context_updates.get("step.turns"),
            Some(&serde_json::json!(0))
        );

        let tmp = tempfile::tempdir().unwrap();
        let program = stub(
            tmp.path(),
            "echo '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"codex done\"}}'",
        );
        let outcome = run(LlmCliProvider::Codex, program).await.unwrap();
        assert_eq!(outcome.status, StageStatus::Success);
        assert_eq!(outcome.notes, "codex done");
        assert!(!outcome.context_updates.contains_key("step.cost_usd"));
    }

    // AC1 at the stage level: each recorded fixture gives the same Outcome
    // text through the handler as through the parser.
}

// --- claude_outcome: one AgentResult per row of the mapping ---

mod claude_outcomes {
    use std::path::{Path, PathBuf};

    use attractor_agent_handler::{AgentResult, AgentStatus, FailureClass, Usage};
    use attractor_types::{AgentFiles, AttractorError, StageStatus};

    use super::claude::{agent_failure_files, crash_message, AgentAttempt};

    use super::*;

    fn completed(text: &str, status: AgentStatus) -> AgentResult {
        AgentResult {
            status,
            text: text.into(),
            usage: Usage {
                cost_usd: Some(0.25),
                turns: Some(3),
                ..Usage::default()
            },
            ..AgentResult::failed("inv", FailureClass::NoResult, "")
        }
    }

    fn outcome_of(result: &AgentResult) -> attractor_types::Result<Outcome> {
        outcome_in(result, None)
    }

    /// The outcome of attempt 2 of `step`, invocation `inv`, with a 1500 ms
    /// timeout, in `run_dir` if given.
    fn outcome_in(
        result: &AgentResult,
        run_dir: Option<&Path>,
    ) -> attractor_types::Result<Outcome> {
        let node = make_node("step", "box", Some("do work"), HashMap::new());
        let resolved = ResolvedNode {
            node_id: node.id.clone(),
            kind: ResolvedNodeKind::Task,
            handler: crate::HandlerIdentity::Codergen,
            provider: Some(LlmProvider::Claude),
            invocation: Default::default(),
        };
        claude_outcome(
            result,
            &node,
            &resolved,
            &make_minimal_graph(),
            &AgentAttempt {
                attempt: 2,
                timeout_ms: 1500,
                run_dir,
                invocation_id: "inv",
            },
        )
    }

    const RUN: &str = "/logs/p/runs/r1";

    fn files() -> AgentFiles {
        AgentFiles {
            transcript: PathBuf::from("/logs/p/runs/r1/transcripts/inv.jsonl"),
            stderr: PathBuf::from("/logs/p/runs/r1/transcripts/inv.stderr.log"),
        }
    }

    fn crash(detail: &str, stderr_tail: &str) -> AgentResult {
        AgentResult {
            stderr_tail: stderr_tail.into(),
            ..AgentResult::failed("inv", FailureClass::Crash, detail)
        }
    }

    #[test]
    fn agent_failure_files_are_the_run_folders_transcript_and_stderr() {
        assert_eq!(agent_failure_files(None, "inv"), None);
        assert_eq!(
            agent_failure_files(Some(Path::new(RUN)), "inv"),
            Some(files())
        );
    }

    #[test]
    fn crash_message_names_attempt_status_tail_and_files() {
        assert_eq!(
            crash_message(
                "Claude Code",
                2,
                "exited with exit status: 3",
                "one\ntwo",
                Some(&files())
            ),
            "attempt 2: Claude Code exited with exit status: 3; last stderr lines:\none\ntwo\n\
             transcript /logs/p/runs/r1/transcripts/inv.jsonl; \
             stderr /logs/p/runs/r1/transcripts/inv.stderr.log"
        );
    }

    #[test]
    fn crash_message_without_stderr_or_run_folder() {
        assert_eq!(
            crash_message("Claude Code", 1, "exited with exit status: 3", "", None),
            "attempt 1: Claude Code exited with exit status: 3; last stderr lines:\n(no stderr)"
        );
    }

    fn handler_error(message: &str) -> String {
        AttractorError::HandlerError {
            handler: "codergen".into(),
            node: "step".into(),
            message: message.into(),
        }
        .to_string()
    }

    #[test]
    fn completed_is_success_with_todays_context_updates() {
        let outcome = outcome_of(&completed("all done", AgentStatus::Completed)).unwrap();
        assert_eq!(outcome.status, StageStatus::Success);
        assert_eq!(outcome.notes, "all done");
        assert_eq!(outcome.failure_reason, None);
        let updates = &outcome.context_updates;
        assert_eq!(updates["step.result"], "all done");
        assert_eq!(updates["step.completed"], true);
        assert_eq!(updates["step.provider"], "Claude Code");
        assert_eq!(updates["step.cost_usd"], 0.25);
        assert_eq!(updates["step.turns"], 3);
    }

    #[test]
    fn completed_without_cost_records_zero_cost_as_before() {
        let result = AgentResult {
            usage: Usage::default(),
            ..completed("x", AgentStatus::Completed)
        };
        let outcome = outcome_of(&result).unwrap();
        assert_eq!(outcome.context_updates["step.cost_usd"], 0.0);
        assert!(!outcome.context_updates.contains_key("step.turns"));
    }

    #[test]
    fn reported_is_fail_with_todays_reason() {
        let result = completed("it broke", AgentStatus::Failed(FailureClass::Reported));
        let outcome = outcome_of(&result).unwrap();
        assert_eq!(outcome.status, StageStatus::Fail);
        assert_eq!(
            outcome.failure_reason.as_deref(),
            Some("Claude Code returned an error")
        );
        assert_eq!(outcome.context_updates["step.result"], "it broke");
    }

    #[test]
    fn timeout_is_the_retryable_agent_timeout_without_a_run_folder() {
        let result = AgentResult::failed("inv", FailureClass::Timeout, "timed out after 1500ms");
        let error = outcome_of(&result).unwrap_err();
        assert!(error.is_retryable(), "{error}");
        assert_eq!(
            error.to_string(),
            "node 'step' attempt 2 failed: timeout after 1500ms"
        );
        assert!(
            matches!(
                &error,
                AttractorError::AgentTimeout {
                    node,
                    attempt: 2,
                    timeout_ms: 1500,
                    files: None,
                } if node == "step"
            ),
            "{error:?}"
        );
    }

    #[test]
    fn timeout_in_a_run_folder_names_the_transcript_and_stderr() {
        let result = AgentResult::failed("inv", FailureClass::Timeout, "timed out after 1500ms");
        let error = outcome_in(&result, Some(Path::new(RUN))).unwrap_err();
        assert_eq!(
            error.to_string(),
            "node 'step' attempt 2 failed: timeout after 1500ms; \
             transcript /logs/p/runs/r1/transcripts/inv.jsonl; \
             stderr /logs/p/runs/r1/transcripts/inv.stderr.log"
        );
    }

    #[test]
    fn crash_names_attempt_status_and_stderr_tail_without_a_run_folder() {
        let result = crash("exited with exit status: 3", "fake-claude: crashed");
        assert_eq!(
            outcome_of(&result).unwrap_err().to_string(),
            handler_error(
                "attempt 2: Claude Code exited with exit status: 3; \
                 last stderr lines:\nfake-claude: crashed"
            )
        );
    }

    #[test]
    fn crash_in_a_run_folder_also_names_the_transcript_and_stderr() {
        let result = crash("exited with exit status: 3", "fake-claude: crashed");
        assert_eq!(
            outcome_in(&result, Some(Path::new(RUN)))
                .unwrap_err()
                .to_string(),
            handler_error(
                "attempt 2: Claude Code exited with exit status: 3; \
                 last stderr lines:\nfake-claude: crashed\n\
                 transcript /logs/p/runs/r1/transcripts/inv.jsonl; \
                 stderr /logs/p/runs/r1/transcripts/inv.stderr.log"
            )
        );
    }

    #[test]
    fn crash_without_stderr_says_so() {
        let result = crash("exited with signal: 9 (SIGKILL)", "");
        assert_eq!(
            outcome_of(&result).unwrap_err().to_string(),
            handler_error(
                "attempt 2: Claude Code exited with signal: 9 (SIGKILL); \
                 last stderr lines:\n(no stderr)"
            )
        );
    }

    #[test]
    fn a_failed_wait_is_a_crash_message_with_its_detail() {
        let result = crash("execution failed: wait broke", "");
        assert_eq!(
            outcome_in(&result, Some(Path::new(RUN)))
                .unwrap_err()
                .to_string(),
            handler_error(
                "attempt 2: Claude Code execution failed: wait broke; \
                 last stderr lines:\n(no stderr)\n\
                 transcript /logs/p/runs/r1/transcripts/inv.jsonl; \
                 stderr /logs/p/runs/r1/transcripts/inv.stderr.log"
            )
        );
    }

    #[test]
    fn no_result_keeps_its_message_in_a_run_folder() {
        let result = AgentResult {
            stderr_tail: "boom".into(),
            ..AgentResult::failed(
                "inv",
                FailureClass::NoResult,
                "Claude Code produced no output. stderr: boom",
            )
        };
        assert_eq!(
            outcome_in(&result, Some(Path::new(RUN)))
                .unwrap_err()
                .to_string(),
            handler_error("Claude Code produced no output. stderr: boom")
        );
    }

    #[test]
    fn no_result_passes_the_parse_message_through() {
        let result = AgentResult::failed(
            "inv",
            FailureClass::NoResult,
            "Failed to parse Claude output: oops — raw: x",
        );
        assert_eq!(
            outcome_of(&result).unwrap_err().to_string(),
            handler_error("Failed to parse Claude output: oops — raw: x")
        );
    }

    #[test]
    fn launch_not_found_is_cli_not_found() {
        let result = AgentResult {
            launch_error: Some(std::io::ErrorKind::NotFound),
            ..AgentResult::failed("inv", FailureClass::Launch, "No such file or directory")
        };
        let error = outcome_of(&result).unwrap_err();
        assert!(
            matches!(&error, AttractorError::CliNotFound { binary } if binary == "claude"),
            "{error}"
        );
    }

    #[test]
    fn other_launch_failures_are_failed_to_spawn() {
        let result = AgentResult {
            launch_error: Some(std::io::ErrorKind::PermissionDenied),
            ..AgentResult::failed("inv", FailureClass::Launch, "Permission denied")
        };
        assert_eq!(
            outcome_of(&result).unwrap_err().to_string(),
            handler_error("Failed to spawn Claude Code: Permission denied")
        );
        let unknown =
            AgentResult::failed("inv", FailureClass::Launch, "unknown agent profile claude");
        assert_eq!(
            outcome_of(&unknown).unwrap_err().to_string(),
            handler_error("Failed to spawn Claude Code: unknown agent profile claude")
        );
    }
}
