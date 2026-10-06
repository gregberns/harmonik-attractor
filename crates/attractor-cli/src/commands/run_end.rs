//! The end of a `pas run` (ticket 07): `final.json`, the `--json` end line,
//! and removing a successful Run's worktree (its branch stays).
//!
//! The report and the cleanup decision are pure; writing the file is a thin
//! shell below them.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::run_worktree::{self, RunWorktree};

/// How the Run ended, as `final.json` says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FinalStatus {
    Success,
    Failed,
    /// A stop between stages or SIGTERM; the Run can resume.
    Stopped,
}

/// How this `pas run` ended: its status and, for a failure, the Run's error
/// text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunEnding {
    Success,
    Failed(String),
    Stopped,
}

impl RunEnding {
    pub(crate) fn status(&self) -> FinalStatus {
        match self {
            Self::Success => FinalStatus::Success,
            Self::Failed(_) => FinalStatus::Failed,
            Self::Stopped => FinalStatus::Stopped,
        }
    }

    fn error(&self) -> Option<String> {
        match self {
            Self::Failed(error) => Some(error.clone()),
            Self::Success | Self::Stopped => None,
        }
    }
}

/// What became of the Run's worktree and branch at the end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeEnd {
    pub worktree: RunWorktree,
    /// `git rev-parse <branch>`; `None` if git could not say.
    pub final_commit: Option<String>,
    /// The worktree was removed (a clean, successful Run).
    pub removed: bool,
}

/// `runs/<run-id>/final.json`, also the last `--json` stdout line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FinalReport {
    pub v: u32,
    pub run_id: String,
    pub status: FinalStatus,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub base_sha: Option<String>,
    pub final_commit: Option<String>,
    /// The kept worktree; `None` when it was removed or there is none.
    pub worktree: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

impl FinalReport {
    pub(crate) const VERSION: u32 = 1;
}

/// The report for a Run that ended as `ending`, with `git` describing its
/// worktree (`None` outside git or in a dry run).
pub(crate) fn final_report(
    run_id: &str,
    ending: &RunEnding,
    git: Option<&WorktreeEnd>,
    warnings: Vec<String>,
) -> FinalReport {
    FinalReport {
        v: FinalReport::VERSION,
        run_id: run_id.to_string(),
        status: ending.status(),
        branch: git.map(|g| g.worktree.branch.clone()),
        base: git.map(|g| g.worktree.base.clone()),
        base_sha: git.map(|g| g.worktree.base_sha.clone()),
        final_commit: git.and_then(|g| g.final_commit.clone()),
        worktree: git.filter(|g| !g.removed).map(|g| g.worktree.path.clone()),
        error: ending.error(),
        warnings,
    }
}

/// What to do with the Run's worktree at the end (D6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cleanup {
    Remove,
    Keep,
    /// A successful Run left uncommitted changes: keep them, and say so.
    KeepWithWarning(String),
}

/// Only a successful Run with nothing uncommitted outside `.pas/`, and no
/// other process working in the worktree (`shared`, from
/// `--allow-shared-workdir`), is removed; a failure or stop keeps the
/// worktree for a resume or a look.
pub(crate) fn decide_cleanup(
    status: FinalStatus,
    dirty: bool,
    shared: bool,
    worktree: &Path,
) -> Cleanup {
    match (status, shared, dirty) {
        (FinalStatus::Failed | FinalStatus::Stopped, _, _) => Cleanup::Keep,
        (FinalStatus::Success, true, _) => Cleanup::KeepWithWarning(format!(
            "another process is working in {}; the worktree was kept",
            worktree.display()
        )),
        (FinalStatus::Success, false, true) => Cleanup::KeepWithWarning(format!(
            "{} has uncommitted changes; the worktree was kept",
            worktree.display()
        )),
        (FinalStatus::Success, false, false) => Cleanup::Remove,
    }
}

// --- the shell ---

