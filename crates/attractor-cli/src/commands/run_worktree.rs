//! The git worktree and branch each Run works in (design.md §3): branch
//! `pas/run/<run-id>` from a base commit, checked out at
//! `<worktree_root>/<run-id>`, default `<project-root>/.pas/worktrees`.
//!
//! Names and decisions are pure functions; the `git` calls below them are a
//! thin shell that returns typed errors.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Prefix of every Run branch.
const BRANCH_PREFIX: &str = "pas/run/";

/// The base of a new Run's branch when `--base` is not given.
pub(crate) const DEFAULT_BASE: &str = "HEAD";
/// The folder pas keeps its own files in, at the project root.
pub(crate) const PAS_DIR: &str = ".pas";

/// Why the Run's worktree could not be prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorktreeError {
    /// `--base` does not name a commit (or `HEAD` is unborn).
    InvalidBase { base: String },
    /// The Run's worktree path exists but is not that Run's checkout.
    Mismatch { path: PathBuf, found: String },
    /// The Run's branch is checked out in another worktree.
    BranchInUse { branch: String, at: PathBuf },
    /// The workdir is not below the project root (cannot happen for a
    /// workdir git placed in that repository).
    OutsideRepo { workdir: PathBuf, top: PathBuf },
    /// A `git` command failed or could not run.
    Git { args: String, message: String },
    /// A file pas writes next to the worktrees could not be written.
    Io { path: PathBuf, message: String },
}

impl WorktreeError {
    /// The `pas run --json` error code.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::InvalidBase { .. } => "invalid_base",
            Self::Mismatch { .. } => "worktree_mismatch",
            Self::BranchInUse { .. } => "branch_in_use",
            Self::OutsideRepo { .. } | Self::Git { .. } | Self::Io { .. } => "run_setup_failed",
        }
    }
}

impl std::fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBase { base } => write!(f, "--base {base:?} does not name a commit"),
            Self::Mismatch { path, found } => write!(
                f,
                "{} exists but is not this Run's worktree ({found})",
                path.display()
            ),
            Self::BranchInUse { branch, at } => write!(
                f,
                "branch {branch} is already checked out at {}",
                at.display()
            ),
            Self::OutsideRepo { workdir, top } => write!(
                f,
                "workdir {} is not inside the repository {}",
                workdir.display(),
                top.display()
            ),
            Self::Git { args, message } => write!(f, "git {args} failed: {message}"),
            Self::Io { path, message } => write!(f, "cannot write {}: {message}", path.display()),
        }
    }
}

/// The worktree a Run works in, as recorded in `run.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunWorktree {
    pub path: PathBuf,
    pub branch: String,
    /// The base ref as given (`--base`, default `HEAD`).
    pub base: String,
    pub base_sha: String,
}

/// `pas/run/<run-id>`.
pub(crate) fn branch_name(run_id: &str) -> String {
    format!("{BRANCH_PREFIX}{run_id}")
}

/// `<root>/<run-id>`.
pub(crate) fn worktree_path(root: &Path, run_id: &str) -> PathBuf {
    root.join(run_id)
}

/// `<project-root>/.pas/worktrees`.
pub(crate) fn default_worktree_root(top: &Path) -> PathBuf {
    top.join(PAS_DIR).join("worktrees")
}

/// The Run's workdir: the source workdir's place in the repository, inside
/// the worktree (`<top>/sub` maps to `<worktree>/sub`). All paths are
/// canonical.
pub(crate) fn run_workdir(
    worktree: &Path,
    top: &Path,
    source: &Path,
) -> Result<PathBuf, WorktreeError> {
    source
        .strip_prefix(top)
        .map(|sub| worktree.join(sub))
        .map_err(|_| WorktreeError::OutsideRepo {
            workdir: source.to_path_buf(),
            top: top.to_path_buf(),
        })
}

/// What is at the Run's worktree path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathState {
    Missing,
    /// A worktree with this branch checked out.
    OnBranch(String),
    /// Something else: a plain folder, a detached worktree.
    Other(String),
}

