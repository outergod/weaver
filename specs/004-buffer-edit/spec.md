# Feature Specification: Buffer Edit (Slice 004)

**Feature Branch**: `004-buffer-edit`
**Created**: 2026-04-25
**Status**: Draft
**Input**: User description: "Slice 004 — Buffer Edit. In-memory text editing on an opened buffer, dispatched as `EventPayload::BufferEdit` events and applied under a version handshake by `weaver-buffers`. No save-to-disk (slice 005). No agent emitter (slice 006). No concurrent-edit resolution beyond version-handshake last-write-wins."

## Clarifications

### Session 2026-04-25

- Q: When `weaver edit` / `weaver edit-json` dispatch a `BufferEdit`, where does the `version` field come from? → A: Pre-dispatch lookup via the bus inspect path; **no CLI flag exposed** for it. (`--version` is the universal program-version flag; the CLI cannot reuse it. There is no semantic value in letting an operator specify an old version explicitly — there is no rollback semantics in slice 004. Agents in slice 006+ are bus clients, not shell-spawners, so CLI ergonomics need not accommodate them. If the buffer is not currently owned by any `weaver-buffers` instance the CLI fails fast with exit 1 rather than dispatching silently. The lookup → dispatch race is intrinsic and accepted: a concurrent emitter that bumps `buffer/version` between the CLI's lookup and its dispatch causes the CLI's edit to silently drop at the service per the fire-and-forget contract.)
- Q: Should silent-drop records (non-owned entity / stale version / validation failure) surface via `weaver inspect`, beyond the service's `tracing` stderr? → A: **No bus-level surface in slice 004.** Silent drops emit `tracing::debug!` entries on the service stderr only; no new fact family, no new event variant, no rejection observability surface. The original `BufferEdit` event still lands in the core's trace (via provenance machinery), so `weaver inspect --why` on `buffer/version` correctly walks to the most-recently-applied edit; rejected events are absent from that walk. Operators wanting rejection visibility tail service stderr at `RUST_LOG=weaver_buffers=debug`. **Forward direction**: rejection observability is expected to land in a future slice as a *queryable error component* on the buffer entity (consistent with the component infrastructure deferred in `docs/07-open-questions.md §26`), not as bespoke per-rejection facts or events. This decision blocks the agent slice (006) — by then the agent needs to know its bus-side BufferEdit was rejected without subscribing to all `buffer/*` updates, and the requirement is concrete enough to design the component shape correctly.
- Q: Is there an explicit maximum on `edits.len()` in a `BufferEdit` event, beyond the implicit 64 KiB wire-frame bound? → A: **No explicit cap.** The wire-frame size limit (existing 64 KiB constraint per `weaver_core::bus::codec`) is the sole bound. Service-side validation handles arbitrary-sized batches through the standard pipeline (sort-by-offset + linear overlap-scan = O(n)). The `weaver edit-json` CLI's pre-dispatch frame-size check (FR-015) catches oversized inputs at the emitter boundary. Rationale: any specific cap (256? 1024?) is arbitrary without a concrete use case; loosening a cap later is BREAKING; setting none is forward-compatible.
- Q: Should SC-402 (16-edit atomic batch) and SC-403 (100 sequential edits) carry wall-clock budgets analogous to SC-401's ≤500 ms? → A: **No — drop wall-clock from SC-402 and SC-403; KEEP SC-401's ≤500 ms.** SC-402 / SC-403 are intent-driven (work content governs latency floor); imposing a wall-clock forces fragmentation workarounds that fight the atomic-batch architecture (split → smaller batches → more events → more version-handshake races → worse atomicity AND worse total wall-clock). Hardware-flaky in CI without architectural payoff. SC-401 is preserved because it measures *plumbing* latency for a single trivial edit (microsecond work, ≈zero SHA cost) — symmetric with slice-003 SC-302's "external mutation reflected in TUI within 500 ms" — and dropping it would leave slice 004 with no latency contract at all, masking future regressions in the bus dispatch path. Validation-cost regressions (the legitimate "did someone make this O(n³)?" concern) are caught at plan level via property tests on validation time, not at spec level.

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Apply a single edit to an opened buffer (Priority: P1)

An operator has a buffer open under `weaver-buffers` (slice 003) and wants to change a specific range of its in-memory content without touching the file on disk. The operator runs `weaver edit <PATH> <RANGE> <TEXT>`, the edit is dispatched on the bus, the service validates + applies it, and the TUI/inspect surface reflects the new state: `buffer/version` has bumped, `buffer/byte-size` reflects the new length, and `buffer/dirty=true` (because in-memory content now differs from disk). The operator's command exits immediately on successful dispatch — it does not block waiting for application.

**Why this priority**: This is slice 004's defining assertion. It closes the loop on Weaver's editing contract: edits happen on the bus as events, owned by the service that owns the buffer. Every later dogfooding-ladder slice (005 project + save, 006 agent, 007 tool use, 008 dogfooded loop) requires the existence of a wire-level edit event and a service consumer that applies it deterministically. If this story fails, there is no path from "an LLM has a diff" to "that diff is reflected in the buffer the operator sees."

