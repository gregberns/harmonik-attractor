use anyhow;
use attractor_pipeline::{Diagnostic, Severity};
use serde::Serialize;

/// A `pas validate` failure before validation could run. In `--json` mode it
/// is printed as `{"v":1,"ok":false,"error":{..}}` (C6).
struct ValidateError {
    code: &'static str,
    message: String,
}

/// `pas validate --json` payload (C6), fields in contract order.
#[derive(Serialize)]
struct ValidateJson {
    v: u32,
    ok: bool,
    valid: bool,
    diagnostics: Vec<DiagnosticJson>,
}

#[derive(Serialize)]
struct DiagnosticJson {
    severity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_id: Option<String>,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<String>,
}

/// What `pas validate` found: the diagnostics, and how each agent node uses
/// sessions (shown when the pipeline is valid).
struct Checked {
    diagnostics: Vec<Diagnostic>,
    sessions: Vec<attractor_pipeline::AgentSession>,
}

fn check(path: &std::path::Path, allow_test_agents: bool) -> Result<Checked, ValidateError> {
    let source = std::fs::read_to_string(path).map_err(|e| ValidateError {
        code: "io",
        message: e.to_string(),
    })?;
    let graph = attractor_dot::parse(&source)
        .map_err(anyhow::Error::from)
        .and_then(|dot| Ok(attractor_pipeline::PipelineGraph::from_dot(dot)?))
        .map_err(|e| ValidateError {
            code: "invalid_dot",
            message: e.to_string(),
        })?;
    let mut diagnostics = attractor_pipeline::validate(&graph);
    let mut sessions = Vec::new();
    if diagnostics.iter().all(|d| d.severity != Severity::Error) {
        let (agent_diagnostics, agent_sessions) = check_agents(graph, allow_test_agents)?;
        diagnostics.extend(agent_diagnostics);
        sessions = agent_sessions;
    }
    Ok(Checked {
        diagnostics,
        sessions,
    })
}

/// Every agent profile the pipeline uses, checked against the profiles of
/// the current directory's project (built-in, `pas.toml`).
fn check_agents(
    graph: attractor_pipeline::PipelineGraph,
    allow_test_agents: bool,
) -> Result<(Vec<Diagnostic>, Vec<attractor_pipeline::AgentSession>), ValidateError> {
    let invalid_config = |message: String| ValidateError {
        code: "invalid_config",
        message,
    };
    let plan = attractor_pipeline::ExecutionPlan::compile(graph)
        .map_err(|e| invalid_config(e.to_string()))?;
    let configured = attractor_pipeline::RunConfiguration::prepare(plan, Default::default())
        .map_err(|e| invalid_config(e.to_string()))?;
    let profiles = attractor_pipeline::agent_profiles(configured.controls())
        .map_err(|e| invalid_config(e.to_string()))?;
    let agents = crate::agents::agents(profiles, allow_test_agents)
        .map_err(|e| invalid_config(e.to_string()))?;
    Ok((
        attractor_pipeline::check_agents(configured.plan(), &agents),
        attractor_pipeline::agent_sessions(configured.plan(), &agents),
    ))
}

/// One line per agent node: its profile, fidelity and thread.
fn session_lines(sessions: &[attractor_pipeline::AgentSession]) -> Vec<String> {
    sessions
        .iter()
        .map(|s| {
            let fidelity = if s.explicit {
                s.fidelity.as_str().to_string()
            } else {
                format!("{} (default)", s.fidelity.as_str())
            };
            let thread = if s.thread_key == s.node_id {
                String::new()
            } else {
                format!(", thread {}", s.thread_key)
            };
            format!(
                "  {}: profile {}, fidelity {fidelity}{thread}",
                s.node_id, s.profile
            )
        })
        .collect()
}

fn print_human(result: Result<Checked, ValidateError>) -> anyhow::Result<()> {
    let Checked {
        diagnostics,
        sessions,
    } = result.map_err(|error| anyhow::anyhow!("Validation failed: {}", error.message))?;

    if diagnostics.is_empty() {
        println!("Pipeline is valid");
        if !sessions.is_empty() {
            println!("Agent sessions:");
            for line in session_lines(&sessions) {
                println!("{line}");
            }
        }
        return Ok(());
    }

    if super::print_diagnostics(&diagnostics) {
        anyhow::bail!("Validation failed");
    }
    Ok(())
}

