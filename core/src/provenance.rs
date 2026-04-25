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
/// - `Behavior { id }` → `{"type":"behavior","id":"<behavior-id>"}`
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

    /// A human user.
    ///
    /// Slice 002 reserved this as a struct variant carrying an opaque
    /// `UserId` for forward-compatibility. Slice 004 — the first
    /// production user (CLI emitter for `weaver edit` / `weaver edit-json`)
    /// — finalised it as a unit variant: a single-process local editor
    /// has no need to attribute edits across multiple users, and the
    /// timestamp distinguishes successive User-emitted events. If a
    /// future slice introduces multi-user attribution, it can land as
    /// a richer variant under another protocol bump.
    User,

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

    #[error(
        "{field} must be non-empty — wire-derived identity payloads are \
         validated uniformly so malformed frames cannot poison \
         trace/inspection output (F19 review fix)"
    )]
    EmptyIdentityField { field: &'static str },
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

    /// Short, human-readable label for diagnostic rendering.
    /// **Not** a wire format — wire serialization uses serde.
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Behavior { .. } => "behavior",
            Self::Tui => "tui",
            Self::Service { .. } => "service",
            Self::User => "user",
            Self::Host { .. } => "host",
            Self::Agent { .. } => "agent",
        }
    }

    /// Diagnostic rendering that *identifies* the actor, not just its
    /// kind. Callers that want to display "who" in an operator-facing
    /// error (e.g. authority-conflict detail) should use this over
    /// [`Self::kind_label`], which would otherwise elide the
    /// identifier and leave an operator with no way to distinguish
    /// one service instance from another.
    ///
    /// The format per variant:
    ///
    /// - `Core` / `Tui` / `User` — just the kind label (no identifier
    ///   carried; `User` was finalised as a unit variant in slice 004).
    /// - `Behavior` — `"behavior <id>"`.
    /// - `Service` — `"service <service-id> (inst <8-hex>)"`, matching
    ///   the TUI rendering in `tui/src/render.rs`.
    /// - `Host` / `Agent` — `"<kind> <id>"` using the variant's primary
    ///   identifier.
    ///
    /// **Not** a wire format — use serde for serialization. Purely a
    /// diagnostic surface; the exact string shape may tighten in
    /// future slices if an operator-facing tool starts parsing it.
    pub fn identifying_label(&self) -> String {
        match self {
            Self::Core => "core".into(),
            Self::Tui => "tui".into(),
            Self::User => "user".into(),
            Self::Behavior { id } => format!("behavior {id}"),
            Self::Service {
                service_id,
                instance_id,
            } => {
                let hyphenated = instance_id.as_hyphenated().to_string();
                let short = hyphenated.get(..8).unwrap_or(hyphenated.as_str());
                format!("service {service_id} (inst {short})")
            }
            Self::Host { host_id, .. } => format!("host {host_id}"),
            Self::Agent { agent_id, .. } => format!("agent {agent_id}"),
        }
    }

    /// Validate any structural invariants an identity variant carries.
    ///
    /// Serde's derived `Deserialize` bypasses the per-variant
    /// constructors ([`ActorIdentity::service`] et al.), so a wire
    /// frame can carry a `Service` whose `service_id` is empty or
    /// non-kebab-case. This method re-checks those invariants and
    /// is the single place callers use to guard wire-derived
    /// identities (F12 review fix). [`Provenance::new`] also routes
    /// through here so in-process construction is safe too.
    ///
    /// F19 review fix: every identity-carrying variant gets a
    /// non-empty check on its payload fields. The reserved
    /// variants (`Host`, `Agent`) aren't emitted this slice, but
    /// [`validate`] is now the single wire gate — so malformed frames
    /// carrying empty strings must not slip through into
    /// trace/inspection. Kebab-case validation stays scoped to
    /// `Service` since that's the only variant whose identifier
    /// vocabulary the slice commits to. Nested identities
    /// (`Agent::on_behalf_of`) are recursively validated so a chain
    /// with a malformed delegator is caught at the top.
    ///
    /// Slice 004 finalised `User` as a unit variant; it carries no
    /// payload to validate.
    pub fn validate(&self) -> Result<(), ProvenanceError> {
        match self {
            Self::Service { service_id, .. } => validate_kebab_case(service_id),
            Self::Behavior { id } => {
                // F24 review fix: this branch used to fall through
                // as payload-less, letting a wire-derived
                // `Behavior { id: "" }` (or any string) slip into
                // trace/inspection. Non-empty is the wire-edge
                // floor here; stricter family/name vocabulary
                // rules belong inside `BehaviorId` itself and are
                // a follow-up for a later slice.
                if id.as_str().is_empty() {
                    Err(ProvenanceError::EmptyIdentityField {
                        field: "behavior-id",
                    })
                } else {
                    Ok(())
                }
            }
            Self::Host {
                host_id,
                hosted_origin,
            } => {
                if host_id.is_empty() {
                    return Err(ProvenanceError::EmptyIdentityField { field: "host-id" });
                }
                if hosted_origin.file.is_empty() {
                    return Err(ProvenanceError::EmptyIdentityField {
                        field: "hosted-origin.file",
                    });
                }
                if hosted_origin.runtime_version.is_empty() {
                    return Err(ProvenanceError::EmptyIdentityField {
                        field: "hosted-origin.runtime-version",
                    });
                }
                Ok(())
            }
            Self::Agent {
                agent_id,
                on_behalf_of,
            } => {
                if agent_id.is_empty() {
                    return Err(ProvenanceError::EmptyIdentityField { field: "agent-id" });
                }
                if let Some(inner) = on_behalf_of {
                    inner.validate()?;
                }
                Ok(())
            }
            // Payload-less variants have no further invariants to
            // check beyond their type-level structure.
            Self::Core | Self::Tui | Self::User => Ok(()),
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
    /// Construct a `Provenance` after re-validating the actor
    /// identity's per-variant invariants (F12 review fix).
    ///
    /// Wire deserialization bypasses [`ActorIdentity::service`]'s
    /// kebab-case check, so a caller that rebuilds a `Provenance`
    /// around a deserialized identity here is the last checkpoint
    /// before the value reaches the trace / fact store / authority
    /// map. Listener-side bus handlers additionally call
    /// [`ActorIdentity::validate`] directly on inbound fact frames
    /// (defense in depth).
    pub fn new(
        source: ActorIdentity,
        timestamp_ns: u64,
        causal_parent: Option<EventId>,
    ) -> Result<Self, ProvenanceError> {
        source.validate()?;
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
    fn validate_rejects_empty_behavior_id() {
        let id = ActorIdentity::behavior(BehaviorId::new(""));
        assert_eq!(
            id.validate().unwrap_err(),
            ProvenanceError::EmptyIdentityField {
                field: "behavior-id"
            }
        );
    }

    #[test]
    fn validate_rejects_empty_host_id() {
        let id = ActorIdentity::Host {
            host_id: "".into(),
            hosted_origin: HostedOrigin {
                file: "script.lua".into(),
                location: None,
                runtime_version: "5.4.7".into(),
            },
        };
        assert_eq!(
            id.validate().unwrap_err(),
            ProvenanceError::EmptyIdentityField { field: "host-id" }
        );
    }

    #[test]
    fn validate_rejects_empty_hosted_origin_file() {
        let id = ActorIdentity::Host {
            host_id: "lua".into(),
            hosted_origin: HostedOrigin {
                file: "".into(),
                location: None,
                runtime_version: "5.4.7".into(),
            },
        };
        assert_eq!(
            id.validate().unwrap_err(),
            ProvenanceError::EmptyIdentityField {
                field: "hosted-origin.file"
            }
        );
    }

    #[test]
    fn validate_rejects_empty_hosted_runtime_version() {
        let id = ActorIdentity::Host {
            host_id: "lua".into(),
            hosted_origin: HostedOrigin {
                file: "script.lua".into(),
                location: None,
                runtime_version: "".into(),
            },
        };
        assert_eq!(
            id.validate().unwrap_err(),
            ProvenanceError::EmptyIdentityField {
                field: "hosted-origin.runtime-version"
            }
        );
    }

    #[test]
    fn validate_rejects_empty_agent_id() {
        let id = ActorIdentity::Agent {
            agent_id: "".into(),
            on_behalf_of: None,
        };
        assert_eq!(
            id.validate().unwrap_err(),
            ProvenanceError::EmptyIdentityField { field: "agent-id" }
        );
    }

    #[test]
    fn validate_recurses_into_agent_on_behalf_of() {
        // Outer agent is well-formed; the delegator carries an
        // empty service_id that should trip validation.
        let malformed_delegator = ActorIdentity::Service {
            service_id: "".into(),
            instance_id: Uuid::new_v4(),
        };
        let id = ActorIdentity::Agent {
            agent_id: "researcher".into(),
            on_behalf_of: Some(Box::new(malformed_delegator)),
        };
        assert_eq!(id.validate().unwrap_err(), ProvenanceError::EmptyServiceId);
    }

    #[test]
    fn validate_accepts_well_formed_reserved_variants() {
        // User is unit since slice 004 — no payload to validate.
        assert!(ActorIdentity::User.validate().is_ok());
        let host = ActorIdentity::Host {
            host_id: "lua".into(),
            hosted_origin: HostedOrigin {
                file: "script.lua".into(),
                location: Some("line 7".into()),
                runtime_version: "5.4.7".into(),
            },
        };
        assert!(host.validate().is_ok());
        let agent = ActorIdentity::Agent {
            agent_id: "researcher".into(),
            on_behalf_of: Some(Box::new(ActorIdentity::User)),
        };
        assert!(agent.validate().is_ok());
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
        assert_eq!(ActorIdentity::User.kind_label(), "user");
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

    /// Slice 004 contract: `User` is a unit variant on the wire.
    /// Pin the JSON shape so a future refactor can't quietly add a
    /// payload field.
    #[test]
    fn user_unit_variant_json_wire_shape() {
        let s = serde_json::to_string(&ActorIdentity::User).unwrap();
        assert_eq!(s, r#"{"type":"user"}"#);
        let back: ActorIdentity = serde_json::from_str(&s).unwrap();
        assert_eq!(back, ActorIdentity::User);
    }

    proptest! {
        #[test]
        fn ciborium_round_trip_basic_variants(ts in 0u64..u64::MAX) {
            for src in [
                ActorIdentity::Core,
                ActorIdentity::Tui,
                ActorIdentity::User,
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