/// At the end of a Run with a worktree: remove the worktree if the Run
/// succeeded and left nothing uncommitted (D6), and read the branch tip.
/// `repo` is a folder in the source repository. Git failures become
/// warnings: the Run's result stands.
pub(crate) fn finish_worktree(
    repo: &Path,
    worktree: &RunWorktree,
    status: FinalStatus,
    shared: bool,
) -> (WorktreeEnd, Vec<String>) {
    let mut warnings = Vec::new();
    let removed = match status {
        FinalStatus::Failed | FinalStatus::Stopped => false,
        FinalStatus::Success => match run_worktree::is_dirty(&worktree.path) {
            Err(error) => {
                warnings.push(format!(
                    "cannot check {} for uncommitted changes, so it was kept: {error}",
                    worktree.path.display()
                ));
                false
            }
            Ok(dirty) => match decide_cleanup(status, dirty, shared, &worktree.path) {
                Cleanup::Remove => match run_worktree::remove_worktree(repo, &worktree.path) {
                    Ok(()) => true,
                    Err(error) => {
                        warnings.push(format!(
                            "cannot remove the worktree {}: {error}",
                            worktree.path.display()
                        ));
                        false
                    }
                },
                Cleanup::Keep => false,
                Cleanup::KeepWithWarning(warning) => {
                    warnings.push(warning);
                    false
                }
            },
        },
    };
    let final_commit = match run_worktree::branch_tip(repo, &worktree.branch) {
        Ok(sha) => Some(sha),
        Err(error) => {
            warnings.push(format!(
                "cannot read the tip of {}: {error}",
                worktree.branch
            ));
            None
        }
    };
    (
        WorktreeEnd {
            worktree: worktree.clone(),
            final_commit,
            removed,
        },
        warnings,
    )
}

/// Write `report` to `path` atomically: a temp file beside it, then rename.
pub(crate) fn write_final_report(path: &Path, report: &FinalReport) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(report).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree() -> RunWorktree {
        RunWorktree {
            path: PathBuf::from("/repo/.pas/worktrees/r1"),
            branch: "pas/run/r1".to_string(),
            base: "HEAD".to_string(),
            base_sha: "abc".to_string(),
        }
    }

    fn git(removed: bool) -> WorktreeEnd {
        WorktreeEnd {
            worktree: worktree(),
            final_commit: Some("def".to_string()),
            removed,
        }
    }

    #[test]
    fn success_with_a_removed_worktree_has_no_worktree_and_no_error() {
        let report = final_report("r1", &RunEnding::Success, Some(&git(true)), vec![]);
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            serde_json::json!({
                "v": 1,
                "run_id": "r1",
                "status": "success",
                "branch": "pas/run/r1",
                "base": "HEAD",
                "base_sha": "abc",
                "final_commit": "def",
                "worktree": null,
                "warnings": [],
            })
        );
    }

    #[test]
    fn failure_keeps_the_worktree_and_carries_the_error() {
        let ending = RunEnding::Failed("boom".to_string());
        let report = final_report("r1", &ending, Some(&git(false)), vec!["w".to_string()]);
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["status"], "failed");
        assert_eq!(value["worktree"], "/repo/.pas/worktrees/r1");
        assert_eq!(value["error"], "boom");
        assert_eq!(value["warnings"], serde_json::json!(["w"]));
    }

    #[test]
    fn stopped_has_no_error() {
        let report = final_report("r1", &RunEnding::Stopped, Some(&git(false)), vec![]);
        assert_eq!(report.status, FinalStatus::Stopped);
        assert_eq!(report.error, None);
        assert_eq!(report.worktree, Some(worktree().path));
    }

    #[test]
    fn no_worktree_has_null_git_fields() {
        let value =
            serde_json::to_value(final_report("r1", &RunEnding::Success, None, vec![])).unwrap();
        for key in ["branch", "base", "base_sha", "final_commit", "worktree"] {
            assert!(value[key].is_null(), "{key}: {value}");
        }
        assert!(value.get("error").is_none());
    }

    #[test]
    fn only_a_clean_success_removes_the_worktree() {
        let path = Path::new("/wt");
        assert_eq!(
            decide_cleanup(FinalStatus::Success, false, false, path),
            Cleanup::Remove
        );
        assert!(matches!(
            decide_cleanup(FinalStatus::Success, true, false, path),
            Cleanup::KeepWithWarning(w) if w.contains("/wt") && w.contains("uncommitted")
        ));
        for dirty in [false, true] {
            assert!(matches!(
                decide_cleanup(FinalStatus::Success, dirty, true, path),
                Cleanup::KeepWithWarning(w) if w.contains("/wt") && w.contains("another process")
            ));
        }
        for status in [FinalStatus::Failed, FinalStatus::Stopped] {
            for (dirty, shared) in [(false, false), (true, false), (false, true), (true, true)] {
                assert_eq!(decide_cleanup(status, dirty, shared, path), Cleanup::Keep);
            }
        }
    }

    #[test]
    fn the_report_is_written_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("final.json");
        let report = final_report("r1", &RunEnding::Stopped, None, vec![]);
        write_final_report(&path, &report).unwrap();
        let read: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read, serde_json::to_value(&report).unwrap());
        assert!(!tmp.path().join("final.json.tmp").exists());
    }
}
