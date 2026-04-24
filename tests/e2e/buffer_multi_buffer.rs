//! T059 — US3 e2e: multi-buffer within one `weaver-buffers`
//! invocation.
//!
//! Three-process scenario (core + `weaver-buffers` + test
//! subscriber):
//!
//! 1. Spawn core; wait for socket.
//! 2. Create three `tempfile::NamedTempFile` fixtures with distinct
//!    content. Canonicalise each.
//! 3. Subscribe the observer BEFORE spawning `weaver-buffers` so
//!    every bootstrap `FactAssert` is captured on-the-wire.
//! 4. Spawn `weaver-buffers` with all three paths.
//! 5. Collect `buffer/path` facts until each expected entity has
//!    been seen. Assert:
//!      - three distinct entities landed;
//!      - every `buffer/path` fact shares the same
//!        `ActorIdentity::Service { instance_id }` UUID.
//! 6. Mutate fixture B (the middle one). Drain facts until B's
//!    `buffer/dirty=true` arrives; over a poll-interval grace
//!    window, require A and C's `buffer/dirty` to stay `false`.
//!
//! US3 coverage; independence + shared-instance invariant
//! (FR-007 + FR-010 + FR-011a).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};
use uuid::Uuid;

use weaver_buffers::model::buffer_entity_ref;
use weaver_core::bus::client::Client;
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::{Fact, FactValue};
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Publisher poll cadence — lower than production default so the
/// test doesn't idle on 250 ms cycles. Matches the other e2e tests.
const POLL_INTERVAL: &str = "100ms";
/// Hard bound for each "wait for a specific fact" loop. Three
/// bootstraps + one mutation flip should all land comfortably well
/// under this even on cold-cache CI hosts.
const FACT_WAIT_BUDGET: Duration = Duration::from_secs(10);
/// Grace window after B's `dirty=true` lands during which A and C
/// must not flip. Three full poll cycles — enough to catch an
/// accidental common-mutation bug, tight enough to keep the test
/// responsive.
const ISOLATION_GRACE: Duration = Duration::from_millis(300);

