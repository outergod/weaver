# Data Model — Slice 004 (Buffer Edit)

This document captures the new types introduced by slice 004, their wire shapes, validation rules, and the state-transition mapping for accepted/dropped edits. Inherits everything from slice 003's data model (`specs/003-buffer-service/data-model.md`) unchanged unless explicitly noted.

## New types

### `Position`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}
```

A 2D coordinate within a buffer.

- `line` — zero-based line index. The first line of the buffer is line `0`. Line counting follows `\n` as the line separator; `\r\n` is a `\r` byte at the end of the preceding line followed by `\n` as the separator. The number of lines is `bytes.iter().filter(|b| **b == b'\n').count() + 1` (or `+ 0` if the buffer ends in `\n`; see line-index table semantics below).
- `character` — zero-based **UTF-8 byte offset within the line's content**. NOT a UTF-16 code-unit offset (LSP-default departure; see plan §Summary and Assumptions for rationale). Must fall on a UTF-8 codepoint boundary within the line's bytes.

**Wire shape (JSON, kebab-case)**: `{"line": 42, "character": 12}` (struct field names; no enum tagging since `Position` is not a sum type).

**Wire shape (CBOR)**: plain CBOR map with text-string keys `"line"` and `"character"`; values are CBOR unsigned ints. No CBOR tag.

**Lexicographic ordering**: `(a.line, a.character) < (b.line, b.character)` is the implicit ordering used for `Range.start <= Range.end` validation and for sort-by-start in the apply pipeline.

### `Range`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}
```

A half-open interval on 2D buffer coordinates: inclusive `start`, exclusive `end`. Point ranges (`start == end`) represent insertion cursors.

**Validity**:
- `start <= end` (lexicographic compare on `(line, character)`); strict `start > end` is `ApplyError::SwappedEndpoints`.
- Both endpoints fall on UTF-8 codepoint boundaries within their respective lines' content (see Validation rules below).
- Both endpoints lie within the buffer's content (no `end.line > line_count`; see bounds rules below).

**Wire shape (JSON)**: `{"start": {"line": 42, "character": 12}, "end": {"line": 42, "character": 17}}`.

### `TextEdit`

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}
```

One atomic edit operation: replace the bytes within `range` by `new_text.as_bytes()`. The Rust field name is `new_text` (snake_case per Amendment 5 in-language idiom); the JSON field name is `new-text` (kebab-case per Amendment 5 wire idiom).

**Validity** (in addition to `Range` validity):
- NOT (`range.start == range.end AND new_text.is_empty()`) — that's a "nothing edit"; `ApplyError::NothingEdit`.
- `new_text` must be valid UTF-8 (enforced by `String` type at deserialisation).

**Pure-insert**: `range.start == range.end AND new_text != ""` — legitimate; applies as insert-at-position.
**Pure-delete**: `range.start < range.end AND new_text == ""` — legitimate; applies as content removal.

**Wire shape (JSON)**: `{"range": {"start": {...}, "end": {...}}, "new-text": "replacement bytes"}`.

### `EventPayload::BufferEdit`

```rust
pub enum EventPayload {
    BufferOpen { path: String },                                     // existing (slice 003)
    BufferEdit {                                                      // NEW (slice 004)
        entity: EntityRef,
        version: u64,
        edits: Vec<TextEdit>,
    },
}
```

The bus-level edit-dispatch record.

**Fields**:
- `entity` — the buffer to edit (canonical entity, derived from `buffer_entity_ref(canonical_path)` in slice 003).
- `version` — the emitter's snapshot of `buffer/version`. Service accepts iff `version == buffer's current buffer/version`.
- `edits` — atomic batch. Empty `Vec` is a valid no-op; large batches are bounded only by the 64 KiB wire-frame limit (no explicit cap per spec Q3).

**Delivery class**: lossy (per `EventPayload` convention, `docs/02-architecture.md §3.1`). No structured rejection path on the wire.

**Wire shape (JSON, adjacent-tagged kebab-case)**:
```json
{
  "type": "buffer-edit",
  "payload": {
    "entity": 42,
    "version": 7,
    "edits": [
      {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "new-text": "hello "}
    ]
  }
}
```

## Inherited entity & component model (unchanged from slice 003)

### Buffer entity

A file opened by `weaver-buffers`. Addressed by `EntityRef` derived from the file's canonicalised absolute path with bit 61 set (slice-003 namespace). Slice 004 does NOT change the derivation; an edit's `entity` field is bit-for-bit comparable to the entity asserted in `buffer/path` and `buffer/version` facts.

