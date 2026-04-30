//! T024 — slice-004 SC-404 e2e: stale-version dispatches drop silently.
//!
//! Two emitters racing to dispatch `BufferEdit { version: 0, .. }`
//! against the same opened buffer: one wins (publisher applies, bumps
//! `buffer/version` to 1, re-emits the three derived facts); the other
//! is dropped at the version-handshake gate as a stale dispatch (the
//! publisher logs `reason="stale-version"` at debug level and emits no
//! facts).
//!
//! The race is forced deterministically via `tokio::sync::Barrier`:
//! both emitters complete their `InspectRequest` round-trip and arrive
//! at the barrier with `version=0` in hand BEFORE either dispatches.
//!
//! **Deviation from the spec wording**: T024's spec text says "Spawn
//! two `weaver edit` processes". The production CLI offers no hook
//! between its inspect-lookup and dispatch steps, so a deterministic
//! barrier-coordinated race is impossible across two subprocesses
//! without modifying the CLI for tests. Two in-process bus clients
//! exercise the same SERVICE-side stale-drop contract — the CLI's
//! end-to-end wrapping is already covered by T015 + T016 + T018 +
//! T022 + T023. The structural property pinned here is the
//! publisher's version-handshake rejection of a duplicate-version
//! dispatch.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::Barrier;
use tokio::time::{sleep, timeout};

use weaver_buffers::model::buffer_entity_ref;
use weaver_core::bus::client::Client;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::edit::{Position, Range, TextEdit};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::EventId;
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const COLLECT_BUDGET: Duration = Duration::from_secs(5);

#[tokio::test]
async fn racing_emitters_drop_the_stale_dispatch() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    build_buffer_service_binary();

    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("fixture.txt");
    std::fs::write(&fixture_path, b"world").unwrap();
    let canonical = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");
    let entity = buffer_entity_ref(&canonical);

    // Capture weaver-buffers stderr to a tempfile so we can grep for
    // the stale-drop debug line after the service exits. The
    // `RUST_LOG=weaver_buffers=debug` filter pulls in the
    // `tracing::debug!(reason="stale-version", ..)` line emitted by
    // the publisher's reader-loop arm on the duplicate-version event.
    let stderr_path = fixture_dir.join("weaver-buffers.stderr");
    let stderr_file = std::fs::File::create(&stderr_path).expect("create stderr capture");

    let buffers_child = spawn_buffer_service_with_debug_log(&socket, &canonical, stderr_file);
    let buffers_guard = ChildGuard::new(buffers_child);

    let mut observer = Client::connect(&socket, "e2e-observer")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");
    drain_until_buffers_ready(&mut observer).await;

    // Two emitter tasks — A and B — each connect, run an
    // InspectRequest, wait at the barrier, then dispatch a
    // `BufferEdit { version: 0, .. }`.
    let barrier = Arc::new(Barrier::new(2));
    let socket_a = socket.clone();
    let socket_b = socket.clone();
    let barrier_a = barrier.clone();
    let barrier_b = barrier.clone();
    let event_a = build_buffer_edit_event(entity, 0, "A");
    let event_b = build_buffer_edit_event(entity, 0, "B");

    let dispatch_a = tokio::spawn(async move {
        emitter_round_trip(&socket_a, "e2e-emitter-a", entity, &barrier_a, event_a).await
    });
    let dispatch_b = tokio::spawn(async move {
        emitter_round_trip(&socket_b, "e2e-emitter-b", entity, &barrier_b, event_b).await
    });
    let _ = dispatch_a.await.expect("emitter A");
    let _ = dispatch_b.await.expect("emitter B");

    // Drain the observer for a structural budget. Exactly ONE
    // `buffer/version=1` should land (the winner's re-emission burst);
    // the loser's dispatch is dropped at the publisher and emits no
    // facts.
    let mut version_one_count = 0;
    let mut byte_size_post_count = 0;
    let mut dirty_true_post_count = 0;
    let bootstrap_size = 5u64; // "world".len()
    let deadline = Instant::now() + COLLECT_BUDGET;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        let Some(service_id) = buffer_service_id(&fact) else {
            continue;
        };
        if service_id != "weaver-buffers" {
            continue;
        }
        match fact.key.attribute.as_str() {
            "buffer/version" => {
                if let FactValue::U64(1) = fact.value {
                    version_one_count += 1;
                }
            }
            "buffer/byte-size" => {
                if let FactValue::U64(n) = fact.value
                    && n != bootstrap_size
                {
                    byte_size_post_count += 1;
                }
            }
            "buffer/dirty" => {
                if let FactValue::Bool(true) = fact.value {
                    dirty_true_post_count += 1;
                }
            }
            _ => {}
        }
    }

    assert_eq!(
        version_one_count, 1,
        "expected exactly 1 buffer/version=1 (winner re-emit, loser dropped); got {version_one_count}",
    );
    assert_eq!(
        byte_size_post_count, 1,
        "expected exactly 1 post-bootstrap buffer/byte-size update; got {byte_size_post_count}",
    );
    assert_eq!(
        dirty_true_post_count, 1,
        "expected exactly 1 buffer/dirty=true (winner only); got {dirty_true_post_count}",
    );

    // Trigger graceful service shutdown so the trace-buffered stderr
    // flushes to disk. ChildGuard's Drop sends SIGTERM and waits up to
    // 100 ms for cleanup; we wait inline here so we control timing.
    drop(buffers_guard);
    sleep(Duration::from_millis(200)).await;

    // Inspect the captured stderr for the stale-drop debug line.
    let mut stderr_buf = String::new();
    let mut f = std::fs::File::open(&stderr_path).expect("open stderr capture");
    f.read_to_string(&mut stderr_buf).expect("read stderr");

    assert!(
        stderr_buf.contains(r#"reason="stale-version""#),
        "expected `reason=\"stale-version\"` in weaver-buffers stderr; got:\n{stderr_buf}",
    );
    assert!(
        stderr_buf.contains("emitted_version=0"),
        "expected `emitted_version=0` in stale-drop debug line; got:\n{stderr_buf}",
    );
    assert!(
        stderr_buf.contains("current_version=1"),
        "expected `current_version=1` in stale-drop debug line; got:\n{stderr_buf}",
    );
}

