//! Provenance metadata required on every fact, event, and trace entry per
//! L2 constitution Principle 11 and L1 constitution §17 (Multi-Actor
//! Coherence).
//!
//! Slice 002 replaces the opaque `SourceId::External(String)` with a
//! structured [`ActorIdentity`] — one closed enum with per-kind
//! variants materializing the `docs/01-system-model.md §6` actor
//! taxonomy. The shape is committed in
//! `specs/002-git-watcher-actor/` Clarification Q1. See also
//! `docs/07-open-questions.md §25` for the migration rationale.

use crate::types::ids::{BehaviorId, EventId};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Provenance metadata attached to every fact, event, and trace entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub source: ActorIdentity,
    /// Monotonic nanoseconds — by convention, since process start.
    pub timestamp_ns: u64,
    pub causal_parent: Option<EventId>,
}

/// The originating actor for a published bus message or trace entry.
///
/// One closed enum per actor kind in `docs/01-system-model.md §6`.
/// The enum is shipped with every variant required by the §6
/// taxonomy; variants not exercised this slice (`User`, `Host`,
/// `Agent`) are reserved for forward compatibility under a single
/// protocol version bump, so future slices can populate them
/// without another wire-breaking change.
///
/// Wire shape: internally tagged with a `"type"` discriminator and
/// kebab-case variant names / field names (Amendment 5). Examples:
///
/// - `Core` → `{"type":"core"}`
/// - `Behavior { id }` → `{"type":"behavior","id":"core/dirty-tracking"}`
/// - `Tui` → `{"type":"tui"}`
/// - `Service { service_id, instance_id }` →
///   `{"type":"service","service-id":"git-watcher","instance-id":"<uuid>"}`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ActorIdentity {
    /// The core process itself (core-authored facts and events).
    Core,

    /// A registered in-core behavior firing.
    Behavior { id: BehaviorId },

    /// The TUI client.
    Tui,

    /// A governed service on the bus — for example, `weaver-git-watcher`
    /// (slice 002). Carries both the stable service identifier and a
    /// per-invocation instance identifier (UUID v4 per Clarification Q3).
    Service {
        #[serde(rename = "service-id")]
        service_id: String,
        #[serde(rename = "instance-id")]
        instance_id: Uuid,
    },

    /// A human user (reserved — not emitted this slice).
    User { id: UserId },

    /// A language host proxying user code (reserved — not emitted this slice).
    Host {
        #[serde(rename = "host-id")]
        host_id: String,
        #[serde(rename = "hosted-origin")]
        hosted_origin: HostedOrigin,
    },

    /// An agent delegated powers by the user (reserved — not emitted this slice).
    ///
    /// The optional `on_behalf_of` chain carries the delegator per
    /// `docs/05-protocols.md §3.4` (authorship-versus-provenance
    /// distinction).
    Agent {
        #[serde(rename = "agent-id")]
        agent_id: String,
        #[serde(rename = "on-behalf-of")]
        on_behalf_of: Option<Box<ActorIdentity>>,
    },
}

/// Opaque human-user identity. Reserved for forward compatibility;
/// not emitted by any producer in slice 002.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(String);

impl UserId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Origin of language-hosted user code within a language host's
/// provenance. Reserved; not emitted this slice. See
/// `docs/05-protocols.md §3.3`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedOrigin {
    pub file: String,
    pub location: Option<String>,
    #[serde(rename = "runtime-version")]
    pub runtime_version: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProvenanceError {
    #[error("service identifier must be non-empty (L2 P11; slice 002 ActorIdentity::service)")]
    EmptyServiceId,

    #[error("service identifier {0:?} must be kebab-case (L2 Amendment 5)")]
    NonKebabCaseServiceId(String),
}

impl ActorIdentity {
    /// Construct an [`ActorIdentity::Service`] after validating the
    /// service identifier.
    ///
    /// `service_id` MUST be non-empty and kebab-case: lowercase ASCII
    /// (`a`..=`z`), digits, and `-` only; no leading / trailing /
    /// consecutive hyphens. Matches the wire-vocabulary contract in
    /// L2 Amendment 5.
    pub fn service(
        service_id: impl Into<String>,
        instance_id: Uuid,
    ) -> Result<Self, ProvenanceError> {
        let service_id = service_id.into();
        validate_kebab_case(&service_id)?;
        Ok(Self::Service {
            service_id,
            instance_id,
        })
    }

    /// Convenience constructor for [`ActorIdentity::Behavior`].
    pub fn behavior(id: BehaviorId) -> Self {
        Self::Behavior { id }
    }

    /// Convenience constructor for [`ActorIdentity::User`].
    pub fn user(id: UserId) -> Self {
        Self::User { id }
    }

    /// Short, human-readable label for diagnostic rendering.
    /// **Not** a wire format — wire serialization uses serde.
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Behavior { .. } => "behavior",
            Self::Tui => "tui",
            Self::Service { .. } => "service",
            Self::User { .. } => "user",
            Self::Host { .. } => "host",
            Self::Agent { .. } => "agent",
        }
    }
}