**Independent Test**: An operator starts core + `weaver-buffers ./file.txt` + TUI. Once the buffer has bootstrapped (`buffer/version=0` observable), the operator runs `weaver edit ./file.txt 0:0-0:0 "hello "` to insert `hello ` at the start. Within the interactive latency class, the TUI's Buffers section shows the new byte size, `buffer/version=1`, and `buffer/dirty=true`. The file on disk is unchanged. `weaver inspect --why <entity>:buffer/version` walks back to the accepted `BufferEdit` event.

**Acceptance Scenarios**:

1. **Given** a buffer has been opened by `weaver-buffers` and bootstrap has completed (`buffer/version=0`), **When** the operator runs `weaver edit <PATH> 0:0-0:0 "prefix "`, **Then** the CLI looks up the current `buffer/version` via the bus inspect path, dispatches the event with that version, and within the interactive latency class the TUI shows `buffer/version=1`, `buffer/byte-size` increased by 7 (ASCII `prefix ` length), and `buffer/dirty=true`; the file on disk is byte-identical to its pre-edit content.
2. **Given** two concurrent emitters race the same buffer (CLI A and CLI B both run `weaver edit` against `buffer/version=N`), **When** A's `BufferEdit` reaches `weaver-buffers` first and applies (advancing to `N+1`), **Then** B's `BufferEdit` carrying `version=N` arrives stale and is silently dropped by the service; B's command still exits `0` (fire-and-forget); the stale drop is recorded in the service's trace/log; the buffer reflects only A's edit.
3. **Given** a buffer is open at `buffer/version=5`, **When** the operator runs an edit whose `<RANGE>` endpoints fall outside the current content (e.g., `end.line` past EOF, or mid-codepoint boundary), **Then** the CLI dispatches the event with `version=5` (the looked-up current value), the service rejects it at validation, nothing is applied, `buffer/version` stays at `5`, the operator's command still exits `0` on successful dispatch, and the rejection is recorded in the service's trace/log.
4. **Given** the operator runs `weaver edit <PATH> ...` against a path whose corresponding buffer entity has no `buffer/version` fact in the fact store (no `weaver-buffers` owns it), **When** the CLI's pre-dispatch lookup returns no fact, **Then** the CLI exits with code `1` and a "buffer not opened: <PATH>" diagnostic on stderr; no event is dispatched.
5. **Given** a buffer at `buffer/version=7` with a recent edit, **When** the operator runs `weaver inspect --why <entity>:buffer/version`, **Then** the walk lands on the accepted `BufferEdit` event, and the event's provenance renders the emitter's identity (the CLI emitter's `ActorIdentity` — see Assumptions §Emitter identity).

---

### User Story 2 — Atomic batched edits (Priority: P2)

An operator wants to apply multiple edits as a single logical change. The operator runs `weaver edit <PATH> <R1> <T1> <R2> <T2> <R3> <T3>` — positional `(range, text)` pairs — and either all three edits land (single `buffer/version` bump from `N` to `N+1`) or none do. No subscriber ever observes a partial intermediate state.

**Why this priority**: Atomicity is load-bearing for the LLM-future. LLMs produce diffs as batches (e.g., "rename this symbol across 5 sites"). If the service applied edits one-at-a-time as individual events, each LLM-generated batch would compose into N separate `buffer/version` bumps, each observable — and the trace would be N times noisier than the semantic change. Shipping batch atomicity from day one keeps the observable history aligned with the intent.

**Independent Test**: With a buffer at `buffer/version=0`, operator runs a 3-edit batch with all-valid edits — asserts a single version bump to `1` and exactly one round of fact re-emission. Then with `buffer/version=1`, runs a 3-edit batch where the middle edit is out-of-bounds — asserts `buffer/version` stays at `1`, in-memory content unchanged, no fact re-emission for any of the three edits.

**Acceptance Scenarios**:

1. **Given** a buffer at `buffer/version=N`, **When** the operator runs `weaver edit <PATH> <R1> <T1> <R2> <T2> <R3> <T3>` where all three edits pass validation, **Then** the service applies all three edits atomically in descending-offset order, `buffer/version` advances exactly once from `N` to `N+1`, and the new `buffer/byte-size`/`buffer/version`/`buffer/dirty` facts are re-emitted sharing the same `causal_parent` (the accepted `BufferEdit` event's id).
2. **Given** a buffer at `buffer/version=N`, **When** the operator runs a 3-edit batch where one edit fails validation (out-of-bounds, mid-codepoint, or internal overlap), **Then** **no** edit is applied, `buffer/version` stays at `N`, no `buffer/*` facts are re-emitted, the in-memory content is unchanged, and the rejection is recorded.
3. **Given** a buffer at `buffer/version=N`, **When** the operator runs a batch whose two ranges overlap within the batch, **Then** the entire batch is rejected as an intra-batch overlap validation failure; no edit applies.
4. **Given** a buffer at `buffer/version=N`, **When** the operator runs `weaver edit <PATH>` with zero `(range, text)` pairs, **Then** the CLI emits a `warn`-level stderr message "no edits provided; nothing dispatched" and exits `0` without dispatching any event; `buffer/version` stays at `N`.

---

### User Story 3 — JSON-driven edit input (Priority: P3)

A CI pipeline, integration script, or operator produces a JSON array of `TextEdit` objects and pipes it into Weaver via `weaver edit-json <PATH>` (reading JSON from stdin or `--from <file>`). The semantics are identical to the positional `weaver edit` form: atomic batch, pre-dispatch version lookup, fire-and-forget.

**Why this priority**: External tools that already speak structured text-edit shapes (LSP diagnostic responses, AST-rewriting tools, CI lint-fix passes) produce `TextEdit[]` natively; constructing positional `<RANGE> <TEXT>` argv from such tools is brittle. The JSON surface keeps the integration path lossless. Note that the slice-006 agent will be a bus client — not a CLI consumer — so this surface is for *external processes*, not in-process agents.

**Independent Test**: With a buffer open and freshly bootstrapped, operator pipes a JSON array of two `TextEdit`s via `echo '[...]' | weaver edit-json <PATH> -` — asserts identical observable behaviour to the equivalent `weaver edit` positional form. Malformed JSON (trailing comma, missing required field, bad range shape) is rejected at CLI parse time with exit code 1 and a structured error; no event is dispatched.

**Acceptance Scenarios**:

1. **Given** a buffer is open and a syntactically-valid JSON array of one or more `TextEdit` objects, **When** the operator runs `weaver edit-json <PATH> -` piping the JSON to stdin, **Then** the observable behaviour is identical to the equivalent `weaver edit` positional invocation.
2. **Given** a buffer is open, **When** the operator runs `weaver edit-json <PATH> --from <file>` where the file contains valid JSON, **Then** semantics are as above with the file read as the input source instead of stdin.
3. **Given** an operator pipes malformed JSON (truncated, missing required fields, trailing garbage), **When** `weaver edit-json` parses stdin, **Then** the CLI exits with code `1` and a structured error diagnostic; no event is dispatched on the bus.
4. **Given** an operator pipes a JSON payload whose serialised `EventPayload::BufferEdit` would exceed the bus-frame size limit, **When** `weaver edit-json` attempts to dispatch, **Then** the CLI rejects the input at parse time with exit code 1 and a "payload too large" diagnostic rather than attempting a dispatch that would fail at the bus codec layer.

---

### Edge Cases

- **Edit targets a buffer that is not owned by any running `weaver-buffers` instance**: at the **CLI layer**, the pre-dispatch lookup returns no `buffer/version` fact and the CLI fails fast with exit `1` ("buffer not opened") without dispatching. At the **wire layer** (an external bus client that bypasses the lookup), the event would dispatch and silently evaporate per events-are-lossy semantics — no service consumes it, no trace entry, no `buffer/version` bump. CLI emitters are protected by the lookup; arbitrary bus emitters are not.
- **Edit targets an entity owned by a different service kind (e.g., a `repo/*` entity)**: only `weaver-buffers` subscribes to / dispatches on `EventPayload::BufferEdit`; other services ignore the payload variant. Same silent no-op result as above.
- **`Range.start > Range.end` (swapped endpoints)**: validation failure; the batch it belongs to is rejected.
- **`Range.end.line >= line_count` or `Range.end.character > line_byte_length`**: out-of-bounds; rejected at validation.
- **`Range.start == Range.end AND new_text == ""`**: a nothing-edit; rejected at validation. Legitimate pure-insert has `start == end, new_text != ""`. Legitimate pure-delete has `start < end, new_text == ""`.
- **`Range` endpoints land mid-codepoint (inside a multi-byte UTF-8 sequence)**: rejected at validation as an encoding-boundary violation. The `character` offset must land on a UTF-8 code-point boundary within the line's byte content.
- **`new_text` contains `\n`**: permitted. Subsequent lines shift; `buffer/byte-size` reflects the new length; future edits operate on the re-layouted content.
- **`new_text` contains invalid UTF-8 / non-UTF-8 bytes**: rejected at CLI parse time. `new_text` is a Rust `String` (UTF-8 by construction); the JSON and CLI parsers enforce UTF-8 at their boundaries.
- **Two concurrent emitters race the same `version`**: both dispatch at `version=N`. The event that reaches the service first applies, bumping `version` to `N+1`. The second event arrives carrying stale `version=N`; silently dropped. Last-write-wins with no merge, no conflict resolution; the slow emitter gets no error frame.
- **Duplicate event delivery**: if the same `BufferEdit` event id is somehow received twice by the service, the second receipt carries a stale `version` (because the first receipt bumped it) and is silently dropped by the stale-version gate. No explicit event-id deduplication is shipped.
- **Emitter supplies a `version` the buffer has never held** (future-version, not just stale): same treatment as stale — silently dropped. Only exact equality to current `buffer/version` accepts.
- **Empty `edits: []` in the dispatched event**: valid at the wire; the service applies the atomic batch (which contains nothing), so no `buffer/version` bump, no fact re-emission. Event is traced at `debug` level. See Assumptions §Empty-batch for why the CLI catches this at parse time.
- **`weaver-buffers` crashes mid-batch**: batch atomicity is guaranteed structurally — validation-then-apply is synchronous within a single reader-loop iteration; crash during validation leaves buffer unchanged, crash during apply is treated as an unrecoverable invariant violation (panic → process exit → core's `release_connection` retracts every `buffer/*` fact). No subscriber ever observes a partial state.

## Requirements *(mandatory)*

### Functional Requirements

**Wire surface — new event variant:**

- **FR-001**: The system MUST extend `EventPayload` with a new variant `BufferEdit { entity: EntityRef, version: u64, edits: Vec<TextEdit> }`. Wire tag `"buffer-edit"` (adjacent-tagged, kebab-case per L2 Amendment 5). The variant MUST serialise via the existing adjacent-tag enum machinery; no new CBOR tags are introduced.
- **FR-002**: The system MUST introduce supporting types `TextEdit { range: Range, new_text: String }`, `Range { start: Position, end: Position }`, `Position { line: u32, character: u32 }` serialised as plain CBOR/JSON structs (no CBOR tag). Field names use kebab-case on JSON; snake_case in Rust source. Deserialisation MUST enforce UTF-8 on `new_text`.
- **FR-003**: `Position.character` MUST denote a **UTF-8 byte offset within the line's content**. For line `L` with byte content `bytes_L`, `Position { line: L, character: c }` refers to the byte offset `c` within `bytes_L`. Line terminators (`\n`, `\r\n`) do NOT count toward a line's byte length — they are the separator between lines. `\r\n` is encoded as two bytes at the preceding line's terminator, not as "character 0" of the next line. The encoding choice MUST be documented on the wire contract.
- **FR-004**: The bus protocol MUST advance from `0x03 → 0x04`. `Hello.protocol_version = 0x04`; mismatched handshakes receive `Error { category: "version-mismatch", detail: "bus protocol 0x04 required; received 0x03" }` and connection close. The bump is MAJOR because subscribers that cannot handle the new `EventPayload` variant produce `Error { category: "decode", context: "unknown EventPayload variant: buffer-edit" }`; forward compatibility for the unknown-variant path remains future work.

**Consumer — `weaver-buffers` applies edits:**

- **FR-005**: `weaver-buffers` MUST extend its reader-loop to dispatch `BusMessage::Event(Event { payload: EventPayload::BufferEdit { entity, version, edits }, .. })` through a handler paralleling slice 003's `dispatch_buffer_open`. The handler MUST:
  1. Look up the `BufferState` for `entity` in the per-invocation registry; if not owned, silently no-op (event traced at `debug`).
  2. Compare the event's `version` to the buffer's current `buffer/version`. If unequal (stale or future), silently drop (event traced at `debug` with mismatch detail). No fact re-emission.
  3. Validate every `TextEdit` in the batch against the in-memory content: bounds (`end.line < line_count`, `end.character <= line_byte_length`), UTF-8 codepoint boundaries, no intra-batch overlap, no nothing-edits. Validation failure drops the entire batch silently (event traced at `debug` with validation-error detail).
  4. On full validation success, apply every edit in a single atomic operation using descending-offset application order (per FR-007), update the in-memory content, recompute `memory_digest = sha256(content)`, bump `buffer/version` by exactly `1`.
  5. Re-emit `buffer/byte-size`, `buffer/version`, and `buffer/dirty` facts (authoring identity: the buffer service's own `ActorIdentity::Service`; `causal_parent = Some(event.id)`).
- **FR-006**: The batch MUST be **validated in full before any edit is applied**. Validation and application MUST happen within a single reader-loop iteration with no interleaved bus writes, ensuring no subscriber observes a partial state regardless of validation outcome.
- **FR-007**: Edits within a batch MUST be applied in descending-start-offset order (LSP-compatible), so that earlier positions are not shifted by later applications. Intra-batch range overlap is a validation failure per FR-005 step 3 — overlap-compose or overlap-apply-in-order semantics are NOT introduced.
- **FR-008**: An empty batch (`edits: []`) MUST be a no-op at the consumer: validation succeeds trivially, no edit applies, no `buffer/version` bump, no fact re-emission. The event is traced at `debug` level.
- **FR-009**: On accepted edit, the service MUST re-emit (in any order within the same synchronous write burst): `buffer/byte-size` (with the new in-memory length), `buffer/version` (with `N+1`), and `buffer/dirty` (with `memory_digest != sha256(disk_content)`; in slice 004 this is always `true` because no save-to-disk path exists — but the comparison is computed, not hardcoded, so slice 005's save path flips it correctly). All three share the same `causal_parent = Some(event.id)`.

**Emitter — CLI subcommands on `weaver`:**

- **FR-010**: The `weaver` CLI MUST grow two new subcommands:
  - `weaver edit <PATH> [<RANGE> <TEXT>]* [--socket <PATH>]` — positional `(range, text)` pairs; one or more pairs required in the non-empty-batch form; zero pairs produces a `warn`-level stderr message and dispatches nothing (see FR-014). **No flag accepts a buffer-version override**; the version is always sourced from a pre-dispatch lookup per FR-013.
  - `weaver edit-json <PATH> [--from <PATH>|-] [--socket <PATH>]` — reads a JSON `Vec<TextEdit>` from stdin (the `-` form) or from the file named by `--from`. Same version-source rule as `weaver edit`; no override flag.
- **FR-011**: `<RANGE>` grammar on the `weaver edit` CLI: `<start-line>:<start-char>-<end-line>:<end-char>`, where all four components are decimal `u32` integers, `start-char` and `end-char` are UTF-8 byte offsets within the respective line (per FR-003). Example: `0:0-0:5` deletes/replaces the first 5 bytes of line 0. Parse failure exits with code `1`.
- **FR-012**: Both subcommands MUST canonicalise the input path (matching `weaver-buffers`'s slice-003 canonicalisation), derive the buffer entity, construct a `BufferEdit` event with the supplied `version`, dispatch it on the bus, and exit `0`. Fire-and-forget: the CLI does NOT subscribe post-dispatch, does NOT wait for `buffer/version` bump confirmation, and has no exit-code signal distinguishing "applied" from "silently dropped by the service".
- **FR-013**: Both subcommands MUST source the `BufferEdit` event's `version` field by **pre-dispatch lookup** of the current `buffer/version` fact for the target entity, via the bus inspect path (the same library function `weaver inspect <entity>:buffer/version` uses; in-process call, not a subprocess shell-out). No CLI flag exposes a version override — the lookup is the canonical source. If the lookup returns no fact (the buffer is not currently owned by any `weaver-buffers` instance), the CLI MUST exit `1` with a "buffer not opened: <PATH>" diagnostic on stderr; no event is dispatched. The lookup → dispatch window is intrinsically racy: another emitter that bumps `buffer/version` between the CLI's lookup and its dispatch causes the CLI's edit to silently drop at the service per the fire-and-forget contract (FR-012).
- **FR-014**: If `weaver edit` is invoked with zero positional pairs (zero-edit batch) the CLI MUST emit a `warn`-level stderr message "no edits provided; nothing dispatched" and exit `0` without dispatching an event.
- **FR-015**: `weaver edit-json` MUST reject malformed JSON, missing required fields, non-UTF-8 `new_text`, or a payload whose serialised `BufferEdit` wire form would exceed the 64 KiB bus-frame limit at CLI parse time with exit code `1` and a structured error diagnostic. No event is dispatched on parse failure. The 64 KiB bus-frame limit is the **sole bound on `edits.len()`** in slice 004 — the spec does NOT introduce an explicit batch-size cap; service-side validation handles arbitrary-sized batches through the standard pipeline.

**Causal chain & inspection:**

- **FR-016**: On accepted edit, every re-emitted fact MUST carry `causal_parent = Some(event.id)` where `event` is the applied `BufferEdit` event. This permits `weaver inspect --why <entity>:buffer/version` to walk from the fact back to the event and thence to the emitter's identity via event provenance.
- **FR-017**: The slice MUST preserve slice-003's FR-011a `BufferOpen` idempotence invariant. `BufferEdit` does not interfere: applying edits updates content but leaves the buffer entity's ownership in the registry unchanged; a subsequent `BufferOpen` for the same entity remains a no-op.

**Trace / observability of silent drops:**

- **FR-018**: Every silently-dropped edit (non-owned entity, stale version, validation failure) MUST produce a `tracing` entry at `debug` level with fields: `event_id`, `entity`, `emitted_version`, `current_version`, and a short reason category (`unowned-entity`, `stale-version`, `future-version`, `validation-failure-<kind>`). The service MUST NOT publish any bus-level surface for rejections in slice 004 — no new fact family, no new event variant, no inspection-visible record. The original `BufferEdit` event still lands in the core's trace via existing provenance machinery (so `weaver inspect --why` walks remain meaningful for the *applied* edit chain). Forward direction (slice-N, not slice 004): rejection observability is expected to arrive as a queryable error component on the buffer entity per the component-infrastructure deferral in `docs/07-open-questions.md §26`.

**Known gaps (documented, not closed this slice):**

- **FR-019**: Slice-003's FR-021 ("`EventPayload` lacks per-connection identity binding") is inherited **and becomes non-theoretical in slice 004**. `EventPayload::BufferEdit` is the first event variant whose expected producers include non-service clients (the CLI today, the agent tomorrow). Any local process with a bus connection can dispatch a `BufferEdit` carrying arbitrary `ActorIdentity`. Slice 004 MUST document this gap prominently (see §Known Hazards) and MUST NOT close it — enforcement is reserved for a future soundness slice that touches all event-payload types together. The gap MUST be closed before slice 006 (agent) ships.
- **FR-020**: Slice-003's FR-022 ("First-to-claim `service_id` squatting") remains open. A malicious consumer could hijack `service_id = "weaver-buffers"` and apply adversarial edit semantics. Deferred to the capability-model slice.
- **FR-021**: No undo, no edit-log, no transactional rollback beyond intra-batch atomicity. The trace preserves the authored `BufferEdit` events, but "reverting to version N-k" requires replaying authored content, not traversing an undo stack.

### Key Entities

- **`TextEdit`**: One atomic edit operation — a range to replace and the replacement text. Fields: `range: Range`, `new_text: String`. Wire identity: a plain struct, adjacent-tagged on enum boundaries, no CBOR tag. Validity constraints: range falls on UTF-8 codepoint boundaries; range endpoints lie within current buffer content; `range.start <= range.end`; NOT both (`start == end AND new_text == ""`).
- **`Range`**: A half-open interval on 2D buffer coordinates — inclusive start, exclusive end. Fields: `start: Position`, `end: Position`. Point ranges (`start == end`) represent insertion cursors.
- **`Position`**: A 2D coordinate within a buffer — zero-based line, zero-based UTF-8-byte-offset within line. Fields: `line: u32`, `character: u32`. `character` is **UTF-8 bytes within the line's content**, NOT UTF-16 code units.
- **`BufferEdit` event**: The bus-level edit-dispatch record. Fields: `entity: EntityRef` (the buffer to edit), `version: u64` (emitter's snapshot of `buffer/version`; the handshake is `emitted_version == current_version`), `edits: Vec<TextEdit>` (atomic batch). Delivery class: lossy (per `EventPayload` convention); no structured rejection path.
- **`buffer/version` fact family** (unchanged shape from slice-003 forward-compat scaffold; acquires a mutation consumer this slice): `FactValue::U64`, starts at `0` at bootstrap, bumps by `1` on each accepted edit. Used as the handshake token and as the causal anchor for the edit's fact re-emission burst.
- **In-memory content (unchanged from slice 003, with structural mutation API added)**: The `BufferState`'s `content` vector gains an `apply_edits(&mut self, edits: &[TextEdit]) -> Result<(), ApplyError>` method that preserves the slice-003 `memory_digest == sha256(content)` invariant structurally. Called only by the service's own dispatch handler; no external API for direct mutation.

## Affected Public Surfaces *(mandatory)*

### Fact Families & Authorities

- **Authority**: `weaver-buffers` retains single-writer authority over the `buffer/*` family for every buffer entity it owns. Slice 004 does NOT introduce a new authority — it extends the existing service's mutation surface. Per-entity, per-connection (slice-002 F10).
- **Fact families touched**:
  - `buffer/version` — **modified** (gains a mutation consumer; bootstrap shape unchanged from PR #10). Each accepted edit bumps the counter by exactly `1`; stale drops leave it unchanged.
  - `buffer/byte-size` — **modified** (re-emitted on accepted edit with the new length). Schema unchanged.
  - `buffer/dirty` — **modified** (re-emitted on accepted edit with recomputed value; in slice 004 always `true` post-edit because no disk-write path exists). Schema unchanged.
  - `buffer/path`, `buffer/observable` — **read-only** (never re-emitted by the edit path; edits do not alter path or observability).
  - `watcher/status` — **read-only** (edits never change service lifecycle).
- **Schema impact**: **Additive** at every fact-family level; no `buffer/*` family's value type, cardinality, or authority changes. The wire-level break is at the `EventPayload` layer, NOT at the fact layer.

### Other Public Surfaces

- **Bus protocol**: bumps from `0x03` to `0x04`. Enumerated changes:
  - `EventPayload::BufferEdit { entity: EntityRef, version: u64, edits: Vec<TextEdit> }` added.
  - `TextEdit`, `Range`, `Position` struct types added (plain serialisation; no new CBOR tags).
  - `Hello.protocol_version` advances `0x03 → 0x04`; mismatched clients receive `Error { category: "version-mismatch", detail: "bus protocol 0x04 required; received 0x03" }` and close.
- **CBOR tag scheme**: **no new tags.** `EntityRef` (tag 1000), `Keyword` (tag 1001), `ActorIdentity` (tag 1002) all reused unchanged. Rationale: `FactValue::U64` precedent in slice 003 — new variants / new struct types ride plain struct serialisation.
- **Action-type identifiers**: not affected.
- **CLI flags + structured output shape**:
  - Existing `weaver` CLI: `edit` and `edit-json` subcommands **added** (MINOR additive). `weaver --version` JSON field `bus_protocol` advances `0.3.0 → 0.4.0`.
  - Existing `weaver-buffers`, `weaver-git-watcher`, `weaver-tui`: `--version` JSON field `bus_protocol` advances `0.3.0 → 0.4.0` (constant-driven). No CLI-surface changes beyond the version bump.
  - `weaver inspect`: shape unchanged; `--why` walks now traverse `BufferEdit` events transparently via existing causal-parent machinery.
- **Configuration schema**: no changes.
- **Steel host primitive ABI**: not affected.

### Failure Modes *(mandatory)*

- **Degradation taxonomy**: unchanged from slice 003. `watcher/status` transitions are not driven by edit activity. A catastrophic edit-application failure (invariant violation, e.g., post-apply `memory_digest` drift) is treated as an unrecoverable programmer bug — `panic!` / process exit / core's `release_connection` retracts every `buffer/*` fact — matching slice-003's posture.
- **Failure facts**: no new fact families introduced for edit-failure surfacing. Silent drops are observable only via `tracing` output at `debug` level per FR-018 (subject to the clarification about inspection-visible trace surfacing).
- **Emitter-side failure**: `weaver edit` / `weaver edit-json` exit codes:
  - `0` — event dispatched successfully (does NOT imply applied).
  - `1` — CLI parse error, malformed input, path canonicalisation failure, JSON validation failure, **or pre-dispatch lookup found no `buffer/version` fact for the target entity (buffer not opened)**.
  - `2` — bus unavailable (socket missing, handshake failed); aligns with `weaver-buffers` exit-code convention.
  - **No new exit code for "stale version / edit dropped at the service"** — stale rejection is silent per FR-018 + FR-012; the CLI cannot detect it post-dispatch.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-401**: Following a `weaver edit <PATH> <RANGE> <TEXT>` invocation against a buffer at `buffer/version=0`, an observer subscribed to `buffer/*` observes `buffer/version=1` and the updated `buffer/byte-size` within the interactive latency class (operator-perceived ≤500 ms, matching slice-003 SC-302 budget).
- **SC-402**: A 16-edit atomic batch applied to an opened buffer either (a) produces exactly one `buffer/version` bump and one fact-re-emission burst observable on subscribers, OR (b) produces zero bumps and zero re-emission (validation failed). No subscriber ever observes a partial state of the batch. **No wall-clock budget**: batch latency floor is governed by work content (validation + apply + SHA-256 recompute) and may legitimately scale with batch size and buffer size; imposing a wall-clock would force fragmentation workarounds that damage atomicity.
- **SC-403**: 100 sequential `weaver edit` invocations against the same buffer (each invocation does its own pre-dispatch `buffer/version` lookup per FR-013) produce exactly `buffer/version=100`. No gaps, no duplicates. **No wall-clock budget**: total elapsed time is dominated by per-invocation process-spawn cost (hardware-dependent, ~100–150 ms × 100 ≈ ~15 s on commodity hardware); the structural gap-free invariant is the load-bearing assertion. Total wall-clock MAY be reported informationally to stderr by the test runner but MUST NOT gate pass/fail.
- **SC-404**: A stale-version edit (emitter's `version` ≠ current `buffer/version`) never bumps `buffer/version`, never mutates in-memory content, never re-emits `buffer/*` facts, and never produces a `FactRetract`. The operator's `weaver edit` exits `0`; the drop is observable only via the service's trace/log output.
- **SC-405**: `weaver inspect --why <entity>:buffer/version` on a buffer at `buffer/version > 0` walks to the most-recently-applied `BufferEdit` event.
- **SC-406**: The two MVP emitter surfaces (`weaver edit` positional and `weaver edit-json` JSON) produce byte-identical `EventPayload::BufferEdit` wire payloads for semantically-equivalent inputs — asserted via a property test over randomly-generated edit batches.

## Known Hazards

*Slice-004 LIMITATIONS, not requirements. Documented because dependent slices need to know what slice 004 does NOT solve.*

- **Unauthenticated edit channel** (expanded from slice-003 FR-021): slice 004 ships the first event variant whose expected producers include non-service clients. Any process with a bus connection can dispatch `EventPayload::BufferEdit` carrying any `ActorIdentity` it constructs client-side; the listener validates identity *well-formedness* (slice-003 F15) but NOT *conn-binding* (the producer's connection is not cross-checked against the identity's service-id / UUID). Consequence: a malicious local process could emit edits impersonating any actor, and `weaver inspect --why` would attribute the edit to the spoofed identity. Closing this requires a wire-level handshake where the core binds `ActorIdentity` to the connection at handshake time and rejects events whose `provenance.source` doesn't match. Deferred to a future soundness slice; MUST be closed before slice 006 (agent) ships.
- **Silent drops**: lossy event delivery means `weaver edit` cannot distinguish "applied" from "dropped on the wire" from "stale-version rejected" from "validation failed." The slice documents this as an accepted UX cost. Emitters needing confirmation can subscribe to `buffer/version` post-dispatch and wait for a matching bump, but slice 004 does not ship a blocking mode on either CLI subcommand.
- **O(n) SHA-256 on every accepted edit**: every accepted batch recomputes the full `memory_digest = sha256(content)`. Acceptable for MVP-size buffers (≤ few MiB). For multi-MiB buffers, the per-edit CPU cost becomes visible; a future slice may introduce an incremental or rolling hash.
- **No undo / no edit-log**: rejected edits are lost; accepted edits are traced but there is no `buffer/undo` fact family, no replay-to-N semantics, no transactional snapshots. An operator who applies a bad edit must emit an inverse edit manually.
- **No bus-level rejection observability**: silent-drop records are stderr-only (`tracing::debug`). External consumers (the future agent on the bus) cannot observe their own rejections without polling `buffer/version`. Forward direction is a queryable error component on the buffer entity (slice that introduces component infrastructure); not addressed here.
- **No cross-buffer atomicity**: a batch operates on exactly one `entity`. Multi-buffer refactors ("rename across 5 files") require N separate `BufferEdit` events; the cross-buffer atomic guarantee is absent. Project-level transactions are a slice-005+ concern.
- **No disk writes**: `buffer/dirty=true` is the user-visible signal that memory has diverged from disk, but slice 004 ships no path to reconcile them. Save-to-disk is slice 005's defining feature.

## Assumptions

*Commitments made when the feature description did not specify certain details; revisited in `/speckit.clarify` if any prove load-bearing.*

- **Authority split, not buffer-service-owns-emission**: the emitter and the consumer are separate processes. `weaver-buffers` does not dispatch `BufferEdit` events to itself in-process; it consumes events produced by distinct emitters (CLI, future agent, future UI). Locked from the slice-004 proposal drawer; NOT revisited in clarify.
- **LSP-style coordinates, UTF-8 bytes within line for `character`**: the coordinate system is `Range { start: Position, end: Position }`, `Position { line: u32, character: u32 }`, matching LSP 3.17 `TextDocumentContentChangeEvent` shape but with `character` counting **UTF-8 bytes** instead of the LSP-default UTF-16 code units. Rationale: Rust-native `&str` slicing is UTF-8-byte-addressed; `positionEncodings` capability negotiation in LSP 3.17 makes UTF-8 a first-class LSP option for future interop (rust-analyzer, clangd already prefer it). LSP-default UTF-16 would force a translation shim on every edit application for no structural benefit until Weaver interops with a legacy UTF-16-only server. Switching later is a BREAKING bus-protocol bump.
- **Descending-offset batch application** (LSP-compatible). Applying later edits first ensures earlier positions are not invalidated by subsequent applications. Overlapping ranges within a batch are rejected as a validation failure — NOT composed or apply-in-order'd.
- **Atomic-all-or-nothing batches**. Validation happens on the entire batch before any edit applies. If edit 2 of 3 fails, the whole batch is rejected; no partial application is observable. The validation-then-apply sequence is synchronous within one reader-loop iteration.
- **Fire-and-forget CLI semantics**. `weaver edit` and `weaver edit-json` dispatch their event and exit `0` on successful send. They do NOT subscribe, do NOT wait for `buffer/version` confirmation, and have NO exit-code signal for stale-version or validation rejection by the service. Future slices can ship a blocking / confirm mode.
- **Silent-drop semantics on the wire**. Events are lossy-class per `docs/02-architecture.md §3.1`. Slice 004 does NOT teach the core to send structured error frames for rejected events — that would violate the lossy-delivery contract and require re-architecting the event path. Stale / invalid / unowned edits are silently dropped, traced, and recoverable only by the emitter choosing to observe via subscription.
- **`start > end` swapped**. Treated as a validation failure, not auto-normalised. An emitter constructing a swapped range is almost certainly a bug; silent normalisation would mask it.
- **`end` past EOF / past EOL**. Rejected at validation, NOT clamped. Clamping silently changes the semantic intent; if an emitter wants "to end of file" semantics, it should compute the EOF position itself.
- **Line ending handling**: bytes count bytes. `\r\n` is two bytes at the preceding line's terminator; neither counts toward the following line's `character` offset. `\n` in `new_text` is permitted and splits/shifts subsequent lines. The spec does NOT normalise line endings on input.
- **Empty-batch edge**: at the CLI layer, `weaver edit <PATH>` with zero `(range, text)` pairs emits a `warn` on stderr and dispatches nothing (FR-014). At the wire layer, a received `EventPayload::BufferEdit { edits: [] }` is a well-formed no-op (FR-008). The wire form exists because a future agent or tool may construct an empty batch legitimately as a handshake probe; forbidding it at the wire would require additional validation machinery for no clear benefit.
- **Idempotence of repeat-delivery**: no explicit event-id deduplication. Re-delivery of the same `BufferEdit` event to the same service carries a stale `version` on the second receipt (the first receipt bumped it) and is silently dropped by the stale-version gate. Sufficient for the in-process reality of slice 004.
- **Tracing verbosity**: accepted-edit events emit at `info` level (consistent with slice 003's `watcher/status` transitions being `info`); silent drops emit at `debug` level with mismatch/validation detail. Parse-time CLI errors emit at `error`. Matches slice 003's convention.
- **Emitter identity on CLI-dispatched edits**: `weaver edit` and `weaver edit-json` stamp `ActorIdentity::User` on dispatched `BufferEdit` events. The `User` variant was reserved in slice 002 for human-initiated actions via a CLI surface; slice 004 is its first production use. Slice 006 (agent) will introduce `ActorIdentity::Agent` for LLM-initiated edits. Inspection renders edit attribution accordingly.
- **`Range.start.line == end.line AND start.character == end.character AND new_text != ""`**: pure insert at a point cursor — legitimate; applies as insert-at-position.
- **`Range.start < Range.end AND new_text == ""`**: pure delete — legitimate; applies as content removal.
- **Wire-frame size limit**: `weaver edit-json` pre-checks that the serialised `BufferEdit` event will fit within the 64 KiB `write_message` frame limit (`weaver_core::bus::codec`). Oversized inputs are rejected at CLI parse time rather than attempting a bus dispatch that would fail at the codec layer.

## Dependencies

- **Slice 003 (Buffer Service)** — slice 004 consumes the `BufferRegistry`/`BufferState`/`buffer_bootstrap_facts` seams verbatim. The `buffer/version=0` forward-compat bootstrap fact shipped in PR #10 is the handshake foundation.
- **Slice 002 (Git-Watcher)** — no direct dependency, but the structured `ActorIdentity` machinery is slice-002 heritage. `ActorIdentity::User` variant reserved at slice 002; first used here.
- **Slice 001 (Hello-fact)** — no direct dependency.
- **L1 Constitution §3 (Fact space + events)**: edits are events; events commute; slice 004 does not re-open this.
- **L2 Amendment 1 (Conventional Commits)**: BREAKING footer on the protocol-bump commit.
- **L2 Amendment 5 (Wire-stability)**: kebab-case for the new variant tag and struct field names on the wire.
