//! Repository observation. Pure reads, called once per poll tick.
//! See `specs/002-git-watcher-actor/research.md` §1.
//!
//! The observer holds a `gix::Repository` handle for the duration of
//! the watcher process; each poll reopens the local read-state via
//! the handle rather than re-opening the repository from disk. The
//! handle is internally cloneable and cheap per gix's contract.
//!
//! **Research §1 deviation for the dirty-check**: HEAD kind,
//! branch name, and head-commit SHA are read via `gix` as planned.
//! The dirty-check (Clarification Q5: working-tree-or-index vs HEAD,
//! untracked excluded) is implemented as a short-lived `git diff HEAD
//! --quiet` invocation because `gix` 0.66's high-level `status::Platform`
//! iterator API requires shape that is disproportionate for this
//! slice's narrow dirty semantic. A follow-up slice may migrate
//! back to pure `gix` once the API stabilizes. The shell-out cost
//! (~20 ms per poll at the default 250 ms cadence) is well within
//! the operator-perceived budget (SC-002: ≤ 500 ms).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::model::{ObserverError, WorkingCopyState, working_copy_state_from_head};

/// One poll-tick's observation of a repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    /// Working-copy state — exactly one variant asserted per repo per
    /// the mutex invariant (see `data-model.md` §6.3 and
    /// `07-open-questions.md §26`).
    pub state: WorkingCopyState,
    /// `true` iff working tree OR index differs from HEAD, with
    /// untracked files excluded (Clarification Q5).
    pub dirty: bool,
    /// HEAD commit SHA if the repo has at least one commit, else
    /// `None` (for `Unborn` state).
    pub head_commit: Option<String>,
}

/// Observes one repository. The underlying `gix::Repository` handle
/// shares state internally.
pub struct RepoObserver {
    repo: gix::Repository,
    path: PathBuf,
}