#[tokio::test]
async fn buffer_multi_buffer_bootstrap_and_isolated_mutation() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    build_weaver_binary();
    build_buffer_service_binary();

    // Three fixtures with distinct content. Distinct content keeps
    // the per-buffer `memory_digest`s distinct too — a silent
    // cross-wiring bug in entity derivation would produce the same
    // digest across files and surface here.
    let fixture_dir = tempdir();
    let path_a = fixture_dir.join("alpha.txt");
    let path_b = fixture_dir.join("beta.txt");
    let path_c = fixture_dir.join("gamma.txt");
    std::fs::write(&path_a, b"alpha\n").expect("write A");
    std::fs::write(&path_b, b"beta\n").expect("write B");
    std::fs::write(&path_c, b"gamma\n").expect("write C");
    let canon_a = std::fs::canonicalize(&path_a).expect("canonicalise A");
    let canon_b = std::fs::canonicalize(&path_b).expect("canonicalise B");
    let canon_c = std::fs::canonicalize(&path_c).expect("canonicalise C");
    let entity_a = buffer_entity_ref(&canon_a);
    let entity_b = buffer_entity_ref(&canon_b);
    let entity_c = buffer_entity_ref(&canon_c);
    assert!(
        entity_a != entity_b && entity_b != entity_c && entity_a != entity_c,
        "three distinct canonical paths must derive three distinct buffer entities: \
         {entity_a:?} / {entity_b:?} / {entity_c:?}"
    );

    let mut observer = Client::connect(&socket, "e2e-observer")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");

    let _buffers = ChildGuard::new(spawn_buffer_service(
        &socket,
        &[canon_a.clone(), canon_b.clone(), canon_c.clone()],
    ));

    // --- Phase 1: bootstrap landing + shared-instance assertion ---

    let expected_entities = [entity_a, entity_b, entity_c];
    let mut path_facts_by_entity: std::collections::HashMap<EntityRef, Fact> =
        std::collections::HashMap::new();
    let mut dirty_by_entity: std::collections::HashMap<EntityRef, bool> =
        std::collections::HashMap::new();

    let bootstrap_deadline = Instant::now() + FACT_WAIT_BUDGET;
    let bootstrap_incomplete = |paths: &std::collections::HashMap<EntityRef, Fact>,
                                dirty: &std::collections::HashMap<EntityRef, bool>|
     -> bool {
        expected_entities.iter().any(|e| !paths.contains_key(e))
            || expected_entities.iter().any(|e| !dirty.contains_key(e))
    };
    while Instant::now() < bootstrap_deadline
        && bootstrap_incomplete(&path_facts_by_entity, &dirty_by_entity)
    {
        let remaining = bootstrap_deadline.saturating_duration_since(Instant::now());
        let Ok(Ok(msg)) = timeout(remaining, observer.recv()).await else {
            break;
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        if buffer_service_id(&fact) != Some("weaver-buffers") {
            continue;
        }
        let entity = fact.key.entity;
        match fact.key.attribute.as_str() {
            "buffer/path" if expected_entities.contains(&entity) => {
                path_facts_by_entity.entry(entity).or_insert(fact);
            }
            "buffer/dirty" if expected_entities.contains(&entity) => {
                if let FactValue::Bool(b) = fact.value {
                    dirty_by_entity.insert(entity, b);
                }
            }
            _ => {}
        }
    }

    for e in &expected_entities {
        assert!(
            path_facts_by_entity.contains_key(e),
            "bootstrap did not deliver buffer/path for entity {e:?} within {FACT_WAIT_BUDGET:?}"
        );
    }

    let instance_ids: std::collections::HashSet<Uuid> = path_facts_by_entity
        .values()
        .filter_map(instance_id_of)
        .collect();
    assert_eq!(
        instance_ids.len(),
        1,
        "all three buffer/path facts must share one asserting_instance; got {instance_ids:?}"
    );

    // Three distinct entities actually came through (redundant with
    // the HashMap keys but explicit about the FR-007 invariant).
    let distinct_entities: std::collections::HashSet<EntityRef> =
        path_facts_by_entity.keys().copied().collect();
    assert_eq!(
        distinct_entities.len(),
        3,
        "expected three distinct buffer entities on the wire, got {distinct_entities:?}"
    );

    // Pre-mutation sanity: every buffer reported dirty=false in its
    // bootstrap (edge-trigger baseline for the mutation phase).
    for e in &expected_entities {
        assert_eq!(
            dirty_by_entity.get(e).copied(),
            Some(false),
            "bootstrap did not deliver buffer/dirty=false for entity {e:?}"
        );
    }

    // --- Phase 2: mutate B; A and C must stay clean ---

    std::fs::write(&canon_b, b"beta mutated\n").expect("mutate B");
    let mutation_start = Instant::now();

    let mutation_deadline = mutation_start + FACT_WAIT_BUDGET;
    let mut saw_b_dirty = false;
    while Instant::now() < mutation_deadline {
        let remaining = mutation_deadline.saturating_duration_since(Instant::now());
        let Ok(Ok(msg)) = timeout(remaining, observer.recv()).await else {
            break;
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        if buffer_service_id(&fact) != Some("weaver-buffers")
            || fact.key.attribute != "buffer/dirty"
        {
            continue;
        }
        let FactValue::Bool(new_dirty) = fact.value else {
            continue;
        };
        let entity = fact.key.entity;
        if entity == entity_a || entity == entity_c {
            assert!(
                !new_dirty,
                "expected only B's buffer/dirty to flip after mutation, \
                 but {entity:?} flipped to true"
            );
            continue;
        }
        if entity == entity_b && new_dirty {
            eprintln!(
                "[t059] B's buffer/dirty false→true in {:?}",
                mutation_start.elapsed()
            );
            saw_b_dirty = true;
            break;
        }
    }
    assert!(
        saw_b_dirty,
        "B's buffer/dirty=true was not observed within {FACT_WAIT_BUDGET:?}"
    );

    // Grace window after the flip: keep draining the bus so any
    // spurious A/C flip would fire the same assert above. A genuine
    // "only B flipped" run sees nothing here and exits quietly.
    let grace_deadline = Instant::now() + ISOLATION_GRACE;
    while Instant::now() < grace_deadline {
        let remaining = grace_deadline.saturating_duration_since(Instant::now());
        let Ok(Ok(msg)) = timeout(remaining, observer.recv()).await else {
            break;
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        if buffer_service_id(&fact) != Some("weaver-buffers")
            || fact.key.attribute != "buffer/dirty"
        {
            continue;
        }
        let FactValue::Bool(new_dirty) = fact.value else {
            continue;
        };
        let entity = fact.key.entity;
        if (entity == entity_a || entity == entity_c) && new_dirty {
            panic!(
                "during isolation grace window, {entity:?} flipped to dirty=true — \
                 multi-buffer independence violated"
            );
        }
    }
}

fn buffer_service_id(fact: &Fact) -> Option<&str> {
    match &fact.provenance.source {
        ActorIdentity::Service { service_id, .. } => Some(service_id.as_str()),
        _ => None,
    }
}

fn instance_id_of(fact: &Fact) -> Option<Uuid> {
    match &fact.provenance.source {
        ActorIdentity::Service { instance_id, .. } => Some(*instance_id),
        _ => None,
    }
}

// ---- subprocess helpers (mirror buffer_external_mutation.rs) ----

fn tempdir() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    let p = std::env::temp_dir().join(format!("weaver-buf-multi-e2e-{pid}-{tick}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn build_weaver_binary() -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "--quiet", "-p", "weaver_core", "--bin", "weaver"])
        .status()
        .expect("cargo build weaver");
    assert!(status.success());
    bin_path("weaver")
}

fn build_buffer_service_binary() -> PathBuf {
    let status = Command::new("cargo")
        .args([
            "build",
            "--quiet",
            "-p",
            "weaver-buffers",
            "--bin",
            "weaver-buffers",
        ])
        .status()
        .expect("cargo build weaver-buffers");
    assert!(status.success());
    bin_path("weaver-buffers")
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

fn spawn_buffer_service(socket: &Path, paths: &[PathBuf]) -> std::process::Child {
    let bin = build_buffer_service_binary();
    let mut cmd = Command::new(&bin);
    for p in paths {
        cmd.arg(p);
    }
    cmd.arg("--socket")
        .arg(socket)
        .arg(format!("--poll-interval={POLL_INTERVAL}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn weaver-buffers")
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
    std::env::temp_dir().join(format!("weaver-buf-multi-e2e-{pid}-{tick}.sock"))
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
