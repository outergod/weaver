//! T022 — slice-004 SC-406 e2e: `weaver edit` and `weaver edit-json`
//! emit byte-identical `EventPayload::BufferEdit` payloads for
//! semantically-equivalent inputs.
//!
//! For a randomly-generated `Vec<TextEdit>` batch B, we run both
//! emitter forms (positional and JSON) against a fake-core test
//! harness that captures the dispatched `BusMessage::Event` envelope.
//! The deterministic core of the payload — `entity`, `version`, and
//! `edits` — MUST be byte-identical between the two emitters; the
//! per-invocation envelope fields (`Event.id`, `Provenance.timestamp_ns`)
//! differ by construction and are stripped before comparison.
//!
//! The test substitutes a `UnixListener`-based harness for the real
//! core. The harness completes the handshake, returns
//! `buffer/version=0` to the inspect-lookup, and captures the next
//! `BusMessage::Event` frame; the proptest then compares the two
//! captured envelopes' payloads.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use proptest::prelude::*;
use proptest::test_runner::Config as ProptestConfig;
use tempfile::NamedTempFile;
use tokio::net::UnixListener;
use tokio::runtime::Builder;

use weaver_core::bus::codec::{read_message, write_message};
use weaver_core::types::edit::{Position, Range, TextEdit};
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::FactValue;
use weaver_core::types::ids::EventId;
use weaver_core::types::message::{BusMessage, InspectionDetail, LifecycleSignal};

/// Bounded line/character so the JSON payload doesn't blow past the
/// 64 KiB wire-frame limit — we're testing emitter equivalence, not
/// bounds. Small `new_text` length similarly keeps single-edit cost low.
const MAX_LINE: u32 = 64;
const MAX_CHAR: u32 = 64;
const MAX_NEW_TEXT_LEN: usize = 16;
const MAX_BATCH_LEN: usize = 8;

fn arb_position() -> impl Strategy<Value = Position> {
    (0..=MAX_LINE, 0..=MAX_CHAR).prop_map(|(line, character)| Position { line, character })
}

fn arb_text_edit() -> impl Strategy<Value = TextEdit> {
    // ASCII-only `new_text` so the positional form's argv survives any
    // shell-quoting concerns the spawned process inherits. Non-empty
    // by construction so a `start == end` range remains a legitimate
    // pure-insert (per data-model R5: nothing-edit only when both
    // range and new_text are empty).
    (
        arb_position(),
        arb_position(),
        proptest::collection::vec(0x20u8..=0x7Eu8, 1..=MAX_NEW_TEXT_LEN),
    )
        .prop_map(|(start, end, txt)| TextEdit {
            range: Range { start, end },
            new_text: String::from_utf8(txt).expect("ascii-only by construction"),
        })
}

fn arb_batch() -> impl Strategy<Value = Vec<TextEdit>> {
    proptest::collection::vec(arb_text_edit(), 1..=MAX_BATCH_LEN)
}

proptest! {
    #![proptest_config(ProptestConfig {
        // 256 cases keeps the spec's coverage promise. Each case
        // spawns the `weaver` binary twice — at ~30 ms / spawn the
        // total runtime budget is a few seconds. The binary is
        // already cargo-cached after the first invocation, so the
        // marginal per-case cost is process startup + handshake +
        // one inspect round-trip.
        cases: 256,
        ..ProptestConfig::default()
    })]

    #[test]
    fn weaver_edit_and_edit_json_dispatch_byte_identical_payloads(
        edits in arb_batch(),
    ) {
        let weaver = build_weaver_binary();

        // A single fixture file shared between the two invocations.
        // Both invocations canonicalise the same path so they derive
        // the same `EntityRef` — any divergence here would surface as
        // a payload mismatch on `entity`.
        let fixture = NamedTempFile::new().expect("tempfile");
        std::fs::write(fixture.path(), b"weaver-edit-emitter-parity-fixture\n").expect("seed fixture");
        let canonical = std::fs::canonicalize(fixture.path()).expect("canonicalize");

        let positional_payload = capture_dispatch_via_positional(&weaver, &canonical, &edits);
        let json_payload = capture_dispatch_via_json(&weaver, &canonical, &edits);

        let payload_eq = positional_payload == json_payload;
        prop_assert!(
            payload_eq,
            "positional and JSON emitters dispatched divergent payloads:\n  \
             positional = {positional_payload:?}\n  json = {json_payload:?}",
        );

        // Also pin byte-identity of the CBOR-serialised payloads. The
        // structural equality above is the load-bearing assertion;
        // CBOR-byte equality is a second-angle check that catches
        // drift in the serde derive (tag, field order, kebab-case
        // rename) that PartialEq alone wouldn't.
        let mut positional_bytes = Vec::new();
        ciborium::into_writer(&positional_payload, &mut positional_bytes).expect("ciborium");
        let mut json_bytes = Vec::new();
        ciborium::into_writer(&json_payload, &mut json_bytes).expect("ciborium");
        prop_assert_eq!(
            positional_bytes,
            json_bytes,
            "positional and JSON CBOR-serialised payloads diverge on the wire",
        );
    }
}

