//! T061 — US3 e2e: per-buffer `buffer/observable` vs
//! service-level `watcher/status=degraded` orthogonality.
//!
//! FR-016 + FR-016a: `buffer/observable` is per-buffer and
//! edge-triggered; `watcher/status=degraded` is service-level
//! and fires only when every currently-open buffer is
//! simultaneously unobservable.
//!
//! Scenario (three-process: core + `weaver-buffers` + observer):
//!
//!   1. Bootstrap three tempfiles under one invocation. Wait for
//!      `watcher/status=ready`.
//!   2. Delete file A. Assert `buffer/observable=false` for A's
//!      entity. Across a short grace window, assert
//!      `watcher/status` does NOT transition to `degraded`.
//!   3. Delete files B and C. Assert `watcher/status=degraded`
//!      fires exactly once; every per-buffer transition to
//!      `observable=false` lands for B and C in the process.
//!   4. Restore file A (recreate with its original content).
//!      Assert `buffer/observable=true` for A's entity AND
//!      `watcher/status=ready` re-fires.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_buffers::model::buffer_entity_ref;
use weaver_core::bus::client::Client;
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::{Fact, FactValue};
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: &str = "100ms";
/// Generous bound on each "wait for a specific transition" loop.
/// One poll cycle is 100 ms; we allow up to 50× that for cold-
/// cache CI hosts without asserting a spec budget here (FR-016
/// and FR-016a aren't latency-budgeted in slice 003).
const TRANSITION_WAIT: Duration = Duration::from_secs(5);
/// Grace window after A's `observable=false` lands during which
/// `watcher/status` must NOT transition to `degraded`. Three full
/// poll cycles is enough to catch a spurious aggregation bug.
const NOT_DEGRADED_GRACE: Duration = Duration::from_millis(350);

