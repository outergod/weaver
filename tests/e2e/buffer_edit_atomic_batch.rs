//! T018 — slice-004 SC-402 e2e: atomic batched edits.
//!
//! Two scenarios in this file:
//!
//! 1. [`sixteen_edit_happy_batch_lands_atomically`] — bootstrap a buffer
//!    with 16 distinct lines; dispatch a single `weaver edit` invocation
//!    carrying 16 non-overlapping `TextEdit`s; assert the publisher
//!    emits exactly ONE `buffer/version=1` + ONE `buffer/byte-size`
//!    (advanced by exactly 16) + ONE `buffer/dirty=true` re-emission
//!    burst, all three facts sharing one `causal_parent`. The
//!    "exactly one" count is the structural pin for atomic-application
//!    (FR-005a / spec §SC-402): a non-atomic implementation that
//!    bumped per-edit would emit 16 `buffer/version` facts and break
//!    the count.
//!
//! 2. [`three_edit_batch_with_invalid_middle_rejects_whole_batch`] —
//!    bootstrap a buffer; dispatch a 3-edit batch where edit index 1
//!    is bounds-invalid; assert NO `buffer/version`, `buffer/byte-size`,
//!    or `buffer/dirty=true` facts arrive in a structural grace window
//!    (entire batch dropped, no partial application). Then dispatch a
//!    valid single-edit and assert `buffer/version` jumps `0 → 1`
//!    (NOT `1 → 2`) and `buffer/byte-size` reflects only the second
//!    edit's bytes — proving the rejected batch left both content
//!    and version-counter byte-identical to bootstrap.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{sleep, timeout};

use weaver_core::bus::client::Client;
use weaver_core::provenance::ActorIdentity;
use weaver_core::types::fact::{Fact, FactValue};
use weaver_core::types::ids::EventId;
use weaver_core::types::message::{BusMessage, SubscribePattern};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const COLLECT_BUDGET: Duration = Duration::from_millis(5_000);
const REJECTION_GRACE: Duration = Duration::from_millis(1_500);

