//! T027 — slice-005 SC-506 e2e: codec strict-parsing rejection on
//! malformed UUID payload.
//!
//! Two-process scenario (core + this test process). The test
//! constructs a `BusMessage::Event(Event)` frame normally, then
//! patches the inner `Event.id` byte slot to a wrong-length CBOR
//! byte string (8 bytes instead of the required 16). Sending the
//! patched frame post-handshake exercises the listener's codec at
//! the deserialization layer: the `uuid` crate's serde Deserialize
//! requires exactly 16 bytes, so deserialization fails with
//! `CodecError::Decode`.
//!
//! On a codec-decode error the listener's run-message loop returns
//! `Err`; the connection-cleanup path runs and the socket closes.
//! The test asserts: subsequent `recv()` from the test side returns
//! an error (UnexpectedEof / closed) — confirming the listener
//! rejected the frame at the codec layer rather than persisting
//! garbage into the trace.
//!
//! Notes on scope (per slice-005 session-1 narrowing):
//!
//!   * "Wrong version nibble" rejection (e.g., a syntactically-valid
//!     16-byte UUID with version != 8) is NOT exercised here — the
//!     `uuid` crate's `from_bytes` accepts any 16 bytes; version-bit
//!     enforcement is deferred to slice 006 along with FR-029.
//!
//!   * The pre-2026-04-29 spec required Event-with-id rejection on
//!     the inbound channel via an `EventOutbound` envelope split.
//!     The §28(a) re-derivation removed that split; SC-506's residual
//!     enforcement at the codec layer is malformed-byte rejection.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ciborium::Value;
use tokio::io::AsyncWriteExt;
use tokio::time::sleep;
use uuid::Uuid;

use weaver_core::bus::client::Client;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::ids::{EventId, hash_to_58};
use weaver_core::types::message::BusMessage;

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn malformed_uuid_payload_closes_connection_at_codec_layer() {
    let socket = unique_socket_path();
    let _core = ChildGuard::new(spawn_core(&socket));
    wait_for_socket(&socket).await;

    let mut client = Client::connect(&socket, "e2e-malformed")
        .await
        .expect("client connect + handshake");

    // Build a structurally-valid Event with a normal UUIDv8 EventId.
    // Patching happens on the encoded frame, AFTER ciborium would
    // accept the EventId; the test target is the decode path, not
    // the encode path.
    let entity = EntityRef::new(7);
    let prefix = hash_to_58(&Uuid::new_v4());
    let event = Event {
        id: EventId::mint_v8(prefix, 1),
        name: "buffer/save".into(),
        target: Some(entity),
        payload: EventPayload::BufferSave { entity, version: 0 },
        provenance: Provenance::new(ActorIdentity::User, now_ns(), None).expect("provenance"),
    };
    let msg = BusMessage::Event(event);

    let patched = corrupt_event_id_to_short_byte_string(&msg);

    // Send the patched frame as a length-prefixed frame, mirroring
    // the production codec's write_message shape.
    let len = u32::try_from(patched.len()).expect("frame size fits u32");
    client
        .stream
        .write_all(&len.to_be_bytes())
        .await
        .expect("write length");
    client.stream.write_all(&patched).await.expect("write body");
    client.stream.flush().await.expect("flush");

    // The listener's read_message returns Err(CodecError::Decode);
    // run_message_loop returns Err; cleanup runs; the socket closes.
    // Our recv() should fail rather than return any payload.
    let outcome = client.recv().await;
    assert!(
        outcome.is_err(),
        "malformed-UUID frame must trigger connection close; got Ok({outcome:?})",
    );
}

/// Walk a CBOR-encoded `BusMessage::Event(Event)` and patch the
/// inner `Event.id` byte slot from a 16-byte byte string to an
/// 8-byte byte string (a length the `uuid` crate's Deserialize
/// rejects).
///
/// Frame shape per `BusMessage`'s `#[serde(tag="type",
/// content="payload")]`:
///
///   {
///     "type":    "event",
///     "payload": { "id": <16 bytes>, "name": ..., ... },
///   }
///
/// The patching navigates the outer Map's "payload" entry, then the
/// inner Map's "id" entry.
fn corrupt_event_id_to_short_byte_string(msg: &BusMessage) -> Vec<u8> {
    let mut encoded = Vec::new();
    ciborium::into_writer(msg, &mut encoded).expect("encode normally");
    let value: Value = ciborium::from_reader(encoded.as_slice()).expect("decode as Value");

    let Value::Map(mut outer) = value else {
        panic!("BusMessage encodes to a CBOR Map; got {value:?}");
    };
    let mut patched_inner = false;
    for (k, v) in outer.iter_mut() {
        let Value::Text(key) = k else { continue };
        if key != "payload" {
            continue;
        }
        let Value::Map(inner) = v else {
            panic!("BusMessage::Event.payload is the Event struct (a CBOR Map); got {v:?}");
        };
        for (ik, iv) in inner.iter_mut() {
            let Value::Text(ikey) = ik else { continue };
            if ikey == "id" {
                assert!(
                    matches!(iv, Value::Bytes(b) if b.len() == 16),
                    "Event.id encoded as a 16-byte CBOR byte string; got {iv:?}",
                );
                *iv = Value::Bytes(vec![0u8; 8]);
                patched_inner = true;
                break;
            }
        }
    }
    assert!(
        patched_inner,
        "did not find Event.id slot to patch; frame shape changed?",
    );

    let mut out = Vec::new();
    ciborium::into_writer(&Value::Map(outer), &mut out).expect("re-encode");
    out
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
    std::env::temp_dir().join(format!("weaver-eventid-strict-e2e-{pid}-{tick}.sock"))
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
