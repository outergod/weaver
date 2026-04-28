# Research — Slice 005 (Buffer Save)

Phase 0 decisions. Each entry resolves an implementation-level question that the plan depended on. Rationale + alternatives preserved so post-slice reviewers understand *why*, not just *what*.

## 1. `BusMessage` shape under §28(a)

**Decision**: introduce a generic envelope `BusMessage<E>` parameterised over the event payload type. Two type aliases pin the directions:

```rust
pub enum BusMessage<E> {
    Hello { ... },
    Welcome { ... },
    Event(E),
    FactAssert(Fact),
    FactRetract(FactKey),
    SubscribeFacts(SubscribePattern),
    SubscribeEvents(EventSubscribePattern),
    InspectRequest { ... },
    InspectResponse { ... },
    EventInspectRequest { ... },
    EventInspectResponse { ... },
    Error { ... },
    Ping, Pong,
}

pub type BusMessageInbound  = BusMessage<EventOutbound>;
pub type BusMessageOutbound = BusMessage<Event>;
```

Codec is direction-typed:
- `read_message(...) -> BusMessageInbound` — what the listener receives from a connected client.
- `write_message(... , msg: BusMessageOutbound)` — what the listener broadcasts to subscribers.

A producer that wishes to send an event constructs `BusMessageInbound::Event(EventOutbound { .. })`; a subscriber receives `BusMessageOutbound::Event(Event { .. })`. The wire byte representation is identical for non-Event variants; only the `Event` variant's payload differs.

**Rationale**:

- The asymmetry under §28(a) is fundamental: outbound events have no ID; at-rest events do. The type system MUST express this — Q1's resolution rejected sentinel-as-meaning. A single `BusMessage` enum with two event variants (`Event(Event)` AND `EventOutbound(EventOutbound)`) would compile-allow producers to construct the wrong variant, requiring runtime rejection at the listener — the same anti-pattern Q1 declined.
- The generic `BusMessage<E>` keeps every direction-agnostic variant defined exactly once. Only the `Event` carrier varies. Maintenance burden under future variant additions is unchanged from slice 004.
- Codec direction-typing prevents a producer from accidentally sending `BusMessageOutbound::Event(Event { id: <forged>, .. })` — the codec functions enforce the right type at the boundary.

**Alternatives considered**:

- **Two parallel non-generic enums** (`BusMessageInbound`, `BusMessageOutbound`) with most variants duplicated — works but every direction-agnostic variant addition (Ping, Hello, Error) must be made twice. Rejected: ceremony without payoff.
- **Single `BusMessage` with two event variants** (`Event(Event)` for outbound + `EventOutbound(EventOutbound)` for inbound) — collapses to runtime discrimination at the listener; Q1's anti-pattern.
- **Refactor `Event::id` to `Option<EventId>`** — equivalent to `EventId::Placeholder` (option-typed sentinel). Rejected at Q1.
- **Wrapper enum `EventForm::{Outbound(EventOutbound), Stamped(Event)}`** — sentinel-by-another-name; same Q1 anti-pattern.

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

## 4. `EventOutbound` ↔ `Event` relationship