### `:content` component (conceptual)

The bytes of the opened file, held in `weaver-buffers`'s memory and mirrored on disk. Slice 003 establishes this as a conceptual component; slice 004 makes it **mutable in-memory** via `BufferState::apply_edits`. Disk content is NOT mutated — save-to-disk is slice 005.

The slice-003 invariant `memory_digest == sha256(content)` is preserved structurally by `apply_edits` (recomputed once at end of accepted batch).

### `BufferState` (slice 003 type, extended)

Slice 003 declared `BufferState { path, entity, content, memory_digest, last_dirty, last_observable }` with all fields private and a fallible `open()` constructor. Slice 004 extends with:

```rust
impl BufferState {
    pub fn apply_edits(&mut self, edits: &[TextEdit]) -> Result<(), ApplyError>;
}
```

The method validates the entire batch first (returning `Err` without mutating on any validation failure), then applies in descending-offset order, then recomputes `memory_digest`. See Validation rules below for the validation pipeline; see `research.md §3` for the apply algorithm.

`BufferState` does NOT track `buffer/version` — the version counter is held by the publisher (alongside the tracked-fact set), since version state is part of the *bus* contract, not the in-memory content.

### Publisher state (slice 003 layout, refactored)

Slice 003's publisher held `Vec<BufferState>` (CLI-argv-ordered) plus `BufferRegistry { owned: HashSet<EntityRef> }`. Slice 004 refactors to:

```rust
pub(crate) struct BufferRegistry {
    pub(crate) buffers: HashMap<EntityRef, BufferState>,
    pub(crate) versions: HashMap<EntityRef, u64>,
}
```

The map's keyset replaces the prior `HashSet` (entity ownership lookup is `buffers.contains_key(&e)`). The `versions` field tracks per-buffer `buffer/version` (initialised to `0` at bootstrap; bumped by `dispatch_buffer_edit` on accepted edit; never decremented). See `research.md §7` for the refactor rationale.

**Bootstrap-tick deterministic event IDs** (from slice 003) are computed at parse time and held in a separate vector (CLI-argv-ordered) — they're slice-003 bootstrap state, not edit state, and don't need to live in the registry.

## Validation rules

`BufferState::apply_edits` runs the following pipeline. Failure at any rule produces an `ApplyError` variant; the buffer is unchanged.

1. **Per-edit pre-check** (in input order, fail-fast on first offender):
   - **R1 (`SwappedEndpoints`)**: `edits[i].range.start > edits[i].range.end` (lexicographic compare).
   - **R2 (`OutOfBounds`)**: `edits[i].range.end.line > line_count`. Special case: `end.line == line_count AND end.character == 0` is permitted (it represents the position immediately past the last line's content); any other `end.line == line_count` value is out-of-bounds.
   - **R3 (`OutOfBounds`)**: `edits[i].range.{start,end}.character > line_byte_length(line)` for the relevant line, where `line_byte_length` is the number of bytes in that line's content (not counting its terminating `\n`). Beyond-EOL is rejected, NOT clamped.
   - **R4 (`MidCodepointBoundary`)**: `edits[i].range.start.character` does not fall on a UTF-8 codepoint boundary within the start line's bytes; same for `range.end.character` within the end line. Detected via `line_bytes.is_char_boundary(character as usize)` (Rust's stdlib `is_char_boundary` operates on `&str` byte-positions).
   - **R5 (`NothingEdit`)**: `edits[i].range.start == edits[i].range.end AND edits[i].new_text.is_empty()`.
   - **R6 (`InvalidUtf8`)**: residual safety — `edits[i].new_text` is not valid UTF-8. Should not fire under normal API use (`String` enforces UTF-8 at deserialisation); included for direct in-process construction.
2. **Sort-and-overlap-scan** (after per-edit pre-check passes):
   - Sort `edits` by `range.start` (lexicographic, stable; preserves input order on ties — though tied starts will trigger overlap on the next step).
   - Linear-scan adjacent pairs: if `sorted[i].range.end > sorted[i+1].range.start`, it's `ApplyError::IntraBatchOverlap { first_index: <pre-sort index of sorted[i]>, second_index: <pre-sort index of sorted[i+1]> }`.
   - Tied starts (`sorted[i].range.start == sorted[i+1].range.start`) are NOT overlap unless one of them has `start < end`. Two pure-inserts at the same point cursor are independent and BOTH apply; their relative apply order under descending-offset reverse iteration is stable-sort-determined.
