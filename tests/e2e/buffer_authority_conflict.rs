//! T060 — US3 e2e: SC-304 authority-conflict.
//!
//! Two `weaver-buffers` instances launched against overlapping
//! paths. The first wins the `buffer/*` authority on the shared
//! entity; the second must exit with code 3
//! (`PublisherError::AuthorityConflict`) within SC-304's ≤1 s
//! wall-clock budget. The first instance's facts must stay
//! unperturbed throughout.
//!
//! Pins the exit-code-3 contract — the complement to T053's
//! exit-code-2 pin.
//!
//! Wall-clock timing is reported via stderr for operator
//! verification against the SC-304 ≤1 s target. The hard
//! deadline is generous (5 s) so cold-cache CI hosts don't
//! false-fail; the reported elapsed is the real pass/fail
//! signal. Hardware-dependent.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_buffers::model::buffer_entity_ref;
use weaver_core::bus::client::Client;
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::Fact;
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const BOOTSTRAP_WAIT: Duration = Duration::from_secs(10);
/// Spec budget: SC-304 says the second instance must exit within
/// 1 s. Reported via stderr for operator pass/fail judgement.
const SC304_BUDGET: Duration = Duration::from_secs(1);
/// Hard deadline for the second instance's exit. Deliberately
/// larger than the spec budget so cold-cache CI hosts don't
/// false-fail on the hard assertion; the observed wall-clock is
/// the real SC-304 signal.
const EXIT_HARD_BUDGET: Duration = Duration::from_secs(5);
/// Exit code for `PublisherError::AuthorityConflict` per
/// `buffers/src/cli.rs::exit_code::AUTHORITY_CONFLICT`.
const EXIT_AUTHORITY_CONFLICT: i32 = 3;

#[tokio::test]
async fn second_instance_on_overlapping_path_exits_authority_conflict() {
    let socket = unique_socket_path();

    build_weaver_binary();
    build_buffer_service_binary();

    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("shared.txt");
    std::fs::write(&fixture_path, b"shared fixture\n").expect("write fixture");
    let canonical = std::fs::canonicalize(&fixture_path).expect("canonicalise fixture");
    let buffer_entity = buffer_entity_ref(&canonical);

    // Subscribe BEFORE spawning either service: we need to observe
    // the first instance's bootstrap land and, crucially, assert
    // that no retraction ever fires for the shared buffer entity
    // during the second instance's lifetime.
    let mut observer = Client::connect(&socket, "e2e-authority-conflict-observer")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");

    // Instance 1 wins the authority on the shared entity.
    let _first = ChildGuard::new(spawn_buffer_service(&socket, &canonical));

    // Gate: wait for instance 1's `watcher/status=ready`. That
    // signal arrives only after every per-buffer bootstrap fact
    // has been asserted, so we know the authority is firmly held
    // before instance 2 starts competing.
    wait_for_first_ready(&mut observer).await;

    // Spawn instance 2. Start the stopwatch before the fork so
    // the reported SC-304 elapsed includes connect + handshake +
    // the attempted bootstrap through to exit.
    let conflict_start = Instant::now();
    let second = spawn_buffer_service(&socket, &canonical);
    let mut second_reaper = Reaper::new(second);

    // Wait for exit on a blocking worker so the tokio runtime
    // stays responsive. Reporting elapsed the moment the reaper
    // returns keeps the timing measurement tight — no
    // concurrent-task drag from a bounded observer drain.
    let (status_result, _reaper) = tokio::task::spawn_blocking(move || {
        let r = second_reaper.try_wait_within(EXIT_HARD_BUDGET);
        (r, second_reaper)
    })
    .await
    .expect("join blocking");
    let elapsed = conflict_start.elapsed();
    eprintln!("[sc-304] second-instance exit observed in {elapsed:?} (budget {SC304_BUDGET:?})");

    let status = status_result
        .expect("try_wait io error")
        .unwrap_or_else(|| {
            panic!(
                "second instance did not exit within hard budget {EXIT_HARD_BUDGET:?}; \
             the authority-conflict classification path is broken"
            )
        });
    assert_eq!(
        status.code(),
        Some(EXIT_AUTHORITY_CONFLICT),
        "second instance must exit {EXIT_AUTHORITY_CONFLICT} (AuthorityConflict); got {status:?}"
    );

    // Post-exit: drain any messages buffered on the observer
    // between first's `ready` and second's exit. Assert none of
    // them retracted the shared buffer entity — authority
    // conflicts are server-side-rejected before any state change,
    // so instance 1's authority must remain intact.
    assert_no_retract_on_shared_entity(&mut observer, buffer_entity, Duration::from_millis(200))
        .await;

    let _ = std::fs::remove_file(&socket);
}

/// Drain the observer until the first instance's
/// `watcher/status=ready` lands, or panic on timeout. Distinguishes
/// the first instance by its `service_id == "weaver-buffers"` and
/// the `Ready` lifecycle payload shape on `watcher/status`.
async fn wait_for_first_ready(observer: &mut Client) {
    timeout(BOOTSTRAP_WAIT, async {
        loop {
            let msg = observer.recv().await.expect("recv");
            if let BusMessage::FactAssert(fact) = msg
                && is_weaver_buffers(&fact)
                && fact.key.attribute == "watcher/status"
                && let weaver_core::types::fact::FactValue::String(s) = &fact.value
                && s == "ready"
            {
                return;
            }
        }
    })
    .await
    .expect("first instance did not reach watcher/status=ready before conflict test");
}

/// Drain the observer for up to `budget` and panic if any
/// `FactRetract` on `shared_entity` is observed. Runs *after* the
/// second instance has exited, so every server-side reaction to
/// the conflict has already queued on the subscriber's socket;
/// the budget only needs to be long enough to drain the kernel
/// buffer, not to wait for server-side work.
///
/// Poll-loop re-asserts of `buffer/dirty` by the first instance
/// are permitted — they don't perturb authority. Only retracts
/// signal a problem.
async fn assert_no_retract_on_shared_entity(
    observer: &mut Client,
    shared_entity: EntityRef,
    budget: Duration,
) {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let msg = match timeout(remaining, observer.recv()).await {
            Ok(Ok(m)) => m,
            _ => break,
        };
        if let BusMessage::FactRetract { key, .. } = msg
            && key.entity == shared_entity
        {
            panic!(
                "shared buffer entity {shared_entity:?} was retracted during the \
                 authority-conflict window — first instance's authority was perturbed"
            );
        }
    }
}

fn is_weaver_buffers(fact: &Fact) -> bool {
    matches!(
        &fact.provenance.source,
        ActorIdentity::Service { service_id, .. } if service_id == "weaver-buffers"
    )
}

// ---- subprocess helpers (mirror disconnect.rs) ----

fn tempdir() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    let p = std::env::temp_dir().join(format!("weaver-buf-conflict-{pid}-{tick}"));
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
    std::env::temp_dir().join(format!("weaver-buf-conflict-{pid}-{tick}.sock"))
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

/// Variant guard that exposes a synchronous `try_wait_within` for
/// exit-status inspection (T053 / T060). On drop, falls back to
/// kill + wait so no zombies leak on the panic path.
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
