# Research — Slice 005 (Buffer Save)

Phase 0 decisions. Each entry resolves an implementation-level question that the plan depended on. Rationale + alternatives preserved so post-slice reviewers understand *why*, not just *what*.

## 1. `BusMessage` shape under §28(a) — SUPERSEDED

The 2026-04-27 direction introduced a generic `BusMessage<E>` with `BusMessageInbound = BusMessage<EventOutbound>` and `BusMessageOutbound = BusMessage<Event>`, plus direction-typed codec entry points, to encode the asymmetry "producers send no `id`; listener stamps". The 2026-04-29 constitutional re-derivation found listener-stamping misaligned on `docs/00-constitution.md` §2/§11/§12/§15/§16 (originator-pattern bootstrap-chain regression) and in tension with §1/§6/§17. The replacement direction (UUIDv8 with hashed producer-instance-id prefix; see §12 below) keeps producers the authoritative ID source, so there is no inbound/outbound asymmetry on the `Event` carrier — `BusMessage` reverts to its slice-004 non-generic shape. The generic refactor and direction-typed codec siblings revert in the implementation step that follows the spec amendment.

## 2. Inode capture mechanism

**Decision**: `std::os::unix::fs::MetadataExt::ino()` on the result of `std::fs::metadata(path)` invoked at `BufferState::open` time, immediately after path canonicalisation. The returned `u64` becomes a new private immutable `inode: u64` field on `BufferState`.

```rust
use std::os::unix::fs::MetadataExt;
let metadata = std::fs::metadata(&canonical_path)?;
let inode = metadata.ino();
```

Pre-rename inode check at save time: `std::fs::metadata(&path).map(|m| m.ino())` — match against captured value; differing → `WEAVER-SAVE-005`; `Err(NotFound)` → `WEAVER-SAVE-006`.

**Rationale**:

- Std-only — no new direct dependency, no AGPL-compatibility audit. `MetadataExt` is stable since Rust 1.1.
- Returns `u64` directly; matches the storage type on `BufferState`.
- Same mechanism as `stat(2)` underlying — no reachability surprise.
- POSIX-only is acceptable: the slice's target platform is Linux + macOS desktop (per Technical Context); Windows support would require a different identity primitive (`GetFileInformationByHandle::FileIndex`) but is out of scope.

**Alternatives considered**:

- **`nix::sys::stat::fstat`** — adds the `nix` crate as a direct dep just for one syscall wrapper. Rejected.
- **`rustix::fs::statat`** — same objection.
- **No inode capture; rely on path-existence check only** — fails to distinguish atomic-replace (different inode, same path) from in-place edit. Rejected: SC-502 acceptance scenario 3 explicitly covers atomic-replace.
- **Capture device-id + inode** (`(dev, ino)` pair) — over-defensive given the same-filesystem invariant from same-directory tempfile placement. Bind-mount edge cases exist but realistic exposure is zero in MVP. Adds a field for hypothetical robustness; defer if a real exposure surfaces.

## 3. I/O-failure injection for SC-504

**Decision**: extract the tempfile-write-fsync-rename sequence into an internal helper `atomic_write_with_hooks(path, contents, before: BeforeStep)` where `BeforeStep` is a closure called *before* each I/O syscall; the closure can return `Err(io::Error)` to short-circuit. Production callers pass a no-op closure (compiles to a constant); test callers pass a step-counting closure that injects errors at chosen steps.

```rust
pub(crate) enum WriteStep { OpenTempfile, WriteContents, FsyncTempfile, RenameToTarget, FsyncParentDir }

pub(crate) fn atomic_write_with_hooks<F>(
    path: &Path,
    contents: &[u8],
    mut before: F,
) -> Result<(), io::Error>
where F: FnMut(WriteStep) -> Result<(), io::Error>,
```

`BufferState::save_to_disk` calls `atomic_write_with_hooks(path, &self.content, |_| Ok(()))` in production. The test binary `tests/e2e/buffer_save_atomic_invariant.rs` calls it with a closure that returns `Err(io::Error::new(ErrorKind::OutOfStorage, "ENOSPC"))` on `WriteStep::RenameToTarget`. The test asserts (a) the original disk file is byte-identical to its pre-save state, (b) the tempfile has been cleaned up.

