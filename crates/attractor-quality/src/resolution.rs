use crate::manifest::{Manifest, ResolvedManifest};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// The project folder: the config, and the run data next to it.
pub const PROJECT_DIR: &str = ".harmonik-attractor";
/// The project config's file name inside [`PROJECT_DIR`].
pub const CONFIG_FILE: &str = "config.toml";

/// `<dir>/.harmonik-attractor/config.toml`.
pub fn config_path(dir: &Path) -> PathBuf {
    dir.join(PROJECT_DIR).join(CONFIG_FILE)
}

#[derive(Debug, Error)]
pub enum ResolutionError {
    #[error(".harmonik-attractor/config.toml not found (searched up to 16 directory levels)")]
    NotFound,
    #[error(".harmonik-attractor/config.toml at {path} is malformed: {source}")]
    Malformed {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error(".harmonik-attractor/config.toml at {path} is invalid: {reason}")]
    Invalid { path: PathBuf, reason: String },
}

pub fn resolve(start_dir: &Path) -> Result<ResolvedManifest, ResolutionError> {
    let mut current = start_dir.to_path_buf();
    let mut ascents = 0;

    loop {
        let candidate = config_path(&current);
        if candidate.exists() {
            let content =
                std::fs::read_to_string(&candidate).map_err(|e| ResolutionError::Invalid {
                    path: candidate.clone(),
                    reason: e.to_string(),
                })?;
            let manifest: Manifest =
                toml::from_str(&content).map_err(|e| ResolutionError::Malformed {
                    path: candidate.clone(),
                    source: e,
                })?;
            let blake3_hash = blake3::hash(content.as_bytes()).to_hex().to_string();
            return Ok(ResolvedManifest {
                manifest,
                path: candidate,
                blake3_hash,
            });
        }

        // Stop if .git directory found (workspace root)
        if current.join(".git").exists() {
            break;
        }

        ascents += 1;
        if ascents >= 16 {
            break;
        }

        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => break,
        }
    }

    Err(ResolutionError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_valid_manifest(dir: &Path) {
        fs::create_dir_all(dir.join(".harmonik-attractor")).unwrap();
        fs::write(
            dir.join(".harmonik-attractor/config.toml"),
            r#"
[project]
name = "test-project"

[quality]
stages = ["lint", "test"]
"#,
        )
        .unwrap();
    }

    #[test]
    fn resolves_manifest_in_same_dir() {
        let tmp = TempDir::new().unwrap();
        write_valid_manifest(tmp.path());
        let result = resolve(tmp.path());
        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.manifest.project.name, "test-project");
        assert!(!r.blake3_hash.is_empty());
    }

    #[test]
    fn resolves_manifest_in_parent_dir() {
        let tmp = TempDir::new().unwrap();
        write_valid_manifest(tmp.path());
        let child = tmp.path().join("subdir");
        fs::create_dir(&child).unwrap();
        let result = resolve(&child);
        assert!(result.is_ok());
    }

    #[test]
    fn returns_not_found_for_empty_dir() {
        let tmp = TempDir::new().unwrap();
        // Create a .git dir to stop walk-up
        fs::create_dir(tmp.path().join(".git")).unwrap();
        let result = resolve(tmp.path());
        assert!(matches!(result, Err(ResolutionError::NotFound)));
    }

    #[test]
    fn returns_malformed_for_invalid_toml() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".harmonik-attractor")).unwrap();
        fs::write(
            tmp.path().join(".harmonik-attractor/config.toml"),
            "this is not valid toml!!!",
        )
        .unwrap();
        let result = resolve(tmp.path());
        assert!(matches!(result, Err(ResolutionError::Malformed { .. })));
    }
}