/// What git says about the Run's path and branch before anything changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeState {
    pub path: PathState,
    pub branch_exists: bool,
    /// The worktree the branch is checked out in, if any.
    pub branch_checked_out_at: Option<PathBuf>,
}

/// How to get the Run's worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorktreeAction {
    /// It exists on the Run's branch (a resume).
    Reuse,
    /// `git worktree add <path> <branch>`.
    CheckoutBranch,
    /// `git worktree add -b <branch> <path> <base_sha>`.
    CreateBranch,
}

pub(crate) fn decide_worktree(
    state: &WorktreeState,
    branch: &str,
    path: &Path,
) -> Result<WorktreeAction, WorktreeError> {
    match (&state.path, &state.branch_checked_out_at) {
        (PathState::OnBranch(found), _) if found == branch => Ok(WorktreeAction::Reuse),
        (PathState::OnBranch(found), _) => Err(WorktreeError::Mismatch {
            path: path.to_path_buf(),
            found: format!("on branch {found}"),
        }),
        (PathState::Other(found), _) => Err(WorktreeError::Mismatch {
            path: path.to_path_buf(),
            found: found.clone(),
        }),
        (PathState::Missing, Some(at)) => Err(WorktreeError::BranchInUse {
            branch: branch.to_string(),
            at: at.clone(),
        }),
        (PathState::Missing, None) if state.branch_exists => Ok(WorktreeAction::CheckoutBranch),
        (PathState::Missing, None) => Ok(WorktreeAction::CreateBranch),
    }
}

/// The worktree `git worktree list --porcelain` shows `branch` checked out
/// in, if any.
pub(crate) fn checked_out_at(porcelain: &str, branch: &str) -> Option<PathBuf> {
    let wanted = format!("branch refs/heads/{branch}");
    porcelain.split("\n\n").find_map(|block| {
        let mut lines = block.lines();
        let path = lines.next()?.strip_prefix("worktree ")?;
        lines
            .any(|line| line == wanted)
            .then(|| PathBuf::from(path))
    })
}

/// The warning recorded when the source checkout has uncommitted changes.
pub(crate) fn dirty_warning(top: &Path) -> String {
    format!(
        "{} has uncommitted changes; they are not in the Run's worktree",
        top.display()
    )
}

/// The warning recorded when a Run works in place, outside git.
pub(crate) fn not_a_repo_warning(workdir: &Path) -> String {
    format!(
        "not a git repository: running in {} without a worktree",
        workdir.display()
    )
}

/// Where a Run works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunPlace {
    /// The workdir agents and tool nodes run in.
    pub workdir: PathBuf,
    /// The Run's worktree; `None` when it works in place.
    pub worktree: Option<RunWorktree>,
    /// Recorded in `run.json` and `RunStarted` (a new Run only), and printed.
    pub warnings: Vec<String>,
}

/// What `pas run` asked for, beside the Run.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlaceRequest<'a> {
    /// The caller's workdir, canonical.
    pub source: &'a Path,
    pub run_id: &'a str,
    /// `--base`; a new Run defaults to [`DEFAULT_BASE`].
    pub base: Option<&'a str>,
    /// `--worktree-root` or `pas.toml`'s; `None` uses the default.
    pub root: Option<&'a Path>,
    /// A dry run starts no agent, so it works in place and creates nothing.
    pub dry_run: bool,
}