**Rationale**:

- Hook surface lives in the same file as `save_to_disk`; no abstraction-spanning trait. Test access is via `pub(crate)` visibility from the test crate.
- Closure-as-hook is a Rust idiom; no `Box<dyn Fn>` allocations on the production path because the no-op closure inlines.
- Single hook point covers all five WriteStep variants; tests choose at which step to inject.
- No dependence on platform-specific syscall interception (LD_PRELOAD on Linux, DYLD_INSERT_LIBRARIES on macOS). CI runs identically across both targets.

**Alternatives considered**:

- **LD_PRELOAD shim for `rename(2)` failure** — Linux-only; CI complexity (shim needs to be built per-arch + per-libc); macOS uses a different mechanism. Rejected.
- **Trait-bound `FilesystemOps`** abstraction across the codebase — pervasive change for one test scenario. The save sequence is the only I/O path; abstraction-spanning is overkill.
- **Filesystem quota fixture (tmpfs with bounded size)** — requires mount privileges; not all CI environments allow. Rejected.
- **Read-only target directory** — simulates `EACCES`, not `ENOSPC` or `EXDEV`. Doesn't cover the full FR-015 rename-failure surface. Rejected.

## 4. `EventOutbound` ↔ `Event` relationship — SUPERSEDED

The 2026-04-27 direction introduced an `EventOutbound` struct (slice-001 canonical `Event` shape minus `id`) for the inbound wire shape and `Event::from_outbound(id, outbound)` for listener-side inflation. Under the 2026-04-29 re-derivation (see §12), producers mint UUIDv8 EventIds locally; there is no envelope split. `Event` retains its slice-001 canonical shape (`{ id, name, target, payload, provenance }`); `causal_parent` continues to live on `provenance.causal_parent`. `EventOutbound` and `Event::from_outbound` are removed in the implementation step that follows the spec amendment.

## 5. UUIDv8 producer-prefix scheme

**Decision**: `EventId` becomes `EventId(Uuid)` (16-byte UUIDv8 — UUID format with custom payload, version field set to `0x8`). Producer-side mint helper:

```rust
impl EventId {
    /// Mint a UUIDv8 EventId with the producer's hashed-instance-id prefix in the high 58 bits
    /// of the custom payload, and `time_or_counter` (typically `now_ns()` or a process-monotonic
    /// counter) in the low 64 bits.
    pub fn mint_v8(producer_prefix_58: u64, time_or_counter: u64) -> Self {
        // UUIDv8 layout (RFC 9562):
        //   bytes 0..6   = high 48 bits of custom payload (here: bits 10..58 of producer_prefix)
        //   byte  6 high = version nibble (set to 0x8)
        //   byte  6 low  + byte 7 = next 12 bits of custom payload (here: low 10 bits of producer_prefix + 2 reserved)
        //   byte  8 high = variant bits (set to 0b10)
        //   byte  8 low  + bytes 9..16 = remaining 62 bits of custom payload (here: low 64 bits of time_or_counter, with 2 bits dropped to fit variant)
        // Implementation uses `uuid::Builder::from_custom_bytes` or equivalent; exact bit layout
        // is implementation-internal — what's load-bearing is that distinct (producer_prefix, time)
        // pairs map to distinct UUIDs and that the prefix is recoverable for display purposes.
        ...
    }

    pub const fn nil() -> Self { EventId(Uuid::nil()) }

    /// Deterministic constructor for tests; wraps a u128 into a UUID.
    pub fn for_testing(value: u128) -> Self { EventId(Uuid::from_u128(value)) }
}
```

Producer-side prefix derivation:

```rust
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

fn hash_to_58(uuid: &Uuid) -> u64 {
    let mut h = DefaultHasher::new();
    uuid.hash(&mut h);
    // Mask to 58 bits — high 6 bits dropped to leave room for UUIDv8 version + variant fields.
    h.finish() & ((1 << 58) - 1)
}
```

For Service producers: `hash_to_58(&actor_identity.instance_id)`. For non-Service producers: each producer process generates a per-process UUIDv4 at startup (`OnceLock<Uuid>` initialised lazily on first emit), hashes that to 58 bits.