#[tokio::test]
async fn degraded_observable_per_buffer_vs_service_level() {
    let socket = unique_socket_path();

    build_weaver_binary();
    build_buffer_service_binary();

    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    let fixture_dir = tempdir();
    let path_a = fixture_dir.join("a.txt");
    let path_b = fixture_dir.join("b.txt");
    let path_c = fixture_dir.join("c.txt");
    let content_a = b"alpha\n";
    let content_b = b"beta\n";
    let content_c = b"gamma\n";
    std::fs::write(&path_a, content_a).expect("write A");
    std::fs::write(&path_b, content_b).expect("write B");
    std::fs::write(&path_c, content_c).expect("write C");
    let canon_a = std::fs::canonicalize(&path_a).expect("canonicalise A");
    let canon_b = std::fs::canonicalize(&path_b).expect("canonicalise B");
    let canon_c = std::fs::canonicalize(&path_c).expect("canonicalise C");
    let entity_a = buffer_entity_ref(&canon_a);
    let entity_b = buffer_entity_ref(&canon_b);
    let entity_c = buffer_entity_ref(&canon_c);

    let mut observer = Client::connect(&socket, "e2e-degraded-observer")
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

    wait_for_watcher_status(&mut observer, "ready")
        .await
        .expect("first ready");

    // --- Phase 1: delete A; per-buffer transition only ---

    std::fs::remove_file(&canon_a).expect("delete A");
    wait_for_observable(&mut observer, entity_a, false)
        .await
        .expect("A observable=false after delete");

    // Grace window: service-level status must NOT flip to
    // `degraded` while B and C are still observable.
    let grace_deadline = Instant::now() + NOT_DEGRADED_GRACE;
    while Instant::now() < grace_deadline {
        let remaining = grace_deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        if is_weaver_buffers(&fact)
            && fact.key.attribute == "watcher/status"
            && let FactValue::String(s) = &fact.value
            && s == "degraded"
        {
            panic!(
                "watcher/status=degraded fired while B and C were still observable — \
                 FR-016a aggregation bug"
            );
        }
    }

    // --- Phase 2: delete B and C; service-level transition fires ---

    std::fs::remove_file(&canon_b).expect("delete B");
    std::fs::remove_file(&canon_c).expect("delete C");

    // Await degraded. While waiting, collect per-buffer
    // observable=false transitions for B and C so we can assert
    // both lost observability before the aggregate fired.
    let mut saw_observable_false: std::collections::HashSet<EntityRef> =
        std::collections::HashSet::new();
    saw_observable_false.insert(entity_a); // phase 1 confirmed.
    let degrade_deadline = Instant::now() + TRANSITION_WAIT;
    let mut degraded_landed = false;
    while Instant::now() < degrade_deadline && !degraded_landed {
        let remaining = degrade_deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        if !is_weaver_buffers(&fact) {
            continue;
        }
        match fact.key.attribute.as_str() {
            "buffer/observable" => {
                if let FactValue::Bool(false) = fact.value {
                    saw_observable_false.insert(fact.key.entity);
                }
            }
            "watcher/status" => {
                if let FactValue::String(s) = &fact.value
                    && s == "degraded"
                {
                    degraded_landed = true;
                }
            }
            _ => {}
        }
    }
    assert!(
        degraded_landed,
        "watcher/status=degraded did not fire within {TRANSITION_WAIT:?} \
         after all three files were deleted"
    );
    for e in [entity_a, entity_b, entity_c] {
        assert!(
            saw_observable_false.contains(&e),
            "expected buffer/observable=false for entity {e:?} before aggregate degraded"
        );
    }

    // --- Phase 3: restore A; aggregate recovers, A recovers ---

    std::fs::write(&canon_a, content_a).expect("restore A");

    // Ordering of the two "recovery" facts depends on the poll
    // tick: within one tick the publisher first publishes
    // per-buffer `observable=true`, then (if the aggregate flip
    // applies) `watcher/status=ready`. Track both and pass once
    // we've observed each at least once.
    let mut saw_a_observable_true = false;
    let mut saw_ready_again = false;
    let recovery_deadline = Instant::now() + TRANSITION_WAIT;
    while Instant::now() < recovery_deadline && !(saw_a_observable_true && saw_ready_again) {
        let remaining = recovery_deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        if !is_weaver_buffers(&fact) {
            continue;
        }
        if fact.key.attribute == "buffer/observable"
            && fact.key.entity == entity_a
            && matches!(fact.value, FactValue::Bool(true))
        {
            saw_a_observable_true = true;
        }
        if fact.key.attribute == "watcher/status"
            && let FactValue::String(s) = &fact.value
            && s == "ready"
        {
            saw_ready_again = true;
        }
    }
    assert!(
        saw_a_observable_true,
        "A did not transition buffer/observable=true after restore within {TRANSITION_WAIT:?}"
    );
    assert!(
        saw_ready_again,
        "watcher/status=ready did not re-fire after A was restored within {TRANSITION_WAIT:?}"
    );

    let _ = std::fs::remove_file(&socket);
}

/// Wait until a `watcher/status=<label>` FactAssert from
/// `weaver-buffers` lands. Panic on timeout via the `Err` path.
async fn wait_for_watcher_status(observer: &mut Client, label: &str) -> Result<(), Duration> {
    let deadline = Instant::now() + TRANSITION_WAIT;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        if is_weaver_buffers(&fact)
            && fact.key.attribute == "watcher/status"
            && let FactValue::String(s) = &fact.value
            && s == label
        {
            return Ok(());
        }
    }
    Err(TRANSITION_WAIT)
}

/// Wait until `buffer/observable=<target>` lands for `entity` on
/// the `weaver-buffers` service.
async fn wait_for_observable(
    observer: &mut Client,
    entity: EntityRef,
    target: bool,
) -> Result<(), Duration> {
    let deadline = Instant::now() + TRANSITION_WAIT;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        if is_weaver_buffers(&fact)
            && fact.key.attribute == "buffer/observable"
            && fact.key.entity == entity
            && matches!(fact.value, FactValue::Bool(v) if v == target)
        {
            return Ok(());
        }
    }
    Err(TRANSITION_WAIT)
}

fn is_weaver_buffers(fact: &Fact) -> bool {
    matches!(
        &fact.provenance.source,
        ActorIdentity::Service { service_id, .. } if service_id == "weaver-buffers"
    )
}

// ---- subprocess helpers (mirror buffer_multi_buffer.rs) ----

fn tempdir() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    let p = std::env::temp_dir().join(format!("weaver-buf-degraded-e2e-{pid}-{tick}"));
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
    std::env::temp_dir().join(format!("weaver-buf-degraded-e2e-{pid}-{tick}.sock"))
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
