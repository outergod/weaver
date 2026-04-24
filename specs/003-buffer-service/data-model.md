# Data Model — Slice 003 (Buffer Service)

Full type-and-fact model touched by slice 003. Complements `contracts/bus-messages.md` (wire shapes) and `contracts/cli-surfaces.md` (shell surfaces). Shapes here are authoritative in-process Rust types; wire representations serialize from them via `serde` + `ciborium`.

## Entities

### Buffer entity

Opaque, path-derived `EntityRef` addressing one opened file.

- **Identity**: derived from the canonicalized absolute path of the file via a stable hasher, with **bit 61 reserved** to distinguish buffer entities from repo (bit 63) and watcher-instance (bit 62) namespaces. See [§Entity-id derivation](#entity-id-derivation) below for the precise derivation.
- **Lifecycle**: comes into existence the moment the buffer service successfully opens a file and publishes the entity's bootstrap facts; dissolves when every fact it anchors is retracted (either by clean shutdown, by the core's `release_connection` on a dropped bus connection, or by future explicit buffer-close events in slice 004+).
- **Interpretation**: opaque per constitution §3. Interpretation arises from the asserted facts (path, size, dirty, observable) and the buffer's implicit `:content` component.
- **Multi-instance same path**: opening the same path from two separate `weaver-buffers` invocations hashes to the same `EntityRef`. The core's per-`(family, entity)` authority check rejects the second claim (first-instance wins).

### `:content` component (conceptual)

The bytes of the opened file, held in the buffer service's memory and mirrored on disk. **Not implemented as an in-code `Component` type this slice.**

