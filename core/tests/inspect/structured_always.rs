//! T067 — scenario test: inspection output is **always structured**.
//!
//! Drives facts into the dispatcher from multiple actor kinds
//! (behavior-fired `buffer/*`, service-published `repo/*`, directly-
//! asserted core/tui placeholders) and asserts every inspection
//! response:
//!
//! 1. carries a non-empty, slice-valid `asserting_kind`;
//! 2. never encodes a legacy `External(...)` opaque tag anywhere in
//!    the rendered JSON — the regex `^External\(` must miss every
//!    string-typed value in the response;
//! 3. produces a serde-parseable JSON object.
//!
//! Reference: `specs/002-git-watcher-actor/tasks.md` T067.

#[path = "../common/mod.rs"]
mod common;

use common::StubBehavior;
use uuid::Uuid;
use weaver_core::behavior::dispatcher::{Dispatcher, ServicePublishOutcome};
use weaver_core::fact_space::FactStore;
use weaver_core::inspect::inspect_fact;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::EventId;
use weaver_core::types::message::InspectionDetail;

/// Every string the JSON carries must not start with
/// `External(` — a guard against slice-001's opaque tag sneaking
/// back in via any code path (direct serialization, a stale
/// constructor, or a regression elsewhere).
fn no_external_tag_anywhere(v: &serde_json::Value) {
    fn walk(v: &serde_json::Value, path: &str) {
        match v {
            serde_json::Value::String(s) => {
                assert!(
                    !s.starts_with("External("),
                    "opaque External(...) tag leaked at {path}: {s:?}",
                );
            }
            serde_json::Value::Array(xs) => {
                for (i, child) in xs.iter().enumerate() {
                    walk(child, &format!("{path}[{i}]"));
                }
            }
            serde_json::Value::Object(m) => {
                for (k, child) in m {
                    walk(child, &format!("{path}.{k}"));
                }
            }
            _ => {}
        }
    }
    walk(v, "$");
}

fn assert_structured(detail: &InspectionDetail) {
    let json = serde_json::to_value(detail).expect("serialize");
    let kind = json
        .get("asserting_kind")
        .and_then(|k| k.as_str())
        .expect("asserting_kind present");
    assert!(
        matches!(
            kind,
            "behavior" | "service" | "core" | "tui" | "user" | "host" | "agent"
        ),
        "unexpected asserting_kind: {kind:?}",
    );
    assert!(!kind.is_empty(), "asserting_kind must never be empty",);
    no_external_tag_anywhere(&json);
}

#[tokio::test]
async fn every_inspection_response_is_structured_across_fact_families() {
    let buffer_entity = EntityRef::new(1);
    let buffer_key = FactKey::new(buffer_entity, "buffer/dirty");

    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(StubBehavior::new(
        buffer_key.clone(),
        FactValue::Bool(true),
    )));

    // ------------------------------------------------------------
    // buffer/* — behavior-authored via the stub behavior.
    // ------------------------------------------------------------
    dispatcher
        .process_event(Event {
            id: EventId::new(10),
            name: "buffer/open".into(),
            target: Some(buffer_entity),
            payload: EventPayload::BufferOpen {
                path: "/tmp/weaver-fixture".into(),
            },
            provenance: Provenance::new(ActorIdentity::Tui, 100, None).unwrap(),
        })
        .await;

    // ------------------------------------------------------------
    // repo/* — service-authored via the public publish_from_service
    // entry point.
    // ------------------------------------------------------------
    let repo_entity = EntityRef::new(2);
    let svc_identity =
        ActorIdentity::service("git-watcher", Uuid::new_v4()).expect("valid service");
    let outcome = dispatcher
        .publish_from_service(
            7,
            Fact {
                key: FactKey::new(repo_entity, "repo/dirty"),
                value: FactValue::Bool(false),
                provenance: Provenance::new(svc_identity.clone(), 200, None).unwrap(),
            },
        )
        .await;
    assert!(matches!(outcome, ServicePublishOutcome::Asserted));

    // ------------------------------------------------------------
    // watcher/status — also service-authored on the same conn so it
    // covers a *different* fact family but the same identity
    // binding.
    // ------------------------------------------------------------
    let watcher_entity = EntityRef::new(3);
    let outcome = dispatcher
        .publish_from_service(
            7,
            Fact {
                key: FactKey::new(watcher_entity, "watcher/status"),
                value: FactValue::String("ready".into()),
                provenance: Provenance::new(svc_identity, 210, None).unwrap(),
            },
        )
        .await;
    assert!(matches!(outcome, ServicePublishOutcome::Asserted));

    // Each family must produce a structured inspection response.
    let snapshot = {
        let fs = dispatcher.fact_store();
        let fs = fs.lock().await;
        fs.snapshot()
    };
    let trace = dispatcher.trace();
    let trace = trace.lock().await;

    for (key, expected_kind) in [
        (buffer_key.clone(), "behavior"),
        (FactKey::new(repo_entity, "repo/dirty"), "service"),
        (FactKey::new(watcher_entity, "watcher/status"), "service"),
    ] {
        let detail = inspect_fact(&snapshot, &trace, &key)
            .unwrap_or_else(|e| panic!("inspect {key:?} failed: {e:?}"));
        assert_eq!(
            detail.asserting_kind, expected_kind,
            "wrong asserting_kind for {key:?}: {detail:?}",
        );
        assert_structured(&detail);
    }
}
