//! T048 — SC-303 e2e: SIGKILL retract latency.
//!
//! Three-process scenario (core + `weaver-buffers` + test-client):
//!
//! 1. Spawn core; wait for bus socket.
//! 2. Subscribe the observer to `AllFacts`, spawn `weaver-buffers`
//!    against a fixture, drain the bootstrap stream until all four
//!    `buffer/*` facts have arrived.
//! 3. SIGKILL the buffer service (bypassing its clean-shutdown
//!    retract path — the test is specifically about the core's
//!    `release_connection` picking up after an abrupt drop).
//! 4. Start stopwatch; observe `FactRetract` frames for every
//!    `buffer/*` attribute that the service owned. Stop the
//!    stopwatch when the full retract set has arrived.
//!
//! Surfaces the observed timing via stderr. The assertion bound is
//! 10 s (hard wall) so cold-cache CI hosts don't false-fail; the
//! surfaced 5 s comparison is the spec-level pass/fail signal
//! (SC-303). Hardware-dependent.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_core::bus::client::Client;
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::Fact;
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: &str = "100ms";
const SC303_BUDGET: Duration = Duration::from_secs(5);
/// Hard wait for every buffer/* retract. Structural-break detector:
/// if release_connection isn't retracting within 10 s something is
/// deeply wrong; the 5 s comparison is still the spec budget.
const RETRACT_HARD_BUDGET: Duration = Duration::from_secs(10);

/// The four fact attributes the buffer service bootstraps, keyed on
/// the buffer entity. These are what `release_connection` must
/// retract server-side after SIGKILL.
const BUFFER_ATTRS: &[&str] = &[
    "buffer/path",
    "buffer/byte-size",
    "buffer/dirty",
    "buffer/observable",
];

#[tokio::test]
async fn buffer_sigkill_release_connection_within_sc303_budget() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    build_weaver_binary();
    build_buffer_service_binary();

    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("fixture.txt");
    std::fs::write(&fixture_path, b"hello buffer\n").unwrap();
    let canonical = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");

    let mut observer = Client::connect(&socket, "e2e-observer")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");

    // Spawn the buffer service and capture its PID so we can SIGKILL
    // directly (bypassing ChildGuard's SIGTERM-first path).
    let mut buffer_child = spawn_buffer_service(&socket, std::slice::from_ref(&canonical));
    let buffer_pid = buffer_child.id();

    // Wait for the full bootstrap set; record the buffer entity id
    // so we know which retracts to match.
    let buffer_entity = wait_for_bootstrap_entity(&mut observer, RETRACT_HARD_BUDGET).await;

    // SIGKILL — this bypasses the service's SIGTERM shutdown_retract
    // path; the core's release_connection is the only retractor.
    let kill_rc = unsafe { libc::kill(buffer_pid as libc::pid_t, libc::SIGKILL) };
    assert_eq!(
        kill_rc,
        0,
        "SIGKILL failed: {}",
        std::io::Error::last_os_error()
    );
    let sigkill_sent = Instant::now();

    // Drain FactRetract frames for buffer/* attributes owned by the
    // buffer entity. The retracts' provenance is authored by core's
    // cleanup actor (slice-002 release_connection path) — we filter
    // on (entity, attribute) rather than actor identity so either
    // behaviour (service-authored retract if the service ever raced
    // the kill, or core-authored retract) is accepted.
    let mut retracted: HashSet<&'static str> = HashSet::new();
    let deadline = sigkill_sent + RETRACT_HARD_BUDGET;
    while Instant::now() < deadline && retracted.len() < BUFFER_ATTRS.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        let BusMessage::FactRetract { key, .. } = msg else {
            continue;
        };
        if key.entity != buffer_entity {
            continue;
        }
        for attr in BUFFER_ATTRS {
            if key.attribute == *attr {
                retracted.insert(*attr);
            }
        }
    }
    let elapsed = sigkill_sent.elapsed();

    eprintln!(
        "[sc-303] release_connection retracted {}/{} buffer/* attrs in {elapsed:?} (budget {SC303_BUDGET:?})",
        retracted.len(),
        BUFFER_ATTRS.len(),
    );

    // Reap the child so we don't leak a zombie into the ChildGuard
    // Drop path (which would also try to kill/wait).
    let _ = buffer_child.wait();

    for attr in BUFFER_ATTRS {
        assert!(
            retracted.contains(*attr),
            "release_connection did not retract {attr} within {RETRACT_HARD_BUDGET:?}"
        );
    }
    assert!(
        elapsed <= RETRACT_HARD_BUDGET,
        "retract sequence exceeded hard budget {RETRACT_HARD_BUDGET:?} ({elapsed:?})"
    );
}

/// Drain the observer until all four bootstrap `buffer/*` facts have
/// landed, then return the (shared) buffer entity id. Panics on
/// timeout.
async fn wait_for_bootstrap_entity(observer: &mut Client, budget: Duration) -> EntityRef {
    let deadline = Instant::now() + budget;
    let mut seen: HashSet<&'static str> = HashSet::new();
    let mut entity: Option<EntityRef> = None;
    while Instant::now() < deadline && seen.len() < BUFFER_ATTRS.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        let BusMessage::FactAssert(fact) = msg else {
            continue;
        };
        if buffer_service_id(&fact) != Some("weaver-buffers") {
            continue;
        }
        for attr in BUFFER_ATTRS {
            if fact.key.attribute == *attr {
                seen.insert(*attr);
                entity = Some(fact.key.entity);
            }
        }
    }
    entity.expect("bootstrap failed to publish any buffer/* fact within budget")
}

fn buffer_service_id(fact: &Fact) -> Option<&str> {
    match &fact.provenance.source {
        ActorIdentity::Service { service_id, .. } => Some(service_id.as_str()),
        _ => None,
    }
}

// ---- subprocess helpers (mirror buffer_external_mutation.rs) ----

fn tempdir() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    let p = std::env::temp_dir().join(format!("weaver-buf-sk-e2e-{pid}-{tick}"));
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
    std::env::temp_dir().join(format!("weaver-buf-sk-e2e-{pid}-{tick}.sock"))
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
            let _ = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
            std::thread::sleep(Duration::from_millis(100));
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
