# Research — Slice 004 (Buffer Edit)

Phase 0 decisions. Each entry resolves an implementation-level question that the plan depended on. Rationale + alternatives preserved so post-slice reviewers understand *why*, not just *what*.

## 1. Module placement for the new struct types

**Decision**: New file `core/src/types/edit.rs` holding `TextEdit`, `Range`, `Position` and the `kebab-case` serde derives. Sibling of the existing `core/src/types/event.rs`. Re-exported through `core/src/types/mod.rs` and `core/src/lib.rs`.

**Rationale**:

- `Range` / `Position` / `TextEdit` are constitutional types of the bus protocol — they appear inside `EventPayload::BufferEdit`. Their natural home is `core/src/types/`, alongside `event.rs`, `fact.rs`, `entity_ref.rs`, `ids.rs`. Slice 003's `FactValue::U64` lives in `fact.rs` for the same reason: type-of-the-protocol lives in `core/src/types/`.
- A standalone module (`edit.rs`) keeps `event.rs` from growing — `EventPayload::BufferEdit { entity, version, edits: Vec<TextEdit> }` references the types but does not need to define them.
- The module file gives a natural home for the `kebab-case` JSON tests, the CBOR round-trip proptest, and the `Position::is_codepoint_boundary` helper.

**Alternatives considered**:

- **Inline the types in `event.rs`** — rejected: the file already mixes the `Event` envelope, the `EventPayload` enum, and the variant-payload scaffolding. Adding three more public types muddies the file's narrative.
- **Standalone `edit-types` workspace crate** — rejected: types are pure data with no behaviour, and they only matter on the bus boundary. Workspace fragmentation for three structs is over-architecture (P4).
- **Live in `buffers/src/`** — rejected: any subscriber on the bus (TUI, agent, CI script) needs to deserialise `EventPayload::BufferEdit`. Putting the types behind the `weaver-buffers` crate boundary forces those consumers to depend on the service crate, which is a layering inversion (the bus protocol is `core`'s domain; services consume it, not the other way around).

## 2. In-process inspect-lookup for CLI pre-dispatch `buffer/version` query

**Decision**: The `weaver edit` and `weaver edit-json` subcommand handlers reuse the existing `weaver_core::cli::inspect` library function — calling it as an in-process Rust function with the `<entity>:buffer/version` fact key as input. They do NOT spawn a child `weaver inspect` process; they do NOT invent a new RPC primitive. The library function's existing `BusMessage::InspectRequest` / `InspectResponse` round-trip handles the bus interaction.

**Rationale**:

- The `weaver inspect <entity>:<attribute>` machinery already exists (slice 002 introduced `BusMessage::InspectRequest`, slice 003 used it for `weaver-buffers` attribution). Re-using it from a sibling subcommand is a one-line library call, not new architecture.
- A child-process shell-out would inflate per-edit latency (process spawn ~30 ms baseline) and complicate error propagation across the process boundary. In-process keeps SC-401 ≤500ms comfortable.
- The lookup is a single `InspectRequest`/`InspectResponse` round-trip on the same bus connection the subcommand will use to dispatch the `BufferEdit` event. One handshake, two messages — minimal overhead.
- Buffer-not-opened detection is mechanically clean: `InspectResponse` returns `FactNotFound` (existing slice-002 result variant); the CLI handler maps that to `WEAVER-EDIT-001` exit 1.

**Alternatives considered**:

- **Subscribe to `buffer/*` for the entity, wait for snapshot replay, take the version** — works but is heavier (full subscription teardown after one read). Defeats the simpler `Inspect` API.
- **Skip the lookup; emitter constructs the version client-side from a previous read** — re-introduces the `--version` flag the operator explicitly rejected during clarify. Out.
- **Wait for two round-trips: lookup, then a confirm subscription** — converts fire-and-forget into something synchronous, contradicting the spec's "fire-and-forget CLI" Assumption.

## 3. Apply-edits algorithm

**Decision**: `BufferState::apply_edits(&mut self, edits: &[TextEdit]) -> Result<(), ApplyError>` performs:

1. **Validate the entire batch** before mutating anything:
   - For each `TextEdit`: `range.start.line < line_count`; `range.end.line < line_count` OR (`range.end.line == line_count AND range.end.character == 0`); `range.start.character` falls on a UTF-8 codepoint boundary within the start line's byte content; `range.end.character` falls on a UTF-8 codepoint boundary within the end line's byte content; `range.start <= range.end` (lexicographic compare of `(line, character)`); NOT (`range.start == range.end AND new_text.is_empty()`); `new_text` is valid UTF-8 (already enforced by `String` type at deserialisation).
   - Sort the batch by `range.start` (lexicographic `(line, character)`). Linear-scan adjacent pairs: reject if any pair's `prev.end > next.start` — that's intra-batch overlap.
2. **Apply** in **descending-offset order** (reverse of the sorted batch above) so earlier positions are not shifted by later applications:
   - For each edit: convert `(line, character)` to a single byte offset within `self.content` via the line-index table; splice `self.content[start_byte..end_byte]` to `new_text.as_bytes()`.
3. **Recompute** `self.memory_digest = sha256(&self.content)` once at end.

Returns `Err(ApplyError)` with the first detected validation failure (caller treats the whole batch as rejected); on `Ok(())` the buffer state is consistent and the digest invariant holds structurally.

**Rationale**:

- LSP 3.17 specifies descending-offset application for the same reason: it preserves position semantics across multiple edits without per-edit recomputation. Slice 004 inherits this convention so future LSP interop has zero impedance mismatch.
- Pre-validation guarantees atomicity at the API boundary: the only way to observe a partial state is for the caller to apply edits one-at-a-time, which `apply_edits` does NOT do.
- Single SHA-256 recompute (not per-edit) bounds CPU cost on the batched-edit path. Buffer size dominates; batch size is irrelevant.
- Line-index table: build on demand inside `apply_edits` (or cache on `BufferState` if profiling shows it matters; not yet measured). Linear scan over `self.content` to find newline positions is O(n) on buffer size — same as the SHA-256 recompute, so it does not change the asymptotic class.

**Alternatives considered**:

- **Apply-as-validate (apply edit i, then validate edit i+1 against the now-mutated state)** — rejected: violates atomicity. If edit i+1 fails, partial state is observable to a caller that aborts and never calls a rollback.
- **Snapshot-and-restore on failure (clone content; on Err, restore)** — rejected: doubles memory peak; pre-validation is structurally cleaner.
- **Apply in ascending order, rewriting subsequent positions** — rejected: O(n²) in batch size; bug-prone (off-by-one errors when computing position deltas across `\n` insertions). Descending-offset is the standard LSP convention precisely to avoid this.
- **Allow intra-batch overlap by composing edits left-to-right** — rejected per spec FR-007: composition semantics are subtle (does "delete `[0:0-0:5]` then insert `'X'` at `0:3`" mean insert into the original content or the post-delete content?), and the operator/agent is better served by a clean "you wrote an overlap, fix it" error than a plausibly-wrong silent compose.

## 4. ApplyError taxonomy

**Decision**: `enum ApplyError { OutOfBounds { edit_index: usize, detail: String }, MidCodepointBoundary { edit_index: usize, side: BoundarySide, line: u32, character: u32 }, IntraBatchOverlap { first_index: usize, second_index: usize }, NothingEdit { edit_index: usize }, SwappedEndpoints { edit_index: usize }, InvalidUtf8 { edit_index: usize } }` where `BoundarySide` is `enum { Start, End }`.

`InvalidUtf8` is a residual safety check; in practice `new_text: String` enforces UTF-8 at deserialisation, so this variant only fires under direct internal API use (a unit-test corner). Each variant has a `Display` impl that contributes to the `tracing::debug!` reason category (`validation-failure-out-of-bounds`, `validation-failure-mid-codepoint-boundary`, `validation-failure-intra-batch-overlap`, `validation-failure-nothing-edit`, `validation-failure-swapped-endpoints`, `validation-failure-invalid-utf8`).

**Rationale**:

- Per-edit `edit_index` is essential for diagnosis: a 16-edit batch failing on edit 9's range needs to name `9`. The structure goes into `tracing::debug` fields per FR-018.
- Six variants cover the spec's enumerated failure modes (FR-005 step 3 + Edge Cases). Adding more would be over-design without a use case.
- `BoundarySide` distinguishes start-vs-end mid-codepoint failures; this matters because LLMs commonly produce off-by-one errors that land on one side or the other.

**Alternatives considered**:

- **One opaque `Invalid(String)` variant** — rejected: collapses the structured-error promise from P6 (errors reference fact-space state). The CLI / service trace lose the structured fields needed for `debug!`.
- **Separate `ValidationError` and `ApplyError` types** — rejected: nothing fails in apply that didn't fail in validate (apply is mechanical post-validation). One taxonomy; one error type.

## 5. Wire-frame size pre-check for `weaver edit-json`

**Decision**: `weaver edit-json` reads the JSON input fully, parses it into `Vec<TextEdit>`, constructs the `Event { payload: EventPayload::BufferEdit { entity, version: <looked-up>, edits } }` envelope (with placeholder `EventId::new(0)` and a freshly-stamped `Provenance`), serialises it via `ciborium::into_writer` into a `Vec<u8>`, and checks `serialised.len() <= weaver_core::bus::codec::MAX_FRAME_SIZE` (currently 64 KiB). On overflow: exit 1 with `WEAVER-EDIT-004 — serialised BufferEdit (<n> bytes) exceeds wire-frame limit (65 536 bytes)`. On underflow: dispatch via the existing `Client::send` path.

**Rationale**:

- The 64 KiB limit is enforced by `weaver_core::bus::codec::write_message`. Hitting it at dispatch produces a `CodecError::FrameTooLarge { size, max }` which currently maps (in slice 003's `classify_write_error`) to `PublisherError::Client` → exit 10 in the service. From the emitter's side, that means the bus client gets a generic codec error well after the dispatch attempt — too late to give the operator a useful diagnostic.
- Pre-check at parse time gives the operator a precise error before dispatch; this is consistent with `clap`'s approach to argument validation generally.
- Pre-serialisation cost is bounded by JSON-input size; it's the same work that would happen at dispatch anyway, just done early enough to gate the diagnostic.

**Alternatives considered**:

- **No pre-check; rely on `Client::send` to error** — rejected: error message reaches stderr after a partial network round-trip with worse diagnostic shape.
- **Streaming dispatch with chunking** — rejected: protocol does not support multi-frame events (slice 003's contract pins one frame per message; multi-frame is a future-slice concern).
- **Estimate frame size from `Vec<TextEdit>::len()` + new_text byte sums** — rejected: estimation is approximate; CBOR adjacent-tag overhead varies. Pre-serialisation is exact.

## 6. Emitter identity choice

**Decision**: `weaver edit` and `weaver edit-json` stamp `ActorIdentity::User` on dispatched `BufferEdit` events. The `User` variant was reserved at slice 002 for human-initiated CLI actions (per slice-002's data-model footnote on `ActorIdentity` variants); slice 004 is its first production use.

**Rationale**:

- `ActorIdentity::User` is the constitutionally-correct identity for a human-initiated CLI invocation: the operator is the actor; `weaver` is the conduit, not the actor. Stamping `ActorIdentity::Tui` (the slice-001 placeholder) would conflate two different surfaces (a CLI subcommand is not a TUI keystroke).
- `ActorIdentity::Service` would require a `service_id` and `instance_id` — neither exists for a one-shot CLI invocation. Forcing a synthetic service-id ("weaver-edit") would create a phantom service in the trace that has no lifecycle and no authority semantics.
- `ActorIdentity::Behavior` requires a `behavior_id`; this is not a behavior.
- Slice 006's agent will introduce `ActorIdentity::Agent` (or analog) for LLM-initiated edits. Slice 004 wires only `User`; the future agent slice adds the additional variant in the same enum.

**Alternatives considered**:

- **Reuse `ActorIdentity::Tui`** — rejected per the conflation argument above; also misleads `weaver inspect` rendering.
- **New `ActorIdentity::Cli` variant** — rejected: the CLI is a *conduit* for a User identity, not an identity itself. Multiple CLI subcommands sharing one identity variant is fine; what matters is the actor-of-record.
- **Extend `User` with a tool-name string** — speculative; `User` is currently a unit variant. Adding fields would be a wire-protocol change. Defer until a concrete need surfaces.

## 7. `BufferRegistry` extension to entity-keyed `BufferState`

**Decision**: Refactor `weaver-buffers`'s publisher to hold `HashMap<EntityRef, BufferState>` (the per-instance buffer-state map) keyed by entity ref. The slice-003 `BufferRegistry { owned: HashSet<EntityRef> }` becomes redundant — the map's keyset IS the registry. The map replaces slice-003's `Vec<BufferState>` ordered-by-CLI iteration. The poll loop iterates `map.values_mut()`; CLI argv order is preserved separately for bootstrap deterministic-event-id derivation (already done at parse time in slice 003).

**Rationale**:

- Slice 004's `dispatch_buffer_edit` needs entity-keyed lookup: given an event `BufferEdit { entity: E, .. }`, find the matching `BufferState` in O(1). The slice-003 `Vec<BufferState>` requires a linear scan — fine for small N, awkward for the new dispatch path.
- The slice-003 `BufferRegistry::is_owned` predicate is replaced by `map.contains_key(&entity)` — same constant-time semantics.
- Bootstrap-event-id determinism (slice-003 used CLI argv index `idx` as `EventId`) is preserved by capturing the order-determined IDs at bootstrap time rather than at iteration time. The post-bootstrap iteration order doesn't need to match argv order.
- Poll-loop iteration over `HashMap::values_mut()` is non-deterministic in order, but the slice-003 contract already does NOT depend on inter-buffer ordering during the poll loop — each buffer's facts are independent.

**Alternatives considered**:

- **Keep `Vec<BufferState>` + add a side-table `HashMap<EntityRef, usize>` for index lookup** — works but introduces a synchronisation invariant (the two structures must stay aligned across mutations). Not worth the complication for a single-process service with no shared mutability.
- **`BTreeMap<EntityRef, BufferState>`** — gives ordered iteration but adds log(n) cost per access. EntityRef has no semantic ordering anyway (it's a hash); ordered iteration is unmotivated.
- **Two parallel collections (registry HashSet + state Vec)** — explicit, but the registry-and-state coupling is so tight (every `mark_owned` correlates with a `Vec::push`) that one container is structurally cleaner.

## 8. Validation order for batch edits

**Decision**: Validate per-edit constraints first (bounds, codepoint boundaries, swapped endpoints, nothing-edit, UTF-8) by iterating the batch in input order. THEN sort by `range.start` and linear-scan for intra-batch overlap. Per-edit failures fire-fast on the first offender (returning `Err(ApplyError::*) { edit_index: i, .. }`); overlap detection requires the sort and so happens after the per-edit phase.

**Rationale**:

- Per-edit validation is independent and cheap; failing fast on the first offender gives the operator the smallest diagnostic.
- Overlap detection inherently requires a sort (or O(n²) all-pairs scan). For consistency with the descending-offset apply order, sorting by `range.start` is a natural fit and is reused by the apply step (the result of the sort feeds the descending-offset reverse iterator).
- Per-edit validation must NOT short-circuit overlap detection — both must run to completion before mutation. The two-phase structure makes that invariant readable.

**Alternatives considered**:

- **Single sweep**: sort first, then validate per-edit + overlap in one pass — rejected: a per-edit validation failure would surface AFTER the sort cost, slightly less efficient on small failing batches.
- **All-pairs O(n²) overlap detection without sorting** — rejected: trivially inefficient at scale (a 1000-edit batch becomes O(1M) comparisons). Sort is O(n log n).

## 9. e2e fixture approach (extends slice 003)

**Decision**: Slice-004 e2e tests adopt slice-003's four-process pattern (core + git-watcher + buffer-service + test-client) and extend it with a fifth "process" — the `weaver edit` / `weaver edit-json` invocation. The fifth process is short-lived (dispatch-and-exit); the test harness uses a `std::process::Command` wrapper (not `ChildGuard`, since it's reaped synchronously after exit). Bus interaction follows the same `Client::connect` + handshake path, just from a fresh connection each invocation.

**Rationale**:

- Re-using the slice-003 harness scaffolding minimises new test infrastructure. Adding a `Command::output()`-style invocation on top is mechanically straightforward.
- A short-lived edit invocation per-test exercises the production CLI path exactly. Tests do NOT bypass the CLI by hand-constructing `BufferEdit` events (that would skip the in-process inspect-lookup and the wire-frame pre-check, missing real bugs).
- Property-based tests (SC-406 wire-equivalence) live in a unit-test module under `tests/e2e/buffer_edit_emitter_parity.rs`; the proptest harness invokes `weaver edit` and `weaver edit-json` for each generated batch and asserts byte-identical wire payloads.

**Alternatives considered**:

- **Hand-construct `BufferEdit` events in test code** — rejected: bypasses CLI, misses CLI-layer regressions.
- **Spawn `weaver edit` as a long-running subprocess** — rejected: the CLI is fire-and-forget per FR-012; no long-running form exists.

## 10. Idempotence under repeat-delivery

**Decision**: No explicit event-id deduplication is shipped. Re-delivery of the same `BufferEdit` event to the same service carries the same `version` as the first receipt; the first receipt bumps the buffer's `buffer/version` to `version + 1`, so the second receipt's `version` matches the now-stale pre-bump value and is silently dropped by the stale-version gate (FR-005 step 2). The second receipt fires the same `tracing::debug!` line as a stale-version drop, with the original event-id intact for diagnosis.

**Rationale**:

- The version handshake mechanically de-duplicates same-event re-delivery without any extra state. This is "the version IS the dedup token" insight.
- Adding an explicit `seen_event_ids: HashSet<EventId>` would grow unbounded in the publisher's memory; bounding it (LRU cache) introduces tuning knobs without architectural payoff.
- Slice-003's `BufferOpen` idempotence (FR-011a) used a different mechanism (registry-of-owned-entities), but that was for a different invariant (don't re-bootstrap an already-owned buffer); the version-handshake here covers the analogous concern at the edit layer.

**Alternatives considered**:

- **Explicit event-id dedup** — over-design; unbounded state.
- **Global monotonic ordering on per-buffer event IDs** — over-design; `version` already is the per-buffer monotonic counter and serves the same purpose.

## 11. Bus protocol bump semantics for new struct types

**Decision**: Adding `EventPayload::BufferEdit` and the supporting `TextEdit`/`Range`/`Position` types is treated as a MAJOR bus-protocol bump (`0.3.0 → 0.4.0`) because subscribers that cannot decode the new variant produce `Error { category: "decode", context: "unknown EventPayload variant: buffer-edit" }` under the existing ciborium adjacent-tag default — same compatibility constraint as slice-003's `FactValue::U64` introduction. No new CBOR tags are registered.

**Rationale**:

- Slice 003's `contracts/bus-messages.md §Versioning policy` already documented this constraint: "Additive `FactValue` variants land as MINOR under CBOR's adjacent-tagged unknown-variant tolerance — but only if subscribers handle unknown `type` values gracefully. The core's deserializer currently defaults to `Error { category: "decode", context: "unknown FactValue variant: <X>" }`, which remains a compatibility constraint."
- The same constraint applies to additive `EventPayload` variants. Until subscribers adopt skip-and-log behaviour for unknown variants, additions land as MAJOR.
- The forward-compat enabler — subscribers tolerating unknown variants — is a future-slice concern with its own design surface (default behaviour, opt-in pattern, error reporting). Premature here.

**Alternatives considered**:

- **MINOR bump because the change is "additive only"** — rejected: subscribers without re-build break. Per L2 P7/P8, that's a MAJOR signal regardless of formal additivity.
- **Defer the bump until subscribers tolerate unknown variants** — would block slice 004 indefinitely on unrelated infrastructure. Reject.

## 12. Lint / format stance on new code

**Decision**: All slice-004 new code (in `core/src/types/edit.rs`, `core/src/cli/edit.rs`, `buffers/src/publisher.rs` extensions, `buffers/src/model.rs` extensions) adopts the workspace-level clippy/format gate with no per-file deviation. Per L2 Amendment 6: `cargo clippy --all-targets --workspace -- -D warnings` and `cargo fmt --all -- --check` MUST pass.

**Rationale**:

- L2 Amendment 6 binds every Rust file to the floor.
- No slice-004-local rationale to add pedantic/nursery lints — slice 003 didn't, and consistency matters.

**Alternatives considered**:

- **Enable `clippy::pedantic` for new modules** — Amendment 6 calls this SUGGESTED but not required; slice 004 is not the slice to introduce stricter local lints.