fn validate_kebab_case(id: &str) -> Result<(), ProvenanceError> {
    if id.is_empty() {
        return Err(ProvenanceError::EmptyServiceId);
    }
    if id.starts_with('-') || id.ends_with('-') || id.contains("--") {
        return Err(ProvenanceError::NonKebabCaseServiceId(id.into()));
    }
    for c in id.chars() {
        match c {
            'a'..='z' | '0'..='9' | '-' => {}
            _ => return Err(ProvenanceError::NonKebabCaseServiceId(id.into())),
        }
    }
    Ok(())
}

impl Provenance {
    /// Construct a `Provenance`.
    ///
    /// Returns `Result` for backward compatibility with existing call
    /// sites; all well-formed [`ActorIdentity`] variants produce `Ok`.
    /// Invalid service identifiers are rejected at
    /// [`ActorIdentity::service`] construction, before reaching this
    /// constructor.
    pub fn new(
        source: ActorIdentity,
        timestamp_ns: u64,
        causal_parent: Option<EventId>,
    ) -> Result<Self, ProvenanceError> {
        Ok(Self {
            source,
            timestamp_ns,
            causal_parent,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rejects_empty_service_id() {
        assert_eq!(
            ActorIdentity::service("", Uuid::nil()).unwrap_err(),
            ProvenanceError::EmptyServiceId
        );
    }

    #[test]
    fn rejects_non_kebab_case_service_id() {
        assert!(ActorIdentity::service("GitWatcher", Uuid::nil()).is_err());
        assert!(ActorIdentity::service("git_watcher", Uuid::nil()).is_err());
        assert!(ActorIdentity::service("-git-watcher", Uuid::nil()).is_err());
        assert!(ActorIdentity::service("git-watcher-", Uuid::nil()).is_err());
        assert!(ActorIdentity::service("git--watcher", Uuid::nil()).is_err());
        assert!(ActorIdentity::service("git watcher", Uuid::nil()).is_err());
    }

    #[test]
    fn accepts_kebab_case_service_id() {
        assert!(ActorIdentity::service("git-watcher", Uuid::new_v4()).is_ok());
        assert!(ActorIdentity::service("a", Uuid::nil()).is_ok());
        assert!(ActorIdentity::service("weaver-cli", Uuid::nil()).is_ok());
        assert!(ActorIdentity::service("abc-123-xyz", Uuid::nil()).is_ok());
    }

    #[test]
    fn accepts_valid_sources() {
        assert!(Provenance::new(ActorIdentity::Core, 0, None).is_ok());
        assert!(Provenance::new(ActorIdentity::Tui, 1, None).is_ok());
        assert!(
            Provenance::new(
                ActorIdentity::behavior(BehaviorId::new("core/dirty-tracking")),
                2,
                Some(EventId::new(7)),
            )
            .is_ok()
        );
        let svc = ActorIdentity::service("git-watcher", Uuid::new_v4()).unwrap();
        assert!(Provenance::new(svc, 3, None).is_ok());
    }

    #[test]
    fn json_round_trip_behavior() {
        let p = Provenance::new(
            ActorIdentity::behavior(BehaviorId::new("core/dirty-tracking")),
            1000,
            Some(EventId::new(42)),
        )
        .unwrap();
        let s = serde_json::to_string(&p).unwrap();
        let back: Provenance = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
        // Wire shape spot-check: kebab-case field names + adjacent-tagged.
        assert!(s.contains("\"type\":\"behavior\""));
        assert!(s.contains("\"id\":\"core/dirty-tracking\""));
    }

    #[test]
    fn json_round_trip_service() {
        let instance = Uuid::new_v4();
        let src = ActorIdentity::service("git-watcher", instance).unwrap();
        let p = Provenance::new(src, 12345, None).unwrap();
        let s = serde_json::to_string(&p).unwrap();
        let back: Provenance = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
        // Wire shape spot-check: kebab-case renamed fields.
        assert!(s.contains("\"type\":\"service\""));
        assert!(s.contains("\"service-id\":\"git-watcher\""));
        assert!(s.contains("\"instance-id\""));
    }

    #[test]
    fn kind_label_matches_serde_tag() {
        assert_eq!(ActorIdentity::Core.kind_label(), "core");
        assert_eq!(ActorIdentity::Tui.kind_label(), "tui");
        assert_eq!(
            ActorIdentity::behavior(BehaviorId::new("x")).kind_label(),
            "behavior"
        );
        assert_eq!(
            ActorIdentity::service("s", Uuid::nil())
                .unwrap()
                .kind_label(),
            "service"
        );
    }

    proptest! {
        #[test]
        fn ciborium_round_trip_basic_variants(ts in 0u64..u64::MAX) {
            for src in [
                ActorIdentity::Core,
                ActorIdentity::Tui,
                ActorIdentity::behavior(BehaviorId::new("core/dirty-tracking")),
            ] {
                let p = Provenance::new(src, ts, None).unwrap();
                let mut buf = Vec::new();
                ciborium::into_writer(&p, &mut buf).unwrap();
                let back: Provenance = ciborium::from_reader(buf.as_slice()).unwrap();
                prop_assert_eq!(&p, &back);
            }
        }

        #[test]
        fn any_kebab_identifier_accepted(
            id in "[a-z][a-z0-9]{0,8}(-[a-z0-9]{1,5}){0,3}"
        ) {
            prop_assert!(ActorIdentity::service(id, Uuid::new_v4()).is_ok());
        }
    }
}