`TraceStore::by_event` becomes `HashMap<EventId, TraceSequence>` where `EventId(Uuid)` is the key. Insert is collision-free for any two distinct producer-mint events because the prefix-namespace partition is by-construction disjoint across producers.

**Rationale**:

- **Collision-freedom by construction**: distinct producers occupy distinct 58-bit-prefix namespaces. The hash maps the producer's internal UUID identity to a partition; two producers with distinct UUIDs cannot collide unless the SipHash collides on that pair (probability ~2⁻⁵⁸ per pair, structurally negligible at K-producer scales the system will encounter).
- **No listener-side coordination**: each producer's mint is purely local. Bootstrap-chain affordance (originator pattern: producer publishes one event AND immediately publishes facts whose `causal_parent` is that event's id) works trivially because the producer's local id is final and known immediately at construction.
- **Single-typed key**: `HashMap<EventId, _>` stays one field. The tuple form `(UUIDv4, producer_id)` would force a wrapper struct with manual `Hash`/`Eq`/`Ord` impls, plus every wire-level fact's `causal_parent: Option<EventId>` would become a tuple (two CBOR fields per occurrence) — ergonomic and wire-bloat cost across the entire trace.
- **Wire compactness**: 16 bytes per `EventId`. Tuple form would be 16 + N bytes per `EventId`, compounding across `causal_parent` chains.
- **Display**: existing `uuid` crate's `Display` impl renders any UUID; client-side passive caching binds prefix → friendly_name (see §12) for human-readable display, with full UUID available via `--output=json`.
- **Future security check**: prefix-vs-provenance verification is bit-extraction on UUIDv8; on the tuple form it would be a field comparison — both work, UUIDv8 keeps the check more contained.

**Alternatives considered**:

- **Listener-stamped monotonic `u64` counter** (the 2026-04-27 ID-stripped-envelope direction) — REJECTED on constitutional grounds (§2/§11/§12/§15/§16 originator-pattern bootstrap-chain regression; §1/§6/§17 centralisation tension). See §12 below for the per-principle audit.
- **UUIDv4 alone, no producer-ID partitioning** — REJECTED on VM step-clock collision risk (a previous slice's spec flagged this: two producers seeing the same time after a clock jump can collide in `Uuid::new_v4()` if RNG quality varies). Producer-ID partitioning gives by-construction freedom regardless of time component or RNG quality.
- **Composite `(UUIDv4, producer_id)` tuple form** — REJECTED on wire bloat + Hash/Eq complexity at every `EventId` site (see Rationale above).
- **UUIDv7 (timestamp-ordered)** — single-namespace; identical collision concern to UUIDv4 alone in multi-producer scenarios; loses the producer-prefix recoverability for display purposes.

## 6. Tempfile naming + entropy source

**Decision**: tempfile name format `.<basename>.weaver-save.<uuid-v4-suffix>`. The `uuid` crate is already a workspace dependency (used since slice 002 for `ActorIdentity::Service` instance UUIDs). Reusing it via `Uuid::new_v4().simple().to_string()` produces a 32-character hex suffix per tempfile.

Examples:
- target `./file.txt` → tempfile `./.file.txt.weaver-save.f3a7b2c4d8e1450a9bf6c8d04e2a3b5c`
- target `./src/main.rs` → tempfile `./src/.main.rs.weaver-save.<uuid>`

**Rationale**:

- Dot-prefix hides from default `ls`, default `find`, and most editor file pickers — orphaned tempfiles don't pollute operator workflow on first inspection.
- `weaver-save` infix lets operators identify orphan origin during `find . -name '*.weaver-save.*'` cleanup sweeps.
- UUID v4 gives 122 bits of entropy — collision-free in practice under any plausible concurrency.
- Reusing `uuid` (existing dep) avoids adding `rand` as a direct dep (it's already transitively present through `uuid` — no extra `Cargo.lock` ripples).

**Alternatives considered**:

- **`rand::random::<u64>()` for a smaller suffix** — 64 bits is collision-free at human-realistic rates but the entropy gap makes `lsof` / `find` matches less unique under multi-user scenarios. UUID is the conservative choice.
- **`tempfile::NamedTempFile::new_in(dir)`** — the `tempfile` crate is workspace dev-dep only (not production). Promoting it to production-dep adds AGPL-compatibility surface for one syscall wrapper. The handcrafted approach avoids that.
- **Time-based suffix (`<unix-nanos>`)** — collides under sub-microsecond concurrency. Same class of bug as §28's pre-fix state.

## 7. Atomic-rename: fsync of parent directory after rename?

**Decision**: yes — `fsync(2)` on the parent directory file descriptor immediately after the successful `rename(2)` syscall. Implemented as the fifth step in `atomic_write_with_hooks` (`WriteStep::FsyncParentDir`).

```rust
let dir = path.parent().expect("non-root target");
let dir_fd = std::fs::File::open(dir)?;  // O_DIRECTORY implied on Linux
dir_fd.sync_all()?;
```

`sync_all()` on a directory `File` translates to `fsync(2)` on Linux/macOS.

**Rationale**:

- POSIX `rename(2)` is filesystem-atomic w.r.t. observers (readers see old or new, never partial), but the new directory entry's durability requires the parent directory's inode update to persist. Without `fsync(parent_dir)`, a system crash between rename and the filesystem's own background commit can lose the rename, reverting the file to its pre-save state.
- Operator UX commitment: when `weaver save` returns and `buffer/dirty = false` is observable, the file is durably saved. Loss across crash would be a silent UX regression — the operator's intent ("I saved this") would be invisibly lost.
- Latency cost: one extra syscall (~2-5 ms typical on commodity SSD; <1 ms on NVMe). SC-501's 500 ms budget has ~50× margin; the parent fsync is well within tolerance.

**Alternatives considered**:

- **Skip parent fsync; rely on filesystem's eventual consistency** — fast path is identical, but durability invariant is lost. Rejected.
- **Use `O_DIRECT` + write barrier** — Linux-specific; doesn't replace fsync semantics; over-engineering for the durability gain.
- **Sync the entire filesystem (`syncfs`)** — wide blast radius; affects unrelated processes. Rejected.

## 8. §28(a) migration ordering

**Decision**: the `EventId` `u64 → Uuid` type change cascades through every construction site workspace-wide; the wire bump `0x04 → 0x05` is the atomic migration boundary. Two-step landing for incremental CI-greenness:

1. **Type-shape cascade**: rewrite `EventId(u64)` → `EventId(Uuid)` and update every `EventId::new(<u64>)` construction site:
   - Test fixtures: `EventId::new(42)` → `EventId::for_testing(42)`.
   - Sentinel sites: `EventId::ZERO` → `EventId::nil()`. Includes the slice-004 `lookup_event_for_inspect` ZERO-short-circuit.
   - Production callers: temporarily wrap the existing `now_ns()` via `Uuid::from_u128(now_ns() as u128)` so the workspace stays internally consistent at this commit; the producer-mint-site UUIDv8 migration happens in step 2.
2. **Producer-mint-site migration**: per-producer commits (or one combined commit; operator preference — per-producer aligns with PR-discipline "one logical change per commit") replace the `Uuid::from_u128(now_ns() as u128)` placeholder with `EventId::mint_v8(producer_prefix_58, now_ns())`:
   - `core/src/cli/edit.rs` (`weaver edit`, `weaver edit-json`) — User identity, per-process UUIDv4 prefix.
   - `buffers/src/publisher.rs` (poll-tick re-emissions, `bootstrap_tick`) — Service identity, hashed `instance_id` prefix.
   - `git-watcher/src/publisher.rs` (poll-tick re-emissions) — Service identity, hashed `instance_id` prefix.
   - `core/src/cli/save.rs` — User identity, per-process UUIDv4 prefix; born compliant.

The wire bump is enforced at handshake (`Hello.protocol_version = 0x05`); pre-bump clients receive the version-mismatch error and close. Post-bump and pre-bump clients cannot coexist on one bus instance because the `EventId` wire shape changes from CBOR unsigned-int (8 bytes) to CBOR byte-string (16 bytes) — even a wall-clock-ns-derived UUIDv8 will not deserialise as a slice-004 `EventId(u64)`.

**Rationale**:

- Clean break is consistent with operator's "no users yet, wire-stability is not a concern" framing.
- Two-step landing keeps every commit CI-green: step 1 changes only the wire shape (passes round-trip tests against the new shape); step 2 changes only producer-side mint logic (passes property tests against UUIDv8 collision-freedom).
- The four mint sites are all in workspaces under `cargo build` together; atomic migration of step 2 is a single PR / commit set.

**Alternatives considered**:

- **Single-step landing (type cascade + mint logic in one commit)** — possible, but the diff size makes review harder; two-step keeps each commit reviewable.
- **Phased: keep `0x04` accepting `EventId(u64)`, add `0x05` accepting `EventId(Uuid)`, sunset 0x04 in slice 006** — complicates the listener's accept logic for no operator benefit; perpetuates the producer-side wall-clock-ns collision class for one more slice. Rejected.

## 9. `BufferSaveOutcome` taxonomy

**Decision**:

```rust
pub(crate) enum BufferSaveOutcome {
    Saved          { entity: EntityRef, path: PathBuf, version: u64 },
    CleanSaveNoOp  { entity: EntityRef, version: u64 },
    StaleVersion   { event_version: u64, current_version: u64 },
    NotOwned       { entity: EntityRef },
    InodeMismatch  { entity: EntityRef, path: PathBuf, expected: u64, actual: u64 },
    PathMissing    { entity: EntityRef, path: PathBuf },
    TempfileIo     { entity: EntityRef, path: PathBuf, error: io::Error },
    RenameIo       { entity: EntityRef, path: PathBuf, error: io::Error },
}
```

Maps 1:1 to the six diagnostic codes (and one info-level no-op success):

| Outcome variant | Diagnostic code | Tracing level | Re-emit `buffer/dirty`? |
|---|---|---|---|
| `Saved` | (no error code; accepted-save info trace) | `info` | yes (`= false`) |
| `CleanSaveNoOp` | `WEAVER-SAVE-007` | `info` | yes (idempotent `= false`) |
| `StaleVersion` | `WEAVER-SAVE-002` | `debug` | no |
| `NotOwned` | (no diagnostic code; silent debug per FR-003 step 1) | `debug` | no |
| `InodeMismatch` | `WEAVER-SAVE-005` | `warn` | no |
| `PathMissing` | `WEAVER-SAVE-006` | `warn` | no |
| `TempfileIo` | `WEAVER-SAVE-003` | `error` | no |
| `RenameIo` | `WEAVER-SAVE-004` | `error` | no |

`WEAVER-SAVE-001` is CLI-side only (pre-dispatch lookup found no `buffer/version` fact); never reaches `dispatch_buffer_save`.

**Rationale**:

- Per-variant `entity` and `path` fields enable structured `tracing` output without re-derivation. Mirrors slice-004's `BufferEditOutcome` design.
- Separate `Saved` vs `CleanSaveNoOp` variants distinguish the dirty-path success from the clean-path no-op success — both re-emit `buffer/dirty = false`, but only `Saved` performs disk I/O. The trace records the distinction.
- `InodeMismatch` carries `expected` + `actual` for diagnosis; an operator inspecting the trace knows immediately whether the file was atomically replaced (different inode at the same path) vs missing (PathMissing, separate variant).
- `TempfileIo` and `RenameIo` are split because the recovery posture differs: tempfile-IO failures are typically operator-actionable (disk full, permissions); rename-IO failures are typically configuration-actionable (cross-filesystem, read-only mount). Different diagnostic codes (-003, -004) reflect this.

**Alternatives considered**:

- **Single `IoFailure { step: WriteStep, error: io::Error }` variant** — collapses -003 and -004 into one code, losing the tempfile-vs-rename diagnostic split. Rejected: operator inspection benefits from the codes.
- **Add `Refused { reason: RefusalReason }` enclosing inode-mismatch + path-missing** — over-nesting; the two refusals have distinct diagnostic codes and detail fields; flat enum is cleaner.
- **Boolean `applied: bool` + separate detail enum** — separation of concerns sounds clean but every consumer (the publisher's tracing emitter, the test harness, the metrics layer) needs both pieces; flat enum is faster to pattern-match.

## 10. CLI inspect-lookup reuse for `weaver save`

**Decision**: the `weaver save` subcommand handler reuses the same in-process inspect-library function that slice-004's `weaver edit` / `weaver edit-json` use (per slice-004 research §2). Library function: `weaver_core::cli::inspect::lookup_fact(client, entity, attribute) -> Result<FactValue>`. No subprocess; no new RPC primitive.

**Rationale**:

- Slice 004 already proved the pattern: in-process `BusMessage::InspectRequest` round-trip, `InspectionDetail.value: FactValue` carries the looked-up fact value, library function maps `FactNotFound` to a CLI error.
- Slice 005 needs the same lookup shape (`<entity>:buffer/version` → current version u64). One function call; no new architecture.
- Buffer-not-opened detection is mechanically clean: same `FactNotFound` → `WEAVER-SAVE-001` exit 1 path as slice-004's `WEAVER-EDIT-001`.

**Alternatives considered**:

- **Spawn `weaver inspect <entity>:buffer/version --output=json`** — process-spawn latency (~30 ms baseline); same anti-pattern slice 004 already rejected. Out.
- **Skip the lookup; emit at `version=0` and let the service stale-drop** — works but defeats the buffer-not-opened CLI fast-fail. Out.

## 11. Documentation lockstep — `docs/07-open-questions.md §28`

**Decision**: slice 005 closes §28 by updating the entry's status to `RESOLVED` with the 2026-04-29 re-derivation framing. Specifically:

- Status line changes to `RESOLVED`.
- A new paragraph at the top names the resolution: "Resolved in slice 005 (2026-04-29 constitutional re-derivation): producer-minted UUIDv8 EventIds with hashed-producer-instance-id prefix. Service producers hash `ActorIdentity::Service::instance_id` to 58 bits via SipHash; non-Service producers generate a per-process UUIDv4 and hash similarly. The listener does NOT stamp; producer's local id is final. Listener-side prefix-vs-provenance verification is DEFERRED to slice 006 alongside FR-029. See `specs/005-buffer-save/spec.md` FR-019..FR-024 + `specs/005-buffer-save/research.md §5, §12`."
- The "Candidate resolutions" section is preserved but annotated: (a) `[ADOPTED — UUIDv8 with hashed producer-instance-id prefix; spoofing-detection deferred to FR-029 close-out in slice 006]`, (b) `[NOT ADOPTED — addresses inspect-side surface only, not the producer-side mint hazard]`, (c) `[NOT ADOPTED — wire-bloat + Hash/Eq complexity at every EventId site]`.
- The "Revisit triggers" section is preserved with strikethrough annotations indicating each trigger is addressed by the resolution; the "Reopens at" line points at slice 006 for the carry-forward prefix-vs-provenance check.

**Rationale**:

- Constitution §17 (Documentation in lockstep) requires open-question entries to remain authoritative. Slice 005 is the resolving slice; the doc update lands as part of the spec-amendment commit (Step 1 of the slice-005-session-2 rework plan).
- Keeping the candidate-resolution and revisit-trigger context preserves "why this choice over those" for future archaeologists, including the supersession note that the original 2026-04-27 listener-stamping framing was rejected on per-principle constitutional audit (see §12 of this research document for the audit table).

**Alternatives considered**:

- **Delete §28 entirely** — loses the historical context, including the 2026-04-27 → 2026-04-29 re-derivation arc. Rejected.
- **Mark RESOLVED but leave content unchanged** — confusing for future readers (status disagrees with body). Rejected.
- **Open a new §30 for the carry-forward prefix-vs-provenance verification rather than using the existing FR-029 deferral** — fragments the unauthenticated-channel hazard class. The "Reopens at" pointer in §28 + the FR-029 cross-reference is sufficient. Rejected.

## 12. UUIDv8 producer-ID namespacing — constitutional re-derivation (2026-04-29)

**Decision**: `EventId` becomes `EventId(Uuid)` (UUIDv8) with the producer's hashed identity in the high 58 bits and nanoseconds in the low 64 bits (see §5 for wire-shape detail). Producers mint locally; the listener does not stamp.

This decision **supersedes** §1 (BusMessage<E> generic refactor) and §4 (EventOutbound ↔ Event relationship) and **replaces** the original §5 (listener-side AtomicU64 counter) and §8 (producer→EventOutbound migration).

**Why §28(a) listener-stamping was rejected**

Per-principle audit against `docs/00-constitution.md` (v0.2):

| § | Principle | Listener-stamping verdict | UUIDv8 verdict |
|---|---|---|---|
| 1 | No monolithic runtime; no privileged opaque locus | Concentrates ID authority at the listener (a centralisation regression) | Distributes; listener routes/records but doesn't arbitrate ID space |
| 2 | Everything Is Introspectable | Breaks bootstrap → bootstrap-fact chain (`weaver-buffers`'s `bootstrap_tick` shares the BufferOpen event's id as `causal_parent` for its bootstrap facts; under listener-stamping the producer cannot synchronously learn that id) | Producer-minted ID locally known; chain works trivially |
| 6 | Distribution Is First-Class; communication explicit; partial knowledge | Producer never learns its own event's stamped id (knowledge gap, no reconciliation under lossy delivery / fire-and-forget) | Producer's local knowledge matches the trace's stored knowledge |
| 11 | Shared State vs Local View; reconcilable | Producer↔trace divergence with no path | No divergence; agreement by construction |
| 12 | Composition Is First-Class — inspect / debug / "understand why compositions fired" | "Why this fact?" walk loses the bootstrap link | Walk preserved structurally |
| 15 | Provenance Is Mandatory: source, authority, **causal chain**, freshness, derivation | Causal chain truncated for originator-pattern facts | Causal chain preserved end-to-end |
| 16 | Explainability Over Cleverness | Listener-centralisation IS the cleverness; the slice-003 affordance loss IS the explainability cost | Walks back the cleverness; pays no explainability cost |
| 17 | Multi-Actor Coherence; conflicts explicit; no opaque authority | Listener as ID-arbitration authority — centralised | Authority distributed; producer namespace structurally encoded in the ID |

Listener-stamping was misaligned on **§2 / §11 / §12 / §15 / §16** (the chain regression) AND in tension with **§1 / §6 / §17** (centralisation). UUIDv8 with producer-ID namespacing repairs all of these simultaneously.

**Carry-forward constitutional deferral**

UUIDv8 carries one residual deferral: §17 ("make conflicts and overlaps between contributions explicit and inspectable") requires the listener to verify that an inbound event's UUIDv8 producer-prefix matches the connection's authenticated `ActorIdentity`. Without this check, a malicious producer can spoof another producer's prefix silently.

This is the **same hazard class as FR-029** ("unauthenticated edit/save channel — any process with a bus connection can dispatch any ActorIdentity"). FR-029 is already an explicitly-accepted Known Hazard for slice-005, deferred to close before slice-006. The UUIDv8-prefix-verification check joins FR-029's deferral — slice-006 (agent emitter introduction; first non-CLI ActorIdentity producer) is the natural close-out for both.

**Hash function: SipHash via `DefaultHasher`**

Stability across re-invocations matters less than collision-resistance, because slice-005 traces are in-memory only (do not survive listener restart). `std::collections::hash_map::DefaultHasher` (SipHash) is already in std; in-process stable; cross-rust-std-version stability is not a load-bearing property here. `xxhash-rust` is the natural upgrade path if a future slice persists traces; deferred until that need surfaces.

**Per-process UUIDv4 source for non-Service producers**

Each producer process generates a per-process UUIDv4 at startup, held in a `OnceLock<Uuid>` (or the equivalent for the producer's runtime). The first call to `EventId::mint_v8` on the producer initialises the UUIDv4; all subsequent mints in the process reuse it. Producer restart yields a fresh UUIDv4 — that is fine; in-memory traces don't survive listener restart anyway.

**Alternatives considered (re-stated for completeness)**

- **Listener-stamped IDs** — rejected per the per-principle audit above.
- **UUIDv4 alone, no producer-ID partitioning** — rejected on VM step-clock collision risk.
- **(UUIDv4, producer_id) tuple form** — rejected on wire bloat + Hash/Eq complexity at every `EventId` site (`HashMap<EventId, _>` becomes a tuple-keyed map; every `causal_parent: Option<EventId>` is two CBOR fields per occurrence).
- **UUIDv7** — single-namespace; same collision concern as UUIDv4 alone in multi-producer scenarios; loses the producer-prefix recoverability for display purposes.

---

*Phase 0 complete. All implementation-level decisions named with rationale + alternatives. Phase 1 artifacts (data-model.md, contracts/bus-messages.md, contracts/cli-surfaces.md, quickstart.md) follow.*
