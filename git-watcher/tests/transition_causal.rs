//! T061 — scenario test: for every `repo/state/*` variant transition
//! the publisher emits, the retract of the outgoing variant and the
//! assert of the incoming variant share the same `causal_parent`
//! EventId. This is what lets consumers correlate the retract+assert
//! pair as describing a single transition (L2 P11).
//!
//! Reference: `specs/002-git-watcher-actor/tasks.md` T061.

use std::collections::HashSet;
use std::path::Path;

use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::FactKey;
use weaver_core::types::ids::EventId;

use weaver_git_watcher::model::WorkingCopyState;
use weaver_git_watcher::observer::Observation;
use weaver_git_watcher::publisher::test_support::{FactOp, transition_ops};

fn obs(state: WorkingCopyState) -> Observation {
    Observation {
        state,
        dirty: false,
        head_commit: Some("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".into()),
    }
}

/// Extract the first retract+assert pair whose keys are in the
/// `repo/state/*` family from an ops list. Panics if fewer than one
/// of each is present (caller's responsibility to only invoke on
/// variant-changing transitions).
fn extract_state_transition_pair(ops: &[FactOp]) -> (EventId, EventId, &str, &str) {
    let mut retract: Option<(EventId, &str)> = None;
    let mut assert: Option<(EventId, &str)> = None;
    for op in ops {
        match op {
            FactOp::Retract { key, causal_parent } if key.attribute.starts_with("repo/state/") => {
                retract = Some((
                    causal_parent.expect("retract must carry causal parent"),
                    key.attribute.as_str(),
                ));
            }
            FactOp::Assert {
                key, causal_parent, ..
            } if key.attribute.starts_with("repo/state/") => {
                assert = Some((
                    causal_parent.expect("assert must carry causal parent"),
                    key.attribute.as_str(),
                ));
            }
            _ => {}
        }
    }
    let (r_ev, r_attr) = retract.expect("expected a repo/state/* retract");
    let (a_ev, a_attr) = assert.expect("expected a repo/state/* assert");
    (r_ev, a_ev, r_attr, a_attr)
}

fn transition_shares_causal_parent(prev: WorkingCopyState, next: WorkingCopyState) {
    let repo_entity = EntityRef::new(1);
    let repo_path = Path::new("/tmp/fictional-repo");
    let tracked: HashSet<FactKey> = HashSet::new();
    let poll_tick = EventId::for_testing(42);
    let ops = transition_ops(
        repo_entity,
        repo_path,
        &obs(prev),
        &obs(next),
        &tracked,
        poll_tick,
    );

    let (retract_ev, assert_ev, retract_attr, assert_attr) = extract_state_transition_pair(&ops);
    assert_eq!(
        retract_ev, assert_ev,
        "retract ({retract_attr}) and assert ({assert_attr}) must share causal_parent"
    );
    assert_eq!(
        retract_ev, poll_tick,
        "causal_parent must equal the triggering poll tick EventId"
    );
}

#[test]
fn on_branch_to_detached_pair_shares_causal_parent() {
    transition_shares_causal_parent(
        WorkingCopyState::OnBranch {
            name: "main".into(),
        },
        WorkingCopyState::Detached {
            commit: "feedface".into(),
        },
    );
}

#[test]
fn detached_to_on_branch_pair_shares_causal_parent() {
    transition_shares_causal_parent(
        WorkingCopyState::Detached {
            commit: "feedface".into(),
        },
        WorkingCopyState::OnBranch {
            name: "main".into(),
        },
    );
}

#[test]
fn unborn_to_on_branch_pair_shares_causal_parent() {
    transition_shares_causal_parent(
        WorkingCopyState::Unborn {
            intended_branch_name: "main".into(),
        },
        WorkingCopyState::OnBranch {
            name: "main".into(),
        },
    );
}

#[test]
fn on_branch_to_unborn_pair_shares_causal_parent() {
    // Weird-but-possible direction (e.g. `.git/refs/heads/main`
    // deleted out from under us); transition semantics must still
    // hold.
    transition_shares_causal_parent(
        WorkingCopyState::OnBranch {
            name: "main".into(),
        },
        WorkingCopyState::Unborn {
            intended_branch_name: "main".into(),
        },
    );
}

#[test]
fn detached_to_unborn_pair_shares_causal_parent() {
    transition_shares_causal_parent(
        WorkingCopyState::Detached {
            commit: "feedface".into(),
        },
        WorkingCopyState::Unborn {
            intended_branch_name: "main".into(),
        },
    );
}

#[test]
fn unborn_to_detached_pair_shares_causal_parent() {
    transition_shares_causal_parent(
        WorkingCopyState::Unborn {
            intended_branch_name: "main".into(),
        },
        WorkingCopyState::Detached {
            commit: "feedface".into(),
        },
    );
}

#[test]
fn same_variant_payload_change_emits_no_retract() {
    // A branch rename keeps the variant at OnBranch; under the
    // transition contract, no retract is needed (just a re-assert
    // of the state fact with the new payload). Causal-parent
    // semantics only apply to pair transitions, but it's worth
    // locking this down as the counterexample that proves
    // retract+assert pairing fires specifically on discriminator
    // change.
    let repo_entity = EntityRef::new(1);
    let repo_path = Path::new("/tmp/fictional-repo");
    let tracked: HashSet<FactKey> = HashSet::new();
    let ops = transition_ops(
        repo_entity,
        repo_path,
        &obs(WorkingCopyState::OnBranch {
            name: "main".into(),
        }),
        &obs(WorkingCopyState::OnBranch {
            name: "develop".into(),
        }),
        &tracked,
        EventId::for_testing(42),
    );
    let state_retracts = ops
        .iter()
        .filter(|op| {
            matches!(op, FactOp::Retract { key, .. } if key.attribute.starts_with("repo/state/"))
        })
        .count();
    assert_eq!(
        state_retracts, 0,
        "same-variant payload change must not produce a state retract; ops={ops:?}"
    );
}