fn capture_dispatch_via_positional(
    weaver: &Path,
    canonical: &Path,
    edits: &[TextEdit],
) -> EventPayload {
    let socket = unique_socket_path();
    let envelope = run_with_fake_core(&socket, |sock| {
        // `--` after `<PATH>` tells clap "no more flags from here";
        // without it, edit pairs whose `<TEXT>` starts with `-` get
        // misinterpreted as flags and rejected at parse time. Mirrors
        // the standard `cmd PATH -- args` ergonomics clap suggests.
        let mut args: Vec<String> = vec![
            "--socket".into(),
            sock.to_str().unwrap().into(),
            "edit".into(),
            canonical.to_str().unwrap().into(),
            "--".into(),
        ];
        for e in edits {
            args.push(format_range(&e.range));
            args.push(e.new_text.clone());
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = Command::new(weaver)
            .args(&arg_refs)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn weaver");
        assert!(
            out.status.success(),
            "weaver edit must exit 0, status={:?}, stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr),
        );
    });
    envelope.payload
}

fn capture_dispatch_via_json(weaver: &Path, canonical: &Path, edits: &[TextEdit]) -> EventPayload {
    let socket = unique_socket_path();
    let json_input = NamedTempFile::new().expect("tempfile");
    let json = serde_json::to_string(edits).expect("serialise edits to json");
    std::fs::write(json_input.path(), json).expect("write json input");

    let envelope = run_with_fake_core(&socket, |sock| {
        let out = Command::new(weaver)
            .args([
                "--socket",
                sock.to_str().unwrap(),
                "edit-json",
                canonical.to_str().unwrap(),
                "--from",
                json_input.path().to_str().unwrap(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn weaver");
        assert!(
            out.status.success(),
            "weaver edit-json must exit 0, status={:?}, stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr),
        );
    });
    envelope.payload
}

/// Spawn a fake-core listener on `socket`, run `client` (which is
/// expected to invoke `weaver` against the socket), and return the
/// captured `Event` envelope.
///
/// `client` runs synchronously on the caller's thread so its closure
/// is free to borrow stack-local fixtures without `'static`.
fn run_with_fake_core<F: FnOnce(&Path)>(socket: &Path, client: F) -> Event {
    let runtime = Arc::new(
        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime"),
    );

    let socket_owned = socket.to_path_buf();
    let listener = runtime
        .block_on(async { UnixListener::bind(&socket_owned) })
        .expect("bind unix socket");
    let runtime_for_server = Arc::clone(&runtime);

    // Spawn the server task on the runtime; the client closure runs on
    // the test thread (synchronous) so we can spawn `weaver` via
    // std::process::Command without a tokio handle in scope.
    let server_handle = std::thread::spawn(move || {
        runtime_for_server.block_on(async move { run_fake_core(listener).await })
    });

    // Best-effort wait for the listener to be accepting (the
    // UnixListener bound above is sync — accept happens on
    // `run_fake_core`'s first await). 50 ms is enough for the runtime
    // to schedule the server future.
    std::thread::sleep(Duration::from_millis(20));
    client(socket);
    let envelope = server_handle.join().expect("server thread");
    let _ = std::fs::remove_file(socket);
    envelope
}

/// Fake-core handshake + inspect-response + event-capture loop.
/// Mirrors the listener's protocol shape just deeply enough to
/// satisfy `weaver edit` / `weaver edit-json`.
async fn run_fake_core(listener: UnixListener) -> Event {
    let (mut stream, _) = listener.accept().await.expect("accept");

    // 1. Read Hello (any protocol_version is accepted in the harness;
    //    the real core would version-mismatch reject 0x03 clients).
    match read_message(&mut stream).await.expect("read hello") {
        BusMessage::Hello(_) => {}
        other => panic!("expected Hello, got {other:?}"),
    }
    // 2. Send Lifecycle(Ready) to complete the handshake (matches
    //    `Client::connect`'s expectation).
    write_message(&mut stream, &BusMessage::Lifecycle(LifecycleSignal::Ready))
        .await
        .expect("write ready");

    // 3. Read InspectRequest for buffer/version.
    let request_id = match read_message(&mut stream).await.expect("read inspect") {
        BusMessage::InspectRequest {
            request_id,
            fact: _,
        } => request_id,
        other => panic!("expected InspectRequest, got {other:?}"),
    };
    // 4. Send InspectResponse with version=0 (a valid `FactValue::U64`
    //    suffices — both emitters use this version verbatim).
    let detail = InspectionDetail::service(
        EventId::for_testing(1),
        "weaver-buffers".into(),
        uuid::Uuid::new_v4(),
        0,
        0,
        FactValue::U64(0),
    );
    write_message(
        &mut stream,
        &BusMessage::InspectResponse {
            request_id,
            result: Ok(detail),
        },
    )
    .await
    .expect("write inspect-response");

    // 5. Read the dispatched Event.
    match read_message(&mut stream).await.expect("read event") {
        BusMessage::Event(e) => e,
        other => panic!("expected Event, got {other:?}"),
    }
}

fn format_range(r: &Range) -> String {
    format!(
        "{}:{}-{}:{}",
        r.start.line, r.start.character, r.end.line, r.end.character,
    )
}

fn build_weaver_binary() -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "--quiet", "-p", "weaver_core", "--bin", "weaver"])
        .status()
        .expect("cargo build weaver");
    assert!(status.success());
    let target = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target")
        });
    target.join("debug").join("weaver")
}

fn unique_socket_path() -> PathBuf {
    let pid = std::process::id();
    let tick = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = next_socket_counter();
    std::env::temp_dir().join(format!("weaver-edit-parity-{pid}-{tick}-{counter}.sock"))
}

fn next_socket_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static C: AtomicU64 = AtomicU64::new(1);
    C.fetch_add(1, Ordering::Relaxed)
}
