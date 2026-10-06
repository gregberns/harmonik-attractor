//! Run Commit detection (spec File Change 5) and attempt commits (ticket 07).
//!
//! The engine reads `HEAD` before and after each stage attempt. When it moved,
//! the commits in `before..after` are the stage's Run Commits. After each
//! attempt, in the Run's own worktree only, the engine records the attempt as
//! one commit of its own ([`commit_attempt`]).
//!
//! Known limits (observation only, nothing fails): a stage that pulls or
//! switches branches also lists commits it did not author, and with
//! `--allow-shared-workdir` another Run's commits can be listed.

use std::path::Path;
use std::process::Stdio;

use attractor_journal::CommitRef;
use attractor_types::{AttractorError, FailureKind, Outcome, StageStatus};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// An attempt's `Pas-Status` trailer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptStatus {
    Success,
    Fail,
    /// The node asked to be retried (a Retry outcome).
    Retry,
    /// `pas` never finished the attempt; recorded on resume.
    Interrupted,
}

impl AttemptStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Fail => "fail",
            Self::Retry => "retry",
            Self::Interrupted => "interrupted",
        }
    }
}

/// How an attempt ended, as its commit records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AttemptRecord {
    pub status: AttemptStatus,
    pub class: Option<FailureKind>,
}

/// The record of a finished attempt; `None` for a cancelled one, whose
/// changes are committed as `interrupted` on resume instead.
pub(crate) fn attempt_record(result: &Result<Outcome, AttractorError>) -> Option<AttemptRecord> {
    let record = |status, class| Some(AttemptRecord { status, class });
    match result {
        Ok(outcome) => match outcome.status {
            StageStatus::Success | StageStatus::PartialSuccess | StageStatus::Skipped => {
                record(AttemptStatus::Success, None)
            }
            StageStatus::Fail => record(AttemptStatus::Fail, Some(FailureKind::Reported)),
            StageStatus::Retry => record(AttemptStatus::Retry, None),
        },
        Err(AttractorError::Cancelled { .. }) => None,
        Err(error) => record(AttemptStatus::Fail, error.failure_kind()),
    }
}

/// One attempt commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttemptCommit<'a> {
    pub run_id: &'a str,
    pub node: &'a str,
    pub attempt: u32,
    pub record: AttemptRecord,
}

/// `pas(<run-id>): <node> attempt <n> (<status>)`, a blank line, then the
/// `Pas-*` trailers (design §3).
pub(crate) fn attempt_message(commit: &AttemptCommit<'_>) -> String {
    let status = commit.record.status.as_str();
    let mut message = format!(
        "pas({run}): {node} attempt {n} ({status})\n\n\
         Pas-Run: {run}\nPas-Node: {node}\nPas-Attempt: {n}\nPas-Status: {status}\n",
        run = commit.run_id,
        node = commit.node,
        n = commit.attempt,
    );
    if let Some(class) = commit.record.class {
        message.push_str(&format!("Pas-Failure-Class: {}\n", class.as_str()));
    }
    message
}

/// The pathspec of everything in the worktree except `.pas/`.
const ALL_BUT_PAS: [&str; 3] = ["--", ".", ":(exclude).pas"];

/// Commit everything in the worktree at `root` except `.pas/`, hooks skipped,
/// even when nothing changed; returns the new commit's sha.
pub(crate) async fn commit_attempt(
    root: &Path,
    commit: &AttemptCommit<'_>,
) -> Result<String, String> {
    run(root, &[&["add", "-A"][..], &ALL_BUT_PAS[..]].concat(), None).await?;
    let mut args: Vec<&str> = Vec::new();
    // A fixed identity when the repository has none (design §3).
    if !has_identity(root).await {
        args.extend(["-c", "user.name=PAS", "-c", "user.email=pas@localhost"]);
    }
    // None of the repository's hooks run (Q40; --no-verify alone leaves
    // prepare-commit-msg and post-commit), and attempt commits are never
    // signed: signing could prompt or fail in an unattended Run.
    args.extend([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "commit.gpgsign=false",
    ]);
    args.extend([
        "commit",
        "--allow-empty",
        "--no-verify",
        "--quiet",
        "-F",
        "-",
    ]);
    run(root, &args, Some(&attempt_message(commit))).await?;
    head(root)
        .await
        .ok_or_else(|| "no HEAD after the commit".to_string())
}

/// Whether the worktree has changes outside `.pas/`.
pub(crate) async fn is_dirty(root: &Path) -> Result<bool, String> {
    let status = run(
        root,
        &[&["status", "--porcelain"][..], &ALL_BUT_PAS[..]].concat(),
        None,
    )
    .await?;
    Ok(!status.trim().is_empty())
}