**Decision**: two parallel structs in `core/src/types/event.rs`:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct EventOutbound {
    pub payload: EventPayload,
    pub provenance: Provenance,
    pub causal_parent: Option<EventId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct Event {
    pub id: EventId,
    pub payload: EventPayload,
    pub provenance: Provenance,
    pub causal_parent: Option<EventId>,
}

impl Event {
    pub fn from_outbound(id: EventId, outbound: EventOutbound) -> Self {
        Self { id, payload: outbound.payload, provenance: outbound.provenance, causal_parent: outbound.causal_parent }
    }
}
```

`From<(EventId, EventOutbound)> for Event` is exposed for ergonomics. **No reverse conversion** is provided — events do not regress to outbound shape after stamping.

**Rationale**:

- Each type has its own serde derive; codec emits/accepts shapes that are byte-identical to slice-004's `Event` (modulo the absent `id` field on outbound). No special-cased serializer.
- Field-name parity (`payload`, `provenance`, `causal_parent`) makes inflation a trivial copy. Future additions to `Event` add to both structs in lockstep; the relationship `Event = EventOutbound + id` is preserved structurally.
- Public conversion in one direction only — the type system prevents a stamped Event from being downgraded to outbound shape (which would lose its trace identity).

**Alternatives considered**:

- **Single `Event<HasId: Bool>` with const-generic id presence** — Rust's const generics don't support struct-field elision; would require unsafe transmute or duplicate impls. Rejected.
- **`Event` with `id: PhantomId`** — `PhantomId` either holds a real `EventId` or is zero-sized for outbound. Same complexity as Q1's sentinel approach. Rejected.
- **Flatten via `EventOutbound: Deref<Target=Event>`** — semantic violation (`Event` requires `id`; `EventOutbound` doesn't have one).

## 5. Stamped-EventId counter location

**Decision**: an `AtomicU64` field on `TraceStore`, named `next_event_id`, initialised to `1` (skipping `EventId::ZERO`). `TraceStore` exposes:

```rust
impl TraceStore {
    pub(crate) fn stamp_and_insert(&self, outbound: EventOutbound) -> Event {
        let id_raw = self.next_event_id.fetch_add(1, Ordering::Relaxed);
        let id = EventId::new(id_raw);
        let event = Event::from_outbound(id, outbound);
        self.insert_event(event.clone());
        event
    }
}
```

The listener calls `trace_store.stamp_and_insert(outbound)` once per accepted `BusMessageInbound::Event(_)`, then broadcasts the returned stamped event to subscribers via the existing `EventSubscriptions::broadcast` path.

**Rationale**:

- Single owner for stamping authority — `TraceStore` is the natural home because stamped IDs ARE trace identities.
- `Relaxed` ordering is sufficient: the counter increments are independent (no two threads stamp the same ID; the atomic-fetch-add primitive is the synchronisation). Stamped order does not need to match insertion order with finer-than-Relaxed semantics — only uniqueness matters.
- Counter starts at `1`: `EventId::ZERO` retains its sentinel meaning for "no causal parent" lookups (FR-024). Skipping it via the initial value is structurally cleaner than a runtime check.
- Counter resets on restart: the trace itself does not persist across core restarts (in-process trace only); pre-restart stamped IDs are never re-issued because they exist only in memory of the now-defunct trace. Long-running deployment is out of scope until trace persistence (a future slice).

**Alternatives considered**:

- **`Mutex<u64>` instead of `AtomicU64`** — needless contention for an integer increment.
- **Counter in `EventSubscriptions` or `BusListener`** — couples the stamping authority to transport, not to the trace. The trace's `by_event` index is what stamping serves.
- **UUID v7 (timestamp-ordered) instead of `u64`** — wider IDs (128 bits); larger CBOR; doesn't add value over a simple monotonic counter for a single-process trace.

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

**Decision**: all four producer-side `EventId::new(now_ns())` mint sites migrate atomically with the wire-bump `0x04 → 0x05`. No phased migration; no parallel pre/post-bump support.

Migration sites:
1. `core/src/cli/edit.rs` (`weaver edit`, `weaver edit-json`) — replace `Event::new(EventId::new(now_ns()), payload, provenance)` with `EventOutbound { payload, provenance, causal_parent }`.
2. `buffers/src/publisher.rs` — replace producer-minted EventId at poll-tick re-emissions and `bootstrap_tick` allocation. Re-emitter sites (those that consume an inbound stamped Event and re-emit facts referencing `event.id` as `causal_parent`) are unaffected — they continue to read the stamped ID from the inbound `Event`.
3. `git-watcher/src/publisher.rs` — same migration as buffers' poll-tick.
4. `core/src/cli/save.rs` — born compliant; never minted EventId.

The wire bump is enforced at handshake (`Hello.protocol_version = 0x05`); pre-bump clients receive the version-mismatch error and close. Post-bump and pre-bump clients cannot coexist on one bus instance.

**Rationale**:

- Clean break is consistent with operator's "no users yet, wire-stability is not a concern" framing.
- Phased migration would require the listener to accept both `EventOutbound` (post-bump) AND `Event` (pre-bump from un-migrated producers) inbound — defeats the type-system enforcement of FR-021 and re-introduces runtime sentinel checks.
- The four mint sites are all in workspaces under `cargo build` together; atomic migration is a single PR / commit set.

**Alternatives considered**:

- **Phased: keep `0x04` accepting Event-shape, add `0x05` accepting EventOutbound-shape, sunset 0x04 in slice 006** — complicates the listener's accept logic for no operator benefit; perpetuates the producer-side mint hazard for one more slice. Rejected.
- **Per-producer migration without wire bump** — impossible: the wire shape change IS the migration; producers serialise the new shape, listener deserialises it.

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

**Decision**: slice 005 closes §28 by updating the entry's status from "PARTIALLY RESOLVED" (slice-004 closed deterministic instances) to "RESOLVED" (option a chosen, ID-stripped envelope sub-variant). The entry is rewritten:

- Status line changes to `RESOLVED`.
- A new paragraph at the top names the resolution: "Resolved in slice 005: option (a), ID-stripped-envelope sub-variant. Producers serialise `EventOutbound` (no `id`); the listener stamps `Event { id, .. }` on accept. See `specs/005-buffer-save/spec.md` FR-019..FR-024 and `specs/005-buffer-save/research.md §1, §4`."
- The "Candidate resolutions" section is preserved but annotated: (a) `[ADOPTED — ID-stripped envelope]`, (b) `[NOT ADOPTED — would have left trace-store internal gap]`, (c) `[NOT ADOPTED — scope-explosive]`.
- The "Revisit triggers" section is preserved; future reviewers landing at the §28 entry see the resolution + the original trade-off context.

**Rationale**:

- Constitution §17 (Documentation in lockstep) requires open-question entries to remain authoritative. Slice 005 is the resolving slice; the doc update lands as part of the slice's commit set.
- Keeping the candidate-resolution and revisit-trigger context preserves "why this choice over those" for future archaeologists.

**Alternatives considered**:

- **Delete §28 entirely** — loses the historical context. Rejected.
- **Mark RESOLVED but leave content unchanged** — confusing for future readers (status disagrees with body). Rejected.

---

*Phase 0 complete. All implementation-level decisions named with rationale + alternatives. Phase 1 artifacts (data-model.md, contracts/bus-messages.md, contracts/cli-surfaces.md, quickstart.md) follow.*