/// One emitter's connect → inspect-roundtrip → barrier → dispatch
/// flow. Returns when the dispatch frame is on the wire (the kernel
/// flushes the queued event when the connection closes).
async fn emitter_round_trip(
    socket: &Path,
    client_kind: &str,
    entity: EntityRef,
    barrier: &Arc<Barrier>,
    event: Event,
) -> Result<(), String> {
    let mut client = Client::connect(socket, client_kind)
        .await
        .map_err(|e| format!("{client_kind} connect: {e}"))?;
    let request_id = 42;
    client
        .send(&BusMessage::InspectRequest {
            request_id,
            fact: FactKey::new(entity, "buffer/version"),
        })
        .await
        .map_err(|e| format!("{client_kind} inspect-request: {e}"))?;
    // Drain until the matching InspectResponse arrives.
    loop {
        match client
            .recv()
            .await
            .map_err(|e| format!("{client_kind} recv: {e}"))?
        {
            BusMessage::InspectResponse {
                request_id: rid, ..
            } if rid == request_id => break,
            // Drop spurious frames defensively (mirrors the CLI's
            // own emitter loop in `cli/edit.rs`).
            _ => continue,
        }
    }
    // Both emitters arrive at the barrier with `version=0` in hand.
    barrier.wait().await;
    client
        .send(&BusMessage::Event(event))
        .await
        .map_err(|e| format!("{client_kind} dispatch: {e}"))?;
    Ok(())
}

fn build_buffer_edit_event(entity: EntityRef, version: u64, marker: &str) -> Event {
    let now = now_ns();
    let provenance = Provenance::new(ActorIdentity::User, now, None)
        .expect("ActorIdentity::User has no fields to validate");
    // Each emitter's payload is structurally distinct so a defective
    // service that double-applies (instead of stale-dropping) would
    // expose itself via two distinct buffer/byte-size values rather
    // than the assertion-friendly "exactly one re-emission" shape.
    let new_text = format!("{marker}-");
    let edit = TextEdit {
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
        new_text,
    };
    Event {
        id: EventId::new(uuid::Uuid::from_u128(now as u128)),
        name: "buffer/edit".into(),
        target: Some(entity),
        payload: EventPayload::BufferEdit {
            entity,
            version,
            edits: vec![edit],
        },
        provenance,
    }
}

// ───────────────────────────────────────────────────────────────────
// helpers
// ───────────────────────────────────────────────────────────────────

async fn drain_until_buffers_ready(observer: &mut Client) {
    let deadline = Instant::now() + COLLECT_BUDGET;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        let Some(service_id) = buffer_service_id(&fact) else {
            continue;
        };
        if service_id == "weaver-buffers"
            && fact.key.attribute == "watcher/status"
            && let FactValue::String(s) = &fact.value
            && s == "ready"
        {
            return;
        }
    }
    panic!("weaver-buffers did not reach watcher/status=ready within budget");
}

fn buffer_service_id(fact: &Fact) -> Option<&str> {
    match &fact.provenance.source {
        ActorIdentity::Service { service_id, .. } => Some(service_id.as_str()),
        _ => None,
    }
}

fn tempdir() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    let p = std::env::temp_dir().join(format!("weaver-edit-stale-e2e-{pid}-{tick}"));
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

fn spawn_buffer_service_with_debug_log(
    socket: &Path,
    path: &Path,
    stderr: std::fs::File,
) -> std::process::Child {
    let bin = build_buffer_service_binary();
    Command::new(&bin)
        .arg(path)
        .arg("--socket")
        .arg(socket)
        .arg("--poll-interval=100ms")
        .env("RUST_LOG", "weaver_buffers=debug")
        // tracing-subscriber's default formatter colorises tty output;
        // when stderr is piped to a file it usually drops colors, but
        // some CI shells preserve TERM, so force-disable ANSI to keep
        // the captured text grep-friendly.
        .env("NO_COLOR", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
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
    std::env::temp_dir().join(format!("weaver-edit-stale-e2e-{pid}-{tick}.sock"))
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