/// Whether `user.name` and `user.email` are both set for `root`.
async fn has_identity(root: &Path) -> bool {
    let mut both = true;
    for key in ["user.name", "user.email"] {
        let set = run(root, &["config", key], None)
            .await
            .is_ok_and(|value| !value.trim().is_empty());
        both &= set;
    }
    both
}

/// `git -C <root> <args>` with `stdin` written to it; stdout on success, a
/// message naming the command and its stderr otherwise.
async fn run(root: &Path, args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    let describe = || format!("git {}", args.join(" "));
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("{}: {e}", describe()))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(text.as_bytes())
            .await
            .map_err(|e| format!("{}: {e}", describe()))?;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("{}: {e}", describe()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "{} exited with {}: {}",
            describe(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// `git rev-parse HEAD` in `workdir`, or `None` outside a git repository or
/// with an unborn HEAD (both exit non-zero).
pub(crate) async fn head(workdir: &Path) -> Option<String> {
    let output = git(workdir)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Commits in `before..after`, newest first (git's order).
pub(crate) async fn commits_between(
    workdir: &Path,
    before: &str,
    after: &str,
) -> std::io::Result<Vec<CommitRef>> {
    let output = git(workdir)
        .args([
            "log",
            "--no-show-signature",
            "--no-color",
            "--format=%H%x00%s%x00%an%x00%cI",
            &format!("{before}..{after}"),
            "--",
        ])
        .output()
        .await?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "git log {before}..{after} exited with {}",
            output.status
        )));
    }
    Ok(parse_log(&String::from_utf8_lossy(&output.stdout)))
}

