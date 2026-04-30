//! T060 — property test: for any sequence of observations the
//! watcher's transition logic walks through, the count of asserted
//! `repo/state/*` facts for a given repository entity is always ≤ 1.
//!
//! This is the mutex invariant that makes the discriminated-union-
//! by-naming shape sound (see `docs/07-open-questions.md §26`): at
//! every trace prefix exactly one state variant is asserted. The
//! test drives the publisher's pure transition engine
//! (`test_support::transition_ops`) against arbitrary observation
//! sequences — `proptest` generates up to ~20 observations per run,
//! each with an arbitrary `WorkingCopyState`, dirty flag, and
//! optional head commit. After applying the ops for each
//! transition the test checks the invariant.
//!
//! Reference: `specs/002-git-watcher-actor/tasks.md` T060.

use std::collections::HashSet;
use std::path::Path;

use proptest::prelude::*;
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::FactKey;
use weaver_core::types::ids::EventId;

use weaver_git_watcher::model::WorkingCopyState;
use weaver_git_watcher::observer::Observation;
use weaver_git_watcher::publisher::test_support::{FactOp, state_fact, transition_ops};

fn arb_state() -> impl Strategy<Value = WorkingCopyState> {
    prop_oneof![
        "[a-z]{1,8}".prop_map(|name| WorkingCopyState::OnBranch { name }),
        "[a-f0-9]{40}".prop_map(|commit| WorkingCopyState::Detached { commit }),
        "[a-z]{1,8}".prop_map(|name| WorkingCopyState::Unborn {
            intended_branch_name: name,
        }),
    ]
}

fn arb_observation() -> impl Strategy<Value = Observation> {
    (
        arb_state(),
        any::<bool>(),
        proptest::option::of("[a-f0-9]{40}"),
    )
        .prop_map(|(state, dirty, head_commit)| Observation {
            state,
            dirty,
            head_commit,
        })
}

fn apply_ops(ops: Vec<FactOp>, asserted: &mut HashSet<FactKey>) {
    for op in ops {
        match op {
            FactOp::Assert { key, .. } => {
                asserted.insert(key);
            }
            FactOp::Retract { key, .. } => {
                asserted.remove(&key);
            }
        }
    }
}

fn count_state_facts(asserted: &HashSet<FactKey>) -> usize {
    asserted
        .iter()
        .filter(|k| k.attribute.starts_with("repo/state/"))
        .count()
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        .. ProptestConfig::default()
    })]

    /// The mutex invariant holds across every trace prefix: at
    /// every step, the asserted set contains at most one
    /// `repo/state/*` fact per repository entity.
    #[test]
    fn at_most_one_state_fact_asserted_per_entity(
        observations in proptest::collection::vec(arb_observation(), 1..=20),
    ) {
        let repo_entity = EntityRef::new(1);
        let repo_path = Path::new("/tmp/fictional-repo");
        let mut asserted: HashSet<FactKey> = HashSet::new();

        // Bootstrap step: seed the asserted set with the first
        // observation's state fact + repo/path (mimics the
        // publisher's `publish_observation` cold-start path).
        let first = &observations[0];
        let (first_attr, _) = state_fact(&first.state);
        asserted.insert(FactKey::new(repo_entity, first_attr));
        asserted.insert(FactKey::new(repo_entity, "repo/path"));

        prop_assert_eq!(
            count_state_facts(&asserted),
            1,
            "after bootstrap exactly one state fact must be asserted",
        );

        for (i, pair) in observations.windows(2).enumerate() {
            let prev = &pair[0];
            let next = &pair[1];
            let ops = transition_ops(
                repo_entity,
                repo_path,
                prev,
                next,
                &asserted,
                EventId::for_testing((1 + i as u64) as u128),
            );
            apply_ops(ops, &mut asserted);
            let n = count_state_facts(&asserted);
            prop_assert!(
                n <= 1,
                "mutex invariant violated at step {}: {} repo/state/* facts asserted; \
                 prev={:?} next={:?}",
                i + 1,
                n,
                prev.state,
                next.state,
            );
        }
    }

    /// Strengthening: across every transition, at most ONE
    /// repo/state/* assert op appears, and it names the NEW
    /// state's attribute (never the outgoing one).
    #[test]
    fn transition_emits_exactly_one_state_assert_naming_next_variant(
        pair in (arb_observation(), arb_observation()),
    ) {
        let repo_entity = EntityRef::new(1);
        let repo_path = Path::new("/tmp/fictional-repo");
        let tracked: HashSet<FactKey> = HashSet::new();
        let ops = transition_ops(
            repo_entity,
            repo_path,
            &pair.0,
            &pair.1,
            &tracked,
            EventId::for_testing(42),
        );

        let mut state_asserts = 0;
        let (next_attr, _) = state_fact(&pair.1.state);
        for op in &ops {
            if let FactOp::Assert { key, .. } = op {
                if key.attribute.starts_with("repo/state/") {
                    state_asserts += 1;
                    prop_assert_eq!(
                        key.attribute.as_str(),
                        next_attr,
                        "state assert names the wrong variant",
                    );
                }
            }
        }
        prop_assert_eq!(
            state_asserts,
            1,
            "expected exactly one repo/state/* assert per transition, got {}",
            state_asserts,
        );
    }
}