fn print_json(result: Result<Checked, ValidateError>) -> anyhow::Result<()> {
    match result {
        Ok(Checked { diagnostics, .. }) => {
            let valid = !diagnostics.iter().any(|d| d.severity == Severity::Error);
            let payload = ValidateJson {
                v: 1,
                ok: true,
                valid,
                diagnostics: diagnostics
                    .into_iter()
                    .map(|d| DiagnosticJson {
                        severity: match d.severity {
                            Severity::Error => "error",
                            Severity::Warning => "warning",
                            Severity::Info => "info",
                        },
                        node_id: d.node_id,
                        message: d.message,
                        fix: d.fix,
                    })
                    .collect(),
            };
            println!("{}", serde_json::to_string(&payload)?);
            if !valid {
                anyhow::bail!("Validation failed");
            }
            Ok(())
        }
        Err(error) => {
            println!(
                "{}",
                serde_json::json!({
                    "v": 1,
                    "ok": false,
                    "error": {"code": error.code, "message": error.message},
                })
            );
            anyhow::bail!("Validation failed: {}", error.message)
        }
    }
}

pub fn cmd_validate(
    path: &std::path::Path,
    allow_test_agents: bool,
    json: bool,
) -> anyhow::Result<()> {
    let result = check(path, allow_test_agents);
    if json {
        print_json(result)
    } else {
        print_human(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_lines_show_profile_fidelity_and_a_shared_thread() {
        let session =
            |node: &str, fidelity, explicit, thread: &str| attractor_pipeline::AgentSession {
                node_id: node.into(),
                profile: "claude".into(),
                fidelity,
                explicit,
                can_resume: true,
                thread_key: thread.into(),
            };
        assert_eq!(
            session_lines(&[
                session("a", attractor_pipeline::Fidelity::Full, false, "a"),
                session("b", attractor_pipeline::Fidelity::Fresh, true, "t"),
            ]),
            [
                "  a: profile claude, fidelity full (default)",
                "  b: profile claude, fidelity fresh, thread t",
            ]
        );
    }

    /// A runtime node with no explicit `llm_provider` must block `pas
    /// validate` with a non-zero exit (an `Err` here, which main.rs turns
    /// into a non-zero process exit), and the printed diagnostic must name
    /// the offending node id so the pipeline author knows exactly what to
    /// fix.
    #[test]
    fn cmd_validate_fails_on_missing_llm_provider_and_names_the_node() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pipeline.dot");
        std::fs::write(
            &path,
            r#"digraph G {
                start [shape="Mdiamond"]
                work [shape="box", prompt="Do work"]
                done [shape="Msquare"]
                start -> work -> done
            }"#,
        )
        .unwrap();

        let result = cmd_validate(&path, false, false);
        let err = result.expect_err("validation must fail for a missing llm_provider");
        assert!(err.to_string().contains("Validation failed"));

        // The underlying diagnostics (what cmd_validate prints) must name
        // the specific node id, not just report a generic failure.
        let graph = crate::load_pipeline(&path).unwrap();
        let diagnostics = attractor_pipeline::validate(&graph);
        let provider_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.rule == "provider_required")
            .collect();
        assert_eq!(provider_errors.len(), 1);
        assert!(provider_errors[0].message.contains("'work'"));
    }

    /// A pipeline where every runtime node names an explicit `llm_provider`
    /// must validate cleanly (`Ok`), even for a provider other than the
    /// implicit default.
    #[test]
    fn cmd_validate_passes_when_llm_provider_is_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pipeline.dot");
        std::fs::write(
            &path,
            r#"digraph G {
                start [shape="Mdiamond"]
                work [shape="box", llm_provider="codex", prompt="Do work"]
                done [shape="Msquare"]
                start -> work -> done
            }"#,
        )
        .unwrap();

        let result = cmd_validate(&path, false, false);
        assert!(result.is_ok());
    }
}