/// Place a new Run: check the base, warn about a dirty source checkout,
/// create the worktree and branch. Outside git it works in place.
pub(crate) fn place_new_run(request: &PlaceRequest<'_>) -> Result<RunPlace, WorktreeError> {
    let in_place = |warnings| RunPlace {
        workdir: request.source.to_path_buf(),
        worktree: None,
        warnings,
    };
    if request.dry_run {
        return Ok(in_place(Vec::new()));
    }
    let Some(top) = source_top(request.source) else {
        return Ok(in_place(vec![not_a_repo_warning(request.source)]));
    };
    let base = request.base.unwrap_or(DEFAULT_BASE);
    let base_sha = resolve_base(&top, base)?;
    let mut warnings = Vec::new();
    if is_dirty(&top)? {
        warnings.push(dirty_warning(&top));
    }
    let ignore = |dir: &Path| {
        ensure_pas_gitignore(dir).map_err(|e| WorktreeError::Io {
            path: dir.join(".gitignore"),
            message: e.to_string(),
        })
    };
    let root = match request.root {
        // A root inside the repository would show in `git status` as
        // untracked; outside it the .gitignore is harmless.
        Some(root) => {
            ignore(root)?;
            root.to_path_buf()
        }
        None => {
            ignore(&top.join(PAS_DIR))?;
            default_worktree_root(&top)
        }
    };
    let wanted = RunWorktree {
        path: worktree_path(&root, request.run_id),
        branch: branch_name(request.run_id),
        base: base.to_string(),
        base_sha,
    };
    let path = create_or_reuse(&top, &wanted)?;
    Ok(RunPlace {
        workdir: run_workdir(&path, &top, request.source)?,
        worktree: Some(RunWorktree { path, ..wanted }),
        warnings,
    })
}

/// Place a resumed Run in the worktree its `run.json` recorded, whatever
/// the flags say now; a Run recorded without one works in place.
pub(crate) fn place_resumed_run(
    request: &PlaceRequest<'_>,
    recorded: Option<RunWorktree>,
) -> Result<RunPlace, WorktreeError> {
    let source = request.source;
    let Some(recorded) = recorded else {
        return Ok(RunPlace {
            workdir: source.to_path_buf(),
            worktree: None,
            warnings: Vec::new(),
        });
    };
    let top = source_top(source).ok_or_else(|| WorktreeError::OutsideRepo {
        workdir: source.to_path_buf(),
        top: recorded.path.clone(),
    })?;
    let root = request
        .root
        .map(canonical)
        .unwrap_or_else(|| default_worktree_root(&top));
    let warnings = resume_differences(
        &recorded,
        request.base,
        &worktree_path(&root, request.run_id),
    );
    let path = create_or_reuse(&top, &recorded)?;
    Ok(RunPlace {
        workdir: run_workdir(&path, &top, source)?,
        worktree: Some(RunWorktree { path, ..recorded }),
        warnings,
    })
}

/// A resume keeps the Run's recorded worktree and base: one warning for
/// each setting that now asks for something else. `base` is `None` when
/// none was given.
pub(crate) fn resume_differences(
    recorded: &RunWorktree,
    base: Option<&str>,
    wanted_path: &Path,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if wanted_path != recorded.path {
        warnings.push(format!(
            "resuming in the Run's worktree {}, not {}",
            recorded.path.display(),
            wanted_path.display()
        ));
    }
    if let Some(base) = base.filter(|base| *base != recorded.base) {
        warnings.push(format!(
            "resuming from the Run's base {} ({}), not {base}",
            recorded.base, recorded.base_sha
        ));
    }
    warnings
}

// --- the shell: `git` and the file system ---

