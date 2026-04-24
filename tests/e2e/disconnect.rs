//! Core-disconnect e2e: SIGKILL the core; the buffer service must
//! exit cleanly (`PublisherError::BusUnavailable`, exit code 2,
//! per T038), and any live subscriber must surface the disconnect
//! as a recv error — all within the slice-002 5s budget.
//!
//! History: originally T073 (slice 001) drove the retired
//! `core/dirty-tracking` behavior via `BufferEdited`. Session 2
//! `#[ignore]`-gated it for T053 rewrite. The *core dies → observers
//! notice* shape is what slice 001 exercised; slice 003 preserves
//! that shape but replaces the fact-producer with `weaver-buffers`.
//!
//! Complement to `buffer_sigkill.rs` (SC-303), which tests the
//! opposite direction: service killed → core retracts. This test
//! tests: core killed → service exits gracefully + subscribers
//! see the disconnect.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_core::bus::client::{Client, ClientError};
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::fact::Fact;
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const DISCONNECT_BUDGET: Duration = Duration::from_secs(5);
const BOOTSTRAP_WAIT: Duration = Duration::from_secs(10);

/// Exit code for `PublisherError::BusUnavailable` per T038 /
/// `contracts/cli-surfaces.md §weaver-buffers — exit codes`.
const EXIT_BUS_UNAVAILABLE: i32 = 2;

#[tokio::test]
async fn core_sigkill_surfaces_disconnect_to_service_and_subscriber() {
    let socket = unique_socket_path();

    build_weaver_binary();
    build_buffer_service_binary();

    let core = spawn_core(&socket);
    let core_pid = core.id();
    let _core_guard = ChildGuard::new(core);
    wait_for_socket(&socket).await;

    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("fixture.txt");
    std::fs::write(&fixture_path, b"hello buffer\n").unwrap();
    let canonical_fixture = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");

    let mut buffers_reaper = Reaper::new(spawn_buffer_service(&socket, &canonical_fixture));

    // Subscriber waits for the bootstrap to land; otherwise we race
    // the SIGKILL against the service's own startup.
    let mut subscriber = Client::connect(&socket, "e2e-disconnect-observer")
        .await
        .expect("subscriber connect");
    subscriber
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");

    timeout(BOOTSTRAP_WAIT, async {
        loop {
            let msg = subscriber.recv().await.expect("recv");
            if let BusMessage::FactAssert(fact) = msg
                && is_weaver_buffers(&fact)
                && fact.key.attribute.starts_with("buffer/")
            {
                return;
            }
        }
    })
    .await
    .expect("no buffer/* bootstrap FactAssert before SIGKILL");

    // Kill the core abruptly. No retract path here — the bus is gone.
    // `_core_guard` will wait() on the already-dead process at end
    // of scope (or panic) so the zombie is reaped.
    unsafe {
        assert_eq!(
            libc::kill(core_pid as libc::pid_t, libc::SIGKILL),
            0,
            "SIGKILL core failed",
        );
    }

    // 1) Subscriber's recv-loop must terminate with an error within
    //    the disconnect budget.
    let subscriber_start = Instant::now();
    let subscriber_outcome: Result<(), ClientError> = timeout(DISCONNECT_BUDGET, async {
        loop {
            match subscriber.recv().await {
                Ok(_) => continue,
                Err(e) => return Err(e),
            }
        }
    })
    .await
    .expect("subscriber disconnect did not surface within budget");
    let subscriber_elapsed = subscriber_start.elapsed();
    assert!(
        subscriber_outcome.is_err(),
        "subscriber recv must terminate with an error once core is gone",
    );

    // 2) weaver-buffers must exit with EXIT_BUS_UNAVAILABLE within
    //    the same budget window. Use spawn_blocking to keep the
    //    synchronous try_wait polling off the tokio runtime.
    let budget_remaining = DISCONNECT_BUDGET.saturating_sub(subscriber_elapsed);
    let (status, _buffers_reaper) = tokio::task::spawn_blocking(move || {
        let status = buffers_reaper.try_wait_within(budget_remaining);
        (status, buffers_reaper)
    })
    .await
    .expect("join blocking");
    let status = status.expect("try_wait io").expect(
        "weaver-buffers did not exit within disconnect budget; the bus-EOF classification path (T038) is broken",
    );
    assert_eq!(
        status.code(),
        Some(EXIT_BUS_UNAVAILABLE),
        "weaver-buffers exit code must be {EXIT_BUS_UNAVAILABLE} (BusUnavailable per T038); got {status:?}",
    );

    let _ = std::fs::remove_file(&socket);
}

fn is_weaver_buffers(fact: &Fact) -> bool {
    matches!(
        &fact.provenance.source,
        ActorIdentity::Service { service_id, .. } if service_id == "weaver-buffers"
    )
}

// ---- subprocess helpers ----

fn tempdir() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    let p = std::env::temp_dir().join(format!("weaver-disconnect-{pid}-{tick}"));
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

fn spawn_buffer_service(socket: &Path, path: &Path) -> std::process::Child {
    let bin = build_buffer_service_binary();
    Command::new(&bin)
        .arg(path)
        .arg("--socket")
        .arg(socket)
        .arg("--poll-interval=100ms")
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
    std::env::temp_dir().join(format!("weaver-disconnect-{pid}-{tick}.sock"))
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Owns a spawned child; reaps on drop. Matches the inline pattern
/// used by the other slice-003 e2e tests.
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
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Variant guard that additionally exposes a synchronous
/// `try_wait_within` for the exit-status inspection T053 needs.
/// On `Drop` (including panic), falls back to kill + wait so no
/// zombies leak on the failure path.
struct Reaper {
    child: Option<std::process::Child>,
}

impl Reaper {
    fn new(child: std::process::Child) -> Self {
        Self { child: Some(child) }
    }

    fn try_wait_within(
        &mut self,
        deadline: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>> {
        let start = Instant::now();
        loop {
            if let Some(child) = self.child.as_mut()
                && let Some(status) = child.try_wait()?
            {
                return Ok(Some(status));
            }
            if start.elapsed() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