#[tokio::test]
async fn sixteen_edit_happy_batch_lands_atomically() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    build_weaver_binary();
    build_buffer_service_binary();

    let mut observer = Client::connect(&socket, "e2e-observer")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");

    // Sixteen-line fixture: "00\n01\n...\n15\n". Trailing `\n` drops the
    // phantom line per the data-model line-count rule, so line_count=16
    // and lines 0..=15 are addressable. Each line has 2 bytes of content
    // (the digits) so a pure-insert at <line>:0 always lands on a UTF-8
    // codepoint boundary.
    let mut bootstrap_bytes: Vec<u8> = Vec::with_capacity(16 * 3);
    for i in 0..16u32 {
        bootstrap_bytes.extend_from_slice(format!("{i:02}\n").as_bytes());
    }
    let bootstrap_size = bootstrap_bytes.len() as u64;
    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("fixture.txt");
    std::fs::write(&fixture_path, &bootstrap_bytes).unwrap();
    let canonical_fixture = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");

    let _buffers = ChildGuard::new(spawn_buffer_service(
        &socket,
        std::slice::from_ref(&canonical_fixture),
    ));
    drain_until_buffers_ready(&mut observer).await;

    // Build a 16-edit batch: 16 non-overlapping pure inserts at
    // <line>:0-<line>:0 with single-byte payloads. Total wire delta is
    // exactly +16 bytes; non-atomic implementations would expose 16
    // separate version bumps and break the count assertion below.
    let weaver = build_weaver_binary();
    let mut args: Vec<String> = vec![
        "--socket".into(),
        socket.to_str().unwrap().into(),
        "edit".into(),
        canonical_fixture.to_str().unwrap().into(),
    ];
    let inserts: [&str; 16] = [
        "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P",
    ];
    for (line, payload) in inserts.iter().enumerate() {
        args.push(format!("{line}:0-{line}:0"));
        args.push((*payload).into());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let dispatch_start = Instant::now();
    let edit_output = run_weaver(&weaver, &arg_refs);
    assert!(
        edit_output.status.success(),
        "weaver edit must exit 0 (status={:?}, stderr={})",
        edit_output.status,
        String::from_utf8_lossy(&edit_output.stderr),
    );

    // Collect the post-dispatch fact stream until we either see the
    // expected re-emission burst or the budget elapses. Track every
    // observed buffer/version, buffer/byte-size, buffer/dirty fact so
    // the post-collect count assertions can detect any extra emission
    // that would break atomic-batch semantics.
    let expected_byte_size = bootstrap_size + 16;
    let mut version_facts: Vec<(u64, Option<EventId>)> = Vec::new();
    let mut byte_size_facts: Vec<(u64, Option<EventId>)> = Vec::new();
    let mut dirty_true_facts: Vec<Option<EventId>> = Vec::new();

    let deadline = dispatch_start + COLLECT_BUDGET;
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
                if let FactValue::U64(v) = fact.value {
                    version_facts.push((v, fact.provenance.causal_parent));
                }
            }
            "buffer/byte-size" => {
                if let FactValue::U64(n) = fact.value {
                    byte_size_facts.push((n, fact.provenance.causal_parent));
                }
            }
            "buffer/dirty" => {
                if let FactValue::Bool(true) = fact.value {
                    dirty_true_facts.push(fact.provenance.causal_parent);
                }
            }
            _ => {}
        }
        // Once we have all three post-dispatch facts, give the bus a
        // brief grace window so any erroneous extra emission would also
        // be captured before we check the counts.
        let post_dispatch_versions = version_facts.iter().filter(|(v, _)| *v >= 1).count();
        let post_dispatch_byte_sizes = byte_size_facts
            .iter()
            .filter(|(n, _)| *n == expected_byte_size)
            .count();
        if post_dispatch_versions >= 1
            && post_dispatch_byte_sizes >= 1
            && !dirty_true_facts.is_empty()
        {
            sleep(Duration::from_millis(200)).await;
            // Drain whatever is buffered before we evaluate counts.
            while let Ok(Ok(extra)) = timeout(Duration::from_millis(50), observer.recv()).await {
                let BusMessage::FactAssert(fact) = extra else {
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
                        if let FactValue::U64(v) = fact.value {
                            version_facts.push((v, fact.provenance.causal_parent));
                        }
                    }
                    "buffer/byte-size" => {
                        if let FactValue::U64(n) = fact.value {
                            byte_size_facts.push((n, fact.provenance.causal_parent));
                        }
                    }
                    "buffer/dirty" => {
                        if let FactValue::Bool(true) = fact.value {
                            dirty_true_facts.push(fact.provenance.causal_parent);
                        }
                    }
                    _ => {}
                }
            }
            break;
        }
    }

    // Atomic-batch contract: exactly ONE re-emission of each fact
    // post-dispatch, at the expected new value.
    let post_versions: Vec<&(u64, Option<EventId>)> =
        version_facts.iter().filter(|(v, _)| *v >= 1).collect();
    assert_eq!(
        post_versions.len(),
        1,
        "expected exactly 1 buffer/version >= 1, observed {} (full sequence: {:?})",
        post_versions.len(),
        version_facts,
    );
    assert_eq!(
        post_versions[0].0, 1,
        "expected buffer/version=1 after one accepted batch, observed {}",
        post_versions[0].0,
    );
    let post_byte_sizes: Vec<&(u64, Option<EventId>)> = byte_size_facts
        .iter()
        .filter(|(n, _)| *n == expected_byte_size)
        .collect();
    assert_eq!(
        post_byte_sizes.len(),
        1,
        "expected exactly 1 buffer/byte-size={} (one re-emission burst), observed {} (full sequence: {:?})",
        expected_byte_size,
        post_byte_sizes.len(),
        byte_size_facts,
    );
    assert_eq!(
        dirty_true_facts.len(),
        1,
        "expected exactly 1 buffer/dirty=true (one re-emission burst), observed {}",
        dirty_true_facts.len(),
    );

    // The three re-emitted facts MUST share the BufferEdit event's id
    // as their causal_parent (data-model §State-transition mapping).
    let cp_version = post_versions[0].1.expect("version causal_parent is Some");
    let cp_byte_size = post_byte_sizes[0]
        .1
        .expect("byte-size causal_parent is Some");
    let cp_dirty = dirty_true_facts[0].expect("dirty causal_parent is Some");
    assert_eq!(
        cp_version, cp_byte_size,
        "buffer/version and buffer/byte-size must share causal_parent",
    );
    assert_eq!(
        cp_version, cp_dirty,
        "buffer/version and buffer/dirty must share causal_parent",
    );
}

