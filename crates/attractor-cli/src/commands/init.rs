//! `pas init` — initialise a `pas.toml` manifest in the current project.
//!
//! TTY-aware behaviour
//! -------------------
//! * Interactive TTY  — shows discovered toolchains, opens `$EDITOR` for
//!   preview/edit, then asks for final write confirmation.
//! * Non-interactive  — (no TTY, `PAS_NON_INTERACTIVE=1`, `PAS_AGENT=1`, or
//!   `--non-interactive`) writes straight through without blocking stdin.
//!
//! Existing `pas.toml` handling
//! ----------------------------
//! | Mode            | `--force` | Behaviour                              |
//! |-----------------|-----------|----------------------------------------|
//! | interactive     | no        | prompt "Overwrite?" → abort on No      |
//! | interactive     | yes       | proceed silently                       |
//! | non-interactive | no        | error + exit 1                         |
//! | non-interactive | yes       | proceed silently                       |
//!
//! No `.git` handling
//! ------------------
//! | Mode            | `--force` | Behaviour                              |
//! |-----------------|-----------|----------------------------------------|
//! | interactive     | no        | prompt "Initialize anyway?" → abort    |
//! | interactive     | yes       | proceed silently                       |
//! | non-interactive | no        | exit(4)                                |
//! | non-interactive | yes       | proceed silently                       |

use std::io::IsTerminal as _;
use std::path::Path;

use anyhow::Context as _;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Options accepted by `pas init`.
#[derive(Debug, Clone)]
pub struct InitOpts {
    /// Overwrite an existing `pas.toml` without prompting.
    pub force: bool,
    /// Disable interactive prompts regardless of TTY state.
    pub non_interactive: bool,
    /// Skip git-enrichment (detect toolchains only from filesystem).
    #[allow(dead_code)]
    pub no_enrich: bool,
    /// Print what would be written without actually writing it.
    pub dry_run: bool,
}

impl InitOpts {
    /// Returns `true` when no interactive prompts should be shown.
    ///
    /// True when:
    /// - `--non-interactive` flag was passed, OR
    /// - stdin is not a terminal, OR
    /// - `PAS_NON_INTERACTIVE=1`, OR
    /// - `PAS_AGENT=1`
    pub fn is_non_interactive(&self) -> bool {
        self.non_interactive
            || !std::io::stdin().is_terminal()
            || std::env::var("PAS_NON_INTERACTIVE").as_deref() == Ok("1")
            || std::env::var("PAS_AGENT").as_deref() == Ok("1")
    }
}

// ---------------------------------------------------------------------------
// Toolchain detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Toolchain {
    name: &'static str,
    marker: &'static str,
}

const TOOLCHAINS: &[Toolchain] = &[
    Toolchain {
        name: "rust",
        marker: "Cargo.toml",
    },
    Toolchain {
        name: "node",
        marker: "package.json",
    },
    Toolchain {
        name: "python",
        marker: "pyproject.toml",
    },
    Toolchain {
        name: "python",
        marker: "setup.py",
    },
    Toolchain {
        name: "go",
        marker: "go.mod",
    },
    Toolchain {
        name: "java",
        marker: "pom.xml",
    },
    Toolchain {
        name: "java",
        marker: "build.gradle",
    },
];