/// The current branch's upstream as a short name (e.g. `origin/main`), or
/// `None` without one, on a detached HEAD, or outside a repository.
pub(crate) async fn upstream(workdir: &Path) -> Option<String> {
    let output = git(workdir)
        .args([
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Whether `sha` is reachable from `rev` (`git merge-base --is-ancestor`).
/// An unknown commit or any other git failure is an error.
pub(crate) async fn is_ancestor(workdir: &Path, sha: &str, rev: &str) -> std::io::Result<bool> {
    let status = git(workdir)
        .args(["merge-base", "--is-ancestor", sha, rev])
        .status()
        .await?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(std::io::Error::other(format!(
            "git merge-base --is-ancestor {sha} {rev} exited with {status}"
        ))),
    }
}

fn git(workdir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(workdir)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

/// Parse `%H%x00%s%x00%an%x00%cI` lines. Lines without 4 fields are skipped.
fn parse_log(stdout: &str) -> Vec<CommitRef> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\0');
            let (Some(sha), Some(subject), Some(author), Some(ts), None) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                return None;
            };
            (!sha.is_empty()).then(|| CommitRef {
                sha: sha.to_string(),
                subject: subject.to_string(),
                author: author.to_string(),
                ts: ts.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(status: AttemptStatus, class: Option<FailureKind>) -> Option<AttemptRecord> {
        Some(AttemptRecord { status, class })
    }

    #[test]
    fn attempt_record_per_result() {
        let ok = |status| attempt_record(&Ok(Outcome::with_label(status, "x")));
        assert_eq!(
            ok(StageStatus::Success),
            record(AttemptStatus::Success, None)
        );
        assert_eq!(
            ok(StageStatus::PartialSuccess),
            record(AttemptStatus::Success, None)
        );
        assert_eq!(
            ok(StageStatus::Fail),
            record(AttemptStatus::Fail, Some(FailureKind::Reported))
        );
        assert_eq!(ok(StageStatus::Retry), record(AttemptStatus::Retry, None));
        let err = |error: AttractorError| attempt_record(&Err(error));
        assert_eq!(
            err(AttractorError::AgentTimeout {
                node: "n".into(),
                attempt: 1,
                timeout_ms: 1,
                files: None
            }),
            record(AttemptStatus::Fail, Some(FailureKind::Timeout))
        );
        assert_eq!(
            err(AttractorError::CommandTimeout { timeout_ms: 1 }),
            record(AttemptStatus::Fail, Some(FailureKind::Timeout))
        );
        assert_eq!(
            err(AttractorError::CliNotFound { binary: "c".into() }),
            record(AttemptStatus::Fail, Some(FailureKind::Launch))
        );
        for kind in [
            FailureKind::Crash,
            FailureKind::NoResult,
            FailureKind::Launch,
        ] {
            assert_eq!(
                err(AttractorError::AgentFailed {
                    node: "n".into(),
                    kind,
                    message: "m".into()
                }),
                record(AttemptStatus::Fail, Some(kind))
            );
        }
        assert_eq!(
            err(AttractorError::HandlerError {
                handler: "tool".into(),
                node: "n".into(),
                message: "m".into()
            }),
            record(AttemptStatus::Fail, None)
        );
        assert_eq!(err(AttractorError::Cancelled { node: "n".into() }), None);
    }

    #[test]
    fn attempt_message_has_the_subject_and_trailers() {
        let commit = AttemptCommit {
            run_id: "r1",
            node: "work",
            attempt: 2,
            record: AttemptRecord {
                status: AttemptStatus::Fail,
                class: Some(FailureKind::Timeout),
            },
        };
        assert_eq!(
            attempt_message(&commit),
            "pas(r1): work attempt 2 (fail)\n\n\
             Pas-Run: r1\n\
             Pas-Node: work\n\
             Pas-Attempt: 2\n\
             Pas-Status: fail\n\
             Pas-Failure-Class: timeout\n"
        );
        let success = AttemptCommit {
            record: AttemptRecord {
                status: AttemptStatus::Success,
                class: None,
            },
            ..commit
        };
        assert!(!attempt_message(&success).contains("Pas-Failure-Class"));
        assert!(attempt_message(&success).starts_with("pas(r1): work attempt 2 (success)\n"));
    }

    #[test]
    fn parse_log_keeps_git_order() {
        let commits = parse_log(
            "bbb\0two\0Ann\x002026-09-25T10:00:01+00:00\n\
             aaa\0one\0Ann\x002026-09-25T10:00:00+00:00\n",
        );
        let shas: Vec<_> = commits.iter().map(|commit| commit.sha.as_str()).collect();
        assert_eq!(shas, ["bbb", "aaa"]);
        assert_eq!(commits[0].subject, "two");
        assert_eq!(commits[0].author, "Ann");
        assert_eq!(commits[0].ts, "2026-09-25T10:00:01+00:00");
    }

    #[test]
    fn parse_log_keeps_subject_text_intact() {
        let commits = parse_log("abc\0fix: a\tb | ünï “q”\0Bo Li\x002026-01-01T00:00:00Z\n");
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].subject, "fix: a\tb | ünï “q”");
        assert_eq!(commits[0].author, "Bo Li");
    }

    #[test]
    fn parse_log_skips_malformed_lines() {
        assert!(parse_log("").is_empty());
        assert!(parse_log("\n").is_empty());
        assert!(parse_log("gpg: Signature made ...\n").is_empty());
        assert!(parse_log("a\0b\0c\n").is_empty());
        assert!(parse_log("a\0b\0c\0d\0e\n").is_empty());
        assert!(parse_log("\0b\0c\0d\n").is_empty());
        assert_eq!(parse_log("junk\nabc\0s\0a\0t\n").len(), 1);
    }

    #[tokio::test]
    async fn head_is_none_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let inside = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["rev-parse", "--git-dir"])
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(
            !inside,
            "temp dir {} is inside a repository",
            dir.path().display()
        );
        assert_eq!(head(dir.path()).await, None);
    }

    #[tokio::test]
    async fn head_is_none_for_a_missing_workdir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(head(&dir.path().join("missing")).await, None);
    }

    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    #[tokio::test]
    async fn upstream_and_is_ancestor_follow_git() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, work) = (dir.path().join("remote.git"), dir.path().join("work"));
        git_ok(
            dir.path(),
            &["init", "-q", "--bare", remote.to_str().unwrap()],
        );
        git_ok(
            dir.path(),
            &["init", "-q", "-b", "main", work.to_str().unwrap()],
        );
        git_ok(&work, &["commit", "--allow-empty", "-qm", "one"]);
        let pushed = git_ok(&work, &["rev-parse", "HEAD"]);

        // No remote yet: no upstream.
        assert_eq!(upstream(&work).await, None);

        git_ok(
            &work,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git_ok(&work, &["push", "-q", "-u", "origin", "main"]);
        git_ok(&work, &["commit", "--allow-empty", "-qm", "two"]);
        let local = git_ok(&work, &["rev-parse", "HEAD"]);

        assert_eq!(upstream(&work).await.as_deref(), Some("origin/main"));
        assert!(is_ancestor(&work, &pushed, "origin/main").await.unwrap());
        assert!(!is_ancestor(&work, &local, "origin/main").await.unwrap());
        assert!(is_ancestor(&work, &"0".repeat(40), "origin/main")
            .await
            .is_err());

        // A detached HEAD has no upstream.
        git_ok(&work, &["checkout", "-q", "--detach"]);
        assert_eq!(upstream(&work).await, None);
    }

    #[tokio::test]
    async fn upstream_is_none_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(upstream(&dir.path().join("missing")).await, None);
    }

    #[tokio::test]
    async fn commits_between_unknown_revisions_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(commits_between(dir.path(), "deadbeef", "cafebabe")
            .await
            .is_err());
    }
}
