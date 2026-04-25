# Implementation Plan: Buffer Edit

**Branch**: `004-buffer-edit` | **Date**: 2026-04-25 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/004-buffer-edit/spec.md`

## Summary

Slice 004 layers in-memory text editing on top of slice 003's `weaver-buffers` content authority. A new event variant `EventPayload::BufferEdit { entity: EntityRef, version: u64, edits: Vec<TextEdit> }` carries LSP-shape coordinates (`Range { start: Position, end: Position }`, `Position { line: u32, character: u32 }`, `TextEdit { range: Range, new_text: String }`) where `Position.character` denotes a **UTF-8 byte offset within the line's content** — explicitly NOT UTF-16 code units, an LSP-default-departure justified in spec §Assumptions and forward-compatible with LSP 3.17 `positionEncodings` negotiation.

`weaver-buffers` extends its reader-loop with a new arm that mirrors slice-003's `dispatch_buffer_open`/`BufferRegistry` seam — pub(crate) `dispatch_buffer_edit` returns a `BufferEditOutcome` that distinguishes accepted-and-applied from each silent-drop class (non-owned entity, stale version, validation failure). Validation runs on the entire batch before any edit applies (atomic-all-or-nothing); accepted batches apply in descending-offset order (LSP-compatible), bump `buffer/version` by exactly 1, recompute `memory_digest`, and re-emit the updated `buffer/byte-size` / `buffer/version` / `buffer/dirty` facts with `causal_parent = Some(event.id)`.

Two CLI emitter subcommands land on the existing `weaver` binary: `weaver edit <PATH> [<RANGE> <TEXT>]*` (positional pairs) and `weaver edit-json <PATH> [--from <PATH>|-]` (stdin/file JSON). Both perform a pre-dispatch bus inspect lookup for `buffer/version` (no `--version` flag — collision with the universal program-version flag, no semantic value to specifying old versions, agents in slice 006+ will be bus clients not shell-spawners). Buffer not currently owned → CLI exit 1. Lookup → dispatch is intrinsically racy under concurrent emitters; the slow emitter's edit silently drops at the service per the fire-and-forget contract.

The bus protocol bumps **0.3.0 → 0.4.0**, BREAKING but additive only (new variant + new struct types; no removals; no changes to existing message shapes). No new CBOR tags — plain struct serialisation through ciborium's adjacent-tag machinery, mirroring slice 003's `FactValue::U64` precedent. The `Hello.protocol_version` advances `0x03 → 0x04`; mismatched clients receive `Error { category: "version-mismatch", detail: "bus protocol 0x04 required; received 0x03" }`.

This slice is the first non-service event-payload producer wired to the wire. Slice-003's FR-021 (`EventPayload` lacks per-connection identity binding) and FR-022 (`service-id` squatting) become non-theoretical here — any local process with a bus connection can dispatch a `BufferEdit` carrying any `ActorIdentity`. Both gaps are inherited unclosed and re-flagged in spec §Known Hazards; closing them is reserved for a future soundness slice that MUST land before slice 006 (agent) ships. Silent-drop observability stays at `tracing::debug` stderr only — the forward direction (queryable error component on the buffer entity) waits for the component-infrastructure slice that arrives after the present unscheduled deferral.

## Technical Context

**Language/Version**: Rust 2024 edition; resolver = "3" (workspace-level); toolchain pinned to 1.94.0 via `fenix` per `flake.nix` + `rust-toolchain.toml` (unchanged from slices 002/003).
**Primary Dependencies**: existing — `tokio`, `ciborium`, `serde` + `serde_json`, `clap` derive, `miette`, `thiserror`, `tracing` + `tracing-subscriber`, `proptest`, `vergen`, `crossterm`, `uuid` (v4), `humantime`, `tempfile` (workspace dev-dep), `sha2` (slice-003 inheritance). **No new direct dependency** — all new types are pure-Rust struct definitions over the existing serde/ciborium plumbing.
**Storage**: In-memory only (the service's per-buffer byte store gains a structural mutation method that preserves `memory_digest == sha256(content)` invariant). No persistence change. Trace remains in-process. Slice 005 owns disk writes.
**Testing**: `cargo test` (unit + scenario); `proptest` (CBOR + JSON round-trip for `EventPayload::BufferEdit`, `TextEdit`, `Range`, `Position`; SC-406 byte-identical wire-payload property over randomly-generated edit batches comparing the positional and JSON emitter paths). E2e tests extend slice-003's four-process pattern (core + git-watcher + buffer-service + test-client) with a fifth process — the `weaver edit` / `weaver edit-json` invocation that emits the edit event.
**Target Platform**: Linux + macOS desktop. Single machine. Bus over Unix-domain socket as in slices 001/002/003.
**Project Type**: Rust workspace; **no new member crate**. Modifies `core/` (event-payload variant, struct types, listener still routes through existing path, CLI subcommands, `--version` output, bus protocol version), `buffers/` (publisher reader-loop arm + apply path on `BufferState`). `tui/`, `git-watcher/`, `ui/` untouched.
**Performance Goals**: SC-401 — single-edit emit-to-fact-re-emit observable on subscriber ≤ 500 ms (interactive latency class per `docs/02-architecture.md §7.1`, parity with slice-003 SC-302). SC-402 (16-edit atomic batch) and SC-403 (100 sequential edits) are intentionally **structural-only** with no wall-clock budget — batch latency floor is governed by work content (validation + apply + SHA-256 recompute) and imposing a wall-clock would force atomicity-damaging fragmentation.
**Constraints**: Events lossy-class per `docs/02-architecture.md §3.1`; fire-and-forget CLI semantics (no exit-code signal for stale-drop or validation rejection); UTF-8 byte offsets on the wire (not UTF-16); no save-to-disk (slice 005); no concurrent-edit resolution beyond version-handshake last-write-wins; **unauthenticated edit channel** explicitly inherited as a Known Hazard (FR-019) and NOT closed this slice.
**Scale/Scope**: ~700–1100 new LOC across `core` and `buffers`, plus e2e tests. 1 new `EventPayload` variant (`BufferEdit`); 3 new struct types (`TextEdit`, `Range`, `Position`); 2 new CLI subcommands (`edit`, `edit-json`); 1 new pub(crate) handler on the buffers publisher (`dispatch_buffer_edit` + `BufferEditOutcome` + `apply_edits` on `BufferState`); 6 new e2e test files — `buffer_edit_single` (folds the buffer-not-opened path), `buffer_edit_inspect_why`, `buffer_edit_atomic_batch`, `buffer_edit_sequential`, `buffer_edit_stale_drop`, plus the `buffer_edit_emitter_parity` property test (SC-406). No new fact families. Bus protocol 0.3.0 → 0.4.0 (MAJOR).

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Gates derived from `.specify/memory/constitution.md` v0.7.0. Each L2 principle is named with a slice-specific gate. Principles not exercised by this slice are listed with forward-looking triggers.

### Applicable principles (PLANNED — must hold by `/speckit.implement` exit)

- **P1 — Domain modeling without type hierarchy.** `TextEdit`, `Range`, `Position` are flat data structs (no enum-based discriminated union, no trait hierarchy). The dispatch handler's outcome is a small enum (`BufferEditOutcome::{Applied, StaleVersion, FutureVersion, NotOwned, ValidationFailure(ApplyError)}`) — flat variants, not a nested taxonomy. Edit application is a pure function over `&mut BufferState` and `&[TextEdit]`.
- **P2 — Purity at edges, transactional state at core.** `BufferState::apply_edits` is structurally pure: validates the whole batch first (returning `Err(ApplyError)` without mutating on validation failure), then mutates atomically — caller observes either the new state plus consistent `memory_digest`, or the unchanged state and an error. The publisher's bus side-effects (re-emitting `buffer/byte-size`/`buffer/version`/`buffer/dirty`) follow the apply, share a single `causal_parent`, and are serialised through the existing `BusWriter` handle so subscribers see one logical re-emission burst per accepted batch.
- **P3 — Defensive Host, Fault-Tolerant Guest.** N/A — no Steel host primitive added. The L2/arch §9.4.1 contract has no implementation surface this slice.
- **P4 — Simplicity in implementation.** No new abstraction layers. The new `dispatch_buffer_edit` handler mirrors slice-003's `dispatch_buffer_open` exactly: pub(crate) free function, takes `(&mut BufferRegistry, &mut HashMap<EntityRef, BufferState>, &Event)`, returns an outcome enum. The CLI subcommands inline-call the existing inspect-library path for the pre-dispatch lookup; they do NOT spawn a child `weaver inspect` process or invent a new RPC primitive. No new crate; no new dependency; the `BufferRegistry` from slice 003 is extended to hold the buffer-state map already implicit in the publisher's `Vec<BufferState>` (refactored to a keyed lookup).
- **P5 — Serialization and open standards.** Bus: CBOR via `ciborium`; **no new Weaver CBOR tag**. `EventPayload::BufferEdit` rides the existing adjacent-tag enum machinery (`#[serde(tag = "type", content = "payload", rename_all = "kebab-case")]`); `TextEdit`/`Range`/`Position` are plain structs with `#[serde(rename_all = "kebab-case")]` on JSON. Wire variant tag: `"buffer-edit"`. Field names: `start`/`end` on `Range`; `line`/`character` on `Position`; `range`/`new-text` (kebab-case JSON; `new_text` snake_case in Rust) on `TextEdit`; `entity`/`version`/`edits` on the variant. CLI: `--output=json` continues via `serde_json`; new subcommands respect `--output` for emitted diagnostics. Continuous machine integration (the future agent in slice 006) will reach this surface as a bus subscriber — NOT by parsing `weaver edit` stdout — per Amendment 3.
- **P6 — Humane shell.** `clap` derive for `weaver edit` and `weaver edit-json`. Errors use `miette` / `thiserror` and reference fact-space state. Examples:
  - Buffer not opened: `WEAVER-EDIT-001 — buffer not opened: <path> — no fact (entity:<derived>, attribute:buffer/version) is asserted by any authority`.
  - Range parse failure: `WEAVER-EDIT-002 — invalid range "<arg>": expected <start-line>:<start-char>-<end-line>:<end-char>`.
  - JSON parse failure: `WEAVER-EDIT-003 — malformed edit-json input: <serde-json error chain>`.
  - Frame too large: `WEAVER-EDIT-004 — serialised BufferEdit (<n> bytes) exceeds wire-frame limit (65 536 bytes)`.