/// `git -C <dir> <args>`: trimmed stdout, or the failure.
fn git(dir: &Path, args: &[&str]) -> Result<String, WorktreeError> {
    let failed = |message: String| WorktreeError::Git {
        args: args.join(" "),
        message,
    };
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| failed(e.to_string()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(failed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

/// The canonical root of the git worktree `workdir` is in, or `None` when
/// it is not in one or `git` cannot run.
pub(crate) fn source_top(workdir: &Path) -> Option<PathBuf> {
    let top = git(workdir, &["rev-parse", "--show-toplevel"]).ok()?;
    (!top.is_empty()).then(|| canonical(Path::new(&top)))
}

/// The commit `base` names in the repository at `top`.
pub(crate) fn resolve_base(top: &Path, base: &str) -> Result<String, WorktreeError> {
    git(
        top,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{base}^{{commit}}"),
        ],
    )
    .ok()
    .filter(|sha| !sha.is_empty())
    .ok_or_else(|| WorktreeError::InvalidBase {
        base: base.to_string(),
    })
}

/// Whether the checkout at `top` has uncommitted changes or untracked
/// files, outside `.pas/`.
pub(crate) fn is_dirty(top: &Path) -> Result<bool, WorktreeError> {
    let exclude = format!(":(exclude){PAS_DIR}");
    git(top, &["status", "--porcelain", "--", ".", &exclude]).map(|out| !out.is_empty())
}

/// Write `<pas_dir>/.gitignore` containing `*` unless the file exists, so
/// git ignores everything pas keeps there (also used for a worktree root).
pub(crate) fn ensure_pas_gitignore(pas_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(pas_dir)?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(pas_dir.join(".gitignore"))
    {
        Ok(mut file) => std::io::Write::write_all(&mut file, b"*\n"),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// Look at the Run's path and branch in the repository at `top`.
fn worktree_state(top: &Path, path: &Path, branch: &str) -> Result<WorktreeState, WorktreeError> {
    let path_state = if !path.exists() {
        PathState::Missing
    } else if source_top(path).as_deref() != Some(canonical(path).as_path()) {
        PathState::Other("not a git worktree".to_string())
    } else {
        match git(path, &["symbolic-ref", "--short", "-q", "HEAD"]) {
            Ok(found) if !found.is_empty() => PathState::OnBranch(found),
            _ => PathState::Other("detached HEAD".to_string()),
        }
    };
    let branch_ref = format!("refs/heads/{branch}");
    let branch_exists = git(top, &["rev-parse", "--verify", "--quiet", &branch_ref]).is_ok();
    let porcelain = git(top, &["worktree", "list", "--porcelain"])?;
    Ok(WorktreeState {
        path: path_state,
        branch_exists,
        branch_checked_out_at: checked_out_at(&porcelain, branch),
    })
}

/// Create the Run's worktree, or reuse it when it already exists on the
/// Run's branch. Returns its canonical path.
pub(crate) fn create_or_reuse(
    top: &Path,
    worktree: &RunWorktree,
) -> Result<PathBuf, WorktreeError> {
    let path = &worktree.path;
    // Forget worktrees whose folder was deleted, so their branch can be
    // checked out again.
    git(top, &["worktree", "prune"])?;
    let state = worktree_state(top, path, &worktree.branch)?;
    let path_arg = path.to_string_lossy();
    match decide_worktree(&state, &worktree.branch, path)? {
        WorktreeAction::Reuse => {}
        WorktreeAction::CheckoutBranch => {
            git(top, &["worktree", "add", "-q", &path_arg, &worktree.branch])?;
        }
        WorktreeAction::CreateBranch => {
            git(
                top,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    &worktree.branch,
                    &path_arg,
                    &worktree.base_sha,
                ],
            )?;
        }
    }
    Ok(canonical(path))
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BRANCH: &str = "pas/run/r1";

    fn state(path: PathState, branch_exists: bool, at: Option<&str>) -> WorktreeState {
        WorktreeState {
            path,
            branch_exists,
            branch_checked_out_at: at.map(PathBuf::from),
        }
    }

    fn decide(state: &WorktreeState) -> Result<WorktreeAction, WorktreeError> {
        decide_worktree(state, BRANCH, Path::new("/wt/r1"))
    }

    #[test]
    fn names() {
        assert_eq!(branch_name("r1"), BRANCH);
        assert_eq!(
            worktree_path(Path::new("/root"), "r1"),
            PathBuf::from("/root/r1")
        );
        assert_eq!(
            default_worktree_root(Path::new("/repo")),
            PathBuf::from("/repo/.pas/worktrees")
        );
    }

    #[test]
    fn run_workdir_keeps_the_subdirectory() {
        let wt = Path::new("/repo/.pas/worktrees/r1");
        let top = Path::new("/repo");
        assert_eq!(run_workdir(wt, top, top), Ok(wt.to_path_buf()));
        assert_eq!(
            run_workdir(wt, top, Path::new("/repo/a/b")),
            Ok(wt.join("a/b"))
        );
        assert!(matches!(
            run_workdir(wt, top, Path::new("/elsewhere")),
            Err(WorktreeError::OutsideRepo { .. })
        ));
    }

    #[test]
    fn existing_worktree_on_the_branch_is_reused() {
        let s = state(PathState::OnBranch(BRANCH.into()), true, Some("/wt/r1"));
        assert_eq!(decide(&s), Ok(WorktreeAction::Reuse));
    }

    #[test]
    fn missing_path_with_existing_branch_checks_it_out() {
        let s = state(PathState::Missing, true, None);
        assert_eq!(decide(&s), Ok(WorktreeAction::CheckoutBranch));
    }

    #[test]
    fn neither_path_nor_branch_creates_both() {
        let s = state(PathState::Missing, false, None);
        assert_eq!(decide(&s), Ok(WorktreeAction::CreateBranch));
    }

    #[test]
    fn path_on_another_branch_is_a_mismatch() {
        let s = state(PathState::OnBranch("main".into()), true, None);
        let error = decide(&s).unwrap_err();
        assert_eq!(error.code(), "worktree_mismatch");
        assert!(error.to_string().contains("on branch main"), "{error}");
    }

    #[test]
    fn path_that_is_not_a_worktree_is_a_mismatch() {
        let s = state(PathState::Other("not a git worktree".into()), false, None);
        assert_eq!(decide(&s).unwrap_err().code(), "worktree_mismatch");
    }

    #[test]
    fn branch_checked_out_elsewhere_is_in_use() {
        let s = state(PathState::Missing, true, Some("/other"));
        assert_eq!(
            decide(&s),
            Err(WorktreeError::BranchInUse {
                branch: BRANCH.into(),
                at: PathBuf::from("/other"),
            })
        );
        assert_eq!(decide(&s).unwrap_err().code(), "branch_in_use");
    }

    #[test]
    fn checked_out_at_reads_worktree_list() {
        let porcelain = "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\n\
                         worktree /repo/.pas/worktrees/r1\nHEAD def\nbranch refs/heads/pas/run/r1\n\n\
                         worktree /detached\nHEAD 123\ndetached\n";
        assert_eq!(
            checked_out_at(porcelain, BRANCH),
            Some(PathBuf::from("/repo/.pas/worktrees/r1"))
        );
        assert_eq!(
            checked_out_at(porcelain, "main"),
            Some(PathBuf::from("/repo"))
        );
        assert_eq!(checked_out_at(porcelain, "pas/run/r2"), None);
    }

    #[test]
    fn pas_gitignore_is_written_once() {
        let tmp = tempfile::tempdir().unwrap();
        let pas = tmp.path().join(".pas");
        ensure_pas_gitignore(&pas).unwrap();
        assert_eq!(
            std::fs::read_to_string(pas.join(".gitignore")).unwrap(),
            "*\n"
        );
        std::fs::write(pas.join(".gitignore"), "mine\n").unwrap();
        ensure_pas_gitignore(&pas).unwrap();
        assert_eq!(
            std::fs::read_to_string(pas.join(".gitignore")).unwrap(),
            "mine\n"
        );
    }

    fn recorded() -> RunWorktree {
        RunWorktree {
            path: PathBuf::from("/wt/r1"),
            branch: BRANCH.to_string(),
            base: "main".to_string(),
            base_sha: "abc123".to_string(),
        }
    }

    #[test]
    fn resume_with_the_recorded_settings_has_no_warning() {
        let warnings = resume_differences(&recorded(), Some("main"), Path::new("/wt/r1"));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(resume_differences(&recorded(), None, Path::new("/wt/r1")).is_empty());
    }

    #[test]
    fn resume_names_each_differing_setting() {
        let warnings = resume_differences(&recorded(), Some("dev"), Path::new("/other/r1"));
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("/wt/r1") && warnings[0].contains("/other/r1"));
        assert!(warnings[1].contains("main") && warnings[1].contains("dev"));
    }
}
