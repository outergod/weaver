# Implementation Plan: Buffer Save

**Branch**: `005-buffer-save` | **Date**: 2026-04-27 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/005-buffer-save/spec.md`

## Summary

Slice 005 layers disk write-back on top of slice 004's in-memory edit machinery, completing the dirty-state ↔ disk-state coupling that slice 004 scaffolded but could not close. The wire surface gains one new event variant `EventPayload::BufferSave { entity: EntityRef, version: u64 }`; the `weaver` CLI grows one new subcommand (`weaver save <ENTITY>`); `weaver-buffers` extends its dispatcher with a `BufferSave` arm that performs an atomic POSIX disk write (tempfile-in-same-dir + `fsync` + `rename(2)`) gated by a pre-rename inode check against the inode captured at `BufferOpen` time. Save is non-mutating w.r.t. `buffer/version`; on success it re-emits `buffer/dirty = false` with `causal_parent = Some(event.id)`, completing the inverse of slice 004's edit-time `dirty = true` flip. A clean-save flow (FR-005) handles the `buffer/dirty = false` case as a structured no-op success: idempotent fact re-emission + `WEAVER-SAVE-007 "nothing to save"` info-level diagnostic, no disk I/O, no inode check.

The slice **folds in `docs/07-open-questions.md §28(a)`** — under the same wire bump — via a **2026-04-29 constitutional re-derivation** of the original §28(a) direction (which the 2026-04-27 spec proposed as listener-stamped IDs / ID-stripped envelope; superseded). Per-principle audit against `docs/00-constitution.md` found listener-stamping misaligned on §2/§11/§12/§15/§16 (originator-pattern bootstrap-chain regression — `weaver-buffers`'s `bootstrap_tick` shares the BufferOpen event's id as `causal_parent` for its bootstrap facts, which under listener-stamping becomes unknowable to the producer under fire-and-forget / lossy delivery) and in tension with §1/§6/§17 (centralisation of ID-arbitration authority). The replacement direction: **producer-minted UUIDv8** with the producer's hashed identity in the high 58 bits of the custom payload (Service `instance_id` for Service producers, hashed via `std::collections::hash_map::DefaultHasher` SipHash; per-process UUIDv4 for non-Service producers, same hash) and nanoseconds in the low 64 bits. Cross-producer collision is structurally impossible — distinct producers occupy disjoint 58-bit-prefix namespaces. The producer's local id is final; the listener does not stamp. Bootstrap-chain affordance preserved trivially. Carry-forward deferral: listener-side prefix-vs-provenance verification (catching a malicious producer that mints under another producer's prefix) joins FR-029's slice-006 close-out — same hazard class as the unauthenticated edit/save channel. All four current `EventId::new(now_ns())` mint sites migrate (`core/src/cli/edit.rs`, `buffers/src/publisher.rs` poll + bootstrap, `git-watcher/src/publisher.rs`); `core/src/cli/save.rs` is born compliant.

The bus protocol bumps **0x04 → 0x05**, BREAKING for two coupled reasons: the new `BufferSave` variant + the `EventId` wire-shape change from `u64` (8 bytes) to `Uuid` (16 bytes; UUIDv8). `Event`'s envelope shape is unchanged from slice-001 canonical (no envelope split). The slice-004 `validate_event_envelope` `EventId::ZERO`-rejection guard is preserved at the listener as `EventId::nil()`-rejection (semantically unchanged, retargeted at the new wire shape); the consumer-side `lookup_event_for_inspect` short-circuit is preserved as a defence against pre-§28(a) trace entries (FR-024). Wire-stability is not a concern at this stage — operator confirmed Weaver has no users yet.

Six diagnostic codes ship under the `WEAVER-SAVE-NNN` namespace covering buffer-not-opened (-001, CLI side), stale-version drop (-002, debug), tempfile I/O failure (-003, error), `rename(2)` I/O failure (-004, error), inode-mismatch refusal (-005, warn), path-deleted refusal (-006, warn), plus the seventh code WEAVER-SAVE-007 (clean-save no-op, info) introduced under Q2's resolution. Format mirrors slice-004's `WEAVER-EDIT-NNN` discipline (structured `tracing` records; no fact-space surface); forward direction for queryable rejection observability inherits `docs/07-open-questions.md §26`. One slice-004 hygiene carryover lands in this slice: `core/src/cli/edit.rs::handle_edit_json` grows a comment explaining why empty-`[]` JSON is intentionally not short-circuited (slice-004 spec.md §220 reserves wire-level empty `BufferEdit` for future tool handshake-probe affordance).

## Technical Context

**Language/Version**: Rust 2024 edition; resolver = "3" (workspace-level); toolchain pinned to 1.94.0 via `fenix` per `flake.nix` + `rust-toolchain.toml` (unchanged from slices 002/003/004).
**Primary Dependencies**: existing — `tokio`, `ciborium`, `serde` + `serde_json`, `clap` derive, `miette`, `thiserror`, `tracing` + `tracing-subscriber`, `proptest`, `vergen`, `crossterm`, `uuid` (v4), `humantime`, `tempfile` (workspace dev-dep), `sha2`, `rand` (existing — used for tempfile suffix entropy). **No new direct dependency** — POSIX `fstat(2)` for inode capture goes through `std::os::unix::fs::MetadataExt::ino()` (no `nix` / `rustix` needed); atomic rename via `std::fs::rename` (POSIX-atomic on same filesystem, which the same-directory tempfile placement guarantees); `tempfile` is **dev-dep only** — production save uses bare `OpenOptions` + per-entity random-suffix path naming for orphan-identifiability.
**Storage**: Disk write-back to the buffer's canonical path (the path captured at `BufferOpen`). No persistence change for the trace or fact space; both remain in-process. Inode capture extends `BufferState` (an in-process structure introduced in slice 003).
**Testing**: `cargo test` (unit + scenario); `proptest` (CBOR + JSON round-trip for `EventPayload::BufferSave` and the new `EventId(Uuid)` shape; multi-producer EventId-prefix-uniqueness property test for SC-505 — 1000 events, 3 producers, walkback resolution + cross-producer collision-freedom); `tempfile`-based filesystem fixtures for disk-side scenarios (rename-induced inode delta SC-502, deletion SC-503, I/O-failure injection SC-504). E2e tests extend slice-004's five-process pattern with the additional `weaver save` invocation.
**Target Platform**: Linux + macOS desktop. Single machine. Bus over Unix-domain socket. POSIX semantics for `rename(2)` atomicity assumed (both target platforms honour it on same-filesystem rename).
**Project Type**: Rust workspace; **no new member crate**. Modifies `core/` (event-payload variant, `EventId` `u64`→`Uuid` cascade, UUIDv8 mint helper, CLI subcommand, `--version` output, bus protocol version), `buffers/` (publisher reader-loop arm + save method on `BufferState` + inode capture at `BufferOpen` + producer-side UUIDv8 mint with hashed `instance_id` prefix), `git-watcher/` (producer-side UUIDv8 mint with hashed `instance_id` prefix), `tui/` (passive-cache of UUIDv8 prefix → friendly_name binding for human-readable `EventId` rendering). `ui/` untouched (constant inherits).
**Performance Goals**: SC-501 — `weaver save` end-to-end (lookup + dispatch + listener-stamping + accept + atomic write + fact re-emit observable on subscriber) ≤ 500 ms median (interactive latency class per `docs/02-architecture.md §7.1`, parity with slice-004 SC-401). SC-505 — multi-producer stress harness 100% walkback resolution (correctness floor, not latency). SC-507 — clean-save no-op latency informational only (no budget; structurally trivial).
**Constraints**: Events lossy-class per `docs/02-architecture.md §3.1`; fire-and-forget CLI semantics (no exit-code signal for stale-drop, validation rejection, I/O failure, or refusal); UTF-8 byte content (no encoding conversion); no save-as / no auto-save / no concurrent-mutation-guard for in-place external edits (slice-006); **unauthenticated edit/save channel** explicitly inherited as a Known Hazard (FR-029) and NOT closed this slice.
**Scale/Scope**: ~700–1100 new LOC across `core`, `buffers`, `git-watcher`, `tui`, plus e2e tests. 1 new `EventPayload` variant (`BufferSave`); `EventId` wire shape `u64`→`Uuid` (cascading through every construction site workspace-wide; ~50+ sites, mostly mechanical); 1 new mint helper (`EventId::mint_v8(producer_prefix_58, time_or_counter)`); 1 new CLI subcommand (`save`); 1 new pub(crate) handler on the buffers publisher (`dispatch_buffer_save` + `BufferSaveOutcome`); 1 new public method on `BufferState` (`save_to_disk` returning `SaveOutcome`) + 1 new field on `BufferState` (captured `inode: u64`); 7 new diagnostic codes (`WEAVER-SAVE-001..007`); 4 producer-side mint-site migrations to UUIDv8 (`weaver edit`, `weaver-buffers` poll/bootstrap, `weaver-git-watcher` poll); 1 TUI passive-cache extension for prefix → friendly_name binding; 5 new e2e test files — `buffer_save_dirty` (SC-501), `buffer_save_inode_refusal` (SC-502 + SC-503), `buffer_save_atomic_invariant` (SC-504), `buffer_save_clean_noop` (SC-507), `multi_producer_uuidv8` (SC-505). 1 codec-rejection test (`eventid_uuid_strict_parsing` for SC-506). No new fact families. Bus protocol 0.4.0 → 0.5.0 (MAJOR; covers BufferSave variant + EventId wire-shape `u64`→`Uuid`).

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Gates derived from `.specify/memory/constitution.md` v0.7.0. Each L2 principle is named with a slice-specific gate. Principles not exercised by this slice are listed with forward-looking triggers.

### Applicable principles (PLANNED — must hold by `/speckit.implement` exit)

- **P1 — Domain modeling without type hierarchy.** `BufferSaveOutcome` is a small enum (`Applied { entity, version } | StaleVersion | NotOwned | InodeMismatch { expected, actual } | PathMissing | TempfileIo(io::Error) | RenameIo(io::Error) | CleanSaveNoOp { entity, version }`) — flat variants, not a nested taxonomy. `EventId(Uuid)` is a single-field newtype, not a sum type — distinct producers occupy distinct prefix namespaces by construction, expressed in the bit layout, not in the type system. The save method on `BufferState` is a single-purpose function over `&self` and `path: &Path` returning `SaveOutcome`.
- **P2 — Purity at edges, transactional state at core.** `BufferState::save_to_disk` is structurally safe: I/O-effecting but the in-memory `BufferState` is read-only during save (no mutation); failures (I/O error, inode mismatch, missing path) leave both buffer state and target file unchanged. The atomic-rename invariant (`rename(2)` on POSIX same-filesystem) ensures observers either see the pre-save file content or the post-save content — never a partial write. The publisher's bus side-effect (re-emitting `buffer/dirty = false`) follows the apply, shares `causal_parent = Some(event.id)` (the producer-minted UUIDv8 EventId), and is serialised through the existing `BusWriter`.
- **P3 — Defensive Host, Fault-Tolerant Guest.** N/A — no Steel host primitive added. The L2/arch §9.4.1 contract has no implementation surface this slice.
- **P4 — Simplicity in implementation.** The new `dispatch_buffer_save` handler mirrors slice-004's `dispatch_buffer_edit` exactly: pub(crate) free function, takes `(&mut BufferRegistry, &mut HashMap<EntityRef, BufferState>, &Event)`, returns `BufferSaveOutcome`. The `weaver save` CLI inline-calls the existing inspect-library path for the pre-dispatch lookup (slice-004 idiom). No new crate; no new dependency (the `uuid` crate is already in the workspace from slice 002). Tempfile naming is per-call random-suffix in the same directory as the target — no extracted abstraction; the tempfile lifecycle (create + write + fsync + rename + best-effort cleanup) is inline in `save_to_disk`. The `EventId` `u64`→`Uuid` cascade is mechanical; the UUIDv8 mint helper is a single function — no listener-side stamping infrastructure, no envelope-shape split, no architectural redesign.
- **P5 — Serialization and open standards.** Bus: CBOR via `ciborium`; **no new Weaver CBOR tag**. `EventPayload::BufferSave` rides the existing adjacent-tag enum machinery (`#[serde(tag = "type", content = "payload", rename_all = "kebab-case")]`); `EventId(Uuid)` rides plain CBOR-byte-string serialisation via the `uuid` crate's serde derive. Wire variant tag: `"buffer-save"`. Field names: `entity` / `version` (snake_case JSON; same in Rust) on the variant; `Event` field names unchanged from slice 001. `Provenance` continues to expose its existing `source` / `timestamp_ns` / `causal_parent` (snake_case) — `causal_parent` lives there, not on the envelope, per the slice-001 data model. CLI: `--output=json` continues via `serde_json`; `weaver save` respects `--output` for emitted diagnostics. Continuous machine integration (the future agent in slice 006) reaches this surface as a bus subscriber, NOT by parsing `weaver save` stdout — per Amendment 3.
- **P6 — Humane shell.** `clap` derive for `weaver save`. Errors use `miette` / `thiserror` and reference fact-space state. Examples:
  - Buffer not opened: `WEAVER-SAVE-001 — buffer not opened: <entity> — no fact (entity:<derived>, attribute:buffer/version) is asserted by any authority`.
  - (Service-side codes -002..-007 emit via `tracing` with structured fields; not user-presented from CLI exit.)
  - Bus unavailable (exit 2): inherits slice-002's connection-error miette diagnostic.