#[tokio::test]
async fn three_edit_batch_with_invalid_middle_rejects_whole_batch() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    build_weaver_binary();
    build_buffer_service_binary();

    let mut observer = Client::connect(&socket, "e2e-observer")
        .await
        .expect("observer connect");
    observer
        .subscribe(SubscribePattern::AllFacts)
        .await
        .expect("subscribe all");

    let fixture_dir = tempdir();
    let fixture_path = fixture_dir.join("fixture.txt");
    let bootstrap_bytes: &[u8] = b"world";
    let bootstrap_size = bootstrap_bytes.len() as u64;
    std::fs::write(&fixture_path, bootstrap_bytes).unwrap();
    let canonical_fixture = std::fs::canonicalize(&fixture_path).expect("canonicalize fixture");

    let _buffers = ChildGuard::new(spawn_buffer_service(
        &socket,
        std::slice::from_ref(&canonical_fixture),
    ));
    drain_until_buffers_ready(&mut observer).await;

    // Three-edit batch where index 1 is unambiguously out-of-bounds
    // (line 99 vs line_count=1). The validator MUST reject the whole
    // batch and emit no facts.
    let weaver = build_weaver_binary();
    let invalid_dispatch_start = Instant::now();
    let invalid = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "edit",
            canonical_fixture.to_str().unwrap(),
            "0:0-0:0",
            "PRE-",
            "99:0-99:0",
            "boom",
            "1:0-1:0",
            "post",
        ],
    );
    assert!(
        invalid.status.success(),
        "weaver edit dispatch is fire-and-forget, even for batches the \
         service will reject (status={:?}, stderr={})",
        invalid.status,
        String::from_utf8_lossy(&invalid.stderr),
    );

    // Drain the bus for a structural grace window. NO buffer/version,
    // buffer/byte-size, or buffer/dirty=true fact must land — the
    // validator drops the whole batch, the publisher emits no facts.
    let mut spurious_version = 0u32;
    let mut spurious_byte_size = 0u32;
    let mut spurious_dirty_true = 0u32;
    let grace_deadline = invalid_dispatch_start + REJECTION_GRACE;
    while Instant::now() < grace_deadline {
        let remaining = grace_deadline.saturating_duration_since(Instant::now());
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
                if let FactValue::U64(v) = fact.value
                    && v >= 1
                {
                    spurious_version += 1;
                }
            }
            "buffer/byte-size" => {
                if let FactValue::U64(n) = fact.value
                    && n != bootstrap_size
                {
                    spurious_byte_size += 1;
                }
            }
            "buffer/dirty" => {
                if let FactValue::Bool(true) = fact.value {
                    spurious_dirty_true += 1;
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        spurious_version, 0,
        "rejected batch must produce 0 buffer/version bumps (got {spurious_version})",
    );
    assert_eq!(
        spurious_byte_size, 0,
        "rejected batch must produce 0 buffer/byte-size updates (got {spurious_byte_size})",
    );
    assert_eq!(
        spurious_dirty_true, 0,
        "rejected batch must produce 0 buffer/dirty=true updates (got {spurious_dirty_true})",
    );

    // Now dispatch a valid single-edit and observe it lands as the
    // FIRST accepted edit — version 0 → 1, byte-size 5 → 8 (5 + "ONE").
    // If the rejected batch had partially applied, we'd observe a
    // version > 1 or a byte-size != 8 here.
    let valid_dispatch_start = Instant::now();
    let valid = run_weaver(
        &weaver,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "edit",
            canonical_fixture.to_str().unwrap(),
            "0:0-0:0",
            "ONE",
        ],
    );
    assert!(valid.status.success(), "follow-up valid edit must exit 0");

    let mut version_one_observed = false;
    let mut byte_size_eight_observed = false;
    let deadline = valid_dispatch_start + COLLECT_BUDGET;
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
                if let FactValue::U64(v) = fact.value {
                    assert_eq!(
                        v, 1,
                        "follow-up edit must produce buffer/version=1 (got {v}) — \
                         a value > 1 would indicate the rejected batch leaked through",
                    );
                    version_one_observed = true;
                }
            }
            "buffer/byte-size" => {
                if let FactValue::U64(n) = fact.value
                    && n != bootstrap_size
                {
                    assert_eq!(
                        n,
                        bootstrap_size + 3,
                        "follow-up edit (3 bytes inserted) must produce \
                         buffer/byte-size={} (got {n}) — any other value indicates \
                         partial application of the prior rejected batch",
                        bootstrap_size + 3,
                    );
                    byte_size_eight_observed = true;
                }
            }
            _ => {}
        }
        if version_one_observed && byte_size_eight_observed {
            break;
        }
    }
    assert!(
        version_one_observed,
        "follow-up edit's buffer/version=1 must arrive within budget",
    );
    assert!(
        byte_size_eight_observed,
        "follow-up edit's buffer/byte-size=8 must arrive within budget",
    );
}

// ───────────────────────────────────────────────────────────────────
// helpers (mirror buffer_edit_single.rs's pattern; inline per the
// slice-003 convention "no shared harness extraction")
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

fn run_weaver(bin: &Path, args: &[&str]) -> std::process::Output {
    Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spawn weaver")
}

fn tempdir() -> PathBuf {
    let pid = std::process::id();
    let tick = now_ns();
    let p = std::env::temp_dir().join(format!("weaver-edit-batch-e2e-{pid}-{tick}"));
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
    std::env::temp_dir().join(format!("weaver-edit-batch-e2e-{pid}-{tick}.sock"))
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