- Slice 003 ships the *authority* (the buffer service is the owner) and the *derived facts* (`buffer/byte-size`, `buffer/dirty`); it does NOT ship the component-primitive infrastructure (`Component` trait, `get-component` host primitive, update-in-place semantics).
- Internally represented by `buffers::model::BufferState` (see [§Internal types](#internal-types)): `{ path: PathBuf, content: Vec<u8>, memory_digest: [u8; 32] }`.
- Component infrastructure is deferred per `docs/07-open-questions.md §26`; a later slice will formalize the `Component` trait and the `:content` type, at which point `BufferState.content` graduates into a proper `:content` component.

### Buffer service instance

The running `weaver-buffers` process.

- **Identity on the bus**: `ActorIdentity::Service { service_id: "weaver-buffers", instance_id: <UUID v4 at process start> }`. Kebab-case service-id per Amendment 5; UUID v4 per slice 002 Clarification Q3.
- **Instance entity**: a separate `EntityRef` derived from the instance UUID (mirroring slice 002's watcher-instance entity). **Bit 62 reserved** — we reuse the git-watcher's watcher-instance namespace bit intentionally: the instance entity is fundamentally a "running service instance," and the TUI / inspection machinery already distinguishes it from domain entities (repos, buffers) by the entity's asserted facts. See [§Entity-id derivation](#entity-id-derivation).
- **Facts anchored**: `watcher/status` (single-assertion fact, value mirrors `LifecycleSignal`) on the instance entity. No `buffer/*` fact is anchored on the instance entity.

## Fact-family schemas

All new this slice. All v0.1.0. All authored exclusively by `weaver-buffers` (single-writer authority per arch §5). Wire representations follow the existing `FactValue` adjacent-tagging convention.

### `buffer/path`

- **Authority**: `weaver-buffers`
- **Value type**: `FactValue::String` — canonical absolute path (as returned by `std::fs::canonicalize`), UTF-8.
- **Cardinality**: exactly one `buffer/path` fact per buffer entity. Never retracted except on shutdown / disconnect. Never updated (path is the entity's identity; a change of path = a change of entity).
- **Bootstrap order**: first fact asserted on open.

### `buffer/byte-size`

- **Authority**: `weaver-buffers`
- **Value type**: **`FactValue::U64(u64)`** — new `FactValue` variant landing under the bus-protocol MAJOR bump. Byte count of the service's in-memory content (equivalently, the file content read at open time — slice 003 has no in-process mutation).
- **Cardinality**: exactly one per buffer entity.
- **Update trigger**: in slice 003, only on external-mutation recovery from degraded. (Slice 004+ will update on mutation.)

### `buffer/dirty`

- **Authority**: `weaver-buffers`
- **Value type**: `FactValue::Bool` — `true` iff memory digest ≠ disk digest at the last successful observation.
- **Cardinality**: exactly one per buffer entity.
- **Update trigger**: on any observation where dirty state changes (re-assertion overwrites the prior value via same-key-same-owner publish).
- **Semantic note**: this is the *physical* dirty flag (per Clarification Q6-equivalent in Assumptions). Never retracts mid-life; set to `false` when memory == disk, `true` otherwise.

### `buffer/observable`

- **Authority**: `weaver-buffers`
- **Value type**: `FactValue::Bool` — `false` when the buffer's file is mid-session unreadable (permissions change, file deleted, filesystem unmount); `true` when the file is currently readable.
- **Cardinality**: exactly one per buffer entity (while the buffer is tracked). Transitions are edge-triggered per slice 002 F21: published once per healthy→unobservable or unobservable→healthy boundary, NOT per failed poll.
- **Relationship to `watcher/status`**: orthogonal (per Clarification 2026-04-23). Per-buffer transient failure flips `buffer/observable` only. Service-level `watcher/status=degraded` fires only when the service itself cannot serve (all buffers unobservable, or bus unreachable).

### `watcher/status` (reused from slice 002)

- **Authority**: the asserting service (for this slice: `weaver-buffers`). Slice 002's `git-watcher` is a separate authority on a different instance entity; authorities do not collide.
- **Value type**: `FactValue::String` — mirrors `LifecycleSignal` (`started` / `ready` / `degraded` / `unavailable` / `restarting` / `stopped`).
- **Keying**: the service's per-invocation instance entity (derived from the UUID).
- **Vocabulary for slice 003 buffer service**: see [§Service-level lifecycle state machine](#service-level-lifecycle-state-machine).

### Fact-family interaction invariants

1. **Component-discipline invariant (SC-306)**: NO fact value authored by the buffer service carries buffer content. For every `Fact` the service emits, `fact.value` is one of `FactValue::String` (path), `FactValue::U64` (byte-size), or `FactValue::Bool` (dirty / observable). Asserted as a property test over random observation sequences.
2. **Bootstrap mutual presence**: a buffer entity in the fact store either has the full bootstrap set `{path, byte-size, dirty, observable}` asserted, or has no `buffer/*` fact asserted. Partial bootstrap is a contract violation.
3. **`watcher/status` orthogonality**: transitions on `buffer/observable` of a *single* buffer MUST NOT trigger a `watcher/status` re-publication unless the transition leaves zero buffers currently observable.
4. **Dirty ⇔ memory-vs-disk**: `buffer/dirty = f(memory_digest, disk_digest)` is a pure function. The service's poll tick computes `disk_digest`, compares to the cached `memory_digest`, and determines the bool. No state between poll and publication.

## Internal types

In-memory Rust types inside `buffers/src/model.rs`. Not on the wire.

### `BufferState`

```rust
pub struct BufferState {
    /// Canonicalized absolute path. Stable for the lifetime of the state.
    path: PathBuf,

    /// Buffer entity derived from `path` via `buffer_entity_ref(path)`.
    /// Cached to avoid re-derivation on every publish.
    entity: EntityRef,

    /// In-memory content, loaded once at open.
    /// Slice 003: never mutated after open.
    /// Slice 004+: mutated by editing events; digest updated in lockstep.
    content: Vec<u8>,

    /// SHA-256 of `content`. Computed at open; compared to disk digest on each poll.
    memory_digest: [u8; 32],

    /// Last-observed dirty state. Drives edge-triggered publishing.
    last_dirty: bool,

    /// Last-observed observability. Drives edge-triggered publishing.
    last_observable: bool,
}
```

- **Invariant**: `memory_digest == sha256(content)` at all times. Any code path that mutates `content` MUST update `memory_digest` in the same expression; enforced by making `content` private and exposing only `content()` / `set_content(&mut self, ...)` accessors. (Slice 003 never mutates; the set-content path lands in slice 004.)
- **Invariant**: `entity == buffer_entity_ref(path)` — cached; re-derivable.

### `BufferObservation`

```rust
pub struct BufferObservation {
    pub byte_size: u64,     // content.len() as u64
    pub dirty: bool,        // memory_digest != disk_digest
    pub observable: bool,   // true iff disk read succeeded this tick
}
```

Pure output of `observer::observe_buffer(&state) -> Result<BufferObservation, ObserverError>`.

### `ObserverError`

Categorized per FR-016 / FR-017:

```rust
pub enum ObserverError {
    /// File not readable mid-session (transient: permission flicker, mid-rename race).
    /// Publisher flips buffer/observable=false; edge-triggered.
    TransientRead { path: PathBuf, source: std::io::Error },

    /// File does not exist mid-session (deleted, unmounted).
    /// Publisher flips buffer/observable=false; edge-triggered.
    Missing { path: PathBuf },

    /// File exists but is no longer a regular file (replaced by a directory, socket, etc.).
    /// Publisher flips buffer/observable=false; the buffer stays tracked but is "lost."
    NotRegularFile { path: PathBuf },

    /// Startup-only: path does not exist, is a directory, is unreadable, or exceeds memory.
    /// Publisher exits with code 1. Never encountered after bootstrap.
    StartupFailure { path: PathBuf, reason: String },
}
```

## Entity-id derivation

The workspace already uses path-hashing for `git-watcher` repo entities (bit 63) and UUID-hashing for watcher-instance entities (bit 62). Slice 003 adds buffer entities at bit 61.

```rust
/// `buffers/src/publisher.rs` (or a shared `core/src/types/entity_ref.rs` helper)
pub fn buffer_entity_ref(path: &Path) -> EntityRef {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    // Reserve bit 61 so buffer entities don't collide with repo (bit 63)
    // or watcher-instance (bit 62) namespaces. Clear bits 62 and 63 so a
    // collision-free low-order hash doesn't accidentally look like a repo
    // or instance entity.
    let h = (hasher.finish() | (1u64 << 61)) & !((1u64 << 62) | (1u64 << 63));
    EntityRef::new(h)
}
```

- **Input**: the *canonicalized* absolute path (after `std::fs::canonicalize`). Two argv entries that canonicalize to the same path hash to the same entity — the basis of FR-006a (CLI dedup).
- **Output**: a 64-bit value with bit 61 = 1, bits 62/63 = 0, and the remaining 61 bits derived from `DefaultHasher` over the path bytes.
- **Reserved-bit rationale**: a trace inspecting a fact can tell at a glance whether the entity is a buffer, a repo, or a service instance; this is operator-visible information and reduces the "what kind of entity is this?" cognitive load.

Future slices that introduce new entity namespaces (project entities in slice 005, agent entities in slice 006+) will each claim their own reserved bit, documented in the same `entity_ref.rs` module.

## Service-level lifecycle state machine

`watcher/status` values on the service's instance entity, with allowed transitions. Edge-triggered per slice 002 F21: each transition publishes exactly once.

```text
              (process start)
                   │
                   ▼
                started ─────────── (bootstrap all N buffers) ─────────┐
                                                                        ▼
                                                                      ready
                                                                        │
                                        ┌───────────────────────────────┤
                                        │                               │
                                        ▼                               ▼
            (bus unreachable OR all N buffers      (bus regained OR any buffer
             simultaneously unobservable)           becomes observable again)
                                        │                               │
                                        ▼                               │
                                    degraded ──────────────────────────┘
                                        │
                                        │ (SIGTERM / SIGINT / bus EOF)
                                        ▼
                                  unavailable
                                        │
                                        ▼
                                    stopped (process exit)
```

- **`started`**: published once, at process start, *before* any per-buffer bootstrap. `causal_parent = None`.
- **`ready`**: published once, after ALL N initial-bootstrap buffers have published their full bootstrap set successfully. `causal_parent = None`. Never fires per-buffer.
- **`degraded`**: published only when the service itself cannot serve. Two triggers:
  - Bus writer returns an unrecoverable error (and the service has not yet begun shutdown).
  - Every `buffer/observable` currently owned by the service is `false` simultaneously.
- **`ready` (recovery)**: re-published when the `degraded` conditions clear (bus regained, or at least one buffer becomes observable again).
- **`unavailable`**: published at the start of shutdown, *after* per-buffer retractions are issued.
- **`stopped`**: published last before the bus connection closes.
- **`restarting`**: reserved; not emitted by `weaver-buffers` in slice 003.

### Per-buffer observability state machine

Orthogonal to service-level lifecycle. Drives `buffer/observable` on each buffer entity. Edge-triggered per buffer.

```text
   (open succeeds)
        │
        ▼
    observable=true ─ (file becomes unreadable) ─► observable=false
                                                        │
        ┌───────────────────────────────────────────────┤
        │ (file becomes readable again)                 │
        ▼                                               │
    observable=true ◄───────────────────────────────────┘
```

- Publishing only at the boundary: an initial transient read failure fires one `buffer/observable=false`; subsequent failed polls within the same degraded window are silent until the observation succeeds again.
- When the service enters clean shutdown, each buffer's `buffer/observable` is retracted (alongside `buffer/path`, `buffer/byte-size`, `buffer/dirty`) — not asserted as `false`.

## Bootstrap sequence (deterministic)

For a service invoked as `weaver-buffers <path₁> <path₂> ... <pathₙ>` where the paths de-duplicate to a set `{P₁, P₂, …, Pₘ}` (m ≤ n):

```text
1.  Connect + handshake (protocol 0x03).
2.  Publish: watcher/status=started    (instance entity; causal_parent=None).
3.  For each Pᵢ in declaration order (after dedup):
    3a.  Open file: read into memory, compute memory_digest, initialize BufferState.
    3b.  Generate synthesized bootstrap-tick event id Eᵢ (per-buffer).
    3c.  Publish, all with causal_parent=Some(Eᵢ):
         - buffer/path = Pᵢ        (FactValue::String)
         - buffer/byte-size = |content| (FactValue::U64)
         - buffer/dirty = false    (FactValue::Bool)
         - buffer/observable = true (FactValue::Bool)
    3d.  If step 3a fails for any Pᵢ: emit ObserverError::StartupFailure,
         emit a structured miette diagnostic on stderr, retract any facts
         previously asserted in steps 3c, exit with code 1.
4.  Publish: watcher/status=ready      (instance entity; causal_parent=None).
5.  Enter poll loop.
```

Deterministic: for fixed input CLI args and fixed file contents, the bootstrap publishes the same fact sequence, with per-buffer causal parents distinct but reproducible (synthesized from the poll-tick counter initialized at 0).

## Validation rules (for tests)

1. **Path canonicalization idempotence**: `buffer_entity_ref(canonicalize(p)) == buffer_entity_ref(canonicalize(p))` for any `p`. Property test over arbitrary path-shaped strings.
2. **Path-based entity equality**: for any two paths `p₁, p₂` where `canonicalize(p₁) == canonicalize(p₂)`, `buffer_entity_ref(p₁) == buffer_entity_ref(p₂)`.
3. **Reserved-bit invariants**: `buffer_entity_ref(_)` always has bit 61 set and bits 62/63 cleared. Property test.
4. **Bootstrap atomicity**: no e2e test observes a trace state with `buffer/path` asserted but `buffer/dirty` not-yet-asserted for the same buffer entity. (Test harness synchronizes on `watcher/status=ready`; Users MAY observe partial bootstraps in the window between step 3a and step 4, but step 4 guarantees completion.)
5. **F23 live-fact-provenance**: scenario test — inject a behavior-authored `buffer/dirty=true` into the fact store, then have the buffer service assert `buffer/dirty=false` via the bus; `weaver inspect` on that key returns the service's provenance, not the behavior's. (FR-013.)
6. **Component-discipline property** (SC-306): across any sequence of buffer observations, every `FactValue` emitted by the publisher is `FactValue::String` (for path), `FactValue::U64` (for byte-size), or `FactValue::Bool` (for dirty/observable). No `FactValue::Bytes` or similar is ever emitted.
7. **`BufferOpen` idempotence** (FR-011a): firing two successive `BufferOpen { path }` events for the same canonical path to the service's event handler results in exactly one `(entity, path, byte-size, dirty, observable)` fact set on the bus — never two.
8. **Per-buffer edge-triggering** (FR-016): a file remains unreadable for 5 consecutive polls → the bus sees exactly one `buffer/observable=false` assertion in that window.
9. **Service-level edge-triggering** (FR-016a): the bus becomes unreachable → the service re-establishing the connection should NOT re-publish redundant `watcher/status=ready` without a `watcher/status=degraded` transition between them.
10. **Retract-on-clean-shutdown** (FR-020): all `buffer/*` facts the service authored → retracted before `watcher/status=unavailable` fires.
11. **Retract-on-dirty-shutdown** (SC-303): core's `release_connection` retracts all facts owned by the dropped connection within 5 s.

## Cross-surface references

- `contracts/bus-messages.md` — wire shapes for all fact assertions, event variants, lifecycle frames.
- `contracts/cli-surfaces.md` — CLI arguments, exit codes, human/JSON output shapes for `weaver-buffers`; changes to `weaver` (simulate-edit/simulate-clean removal) and `weaver-tui` (Buffers render section).
- `quickstart.md` — end-to-end walkthrough verifying each SC (SC-301..SC-307).
- `spec.md` — user stories, functional requirements, success criteria, Clarifications 2026-04-23.
- `research.md` — library choices, observation strategy, CLI error-classification stance.
- `docs/01-system-model.md §2.4` — Components vs Facts; canonical justification for the slice's content-discipline invariant.
- `docs/07-open-questions.md §18, §26` — undo model and component-infrastructure deferrals this slice orbits.