- **P7 — Public-Surface Enumeration.** Two surfaces touched:
  - **Bus protocol** — MAJOR wire-incompatible change. `Hello.protocol_version` advances `0x04 → 0x05`; `EventPayload::BufferSave { entity, version }` added; `EventId` wire shape changes from `u64` (8 bytes) to `Uuid` (16 bytes; UUIDv8 producer-minted with hashed-instance-id prefix) cascading through `Event.id`, `causal_parent: Option<EventId>` on every fact, `EventInspectRequest.event_id`, etc.; `Event` envelope shape itself is unchanged (no envelope split). The slice-004 `validate_event_envelope` ZERO-rejection at the listener is preserved as `EventId::nil()`-rejection (semantically unchanged, retargeted at the new wire shape). Enumerated in `contracts/bus-messages.md`. No removals at the variant level; the wire-shape adjustment is on the `EventId` carrier inside `Event` and inside facts' `causal_parent`.
  - **CLI + structured output** — MINOR additive. `weaver save` subcommand added to the `weaver` binary; `weaver --version` JSON `bus_protocol` field advances `0.4.0 → 0.5.0`; all four binaries inherit the bumped `bus_protocol` constant in their `--version` output (constant-driven; not a CLI-surface change). Enumerated in `contracts/cli-surfaces.md`.
  - **Fact-family schemas** — **read-only at the schema level**. No new fact family is introduced; `buffer/dirty` (slice-003 schema, slice-004 mutation surface) gains an additional re-emission trigger (accepted save) but its shape, value type, and authority are unchanged. The schema itself stays at v0.1.0.
