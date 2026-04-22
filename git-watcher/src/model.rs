//! Watcher-internal data model. See
//! `specs/002-git-watcher-actor/data-model.md`.
//!
//! `WorkingCopyState` is the discriminated-union-by-naming shape the
//! watcher observes for each repository. It converts 1:1 into the
//! `repo/state/*` family on the bus (see `publisher::publish_state`).

use std::path::Path;

use thiserror::Error;

/// The working-copy state of a git repository — one of three variants
/// this slice ships. Transient operation states (rebase / merge /
/// cherry-pick / revert / bisect) are out of scope per spec
/// Clarification Q4; they produce an `Err(ObserverError::...)` instead
/// of being silently folded into another variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkingCopyState {
    /// HEAD → `refs/heads/<name>`, repository has at least one commit.
    OnBranch { name: String },

    /// HEAD → commit SHA directly (detached HEAD).
    Detached { commit: String },

    /// HEAD → nonexistent ref (new repository, no commits yet).
    Unborn { intended_branch_name: String },
}

impl WorkingCopyState {
    /// Short label for diagnostic rendering (matches the `repo/state/*`
    /// sub-attribute name, minus the `repo/state/` prefix).
    pub fn kind_label(&self) -> &'static str {
        match self {
            WorkingCopyState::OnBranch { .. } => "on-branch",
            WorkingCopyState::Detached { .. } => "detached",
            WorkingCopyState::Unborn { .. } => "unborn",
        }
    }

    /// The full fact attribute string this state asserts under
    /// (slash-delimited, kebab-case per Amendment 5).
    pub fn fact_attribute(&self) -> String {
        format!("repo/state/{}", self.kind_label())
    }
}

/// Errors the observer can surface. These map onto the watcher's
/// `Lifecycle(Degraded)` path — a recoverable read failure — or the
/// hard-fail startup path (FR-005 / exit code 1).
#[derive(Debug, Error)]
pub enum ObserverError {
    #[error("not a git repository: {path}")]
    NotARepository { path: String },

    #[error(
        "repository at {path} is bare; weaver-git-watcher observes \
         working-copy state (repo/dirty, repo/state/*) which has no \
         meaning without a working tree"
    )]
    BareRepositoryUnsupported { path: String },

    #[error(
        "repository at {path} is in a transient operation state unsupported this slice; deferred per Clarification Q4"
    )]
    UnsupportedTransientState { path: String },

    #[error(
        "repository at {path} has an unsupported HEAD shape: symbolic ref points at {ref_name:?}, \
         but this slice's WorkingCopyState::OnBranch is reserved for refs/heads/<name>"
    )]
    UnsupportedHeadShape { path: String, ref_name: String },

    #[error("repository observation failed: {source}")]
    Observation {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
}

/// Resolve a git `gix::head::Kind` into a [`WorkingCopyState`].
///
/// The watcher's poll loop calls this every tick; the function is
/// pure and side-effect free.
///
/// Returns:
/// - `Ok(OnBranch { name })` when HEAD is a symbolic ref to
///   `refs/heads/<name>` **and** at least one commit is present.
/// - `Ok(Detached { commit })` when HEAD points directly at a commit.
/// - `Ok(Unborn { intended_branch_name })` when HEAD is symbolic but
///   points to a ref that does not yet exist (new repo).
/// - `Err(ObserverError::UnsupportedHeadShape)` when HEAD is
///   symbolic but points outside `refs/heads/` (e.g. a tag-ref).
///   The slice's data-model contract reserves `OnBranch` for
///   branch refs only (F26 review fix); anomalous HEAD shapes
///   flip the watcher to `Degraded` rather than silently mis-
///   reporting branch state.
/// - `Err(ObserverError::Observation)` for any underlying `gix` error.
pub fn working_copy_state_from_head(
    head: &gix::Head<'_>,
    repo_path: &Path,
) -> Result<WorkingCopyState, ObserverError> {
    use gix::head::Kind;
    match &head.kind {
        Kind::Symbolic(inner) => {
            // Symbolic ref — only accept `refs/heads/<name>` as
            // OnBranch. A symbolic HEAD targeting refs/tags/,
            // refs/remotes/, or anything else is an unsupported
            // shape for this slice's observation model.
            let full = inner.name.as_bstr().to_string();
            if let Some(name) = full.strip_prefix("refs/heads/") {
                Ok(WorkingCopyState::OnBranch {
                    name: name.to_string(),
                })
            } else {
                Err(ObserverError::UnsupportedHeadShape {
                    path: repo_path.display().to_string(),
                    ref_name: full,
                })
            }
        }
        Kind::Detached { target, .. } => Ok(WorkingCopyState::Detached {
            commit: target.to_hex().to_string(),
        }),
        Kind::Unborn(refname) => {
            let name = strip_heads_prefix(refname.as_bstr().to_string());
            Ok(WorkingCopyState::Unborn {
                intended_branch_name: name,
            })
        }
    }
}

/// Strip the `refs/heads/` prefix from a full ref name, returning the
/// bare branch name. Leaves other ref forms untouched.
fn strip_heads_prefix(full: String) -> String {
    full.strip_prefix("refs/heads/")
        .map(str::to_string)
        .unwrap_or(full)
}

/// Convenience predicate used by the CLI to fail fast on non-git
/// targets before entering the publisher loop.
pub fn is_git_repository(path: &Path) -> bool {
    gix::discover(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_label_matches_fact_attribute() {
        let s = WorkingCopyState::OnBranch {
            name: "main".into(),
        };
        assert_eq!(s.kind_label(), "on-branch");
        assert_eq!(s.fact_attribute(), "repo/state/on-branch");

        let s = WorkingCopyState::Detached {
            commit: "abc".into(),
        };
        assert_eq!(s.kind_label(), "detached");
        assert_eq!(s.fact_attribute(), "repo/state/detached");

        let s = WorkingCopyState::Unborn {
            intended_branch_name: "main".into(),
        };
        assert_eq!(s.kind_label(), "unborn");
        assert_eq!(s.fact_attribute(), "repo/state/unborn");
    }

    #[test]
    fn strip_heads_prefix_returns_branch_name() {
        assert_eq!(strip_heads_prefix("refs/heads/main".into()), "main");
        assert_eq!(
            strip_heads_prefix("refs/heads/feature/x".into()),
            "feature/x"
        );
    }

    #[test]
    fn strip_heads_prefix_leaves_other_refs_untouched() {
        assert_eq!(strip_heads_prefix("HEAD".into()), "HEAD");
        assert_eq!(strip_heads_prefix("refs/tags/v1".into()), "refs/tags/v1");
    }
}
