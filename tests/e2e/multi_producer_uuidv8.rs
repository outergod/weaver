//! T026 — slice-005 SC-505 e2e: multi-producer UUIDv8 prefix-uniqueness
//! under stress.
//!
//! Two-process scenario (core + this test process). Spawns a single
//! `weaver run` core; this process opens four bus connections (3
//! producers + 1 observer) and emits 3000 events total — 1000 per
//! producer — then asserts cross-producer collision-freedom +
//! prefix-namespace partitioning.
//!
//! The §28(a) re-derivation (slice 005 session 1/2) closes the
//! cross-producer wall-clock-ns collision class structurally by
//! placing each producer's 58-bit hashed instance-id in the high
//! bits of every UUIDv8 it mints. This test pins that invariant
//! end-to-end through the listener's accept-and-broadcast path.
//!
//! Per-producer time_or_counter bits use a monotonic 0..1000 counter
//! rather than `now_ns()` so within-producer uniqueness is guaranteed
//! by construction (avoiding sub-nanosecond clock-granularity
//! collisions on fast hardware).
//!
//! Single test function:
//!
//! [`three_producers_emit_3000_events_with_unique_uuidv8_ids`] —
//!  Producer A (Service "producer-a"): 1000 BufferEdit on entity 1.
//!  Producer B (Service "producer-b"): 1000 BufferSave on entity 2.
//!  Producer C (User):                 1000 BufferOpen with unique paths.
//!  Asserts: 3000 unique EventIds + each event's prefix matches its
//!  producer's expected prefix + provenance.source equals the
//!  originating producer's ActorIdentity.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};
use uuid::Uuid;

use weaver_core::bus::client::Client;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::edit::{Position, Range, TextEdit};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::ids::{EventId, hash_to_58};
use weaver_core::types::message::{BusMessage, EventSubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const COLLECT_BUDGET: Duration = Duration::from_secs(30);
const PER_PRODUCER_EVENTS: u64 = 1_000;
const TOTAL_EVENTS: usize = 3 * PER_PRODUCER_EVENTS as usize;

#[tokio::test]
async fn three_producers_emit_3000_events_with_unique_uuidv8_ids() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    // Producer identities. Service producers carry a UUIDv4
    // `instance_id`; the User producer is the unit variant whose
    // prefix is derived from a per-process UUIDv4 stand-in.
    let instance_id_a = Uuid::new_v4();
    let instance_id_b = Uuid::new_v4();
    let user_uuid_c = Uuid::new_v4();
    let prefix_a = hash_to_58(&instance_id_a);
    let prefix_b = hash_to_58(&instance_id_b);
    let prefix_c = hash_to_58(&user_uuid_c);
    assert_ne!(prefix_a, prefix_b, "test setup: distinct producer prefixes");
    assert_ne!(prefix_a, prefix_c, "test setup: distinct producer prefixes");
    assert_ne!(prefix_b, prefix_c, "test setup: distinct producer prefixes");

    let actor_a = ActorIdentity::Service {
        service_id: "producer-a".into(),
        instance_id: instance_id_a,
    };
    let actor_b = ActorIdentity::Service {
        service_id: "producer-b".into(),
        instance_id: instance_id_b,
    };
    let actor_c = ActorIdentity::User;

    let entity_a = EntityRef::new(1);
    let entity_b = EntityRef::new(2);

    // Subscribe before any producer emits, so the listener's
    // broadcast path is already wired.
    let mut observer = Client::connect(&socket, "e2e-observer")
        .await
        .expect("observer connect");
    observer
        .subscribe_events(EventSubscribePattern::PayloadTypes(vec![
            "buffer-edit".into(),
            "buffer-save".into(),
            "buffer-open".into(),
        ]))
        .await
        .expect("subscribe events");

    // Emit producers concurrently. Each producer opens its own bus
    // client so the listener's per-connection state is exercised.
    let socket_a = socket.clone();
    let actor_a_clone = actor_a.clone();
    let task_a = tokio::spawn(async move {
        let mut client = Client::connect(&socket_a, "producer-a")
            .await
            .expect("producer-a connect");
        for i in 0..PER_PRODUCER_EVENTS {
            let event = Event {
                id: EventId::mint_v8(prefix_a, i),
                name: "buffer/edit".into(),
                target: Some(entity_a),
                payload: EventPayload::BufferEdit {
                    entity: entity_a,
                    version: i,
                    edits: vec![TextEdit {
                        range: Range {
                            start: Position {
                                line: 0,
                                character: 0,
                            },
                            end: Position {
                                line: 0,
                                character: 0,
                            },
                        },
                        new_text: "x".into(),
                    }],
                },
                provenance: Provenance::new(actor_a_clone.clone(), now_ns(), None)
                    .expect("provenance a"),
            };
            client
                .send(&BusMessage::Event(event))
                .await
                .expect("producer-a send");
        }
    });

    let socket_b = socket.clone();
    let actor_b_clone = actor_b.clone();
    let task_b = tokio::spawn(async move {
        let mut client = Client::connect(&socket_b, "producer-b")
            .await
            .expect("producer-b connect");
        for i in 0..PER_PRODUCER_EVENTS {
            let event = Event {
                id: EventId::mint_v8(prefix_b, i),
                name: "buffer/save".into(),
                target: Some(entity_b),
                payload: EventPayload::BufferSave {
                    entity: entity_b,
                    version: i,
                },
                provenance: Provenance::new(actor_b_clone.clone(), now_ns(), None)
                    .expect("provenance b"),
            };
            client
                .send(&BusMessage::Event(event))
                .await
                .expect("producer-b send");
        }
    });

    let socket_c = socket.clone();
    let actor_c_clone = actor_c.clone();
    let task_c = tokio::spawn(async move {
        let mut client = Client::connect(&socket_c, "producer-c")
            .await
            .expect("producer-c connect");
        for i in 0..PER_PRODUCER_EVENTS {
            let event = Event {
                id: EventId::mint_v8(prefix_c, i),
                name: "buffer/open".into(),
                target: None,
                payload: EventPayload::BufferOpen {
                    path: format!("/tmp/weaver-stress-fixture-{i}.txt"),
                },
                provenance: Provenance::new(actor_c_clone.clone(), now_ns(), None)
                    .expect("provenance c"),
            };
            client
                .send(&BusMessage::Event(event))
                .await
                .expect("producer-c send");
        }
    });

    let (a, b, c) = tokio::join!(task_a, task_b, task_c);
    a.expect("task-a join");
    b.expect("task-b join");
    c.expect("task-c join");

    // Drain accepted events. Sender → broadcast is fanout from the
    // listener's accept-and-broadcast loop; the unbounded mpsc
    // pulled into the observer's socket lets the observer drain at
    // its own pace.
    let mut by_id: HashMap<EventId, Event> = HashMap::with_capacity(TOTAL_EVENTS);
    let mut prefix_counts: HashMap<u64, usize> = HashMap::new();
    let deadline = Instant::now() + COLLECT_BUDGET;
    while by_id.len() < TOTAL_EVENTS && Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        let BusMessage::Event(event) = msg else {
            continue;
        };
        let prefix = event.id.extract_prefix();
        *prefix_counts.entry(prefix).or_insert(0) += 1;
        let prev = by_id.insert(event.id, event.clone());
        assert!(
            prev.is_none(),
            "duplicate EventId observed: {:?} arrived twice (provenance: {:?})",
            event.id,
            event.provenance.source,
        );
    }

    assert_eq!(
        by_id.len(),
        TOTAL_EVENTS,
        "expected {TOTAL_EVENTS} unique events; observed {} (deadline reached)",
        by_id.len(),
    );

    // Each producer's 1000 events all under its own prefix; no cross-
    // producer leakage. Validates the §28(a) invariant: under
    // well-behaved producers, EventId prefix partitions the producer
    // namespace.
    assert_eq!(
        prefix_counts.get(&prefix_a),
        Some(&(PER_PRODUCER_EVENTS as usize)),
        "producer-a prefix count mismatch: {prefix_counts:?}",
    );
    assert_eq!(
        prefix_counts.get(&prefix_b),
        Some(&(PER_PRODUCER_EVENTS as usize)),
        "producer-b prefix count mismatch: {prefix_counts:?}",
    );
    assert_eq!(
        prefix_counts.get(&prefix_c),
        Some(&(PER_PRODUCER_EVENTS as usize)),
        "producer-c prefix count mismatch: {prefix_counts:?}",
    );
    assert_eq!(
        prefix_counts.len(),
        3,
        "expected exactly 3 distinct prefixes; got {prefix_counts:?}",
    );

    // Provenance source matches the producer for each event — the
    // unique-prefix → unique-producer mapping.
    for event in by_id.values() {
        let prefix = event.id.extract_prefix();
        let expected = if prefix == prefix_a {
            &actor_a
        } else if prefix == prefix_b {
            &actor_b
        } else if prefix == prefix_c {
            &actor_c
        } else {
            panic!("unexpected prefix {prefix} for event {:?}", event.id);
        };
        assert_eq!(
            &event.provenance.source, expected,
            "event {:?} prefix {prefix} maps to wrong producer; provenance.source={:?}",
            event.id, event.provenance.source,
        );
    }
}