- **P8 — SemVer + Keep a Changelog Per Surface.** Bus protocol bumps MAJOR (`0.4.0 → 0.5.0`); `weaver` CLI surface bumps MINOR additive; `weaver-buffers`, `weaver-git-watcher`, `weaver-tui` CLI surfaces are byte-unchanged at the CLI grammar but constant-driven `bus_protocol` advances `0.4.0 → 0.5.0` in their `--version` JSON (mechanically inherited; not a CLI-surface schema event). `CHANGELOG.md` gains entries for the bus protocol bump (BREAKING) and the CLI MINOR additive. Every bus message on the new protocol carries `protocol_version: 0x05`.
- **P9 — Scenario + property-based testing.**
  - **Scenario tests**: edit → save → `buffer/dirty = false` + disk content matches in-memory (US1 / SC-501); save → externally rename → next save fires `WEAVER-SAVE-005`, file untouched (US2 / SC-502); save → externally delete → next save fires `WEAVER-SAVE-006`, no file created (US2 / SC-503); buffer-not-opened CLI exit 1 (US1 #4 / WEAVER-SAVE-001); save against clean buffer fires `WEAVER-SAVE-007` + idempotent re-emission, no mtime change (US1 / SC-507); concurrent saves trivially converge (Edge cases).
  - **Property tests**: CBOR + JSON round-trip on `EventPayload::BufferSave` and the new `EventId(Uuid)` shape over randomly-generated payloads; multi-producer UUIDv8 prefix-uniqueness property (SC-505) — generate K=3 producers each emitting N=1000 events in tight succession; assert `weaver inspect --why` walkback resolves to the correct producer in 100% of cases by cross-checking emitter-identity in event provenance against the originator AND assert no two distinct events across producers share an EventId (the UUIDv8 prefix-namespace partition makes this structurally guaranteed).
  - **I/O-failure injection tests** (SC-504): test harness intercepts `rename(2)` and forces `ENOSPC`; assert original disk file is byte-identical to its pre-save state and `WEAVER-SAVE-004` is emitted. Mechanism candidates evaluated in `research.md §3` (closure-as-hook surface in `atomic_write_with_hooks`).
- **P10 — Regressions captured as scenario tests before fix.** Convention continues. Inherited from slice-004 PR-discipline; no slice-005-specific deviation.
- **P11 — Provenance Everywhere.** Every `BufferSave` event carries `Provenance { source: ActorIdentity::User, timestamp_ns, causal_parent: None }` when emitted by `weaver save`; its `id: EventId` is producer-minted UUIDv8 with the per-process User-prefix. Re-emitted facts on accept share `causal_parent = Some(event.id)`, so `weaver inspect --why <entity>:buffer/dirty` walks from the fact to the originating event and thence to the User identity. `weaver --version` JSON `bus_protocol` advances to `0.5.0`; all four binaries inherit. The §28(a) fold-in resolves the cross-producer wall-clock-ns collision class that was latent since slice 001 — distinct producers occupy disjoint UUIDv8 prefix namespaces, making cross-producer collision structurally impossible.
- **P12 — Determinism and single-VM concurrency discipline.** `BufferState::save_to_disk` is deterministic given `(buffer state at start, target path, captured inode)` — modulo I/O outcomes (which are observable failure modes per P16, not non-determinism). The publisher's reader-loop processes events sequentially per the existing single-task design. UUIDv8 EventId minting is producer-local: each producer's prefix is a deterministic hash of its identity (Service `instance_id` or per-process UUIDv4); the low-bits time/counter is the producer's local invariant. Tempfile suffix uses `rand::random` for entropy — accepted as non-determinism for collision avoidance, scoped to file naming (not behavior).
- **P13 — Observability for Operators.** `tracing` spans wrap each accepted save (at `info` level matching slice-004's accepted-edit cadence) and each silent drop (at `debug` level per FR-013). Inode-mismatch refusal (-005) and path-missing refusal (-006) emit at `warn`; I/O failures (-003, -004) emit at `error`; clean-save no-op (-007) emits at `info`. CLI parse-time errors (-001) emit at `error` on the CLI's stderr. `tracing-subscriber` JSON layer respected via existing `--output=json`. The trace model preserves the original `BufferSave` event's provenance via the existing `dispatcher.process_event` path (now invoked with stamped Event); refused saves are absent from `--why` walks because `buffer/dirty` does not re-emit on refusal.
- **P14 — Steel sandbox discipline.** N/A — no new Steel host primitive.
- **P15 — Schema evolution and trace-store migration.** No fact-family schema changes. `buffer/dirty`'s shape (`FactValue::Bool`, single-value family, `weaver-buffers` authority) is unchanged. Bus-protocol MAJOR bump (`0.4 → 0.5`) requires a wire-incompatibility entry in `CHANGELOG.md`; trace-store migration is N/A while traces are in-memory only. The §28(a) re-derivation restructures the trace store's `by_event` index: previously keyed by `EventId(u64)` producer-minted from wall-clock-ns, now keyed by `EventId(Uuid)` producer-minted UUIDv8 with hashed-instance-id prefix. Long-running deployment traces from pre-§28(a) clients are not migrated — operator confirmed wire-stability is not a concern; pre-fix trace entries with `EventId::ZERO` are not present in any cross-version state because in-memory traces don't survive listener restart.
- **P16 — Failure Modes Are Public Contract.** Slice-005 documents:
  - **CLI exit codes** (`weaver save`): `0` clean (event dispatched); `1` parse error / malformed entity reference / pre-dispatch lookup found no `buffer/version` fact (`WEAVER-SAVE-001`); `2` bus unavailable. **No new exit code for service-side stale-drop / I/O failure / refusal** — those are silent per FR-011; the CLI cannot detect them post-dispatch.
  - **Service-side diagnostic codes**: `WEAVER-SAVE-001` (CLI; not service), `-002` (stale-version, debug), `-003` (tempfile I/O, error), `-004` (rename I/O, error), `-005` (inode mismatch refusal, warn), `-006` (path missing refusal, warn), `-007` (clean-save no-op, info). Tracing-only per FR-018; no bus-level surface in slice 005 (forward direction = queryable error component on buffer entity, post-component-infra slice).
  - **Atomic-rename invariant** (SC-504): under any failure between tempfile open and `rename(2)`, the original disk file is byte-identical to its pre-save state. The invariant lives in the POSIX `rename(2)` contract on same-filesystem renames; the same-directory tempfile placement is the precondition.
- **P17 — Documentation in lockstep.** This slice's design touches `docs/07-open-questions.md §28` (mark RESOLVED with sub-variant `[ADOPTED — UUIDv8 with hashed producer-instance-id prefix; spoofing-detection deferred to FR-029 close-out in slice 006]`, per the 2026-04-29 constitutional re-derivation) and references `docs/02-architecture.md §3.1` (events as lossy-class — confirms save's silent-drop semantics) and `docs/00-constitution.md` §1/§2/§6/§11/§12/§15/§16/§17 (the principles that drove the re-derivation away from listener-stamping). No L1 architectural change required. `CHANGELOG.md` updates land with `/speckit.implement` (the wire change lands with code). `contracts/bus-messages.md` and `contracts/cli-surfaces.md` (this slice's Phase-1 outputs) document the wire and CLI shapes. The slice-004 carryover hygiene (FR-025: `handle_edit_json` empty-`[]` comment) lands as a code-comment task.
- **P18 — Performance Budgets Per Latency Class.** SC-501 declares the single-save plumbing-latency contract at *interactive* (≤ 500 ms median operator-perceived, parity with slice-004 SC-401). The disk-I/O floor (open + write + fsync + rename) is dominated by `fsync` on rotational media; on commodity SSD typical latency is <10 ms. The 500 ms ceiling has ~50× margin on representative hardware. Multi-producer stress harness (SC-505) is correctness-bound, not latency-bound — measured wall-clock is informational only.
- **P19 — Reproducible Builds.** `Cargo.lock` committed; no new dependencies; Steel + bus protocol versions pinned. Build info (per P11) advances `bus_protocol` to `0.5.0` in every binary's `--version` output via the `BUS_PROTOCOL_VERSION` constant.
- **P20 — Retraction Is First-Class.** No new fact-family retraction path; existing slice-003 retraction (SIGTERM → retract every tracked fact; SIGKILL → core's `release_connection` cleans up) is unchanged. Accepted saves **re-assert** the existing tracked fact (`buffer/dirty` with new value `false`) — no retract-and-reassert pair, since the fact value evolves under the existing key. The slice-003 `tracked: HashSet<FactKey>` invariant continues to hold without modification. Refused saves and silent-drop saves do NOT re-assert any fact (no tracked-set adjustment).
- **P21 — AI Agent Conduct.** Continues — Conventional Commits per Amendment 1; regression-tests-before-fix (P10); commits `Co-Authored-By`. Pre-commit hook runs clippy + fmt-check. Agent commits MUST run `scripts/ci.sh` before proposing per Amendment 6.

### Principles not exercised by this slice (justified)

- **P3** — no Steel host primitive added.
- **P14** — no Steel sandbox concerns.

### Additional constraints (must hold by implementation exit)

- **License (Amendment 4).** No new crate, no new inbound dependency. AGPL-3.0-or-later compliance is unchanged.
- **Wire vocabulary naming (Amendment 5).** New enum-tag values are kebab-case: `buffer-save` (event variant tag). Struct field names follow the slice-001 `Event` / `Provenance` canon (snake_case on the wire because those structs carry no `rename_all` annotation): `entity` / `version` on the BufferSave variant; `name` / `target` / `payload` / `provenance` on `Event`; `source` / `timestamp_ns` / `causal_parent` inside `Provenance`. Diagnostic codes: `WEAVER-SAVE-001` through `WEAVER-SAVE-007`. Service-side diagnostic detail-fields: `expected_inode`, `actual_inode`, `errno`, `os_error` (matching slice-004's `WEAVER-EDIT-NNN` snake_case detail-field convention).
- **Code quality gates (Amendment 6).** New code passes `cargo clippy --all-targets --workspace -- -D warnings` and `cargo fmt --all -- --check`. `scripts/ci.sh` runs green before every commit. Pre-commit hook runs the full gate chain.
- **Conventional Commits (Amendment 1).** Per-task commits use conventional types (`feat(bus):`, `feat(buffers):`, `feat(cli):`, `feat(git-watcher):`, `test(buffers):`, `docs(specify):`, `refactor(bus):`). The bus-protocol-bump commit, the `EventPayload::BufferSave` introduction commit, and the `EventId(u64)→EventId(Uuid)` cascade commit each carry `BREAKING CHANGE:` footers. The §28(a) producer-mint-site migration commits are part of the BREAKING set.

**Result**: PASS. No principle violated. No Complexity Tracking entries required. The two surfaces that trigger per-surface versioning (bus protocol MAJOR; `weaver` CLI MINOR) are enumerated in the Phase 1 contracts documents.

### Post-design re-check (after Phase 1 artifacts)

Re-evaluated after `research.md`, `data-model.md`, `contracts/bus-messages.md`, `contracts/cli-surfaces.md`, `quickstart.md` landed:

- All 19 applicable principles still hold.
- **P1** — `BufferSaveOutcome` is a flat 8-variant enum (research.md §9); `EventId(Uuid)` is a single-field newtype; no envelope split, no type hierarchy. No type-taxonomy violation.
- **P2** — `BufferState::save_to_disk` is structurally safe: read-only on `BufferState::content` (no mutation); failures leave both buffer state and target file unchanged. Validation pipeline R1–R6 (data-model.md) is sequential and short-circuiting; no partial states are observable across rejection paths.
- **P5** — wire shape pinned: `EventId(Uuid)` rides plain CBOR-byte-string serialisation via the `uuid` crate's serde derive (research.md §5, contracts/bus-messages.md). No new CBOR tag; `EventPayload::BufferSave` rides existing adjacent-tag machinery; kebab-case throughout.
- **P7/P8** — bus protocol v0.4 → v0.5 (MAJOR) and `weaver` CLI MINOR additive enumerated in `contracts/`. `CHANGELOG.md` updates deferred to `/speckit.implement` (the wire change lands with code).
- **P11** — `ActorIdentity::User` reuse documented in `data-model.md`; `weaver inspect --why` walk over an applied `BufferSave` is exercised in `quickstart.md` (Scenario 1 verify); §28(a) UUIDv8 prefix-uniqueness invariant is verified in Scenario 1 (`event EventId(<prefix>/<short-suffix>)` rendering) and Scenario 5 (multi-producer stress + cross-producer collision-freedom).
- **P15** — no fact-family schema changes confirmed. `TraceStore::by_event` re-keying under §28(a) is documented in research.md §5; trace-store migration policy is N/A while traces are in-memory only.
- **P16** — failure-mode taxonomy fully enumerated: `BufferSaveOutcome` 8 variants (data-model.md §BufferSaveOutcome), 7 `WEAVER-SAVE-NNN` codes with surface + level + detail fields (cli-surfaces.md §WEAVER-SAVE-NNN diagnostic taxonomy), CLI exit codes 0/1/2 (cli-surfaces.md §weaver save §Exit codes).
- **P9** — CBOR + JSON round-trip property tests for `EventPayload::BufferSave` and the new `EventId(Uuid)` shape, multi-producer UUIDv8 prefix-uniqueness property test (SC-505), I/O-failure injection scenario (SC-504), and codec strict-parsing rejection scenario (SC-506) are all itemised across data-model.md, contracts/bus-messages.md §Failure modes, and quickstart.md.
- **P20** — accepted-save fact re-emission overwrites the existing `buffer/dirty` key with new value `false`; no retract-and-reassert; the `tracked: HashSet<FactKey>` invariant is preserved. Refused saves do NOT touch the tracked set.
- **P17** — `docs/07-open-questions.md §28` lockstep update plan documented in research.md §12; will land in the same commit as the §28-resolving code.

**Re-check result**: PASS. Phase 1 design tightened P1/P2/P5/P7/P8/P9/P11/P15/P16/P17/P20 coverage without introducing constitutional tension. No Complexity Tracking entries needed.

## Project Structure

### Documentation (this feature)

```text
specs/005-buffer-save/
├── plan.md              # This file (/speckit.plan output)
├── spec.md              # Phase 0 input — feature specification (Clarifications Sessions 2026-04-27 + 2026-04-29 §28(a) re-derivation)
├── research.md          # Phase 0 — atomic-save algorithm, inode capture mechanism, I/O-failure injection strategy, UUIDv8 producer-prefix scheme, EventId u64→Uuid migration ordering, UUIDv8 producer-ID namespacing constitutional re-derivation
├── data-model.md        # Phase 1 — EventPayload::BufferSave, BufferSaveOutcome, BufferState inode field, EventId(Uuid) shape, save pipeline state-transition mapping
├── quickstart.md        # Phase 1 — six-process walkthrough (core + buffers + git-watcher + TUI/subscriber + weaver edit + weaver save) + SC-501..507 verification
├── contracts/
│   ├── bus-messages.md  # Phase 1 — v0.5 wire: new EventPayload::BufferSave variant + EventId u64→Uuid wire-shape change + UUIDv8 producer-prefix convention
│   └── cli-surfaces.md  # Phase 1 — weaver save subcommand; exit codes; --version constant bump; WEAVER-SAVE-NNN diagnostic taxonomy
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
    ├── lib.rs                    # MODIFIED — re-exports new helpers (`EventId::mint_v8`, `EventId::nil`, prefix-hash helper)
    ├── main.rs                   # unchanged (CLI dispatch lives in cli/)
    ├── bus/
    │   ├── codec.rs              # UNCHANGED at function-signature level; serde-derived `EventId(Uuid)` round-trips through the existing `read_message` / `write_message`
    │   ├── event_subscriptions.rs # UNCHANGED (subscribers receive `Event` with producer-minted UUIDv8 id; mechanics unchanged from slice 004)
    │   └── listener.rs           # MODIFIED — `validate_event_envelope` ZERO-rejection retargeted at `EventId::nil()`. The `lookup_event_for_inspect` short-circuit (FR-024) preserved against `EventId::nil()` walkbacks. UUIDv8-prefix-vs-provenance verification is DEFERRED — joins FR-029's slice-006 close-out.
    ├── types/
    │   ├── event.rs              # MODIFIED — `EventPayload::BufferSave { entity, version }` ADDED. `Event` shape unchanged from slice-001 canonical.
    │   ├── ids.rs                # MODIFIED — `EventId(u64)` → `EventId(Uuid)` with UUIDv8 mint helper `EventId::mint_v8(prefix_58, time_or_counter)`, sentinel `EventId::nil()`, and deterministic-test constructor `EventId::for_testing(value: u128)`. `EventId::ZERO` removed (replaced by `EventId::nil()`). Per `research.md §5, §12`.
    │   └── message.rs            # MODIFIED — `BUS_PROTOCOL_VERSION` 0x04 → 0x05; rendered `BUS_PROTOCOL_VERSION_STR` 0.4.0 → 0.5.0. `BusMessage` shape unchanged.
    ├── trace/
    │   └── store.rs              # MODIFIED — `by_event` index re-keyed at `EventId(Uuid)` (mechanical type-cascade; no behavioural change). `find_event` semantics unchanged at the surface.
    ├── inspect/
    │   └── handler.rs            # MODIFIED — sentinel rendering update (`EventId::ZERO` → `EventId::nil()`); short-circuit semantics preserved. Optional: passive-cache binding extension for human-readable display (slice-005 task T-A4; new file or fold into existing).
    ├── fact_space/               # UNCHANGED
    ├── behavior/                 # MODIFIED (mechanical type-cascade only — `EventId(u64)` → `EventId(Uuid)` at call sites that construct or destructure)
    └── cli/
        ├── mod.rs                # MODIFIED — register `save` subcommand
        ├── args.rs               # MODIFIED — clap derive for `save` subcommand and its args
        ├── edit.rs               # MODIFIED — migrate emission to UUIDv8 with per-process User-prefix (FR-019); FR-025 hygiene comment at `handle_edit_json` post-parse step
        ├── save.rs               # NEW — `weaver save` subcommand handler; in-process inspect-lookup helper (reuses slice-004 path); event construction with UUIDv8-minted EventId
        ├── inspect.rs            # MODIFIED — passive-cache binding for prefix → friendly_name display in walkback rendering (slice-005 task T-A4)
        └── errors.rs             # MODIFIED — `WEAVER-SAVE-001` miette diagnostic code (CLI side); service-side codes -002..-007 emit via tracing only
buffers/
├── Cargo.toml                    # unchanged
└── src/
    ├── main.rs                   # unchanged
    ├── lib.rs                    # MODIFIED — re-export `save_to_disk` + `SaveOutcome` from model.rs
    ├── observer.rs               # unchanged
    ├── publisher.rs              # MODIFIED — subscribe to `payload-type=buffer-save` events on bootstrap (additive; `payload-type=buffer-edit` already wired in slice 004); reader_loop dispatches BufferSave to new dispatch_buffer_save handler. New `dispatch_buffer_save` pub(crate) handler mirrors slice-004 `dispatch_buffer_edit`; per-save flow per FR-003. Migrate poll-tick + bootstrap_tick emissions to UUIDv8 with hashed `instance_id` prefix (FR-019). Bootstrap_tick chain affordance preserved trivially — producer's local id is final.
    └── model.rs                  # MODIFIED — `BufferState::save_to_disk(&self, path: &Path) -> SaveOutcome` returning {Saved | InodeMismatch{expected, actual} | PathMissing | TempfileIo(io::Error) | RenameIo(io::Error) | CleanSaveNoOp}; `BufferState::open` extended to capture `inode: u64` via `MetadataExt::ino()`. New private `inode: u64` field on `BufferState` (immutable post-open).
git-watcher/
├── Cargo.toml                    # unchanged
└── src/
    └── publisher.rs              # MODIFIED — migrate poll-tick emissions to UUIDv8 with hashed `instance_id` prefix (FR-019). No semantic change to git-watching logic.
tui/
├── Cargo.toml                    # unchanged
└── src/
    └── render.rs                 # MODIFIED — passive-cache binding for prefix → friendly_name; `event EventId(<n>)` annotations render `EventId(<friendly_name>/<short-suffix>)` if known, full UUID otherwise (slice-005 task T-A3)
ui/                               # UNCHANGED
tests/e2e/
├── (existing slice-001/002/003/004 tests — UNCHANGED)
├── buffer_save_dirty.rs              # NEW — six-process: core + git-watcher + buffer-service + TUI/subscriber + `weaver edit` + `weaver save`; SC-501 coverage
├── buffer_save_inode_refusal.rs      # NEW — externally rename → save fires WEAVER-SAVE-005 (SC-502); externally delete → save fires WEAVER-SAVE-006 (SC-503)
├── buffer_save_atomic_invariant.rs   # NEW — I/O-failure injection between tempfile write and rename → original file unchanged (SC-504)
├── buffer_save_clean_noop.rs         # NEW — clean-save no-op flow: WEAVER-SAVE-007 + idempotent buffer/dirty re-emission + mtime preserved (SC-507)
├── multi_producer_uuidv8.rs          # NEW — 3 producers × 1000 events stress harness; weaver inspect --why walkback resolves to correct producer 100% + cross-producer collision-freedom (SC-505)
└── eventid_uuid_strict_parsing.rs    # NEW — codec rejects malformed UUID payload (wrong version bits / malformed bytes) at deserialise via uuid crate strict-parsing (SC-506)
docs/07-open-questions.md            # MODIFIED — §28 marked RESOLVED with sub-variant `[ADOPTED — UUIDv8 with hashed producer-instance-id prefix; spoofing-detection deferred to FR-029 close-out in slice 006]`
CHANGELOG.md                          # MODIFIED — bus-protocol MAJOR 0.5.0 entry + weaver CLI MINOR additive entry + §28(a) RESOLVED entry
Cargo.toml                            # workspace — UNCHANGED
```

**Structure Decision**: extend existing crates in place. No new workspace member is needed: the wire surface lives in `core/src/types/`, the consumer in `buffers/src/publisher.rs`, the emitter on the existing `weaver` binary's CLI. This matches slices 003/004's pattern of "consumer in service crate, emitter in `weaver` core CLI". The `EventId(Uuid)` cascade and the UUIDv8 mint helper sit in `core/src/types/ids.rs` because they are constitutional fixtures of the bus protocol; the producer-side mint sites construct via the helper and embed their own hashed-identity prefix locally. Alternatives considered: (a) a `wire-types` workspace member holding `EventId` + `Event` — rejected because slicing types out of `core` for re-import adds ceremony without reuse benefit (no other crate constructs these without depending on `core`); (b) listener-stamped IDs (the 2026-04-27 ID-stripped-envelope direction) — REJECTED on constitutional grounds per the 2026-04-29 re-derivation (see Summary + research.md §12).

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

No violations. Section intentionally empty.

---

*Plan complete (draft). Phase 0 (research.md), Phase 1 (data-model.md, contracts/bus-messages.md, contracts/cli-surfaces.md, quickstart.md), and CLAUDE.md SPECKIT-block update follow as separate artifacts in the next steps of `/speckit.plan`.*