3. **Apply phase** (only if validation passed):
   - Iterate `sorted` in descending order (last edit first).
   - For each: convert `(line, character)` start and end to single byte offsets via the line-index table; splice `self.content[start_byte..end_byte] = new_text.as_bytes()`.
   - Recompute `self.memory_digest = sha256(&self.content)`.

## State-transition mapping

### Accepted-edit transition

```
state-before:  buffer/version = N, buffer/byte-size = S, buffer/dirty = D, content_digest_disk = X
event:         BufferEdit { entity, version: N, edits: [valid] }
service-action:
  1. dispatch_buffer_edit returns Applied(new_state)
  2. publish FactAssert(buffer/byte-size, U64(S')) with causal_parent = Some(event.id)
  3. publish FactAssert(buffer/version, U64(N+1)) with causal_parent = Some(event.id)
  4. publish FactAssert(buffer/dirty, Bool(memory_digest != X)) with causal_parent = Some(event.id)
state-after:   buffer/version = N+1, buffer/byte-size = S' (new in-memory length), buffer/dirty = (memory_digest != X)
```

In slice 004, `memory_digest != X` is **always true** post-edit (because no save-to-disk path resets `X`); slice 005's save-to-disk will enable `buffer/dirty = false` after a successful save.

### Stale/future-version drop

```
state-before:  buffer/version = N
event:         BufferEdit { entity, version: M, .. } where M != N
service-action: dispatch_buffer_edit returns StaleVersion { current: N, emitted: M } or FutureVersion { current: N, emitted: M }
                tracing::debug! with reason "stale-version" or "future-version"
state-after:   unchanged
```

### Validation-failure drop

```
state-before:  buffer/version = N
event:         BufferEdit { entity, version: N, edits: [valid edit, invalid edit, valid edit] }
service-action: dispatch_buffer_edit returns ValidationFailure(ApplyError::*)
                tracing::debug! with reason "validation-failure-<kind>" + edit_index
state-after:   unchanged
```

### Non-owned-entity drop

```
state-before:  service does NOT own entity E
event:         BufferEdit { entity: E, version, edits: .. }
service-action: dispatch_buffer_edit returns NotOwned
                tracing::debug! with reason "unowned-entity"
state-after:   unchanged at this service. (Other services on the bus do not subscribe to BufferEdit.)
```

## Outcome enum (publisher-side)

```rust
pub(crate) enum BufferEditOutcome {
    Applied(EntityRef, u64 /* new version */, BufferState /* post-apply snapshot */),
    NotOwned,
    StaleVersion { current: u64, emitted: u64 },
    FutureVersion { current: u64, emitted: u64 },
    ValidationFailure(ApplyError),
}
```

Mirrors slice-003's `BufferOpenOutcome` shape: pub(crate), small fixed-variant enum, the dispatch handler returns one variant per receipt. The publisher's reader-loop arm matches on the outcome and decides what to publish.

## Validation property tests (P9)

Listed for `/speckit.tasks` to materialise:

- **CBOR + JSON round-trip on `EventPayload::BufferEdit`** under randomly-generated `Vec<TextEdit>`. Asserts `parse(emit(x)) == x` for both wire formats.
- **`apply_edits` validates iff the batch satisfies R1..R6 + no intra-batch overlap**. Generator: random buffers + random edit batches; assert `apply_edits` returns `Ok(())` ↔ batch is valid by the structural rules.
- **`apply_edits` is structurally pure on rejection**: under any failing batch, `state.memory_digest` is unchanged after `apply_edits` returns `Err`.
- **`apply_edits` preserves `memory_digest == sha256(content)` invariant on accept**: under any accepted batch, post-apply `state.memory_digest == sha256(state.content)`.
- **SC-406 emitter equivalence**: random `Vec<TextEdit>` produces byte-identical `EventPayload::BufferEdit` wire payload regardless of whether dispatched via `weaver edit` (positional) or `weaver edit-json` (JSON). Property test runs both CLIs and binary-compares the dispatched bytes.

## Forward references

- The slice-003 `:content` component remains conceptual. Slice 004 does NOT introduce a `Component` trait. Per `docs/07-open-questions.md §26`, component infrastructure is deferred. The forward direction for rejection observability (per spec Clarifications Q2) is a queryable error component on the buffer entity, in whichever slice introduces component infrastructure.
- Slice 005 will add `buffer/dirty = false` flips after successful disk save, completing the dirty-state ↔ disk-state coupling.
- Slice 006's agent will produce `BufferEdit` events as a bus client (NOT a CLI consumer); its `ActorIdentity` variant is TBD (see `research.md §6`).