// ───────────────────────────────────────────────────────────────────
// helpers
// ───────────────────────────────────────────────────────────────────

fn build_weaver_binary() -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "--quiet", "-p", "weaver_core", "--bin", "weaver"])
        .status()
        .expect("cargo build weaver");
    assert!(status.success());
    bin_path("weaver")
}

fn bin_path(name: &str) -> PathBuf {
    let target = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target")
        });
    target.join("debug").join(name)
}

fn spawn_core(socket: &Path) -> std::process::Child {
    let bin = build_weaver_binary();
    Command::new(&bin)
        .arg("run")
        .arg("--socket")
        .arg(socket)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn weaver")
}

async fn wait_for_socket(socket: &Path) {
    let start = Instant::now();
    while !socket.exists() {
        if start.elapsed() > SOCKET_WAIT_TIMEOUT {
            panic!("weaver socket did not appear within {SOCKET_WAIT_TIMEOUT:?}");
        }
        sleep(Duration::from_millis(20)).await;
    }
    sleep(Duration::from_millis(50)).await;
}

fn unique_socket_path() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    std::env::temp_dir().join(format!("weaver-multi-producer-e2e-{pid}-{tick}.sock"))
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

struct ChildGuard {
    child: Option<std::process::Child>,
}

impl ChildGuard {
    fn new(child: std::process::Child) -> Self {
        Self { child: Some(child) }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = nix_signal(child.id(), libc::SIGTERM);
            std::thread::sleep(Duration::from_millis(100));
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn nix_signal(pid: u32, sig: libc::c_int) -> std::io::Result<()> {
    let rc = unsafe { libc::kill(pid as libc::pid_t, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