impl std::fmt::Debug for RepoObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepoObserver")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl RepoObserver {
    /// Open a repository at `path`. Fails fast if `path` is not a git
    /// repository (FR-005 / exit code 1 path).
    ///
    /// The identity path (used for `repo/path` and for the `EntityRef`
    /// that keys the authority mutex) is the *discovered* working-tree
    /// root — never the user-typed input. Two watchers pointed at
    /// different subdirectories of the same repo therefore resolve to
    /// the same `EntityRef` and FR-009's single-writer rule still
    /// holds (F6 review fix).
    ///
    /// Bare repositories are rejected at open time (F9 review fix):
    /// the watcher's entire data model is working-copy state, which
    /// has no meaning without a work tree, and `is_dirty` would thrash
    /// into `Degraded` every poll because `git diff HEAD --quiet`
    /// refuses to run on a bare repo.
    pub fn open(path: &Path) -> Result<Self, ObserverError> {
        let repo = gix::discover(path).map_err(|_| ObserverError::NotARepository {
            path: path.display().to_string(),
        })?;
        let root = repo
            .work_dir()
            .ok_or_else(|| ObserverError::BareRepositoryUnsupported {
                path: repo.git_dir().display().to_string(),
            })?;
        let canon = root
            .canonicalize()
            .map_err(|e| ObserverError::Observation {
                source: Box::new(e),
            })?;
        Ok(Self { repo, path: canon })
    }

    /// The canonical absolute path of the repository's working tree
    /// root (or the `.git` directory for bare repositories).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read one observation of the repository.
    pub fn observe(&self) -> Result<Observation, ObserverError> {
        let head = self.repo.head().map_err(|e| ObserverError::Observation {
            source: Box::new(e),
        })?;
        let state = working_copy_state_from_head(&head)?;
        let head_commit = match &state {
            WorkingCopyState::OnBranch { .. } | WorkingCopyState::Detached { .. } => {
                self.resolve_head_commit()?
            }
            WorkingCopyState::Unborn { .. } => None,
        };
        let dirty = self.is_dirty(&state)?;
        Ok(Observation {
            state,
            dirty,
            head_commit,
        })
    }

    /// Resolve `HEAD` to a commit SHA (hex string). `None` for unborn
    /// branches.
    fn resolve_head_commit(&self) -> Result<Option<String>, ObserverError> {
        match self.repo.rev_parse_single("HEAD") {
            Ok(object_id) => Ok(Some(object_id.to_hex().to_string())),
            Err(_) => Ok(None),
        }
    }

    /// Working-tree-or-index differs from HEAD (per Clarification Q5).
    /// Untracked files are NOT included — matches `git diff HEAD
    /// --quiet` exit semantics exactly.
    ///
    /// For `Unborn` state there is no HEAD to diff against; we return
    /// `false` (clean) — an unborn repo with uncommitted work is a
    /// natural "clean slate" state for this fact's semantics.
    ///
    /// See module-level note on the gix/shell-out trade-off.
    fn is_dirty(&self, state: &WorkingCopyState) -> Result<bool, ObserverError> {
        if matches!(state, WorkingCopyState::Unborn { .. }) {
            return Ok(false);
        }
        let status = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(["diff", "HEAD", "--quiet"])
            .status()
            .map_err(|e| ObserverError::Observation {
                source: Box::new(e),
            })?;
        // exit 0 = clean, exit 1 = dirty, anything else = error.
        match status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            Some(other) => Err(ObserverError::Observation {
                source: format!("git diff HEAD --quiet exited with code {other}").into(),
            }),
            None => Err(ObserverError::Observation {
                source: "git diff HEAD --quiet terminated by signal".into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    // These tests touch real git repos under tempfile. They're fast
    // because we create minimal synthetic repos.

    fn run_git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(dir)
            .args(args)
            .status()
            .expect("git command runs");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo(dir: &Path) {
        run_git(dir, &["init", "-b", "main", "-q"]);
        run_git(
            dir,
            &["config", "user.email", "observer-test@example.invalid"],
        );
        run_git(dir, &["config", "user.name", "Observer Test"]);
    }

    fn commit_one(dir: &Path, path: &str, contents: &str, msg: &str) {
        std::fs::write(dir.join(path), contents).unwrap();
        run_git(dir, &["add", path]);
        run_git(dir, &["commit", "-q", "-m", msg]);
    }

    #[test]
    fn open_fails_for_non_git_directory() {
        let td = tempfile::tempdir().unwrap();
        let err = RepoObserver::open(td.path()).unwrap_err();
        assert!(matches!(err, ObserverError::NotARepository { .. }));
    }

    #[test]
    fn observes_unborn_repository() {
        let td = tempfile::tempdir().unwrap();
        init_repo(td.path());
        let obs = RepoObserver::open(td.path()).unwrap().observe().unwrap();
        assert!(matches!(obs.state, WorkingCopyState::Unborn { .. }));
        assert!(!obs.dirty);
        assert!(obs.head_commit.is_none());
    }

    #[test]
    fn observes_on_branch_after_commit() {
        let td = tempfile::tempdir().unwrap();
        init_repo(td.path());
        commit_one(td.path(), "a.txt", "hello", "initial");
        let obs = RepoObserver::open(td.path()).unwrap().observe().unwrap();
        match obs.state {
            WorkingCopyState::OnBranch { name } => assert_eq!(name, "main"),
            other => panic!("expected OnBranch main, got {other:?}"),
        }
        assert!(!obs.dirty);
        assert!(obs.head_commit.is_some());
    }

    #[test]
    fn detects_dirty_after_modifying_tracked_file() {
        let td = tempfile::tempdir().unwrap();
        init_repo(td.path());
        commit_one(td.path(), "a.txt", "hello", "initial");
        std::fs::write(td.path().join("a.txt"), "hello world").unwrap();
        let obs = RepoObserver::open(td.path()).unwrap().observe().unwrap();
        assert!(
            obs.dirty,
            "modifying a tracked file should mark the repo dirty"
        );
    }

    #[test]
    fn untracked_alone_is_not_dirty_per_q5() {
        let td = tempfile::tempdir().unwrap();
        init_repo(td.path());
        commit_one(td.path(), "a.txt", "hello", "initial");
        std::fs::write(td.path().join("untracked.txt"), "new").unwrap();
        let obs = RepoObserver::open(td.path()).unwrap().observe().unwrap();
        assert!(
            !obs.dirty,
            "untracked-only must not flip dirty per spec Clarification Q5"
        );
    }

    #[test]
    fn detects_dirty_after_staged_change() {
        let td = tempfile::tempdir().unwrap();
        init_repo(td.path());
        commit_one(td.path(), "a.txt", "hello", "initial");
        std::fs::write(td.path().join("a.txt"), "staged change").unwrap();
        run_git(td.path(), &["add", "a.txt"]);
        let obs = RepoObserver::open(td.path()).unwrap().observe().unwrap();
        assert!(obs.dirty, "staged change alone should mark the repo dirty");
    }

    #[test]
    fn open_rejects_bare_repository() {
        // F9 regression: bare repos have no working tree; allowing
        // them would let the dirty-check (`git diff HEAD --quiet`)
        // fail every poll and trap the watcher in Degraded state.
        let td = tempfile::tempdir().unwrap();
        let status = Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(td.path())
            .status()
            .expect("git init --bare runs");
        assert!(status.success());
        let err = RepoObserver::open(td.path()).unwrap_err();
        assert!(
            matches!(err, ObserverError::BareRepositoryUnsupported { .. }),
            "expected BareRepositoryUnsupported, got {err:?}"
        );
    }

    #[test]
    fn identity_path_is_repo_root_even_when_opened_from_subdirectory() {
        // F6 regression: pointing the watcher at a subdirectory must
        // still key by the discovered working-tree root. Otherwise two
        // watchers on the same repo would hash to distinct entities
        // and both claim authority for `repo/*`, bypassing FR-009.
        let td = tempfile::tempdir().unwrap();
        init_repo(td.path());
        commit_one(td.path(), "a.txt", "hello", "initial");
        let subdir = td.path().join("nested/deep");
        std::fs::create_dir_all(&subdir).unwrap();

        let from_root = RepoObserver::open(td.path()).unwrap();
        let from_subdir = RepoObserver::open(&subdir).unwrap();
        assert_eq!(
            from_root.path(),
            from_subdir.path(),
            "subdirectory input must resolve to the same repo root"
        );
    }

    #[test]
    fn detects_detached_head() {
        let td = tempfile::tempdir().unwrap();
        init_repo(td.path());
        commit_one(td.path(), "a.txt", "hello", "initial");
        // Capture the commit sha so we can checkout to it directly.
        let sha_out = Command::new("git")
            .current_dir(td.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let sha = String::from_utf8(sha_out.stdout)
            .unwrap()
            .trim()
            .to_string();
        run_git(td.path(), &["checkout", "-q", &sha]);
        let obs = RepoObserver::open(td.path()).unwrap().observe().unwrap();
        assert!(matches!(obs.state, WorkingCopyState::Detached { .. }));
    }
}