- **P7 — Public-Surface Enumeration.** Two surfaces touched:
  - **Bus protocol** — MAJOR wire-incompatible change. `Hello.protocol_version` advances `0x03 → 0x04`; `EventPayload::BufferEdit { entity, version, edits }` added; `TextEdit`/`Range`/`Position` struct types added (plain serialisation; no CBOR tag). Enumerated in `contracts/bus-messages.md`. No removals; no shape change to existing message kinds.
  - **CLI + structured output** — MINOR additive. `weaver edit` and `weaver edit-json` subcommands added to the `weaver` binary; `weaver --version` JSON `bus_protocol` field advances `0.3.0 → 0.4.0`; all four binaries (`weaver`, `weaver-buffers`, `weaver-git-watcher`, `weaver-tui`) inherit the bumped `bus_protocol` constant in their `--version` output (constant-driven; not a CLI-surface change). Enumerated in `contracts/cli-surfaces.md`.
  - **Fact-family schemas** — **read-only at the schema level**. No new fact family is introduced; no existing schema changes shape, value type, or authority. `buffer/version` (schema unchanged from PR #10's bootstrap forward-compat) gains a *mutation consumer* — the slice-004 publisher bumps it on accepted edit. The schema itself stays at v0.1.0; per `docs/05-protocols.md §7` and L2 P15, gaining a mutation consumer without changing the schema is not a schema event.
- **P8 — SemVer + Keep a Changelog Per Surface.** Bus protocol bumps MAJOR (`0.3.0 → 0.4.0`); `weaver` CLI surface bumps MINOR additive; `weaver-buffers`, `weaver-git-watcher`, `weaver-tui` CLI surfaces are byte-unchanged at the CLI grammar but constant-driven `bus_protocol` advances `0.3.0 → 0.4.0` in their `--version` JSON (mechanically inherited; not a CLI-surface schema event). `CHANGELOG.md` gains entries for the bus protocol bump (BREAKING) and the CLI MINOR additive. Every bus message on the new protocol carries `protocol_version: 0x04`.
- **P9 — Scenario + property-based testing.**
  - **Scenario tests**: bootstrap → `weaver edit` → fact re-emit observed (US1); 16-edit atomic batch happy path + validation-failure path (US2); stale-version drop (US1 #2 + US4); buffer-not-opened CLI exit 1 (US1 #4); `--why` walk to applied `BufferEdit` event (US1 #5); JSON input parity (US3).
  - **Property tests**: CBOR + JSON round-trip on `EventPayload::BufferEdit`, `TextEdit`, `Range`, `Position` over randomly-generated edit batches; SC-406 byte-identical-wire-payload property comparing the `weaver edit` positional emitter and `weaver edit-json` JSON emitter for semantically-equivalent inputs.
  - **Validator property tests**: `apply_edits` accepts a randomly-generated batch iff every `TextEdit` is bounds-valid + UTF-8-codepoint-boundary-valid + non-overlapping within the batch + non-nothing-edit; rejected batches produce no mutation observable via `memory_digest`.
- **P10 — Regressions captured as scenario tests before fix.** Convention continues. Inherited from slice-003 PR-discipline; no slice-004-specific deviation.
- **P11 — Provenance Everywhere.** Every `BufferEdit` event carries `Provenance { source: ActorIdentity::User, timestamp_ns, causal_parent: None }` when emitted by `weaver edit` / `weaver edit-json`. Re-emitted facts on accept share `causal_parent = Some(event.id)`, so `weaver inspect --why <entity>:buffer/version` walks from the fact to the originating event and thence to the User identity. `weaver --version` JSON `bus_protocol` advances to `0.4.0`; all four binaries inherit. The `User` variant of `ActorIdentity` was reserved at slice 002 for human-initiated CLI actions; slice 004 is its first production use.
- **P12 — Determinism and single-VM concurrency discipline.** `apply_edits` is deterministic given `(buffer state at start, batch)`. Validation does not depend on wall-clock or randomness. Apply order (descending offset) is canonical and reproducible. The publisher's reader-loop processes events sequentially per the existing single-task design; no shared mutable state outside the buffer-state map and tracked-fact set already established in slice 003.
- **P13 — Observability for Operators.** `tracing` spans wrap each accepted edit (at `info` level matching slice-003's `watcher/status` cadence) and each silent drop (at `debug` level per FR-018, with structured fields `event_id`, `entity`, `emitted_version`, `current_version`, reason category). CLI parse-time errors emit at `error` level. `tracing-subscriber` JSON layer respected via existing `--output=json`. The trace model (P11/L1 §15) preserves the original `BufferEdit` event's provenance via the existing `dispatcher.process_event` path; rejected edits are absent from `--why` walks.
- **P14 — Steel sandbox discipline.** N/A — no new Steel host primitive.
- **P15 — Schema evolution and trace-store migration.** No fact-family schema changes. `buffer/version`'s shape (`FactValue::U64`, single-value family, `weaver-buffers` authority) is unchanged from PR #10. Bus-protocol MAJOR bump (`0.3 → 0.4`) requires a wire-incompatibility entry in `CHANGELOG.md`; trace-store migration is N/A while traces are in-memory only. The `EventPayload` enum gains a variant under the existing adjacent-tag machinery; subscribers that cannot decode the new variant produce `Error { category: "decode", context: "unknown EventPayload variant: buffer-edit" }` — same compatibility constraint as slice-003's `FactValue::U64` introduction.
- **P16 — Failure Modes Are Public Contract.** Slice-004 documents:
  - **CLI exit codes** (`weaver edit`, `weaver edit-json`): `0` clean (event dispatched); `1` parse error / malformed input / path canonicalisation failure / JSON validation failure / pre-dispatch lookup found no `buffer/version` fact (buffer not opened); `2` bus unavailable. **No new exit code for stale-drop / validation-rejection at the service** — those are silent per FR-018 + FR-012; the CLI cannot detect them post-dispatch.
  - **Service-side silent-drop categories**: `unowned-entity`, `stale-version`, `future-version`, `validation-failure-<kind>`. Tracing-only per FR-018; no bus-level surface in slice 004 (forward direction = queryable error component on buffer entity, post-component-infra slice).
  - **Validation failure kinds**: `out-of-bounds`, `mid-codepoint-boundary`, `intra-batch-overlap`, `nothing-edit`, `swapped-endpoints`, `invalid-utf8`. Each lands as a `tracing::debug!` reason category and a structured ApplyError variant.
- **P17 — Documentation in lockstep.** This slice's design touches `docs/02-architecture.md §3.1` (events as lossy-class — confirms the silent-drop semantics) and `docs/07-open-questions.md §26` (component infrastructure deferral — referenced as the forward direction for rejection observability). No L1 change required. `CHANGELOG.md` updates land with `/speckit.implement` (the actual wire change lands with code). `contracts/bus-messages.md` and `contracts/cli-surfaces.md` (this slice's Phase-1 outputs) document the wire and CLI shapes.
- **P18 — Performance Budgets Per Latency Class.** SC-401 declares the single-edit plumbing-latency contract at *interactive* (≤ 500 ms operator-perceived, parity with slice-003 SC-302). Batch SC-402 / sequential SC-403 are explicitly **NOT** budget-bound (per spec Q4 resolution: imposing a wall-clock would force fragmentation workarounds that fight the atomic-batch architecture). The validation pipeline's CPU cost is O(n log n) on `edits.len()` (sort + linear overlap-scan) — within plan-level performance budgets, not raised to spec-level (a property test on validation time may be added during Phase 6 polish if SC-402 measurements reveal an anomaly).
- **P19 — Reproducible Builds.** `Cargo.lock` committed; no new dependencies; Steel + bus protocol versions pinned. Build info (per P11) advances `bus_protocol` to `0.4.0` in every binary's `--version` output via the `BUS_PROTOCOL_VERSION` constant.
- **P20 — Retraction Is First-Class.** No new fact-family retraction path; existing slice-003 retraction (SIGTERM → retract every tracked fact; SIGKILL → core's `release_connection` cleans up) is unchanged. Accepted edits **re-assert** the existing tracked facts (`buffer/byte-size`, `buffer/version`, `buffer/dirty`) — no retract-and-reassert pair, since fact values evolve under the existing key. The slice-003 `tracked: HashSet<FactKey>` invariant continues to hold without modification.
- **P21 — AI Agent Conduct.** Continues — Conventional Commits per Amendment 1; regression-tests-before-fix (P10); commits `Co-Authored-By`. Pre-commit hook runs clippy + fmt-check. Agent commits MUST run `scripts/ci.sh` before proposing per Amendment 6.

### Principles not exercised by this slice (justified)

- **P3** — no Steel host primitive added.
- **P14** — no Steel sandbox concerns.

### Additional constraints (must hold by implementation exit)

- **License (Amendment 4).** No new crate, no new inbound dependency. AGPL-3.0-or-later compliance is unchanged.
- **Wire vocabulary naming (Amendment 5).** All new identifier values are kebab-case: `buffer-edit` (event variant tag), `new-text` / `range` / `start` / `end` / `line` / `character` / `entity` / `version` / `edits` (struct field names on JSON; snake_case in Rust). Validation-failure reason categories: `unowned-entity`, `stale-version`, `future-version`, `validation-failure-out-of-bounds`, `validation-failure-mid-codepoint-boundary`, `validation-failure-intra-batch-overlap`, `validation-failure-nothing-edit`, `validation-failure-swapped-endpoints`, `validation-failure-invalid-utf8`. Diagnostic codes: `WEAVER-EDIT-001` through `WEAVER-EDIT-004`.
- **Code quality gates (Amendment 6).** New code passes `cargo clippy --all-targets --workspace -- -D warnings` and `cargo fmt --all -- --check`. `scripts/ci.sh` runs green before every commit. Pre-commit hook runs the full gate chain.
- **Conventional Commits (Amendment 1).** Per-task commits use conventional types (`feat(bus):`, `feat(buffers):`, `feat(cli):`, `test(buffers):`, `docs(specify):`). The bus-protocol-bump commit and the `EventPayload::BufferEdit` introduction commit carry `BREAKING CHANGE:` footers.

**Result**: PASS. No principle violated. No Complexity Tracking entries required. The two surfaces that trigger per-surface versioning (bus protocol MAJOR; `weaver` CLI MINOR) are enumerated in the Phase 1 contracts documents.

### Post-design re-check (after Phase 1 artifacts)

Re-evaluated after `research.md`, `data-model.md`, `contracts/bus-messages.md`, `contracts/cli-surfaces.md`, `quickstart.md` landed:

- All 19 applicable principles still hold.
- **P5** — wire shape pinned: `EventPayload::BufferEdit` rides existing adjacent-tag machinery; new struct types serialise as plain CBOR/JSON structs (no new tag); kebab-case throughout. No tag-registry edit required.
- **P7/P8** — bus protocol v0.3 → v0.4 (MAJOR) and `weaver` CLI MINOR additive enumerated in `contracts/`. `CHANGELOG.md` updates deferred to `/speckit.implement` (the wire change lands with code).
- **P11** — `ActorIdentity::User` first production use is documented in `data-model.md`; `weaver inspect --why` walk over an applied `BufferEdit` is exercised in `quickstart.md`.
- **P16** — failure-mode taxonomy (CLI exit codes, service-side silent-drop categories, validation-failure kinds) is fully enumerated in `contracts/bus-messages.md` §Failure modes and `contracts/cli-surfaces.md` §Exit codes.
- **P9** — CBOR + JSON round-trip property tests for the new struct types and SC-406 emitter-equivalence property test are itemised in `data-model.md` validation rules and `quickstart.md` verification steps.
- **P20** — accepted-edit fact re-emission is overwrite (same key, new value); no new retraction path; the tracked-fact set invariant is preserved.

**Re-check result**: PASS. Phase 1 design tightened P5/P7/P11/P16/P9/P20 coverage without introducing constitutional tension. No Complexity Tracking entries needed.

## Project Structure

### Documentation (this feature)

```text
specs/004-buffer-edit/
├── plan.md              # This file (/speckit.plan output)
├── spec.md              # Phase 0 input — feature specification (with Clarifications Session 2026-04-25)
├── research.md          # Phase 0 — apply-edits algorithm, in-process inspect-lookup, ApplyError taxonomy, emitter identity
├── data-model.md        # Phase 1 — TextEdit/Range/Position/BufferEdit; apply pipeline; state-transition mapping
├── quickstart.md        # Phase 1 — five-process walkthrough + SC-401..406 verification
├── contracts/
│   ├── bus-messages.md  # Phase 1 — v0.4 wire: new EventPayload::BufferEdit + TextEdit/Range/Position struct types
│   └── cli-surfaces.md  # Phase 1 — weaver edit + weaver edit-json subcommands; exit codes; --version constant bump
├── checklists/
│   └── requirements.md  # Spec quality checklist (passing post-clarify)
└── tasks.md             # Phase 2 output (/speckit.tasks — NOT created here)
```

### Source Code (repository root)

```text
core/
├── Cargo.toml                    # unchanged at member level (bus-protocol const drives --version JSON)
├── build.rs                      # unchanged (vergen)
└── src/
    ├── lib.rs                    # MODIFIED — re-exports new types (`TextEdit`, `Range`, `Position`)
    ├── main.rs                   # unchanged (CLI dispatch lives in cli/)
    ├── bus/
    │   ├── codec.rs              # unchanged (frame size constant exposed for CLI pre-check)
    │   └── listener.rs           # unchanged — `BusMessage::Event` dispatch path already handles `EventPayload::BufferEdit` via process_event
    ├── types/
    │   ├── event.rs              # MODIFIED — `EventPayload::BufferEdit { entity, version, edits }` ADDED
    │   ├── edit.rs               # NEW — `TextEdit`, `Range`, `Position` struct types + their serde derives + tests
    │   └── message.rs            # MODIFIED — `BUS_PROTOCOL_VERSION` 0x03 → 0x04; rendered `BUS_PROTOCOL_VERSION_STR` 0.3.0 → 0.4.0
    ├── behavior/                 # UNCHANGED
    ├── fact_space/               # UNCHANGED
    ├── trace/                    # UNCHANGED
    ├── inspect/                  # UNCHANGED (existing structured-identity path covers `ActorIdentity::User`)
    └── cli/
        ├── mod.rs                # MODIFIED — register `edit` + `edit-json` subcommands
        ├── args.rs               # MODIFIED — clap derive for new subcommands and their args
        ├── edit.rs               # NEW — `weaver edit` + `weaver edit-json` subcommand handlers; in-process inspect-lookup helper; range argv parser; JSON input parser; frame-size pre-check
        └── errors.rs             # MODIFIED — `WEAVER-EDIT-00{1,2,3,4}` miette diagnostic codes
buffers/
├── Cargo.toml                    # unchanged
└── src/
    ├── main.rs                   # unchanged
    ├── lib.rs                    # MODIFIED — re-export `apply_edits` + `ApplyError` from model.rs
    ├── observer.rs               # unchanged
    ├── publisher.rs              # MODIFIED — extend reader_loop with `BusMessage::Event(EventPayload::BufferEdit { .. })` arm; new `dispatch_buffer_edit` pub(crate) handler mirroring slice-003 `dispatch_buffer_open`; per-tick edit application + version bump + fact re-emit; `BufferRegistry` extended to expose entity-keyed `BufferState` lookup
    └── model.rs                  # MODIFIED — `BufferState::apply_edits(&mut self, &[TextEdit]) -> Result<(), ApplyError>`; `validate_batch`; `ApplyError` enum with all validation-failure kinds
tui/                              # UNCHANGED — existing `buffer/*` subscription already covers re-emitted facts
git-watcher/                      # UNCHANGED
ui/                               # UNCHANGED
tests/e2e/
├── (existing slice-001/002/003 tests — UNCHANGED)
├── buffer_edit_single.rs         # NEW — five-process: core + git-watcher + buffer-service + TUI/subscriber + `weaver edit`; SC-401 coverage
├── buffer_edit_atomic_batch.rs   # NEW — 16-edit batch happy path + 16-edit batch with one validation failure → no mutation; SC-402 coverage
├── buffer_edit_sequential.rs     # NEW — 100 sequential `weaver edit` invocations → version=100; SC-403 coverage
├── buffer_edit_stale_drop.rs     # NEW — concurrent emitters racing same version; SC-404 coverage
├── buffer_edit_inspect_why.rs    # NEW — applied edit + `weaver inspect --why <entity>:buffer/version` walks to the BufferEdit event; SC-405 coverage
└── buffer_edit_emitter_parity.rs # NEW — proptest comparing `weaver edit` and `weaver edit-json` wire payloads; SC-406 coverage
CHANGELOG.md                      # MODIFIED — bus-protocol MAJOR 0.4.0 entry + weaver CLI MINOR additive entry
Cargo.toml                        # workspace — UNCHANGED
```

**Structure Decision**: extend existing crates in place. No new workspace member is needed: the wire surface lives in `core/src/types/`, the consumer in `buffers/src/publisher.rs`, the emitter on the existing `weaver` binary's CLI. This matches slice 003's pattern of "consumer in service crate, emitter in `weaver` core CLI" — and avoids creating a slice-004 crate that would have to be retired when `weaver edit` migrates to a future `weaver-cli` binary past north star (per spec §Locked decisions item 3). Alternatives considered: (a) standalone `weaver-edit` binary with its own crate — rejected because the retirement path is to `weaver-cli`, not to a permanent slice-004 binary; introducing then-retiring a crate adds churn without architectural benefit; (b) a shared `edit-types` workspace member holding `TextEdit`/`Range`/`Position` — rejected because these types are constitutional fixtures of the bus protocol and naturally live in `core/src/types/` alongside `event.rs`. The new types form a coherent `core/src/types/edit.rs` module.

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

No violations. Section intentionally empty.

---

*Plan complete. Phase 0 (research.md), Phase 1 (data-model.md, contracts/bus-messages.md, contracts/cli-surfaces.md, quickstart.md), and CLAUDE.md SPECKIT-block update follow as separate artifacts in the next steps of `/speckit.plan`.*