/// Detect which toolchains are present in `root`.  Deduplicates by name.
fn detect_toolchains(root: &Path) -> Vec<(&'static str, &'static str)> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for tc in TOOLCHAINS {
        if root.join(tc.marker).exists() && seen.insert(tc.name) {
            result.push((tc.name, tc.marker));
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Manifest template
// ---------------------------------------------------------------------------

fn build_manifest(root: &Path, toolchains: &[(&str, &str)]) -> String {
    let project_name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("my-project");
    let project_name_toml = toml::Value::String(project_name.to_string()).to_string();

    // Emit the primary detected toolchain as [toolchain], matching the Manifest schema.
    // Additional toolchains are listed as comments for human reference.
    let toolchain_section = if let Some((primary_lang, _)) = toolchains.first() {
        let extras: Vec<_> = toolchains[1..]
            .iter()
            .map(|(name, marker)| format!("# also detected: {name} ({marker})"))
            .collect();
        let comment_block = if extras.is_empty() {
            String::new()
        } else {
            format!("\n{}", extras.join("\n"))
        };
        format!("[toolchain]\nlanguage = \"{primary_lang}\"{comment_block}\n")
    } else {
        "# no toolchains detected — add [toolchain] manually if needed\n".to_string()
    };

    format!(
        r#"# pas.toml — Pascal's Discrete Attractor manifest
# Generated by `hka init`

[project]
name    = {project_name_toml}
version = "0.1.0"

{toolchain_section}
[quality]
stages = []
# Add your quality check commands here, e.g.:
# stages = ["cargo test", "cargo clippy"]
"#
    )
}

// ---------------------------------------------------------------------------
// Main command
// ---------------------------------------------------------------------------

/// Run `pas init` in `root`.
pub fn cmd_init(root: &Path, opts: &InitOpts) -> anyhow::Result<()> {
    let manifest_path = root.join("pas.toml");
    let git_dir = root.join(".git");
    let non_interactive = opts.is_non_interactive();

    // ── .git guard ──────────────────────────────────────────────────────────
    if !git_dir.exists() && !opts.force {
        if non_interactive {
            eprintln!("hka init: no .git directory found. Use --force to initialise anyway.");
            std::process::exit(4);
        } else {
            // interactive
            let proceed = dialoguer::Confirm::new()
                .with_prompt("No .git found. Initialize anyway?")
                .default(false)
                .interact()
                .unwrap_or(false);
            if !proceed {
                println!("Aborted.");
                return Ok(());
            }
        }
    }

    // ── existing pas.toml guard ─────────────────────────────────────────────
    if manifest_path.exists() && !opts.force {
        if non_interactive {
            anyhow::bail!("pas.toml already exists. Use --force to overwrite.");
        } else {
            // interactive
            let overwrite = dialoguer::Confirm::new()
                .with_prompt("pas.toml already exists. Overwrite?")
                .default(false)
                .interact()
                .unwrap_or(false);
            if !overwrite {
                println!("Aborted.");
                return Ok(());
            }
        }
    }

    // ── detect toolchains ───────────────────────────────────────────────────
    let toolchains = detect_toolchains(root);

    // ── build initial manifest content ──────────────────────────────────────
    let mut manifest_content = build_manifest(root, &toolchains);

    // ── interactive preview / edit ──────────────────────────────────────────
    if !non_interactive && !opts.dry_run {
        // Print detected toolchains summary
        if toolchains.is_empty() {
            println!("Detected: (none)");
        } else {
            let summary = toolchains
                .iter()
                .map(|(name, marker)| format!("{name} ({marker})"))
                .collect::<Vec<_>>()
                .join(", ");
            println!("Detected: {summary}");
        }

        // Open editor for preview/edit
        match dialoguer::Editor::new().edit(&manifest_content) {
            Ok(Some(edited)) => {
                manifest_content = edited;
            }
            Ok(None) => {
                // User closed without saving — use original content
            }
            Err(_) => {
                // No $EDITOR or editor error — use original content
            }
        }

        // Final confirmation
        let write = dialoguer::Confirm::new()
            .with_prompt("Write pas.toml?")
            .default(true)
            .interact()
            .unwrap_or(false);
        if !write {
            println!("Aborted.");
            return Ok(());
        }
    }

    // ── dry run ─────────────────────────────────────────────────────────────
    if opts.dry_run {
        println!("--- pas.toml (dry run) ---");
        print!("{manifest_content}");
        println!("--- end ---");
        return Ok(());
    }

    // ── write manifest ──────────────────────────────────────────────────────
    std::fs::write(&manifest_path, &manifest_content)
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    println!("Created {}", manifest_path.display());

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_opts(force: bool, dry_run: bool) -> InitOpts {
        InitOpts {
            force,
            non_interactive: true,
            no_enrich: true,
            dry_run,
        }
    }

    #[test]
    fn creates_pas_toml_in_git_repo() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"test\"").unwrap();

        let opts = make_opts(false, false);
        cmd_init(tmp.path(), &opts).unwrap();

        let content = fs::read_to_string(tmp.path().join("pas.toml")).unwrap();
        assert!(
            content.contains("[project]"),
            "manifest should have [project] section"
        );
        assert!(content.contains("rust"), "should detect rust toolchain");
    }

    #[test]
    fn non_interactive_without_force_refuses_overwrite() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"test\"").unwrap();

        // First init — force=true to create the file
        let opts = make_opts(true, false);
        cmd_init(tmp.path(), &opts).unwrap();
        assert!(tmp.path().join("pas.toml").exists());

        // Second init without force — should error
        let opts2 = InitOpts {
            force: false,
            non_interactive: true,
            no_enrich: true,
            dry_run: false,
        };
        let result = cmd_init(tmp.path(), &opts2);
        assert!(
            result.is_err(),
            "Should fail when pas.toml exists and non-interactive without --force"
        );
    }

    #[test]
    fn dry_run_does_not_write_file() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();

        let opts = InitOpts {
            force: false,
            non_interactive: true,
            no_enrich: true,
            dry_run: true,
        };
        cmd_init(tmp.path(), &opts).unwrap();
        assert!(
            !tmp.path().join("pas.toml").exists(),
            "dry-run should not write pas.toml"
        );
    }

    #[test]
    fn force_overwrites_existing() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();

        // Write a sentinel file
        fs::write(tmp.path().join("pas.toml"), "old content").unwrap();

        let opts = make_opts(true, false);
        cmd_init(tmp.path(), &opts).unwrap();

        let content = fs::read_to_string(tmp.path().join("pas.toml")).unwrap();
        assert!(
            content.contains("[project]"),
            "overwritten file should be a valid manifest"
        );
    }

    #[test]
    fn detects_node_toolchain() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join("package.json"), "{}").unwrap();

        let opts = make_opts(false, false);
        cmd_init(tmp.path(), &opts).unwrap();

        let content = fs::read_to_string(tmp.path().join("pas.toml")).unwrap();
        assert!(content.contains("node"), "should detect node toolchain");
    }

    #[test]
    fn no_toolchains_produces_valid_manifest() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();

        let opts = make_opts(false, false);
        cmd_init(tmp.path(), &opts).unwrap();

        let content = fs::read_to_string(tmp.path().join("pas.toml")).unwrap();
        assert!(content.contains("[project]"));
        assert!(content.contains("no toolchains detected"));
    }

    #[test]
    fn project_name_is_toml_escaped() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("weird\"name");
        fs::create_dir(&root).unwrap();

        let manifest = build_manifest(&root, &[]);
        let parsed: toml::Value = toml::from_str(&manifest).unwrap();
        assert_eq!(
            parsed
                .get("project")
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str()),
            Some("weird\"name")
        );
    }
}
