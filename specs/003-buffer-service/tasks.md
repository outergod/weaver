---
description: "Task list for Slice 003 — Buffer Service implementation"
---

# Tasks: Buffer Service (Slice 003)

**Input**: Design documents from `/specs/003-buffer-service/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/bus-messages.md, contracts/cli-surfaces.md, quickstart.md

**Tests**: Scenario and property tests are required per L2 Constitution Principle 9 (not TDD — tests land alongside implementation, not before). e2e tests are required per the Success Criteria (SC-301..SC-307). All tests described below are MUST-have, not optional.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing.

## Weaver-specific markers used below

- **{retraction}** — task exercises a retraction path (P20).
- **{schema-migration}** — task touches a fact-family schema (P15); slice 003 is additive-only at the family level.
- **{surface:bus|cli|fact}** — task changes a public surface (P7 + P8); requires a CHANGELOG entry.
- **{latency:interactive}** — operation declared on the ≤100 ms bus-level budget (P18).

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Scaffold the new `buffers` workspace member and wire up the shared dependencies.

- [X] T001 Add `buffers` to the `members` list in the workspace `Cargo.toml`
- [X] T002 [P] Add `sha2` to `[workspace.dependencies]` in the workspace `Cargo.toml`, pinned to a minor version per `research.md §1`
- [X] T003 [P] Create `buffers/Cargo.toml` declaring the `weaver-buffers` binary, `license.workspace = true`, runtime dependencies (`tokio`, `clap` with `derive`, `miette`, `thiserror`, `tracing`, `tracing-subscriber`, `uuid`, `sha2`, `humantime`, `serde`, `ciborium`, path dep on `weaver-core`), and `[dev-dependencies]` with `tempfile.workspace = true` + `proptest.workspace = true` (A2 fix from `/speckit-analyze`: `tempfile` is already a workspace dep — slice 002 uses it via `git-watcher` and `tests/` — but must be inherited explicitly by this crate)
- [X] T004 [P] Create `buffers/src/main.rs` with a clap stub that prints a Phase-1 marker (`"weaver-buffers (slice 003 scaffold)"`) and exits
- [X] T005 [P] Create `buffers/src/lib.rs` with module declarations for `model`, `observer`, `publisher` (each module stubbed with `// TODO: slice 003` comment)
- [X] T006 [P] Create `buffers/README.md` — role (content-backed service), usage snippet, link to `specs/003-buffer-service/spec.md` and `plan.md`
- [X] T007 Verify `cargo build --workspace` passes end-to-end with the new crate compiled; verify `scripts/ci.sh` green (clippy + fmt-check + test). No functional behavior yet — this just confirms the scaffold compiles under the workspace gates.

**Checkpoint**: Scaffold compiles; the `weaver-buffers` binary exists but does nothing useful.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Bus-protocol and core-crate changes that MUST land before any user-story publishing can work. These are wire-breaking and touch shared public surfaces.

**⚠️ CRITICAL**: No user-story work can begin until this phase is complete. Every task here either changes wire shapes or removes slice-001 artifacts the new wire depends on being gone.

### Wire vocabulary changes

- [X] T008 {surface:bus} {surface:fact} Add `FactValue::U64(u64)` variant to `core/src/types/fact.rs` under the existing `#[serde(tag = "type", content = "value", rename_all = "kebab-case")]` adjacent-tag; wire form `{"type":"u64","value":<n>}`
- [X] T009 [P] {surface:bus} Update `core/src/types/fact.rs` unit tests to cover `FactValue::U64` round-trip (JSON + CBOR via `ciborium`)
- [X] T010 {surface:bus} Remove `EventPayload::BufferEdited` and `EventPayload::BufferCleaned` from `core/src/types/event.rs`; remove any downstream unit tests that construct them
- [X] T011 {surface:bus} Add `EventPayload::BufferOpen { path: String }` to `core/src/types/event.rs`; wire form `{"type":"buffer-open","payload":{"path":"<p>"}}`; kebab-case variant tag per Amendment 5
- [X] T012 [P] {surface:bus} Update `core/src/types/event.rs` unit tests: remove `BufferEdited` / `BufferCleaned` round-trip coverage; add `BufferOpen` round-trip coverage (JSON + CBOR)
- [X] T013 {surface:bus} Bump `Hello.protocol_version` constant from `0x02` to `0x03` wherever it is defined in `core/src/bus/`; update the version-mismatch handshake test to assert the new "bus protocol 0x03 required; received 0x02" detail string

### Behavior and CLI removals

- [X] T014 Delete `core/src/behavior/dirty_tracking.rs`; remove the module declaration from `core/src/behavior/mod.rs`; remove the registration call from `core/src/cli/run.rs` (or wherever the dispatcher's behavior list is constructed)
- [X] T015 [P] Remove unit tests that exercise the deleted `DirtyTrackingBehavior` from `core/src/behavior/`
- [X] T016 {surface:cli} Delete `core/src/cli/simulate.rs`; remove the `simulate-edit` / `simulate-clean` subcommand registrations from `core/src/cli/mod.rs` and any `clap` subcommand enum that lists them
- [X] T017 [P] {surface:cli} Remove the `simulate_edit_*` / `simulate_clean_*` CLI unit tests from `core/src/cli/simulate.rs`'s test module (deleted with T016) and from `core/src/cli/mod.rs`'s integration tests if present

### Version-string bumps

- [X] T018 [P] {surface:cli} Update `weaver --version` output: human form shows `bus protocol: v0.3.0`; JSON form field `bus_protocol` is `"0.3.0"`. Source in `core/src/cli/` (wherever the version constant or build-info rendering lives)
- [X] T019 [P] {surface:cli} Same update for `weaver-tui --version` in `tui/src/cli.rs` (or equivalent path)
- [X] T020 [P] {surface:cli} Same update for `weaver-git-watcher --version` in `git-watcher/src/main.rs` or its cli module

### Foundational buffer-crate skeleton

- [X] T021 [P] Add `buffer_entity_ref(path: &Path) -> EntityRef` to `buffers/src/model.rs` reserving bit 61 per `data-model.md §Entity-id derivation`; include unit tests for reserved-bit invariants (bit 61 set, bits 62/63 cleared) and for path-canonicalization idempotence
- [X] T022 [P] Add `watcher_instance_entity_ref(instance: &Uuid) -> EntityRef` to `buffers/src/model.rs` mirroring slice 002's derivation (bit 62 reserved); include unit tests
- [X] T023 [P] Add `BufferState` struct to `buffers/src/model.rs` with private fields (`path`, `entity`, `content`, `memory_digest`, `last_dirty`, `last_observable`); expose a single constructor that reads the file, computes `memory_digest`, and sets initial `last_dirty=false` / `last_observable=true`; include unit tests asserting `memory_digest == sha256(content)` invariant
- [X] T024 [P] Add `BufferObservation` struct and `ObserverError` enum to `buffers/src/model.rs` per `data-model.md §Internal types`

### CHANGELOG scaffolding

- [X] T025 {surface:bus} {schema-migration} Update `CHANGELOG.md` Public Surfaces Tracked section: bus protocol v0.3.0 (was v0.2.0); new fact families `buffer/path`, `buffer/byte-size`, `buffer/dirty`, `buffer/observable` at v0.1.0; CLI MAJOR for `simulate-edit` / `simulate-clean` removal; new `weaver-buffers` binary at 0.1.0. Body section left blank for per-story tasks to fill.

**Checkpoint**: Foundation ready. Wire v0x03 is live; `simulate-edit` / `simulate-clean` are gone; `core/dirty-tracking` behavior is gone; `FactValue::U64` and `EventPayload::BufferOpen` are on the wire; the `buffers` crate has compile-ready types for what US1 will build on. No user-facing behavior observable yet.

---

## Phase 3: User Story 1 — Observe a file's state through a buffer service (Priority: P1) 🎯 MVP

**Goal**: Launching `weaver-buffers <file>` publishes the file's derived facts over the bus; the TUI renders the file's path, size, and dirty state; external mutation flips dirty; SIGTERM / SIGKILL cleanly retract.

**Independent Test**: Three-process scenario (core + TUI + single-buffer `weaver-buffers`) reproduces SC-301, SC-302, SC-303. Tested end-to-end with `tempfile::TempDir` fixtures and `ChildGuard`-style process ownership.

### Observer

- [X] T026 [P] [US1] {latency:interactive} Implement `observer::observe_buffer(state: &BufferState) -> Result<BufferObservation, ObserverError>` in `buffers/src/observer.rs`: stream the on-disk file through a SHA-256 hasher to produce `disk_digest`; compare to `state.memory_digest`; construct `BufferObservation { byte_size, dirty: disk != memory, observable: true }`; include unit tests using in-memory fixtures and `tempfile::NamedTempFile`
- [X] T027 [P] [US1] Categorize `observer::observe_buffer` errors: `TransientRead` (I/O error during read), `Missing` (file no longer exists), `NotRegularFile` (file was replaced by a directory/other); include unit tests for each branch

### Publisher — connection + identity

- [X] T028 [US1] {surface:bus} Implement `publisher::run(paths: Vec<PathBuf>, socket: PathBuf, poll_interval: Duration) -> Result<(), PublisherError>` scaffold in `buffers/src/publisher.rs`: construct `ActorIdentity::Service { service_id: "weaver-buffers", instance_id: Uuid::new_v4() }`; connect to the socket; perform the handshake (reusing `weaver_core::bus::client::Client`); split the stream post-handshake for the reader-loop pattern; depend on T023 for `BufferState` construction, T013 for the protocol version
- [X] T029 [P] [US1] Implement `publisher::reader_loop` in `buffers/src/publisher.rs` mirroring slice 002's reader task: drain server-sent `BusMessage::Error` frames; classify `authority-conflict` (fatal, exit 3), `not-owner` (fatal via AuthorityConflict with prefix, exit 3), other (exit 10). Slice-002 F31 follow-up remains out of scope for this slice per `research.md §9`

### Publisher — bootstrap

- [X] T030 [US1] {surface:fact} {schema-migration} {latency:interactive} Implement per-buffer bootstrap publication in `buffers/src/publisher.rs`: for each `BufferState`, synthesize a bootstrap-tick `EventId` and publish `buffer/path` (String), `buffer/byte-size` (U64), `buffer/dirty=false` (Bool), `buffer/observable=true` (Bool), all carrying `causal_parent = Some(bootstrap_tick)`. Depends on T023, T028. Per `data-model.md §Bootstrap sequence`
- [X] T031 [US1] Publish service-level lifecycle in `buffers/src/publisher.rs`: `watcher/status=started` (once, before bootstrap loop, causal_parent=None) → per-buffer bootstrap → `watcher/status=ready` (once, after all bootstraps, causal_parent=None). Depends on T030
- [X] T032 [US1] Implement fail-fast startup in `buffers/src/publisher.rs`: if `BufferState::open(path)` fails for ANY path, retract any facts already asserted for successful opens and exit with code 1 after emitting a `miette::Diagnostic`. Depends on T023, T030

### Publisher — poll loop

- [X] T033 [US1] {latency:interactive} Implement the poll loop in `buffers/src/publisher.rs`: every `poll_interval`, iterate over `Vec<BufferState>` sequentially; for each call `observer::observe_buffer`; emit edge-triggered transitions per `data-model.md §Per-buffer observability state machine`. Depends on T026, T028, T030
- [X] T034 [US1] {surface:fact} Emit `buffer/dirty` transitions edge-triggered: re-assert only when `dirty` changes; update `state.last_dirty` after publishing. Tests exercise the "repeated same-state observation → no duplicate publish" invariant
- [X] T035 [US1] {surface:fact} Emit `buffer/observable=false` edge-triggered on the first failed observation; emit `buffer/observable=true` on recovery (FR-016, `data-model.md` validation rule 8). Update `state.last_observable`
- [X] T036 [US1] {surface:fact} Implement service-level `watcher/status=degraded` transition: fires only when (bus unreachable, OR all currently-open buffers have `last_observable==false` simultaneously) (FR-016a, `data-model.md` validation rule 9). Recovery re-publishes `ready`. Depends on T035

### Publisher — shutdown + retract

- [X] T037 [US1] {retraction} {surface:fact} Implement shutdown-retract in `buffers/src/publisher.rs`: on SIGTERM / SIGINT, iterate the owned fact set and publish `BusMessage::FactRetract` for each (matching slice 002's `shutdown_retract` pattern); then publish `watcher/status=unavailable` → `watcher/status=stopped`; abort the reader task; exit 0. Depends on T030, T031
- [X] T038 [US1] {retraction} Implement bus-EOF handling: on reader-loop EOF (core gone), return `PublisherError::BusUnavailable` with exit code 2; no retract attempt (bus is gone). Depends on T029

### CLI

- [X] T039 [US1] {surface:cli} Implement the `weaver-buffers` CLI in `buffers/src/main.rs` with clap derive: positional `<PATH>...` (variadic, at least one required), `--poll-interval` (humantime, default 250ms, reject 0ms), `--socket` (honors `WEAVER_SOCKET` env var), `--output=human|json`, `-v/-vv/-vvv`, `--version`. Dispatches to `publisher::run`. Per `contracts/cli-surfaces.md §Binary: weaver-buffers`
- [X] T040 [P] [US1] {surface:cli} Implement `weaver-buffers --version` output (human and JSON forms) per `contracts/cli-surfaces.md`. Reports `bus_protocol: "0.3.0"` and `service_id: "weaver-buffers"`
- [X] T041 [P] [US1] {surface:cli} Implement `miette::Diagnostic` error rendering for startup failures (WEAVER-BUF-001 path-not-openable, WEAVER-BUF-002 directory, WEAVER-BUF-003 too-large, WEAVER-BUF-004 authority-conflict) per `contracts/cli-surfaces.md §Error rendering`
- [X] T042 [P] [US1] Integrate `tracing` spans: wrap each poll tick, each per-buffer observation, each publish call. Honour `--output=human|json` via `tracing-subscriber`'s formatter choice. `-v` / `-vv` / `-vvv` escalate filter verbosity

### TUI — subscription + render

- [X] T043 [US1] {surface:cli} Add `FamilyPrefix("buffer/")` to the TUI's subscription set in `tui/src/client.rs` (alongside the existing `buffer/` subscription only if still needed — actually pre-slice-003 the TUI subscribed to slice 001's `buffer/dirty`, so this refinement may be only re-phrasing; verify the concrete change)
- [X] T044 [US1] {surface:cli} Add the Buffers render section to `tui/src/render.rs` below the existing Repositories section: one row per buffer entity, rendering `<path> [<bytes> bytes] <dirty-badge>` plus the authoring-actor line `by service weaver-buffers (inst <short-uuid>), event <id>, <t>s ago`. Depends on T043. Per `contracts/cli-surfaces.md §Binary: weaver-tui`
- [X] T045 [US1] {surface:cli} In the same `tui/src/render.rs` file introduced by T044, implement the `[observability lost]` badge replacement when `buffer/observable=false` and the `[stale]` marker when the TUI loses its core subscription (both per `contracts/cli-surfaces.md §Display rules`). **Not parallelizable with T044** — same file (A1 fix from `/speckit-analyze`).

### e2e tests (SC-301, SC-302, SC-303)

- [X] T046 [US1] Create `tests/e2e/buffer_open_bootstrap.rs`: four-process scenario (core + git-watcher + buffer-service + test-client) using `tempfile::TempDir` fixture; assert the initial `buffer/path`, `buffer/byte-size`, `buffer/dirty=false`, `buffer/observable=true`, and `watcher/status=ready` are delivered over the bus within 1 s of service start. SC-301 coverage
- [X] T047 [US1] Create `tests/e2e/buffer_external_mutation.rs`: open a buffer, mutate its file via `std::fs::write(path, new_content)`, assert `buffer/dirty` transitions `false → true` within 500 ms (SC-302); then revert content, assert `true → false` within 500 ms
- [X] T048 [US1] {retraction} Create `tests/e2e/buffer_sigkill.rs`: open a buffer, SIGKILL the service, assert every `buffer/*` fact owned by the dropped connection is retracted within 5 s via core's `release_connection`. SC-303 coverage

**Checkpoint**: User Story 1 is fully functional. `weaver-buffers <file>` runs end-to-end against a live core + TUI; external mutation flips dirty within budget; SIGTERM / SIGKILL both retract cleanly. SC-301, SC-302, SC-303 all pass. The slice has an operator-visible MVP.

---

## Phase 4: User Story 2 — Authority handoff (Priority: P2)

**Goal**: Confirm the authority transfer (`buffer/dirty` from behavior to service) is clean at every surface: inspection, CLI, tests. The slice-001 contract is retired; the slice-002 F23 live-fact-provenance invariant survives.

**Independent Test**: Inspection on any buffer fact attributes to `weaver-buffers`. `weaver simulate-edit` / `simulate-clean` no longer exist. An isolated scenario test proves F23 holds for the behavior→service overwrite case (FR-013).

### Inspection verification

- [X] T049 [US2] {surface:cli} e2e or CLI-level test in `tests/e2e/buffer_inspect_attribution.rs`: with a running buffer service, invoke `weaver inspect <entity>:buffer/dirty`; parse the JSON output; assert `asserting_service == "weaver-buffers"`, `asserting_instance` is a UUID, `asserting_behavior` is absent. SC-305 coverage
- [X] T050 [US2] {surface:cli} e2e test in `tests/e2e/buffer_simulate_removed.rs`: invoke `weaver simulate-edit 1` and `weaver simulate-clean 1`; assert exit code 2 (clap parse error) and stderr contains `"unrecognized subcommand"`. Verifies the CLI MAJOR-bump removal. Depends on T016

### F23 live-fact-provenance isolated scenario

- [X] T051 [US2] Create `core/tests/inspect/buffer_behavior_service_overwrite.rs` (scenario test in the core crate's integration test dir): build an in-memory dispatcher; inject a behavior-authored `buffer/dirty=true` fact into the fact store (via a test-only helper or direct construction); then synthesize a service-authored `buffer/dirty=false` FactAssert on the same key via the bus; call `inspect_fact` on that key; assert the returned `InspectionDetail` attributes the fact to the service, NOT to the behavior. Exercises FR-013 + the F23 live-fact-provenance invariant. NOTE: this test constructs both authors manually; it does NOT require the `weaver-buffers` binary to run

### Slice-001 e2e test transformation

- [X] T052 [US2] Transform `tests/e2e/hello_fact.rs`: replace `simulate-edit` / `simulate-clean` invocations with a `weaver-buffers <fixture>` startup (using `tempfile::TempDir` + `std::fs::write`) and file-mutation drive; retain the "publish → observe → retract" skeleton shape; the test name may stay `hello_fact` or be renamed for clarity (author's choice). SC-307 part 1
- [X] T053 [US2] {retraction} Transform `tests/e2e/disconnect.rs`: replace `simulate-edit` driver with a `weaver-buffers` driver; preserve the SC-001-era "service disconnect → facts retract" shape; verify the observer pattern still surfaces the disconnect within the slice-002 5s budget. SC-307 part 2
- [X] T054 [US2] Update any other tests in `tests/e2e/` that referenced `simulate-edit` / `simulate-clean` (e.g., `subscribe_snapshot.rs` if applicable); retire any test whose only purpose was to exercise the deleted CLI and that is not worth rewriting — in that case, document the retirement in the test file's top comment block and in `CHANGELOG.md`

**Checkpoint**: User Story 2 is complete. The slice-001 authority model is wholly retired on the shipping surface; the slice-002 F23 invariant is covered by an isolated scenario test; slice-001 e2e tests are transformed, not dropped.

---

## Phase 5: User Story 3 — Multi-buffer within one invocation (Priority: P3)

**Goal**: One `weaver-buffers` invocation opens N files, publishes N independent buffer entities, each independently observable, all sharing one bus connection and one actor identity.

**Independent Test**: Launching `weaver-buffers ./a.txt ./b.txt ./c.txt` produces three TUI rows; external mutation of one file flips only that entity's dirty; inspect on any buffer fact shows the shared instance UUID; two concurrent invocations on the same path → second exits code 3.

### Multi-buffer publisher support

- [X] T055 [US3] {surface:cli} Implement path de-duplication at CLI parse time in `buffers/src/main.rs` (or a helper in `buffers/src/model.rs`): canonicalize each argv entry, collapse duplicates into one unique-canonical-path set, log a `debug!("deduped {n} path(s)", n = original - unique)` message when the count differs. FR-006a coverage. Depends on T039
- [X] T056 [US3] {surface:bus} Implement the `BufferOpen` idempotence invariant in `buffers/src/publisher.rs`: when the service's internal `BufferOpen` dispatch handler is invoked with a path whose derived entity is already owned, short-circuit to a no-op (no re-read, no fact re-publication, no trace write). FR-011a coverage. Depends on T030. *NOTE: the CLI-side dedup from T055 means this code path is NOT triggered in slice 003 by normal operation; this task implements the defensive handler for slice-004+ external producers, with unit-test coverage*
- [X] T057 [P] [US3] Unit test in `buffers/src/publisher.rs`: construct two `BufferOpen` events for the same canonical path; assert the second fires no `FactAssert` / `FactRetract`. Covers FR-011a + `data-model.md` validation rule 7

### Multi-buffer TUI rendering

- [X] T058 [P] [US3] {surface:cli} Verify (or extend) the TUI's Buffers render section from T044 to render N distinct rows correctly; row ordering is deterministic by `(entity, attribute)` per slice 002 convention. If T044 already covers this (which it should; there's no multi-buffer-specific rendering code beyond "iterate the buffer set"), this is a verification-only task — mark it complete after confirming with a manual TUI walkthrough or integration test

### e2e tests (SC-304)

- [X] T059 [US3] Create `tests/e2e/buffer_multi_buffer.rs`: launch `weaver-buffers` with three `tempfile::NamedTempFile` fixtures; assert three independent buffer entities appear in the fact store; mutate file B externally; assert only B's `buffer/dirty` flips; assert all three facts carry the same `asserting_instance` UUID. US3 coverage
- [X] T060 [US3] Create `tests/e2e/buffer_authority_conflict.rs`: launch a first `weaver-buffers` instance on a path; wait for `watcher/status=ready`; launch a second instance on an overlapping path; assert the second exits code 3 within 1 s; assert the first instance's facts are unperturbed throughout. SC-304 coverage
- [X] T061 [P] [US3] Create `tests/e2e/buffer_degraded_observable.rs`: open three buffers, delete one file, assert `buffer/observable=false` for that entity (edge-triggered once), `watcher/status` stays `ready`; delete the other two, assert `watcher/status=degraded` fires once; restore one file, assert `watcher/status=ready` + `buffer/observable=true` for the restored entity

**Checkpoint**: User Story 3 is complete. Multi-buffer within one invocation works end-to-end; authority-conflict across instances is enforced; per-buffer vs service-level degradation is cleanly separated.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Property tests, CHANGELOG finalization, documentation, and quickstart validation.

### Property tests

- [ ] T062 [P] Create `buffers/tests/component_discipline.rs`: proptest asserting SC-306 — for any random observation sequence over any random content, every `Fact` the publisher emits satisfies `matches!(fact.value, FactValue::String(_) | FactValue::U64(_) | FactValue::Bool(_))`. No `FactValue::Bytes`, no `FactValue::String` containing the file's content
- [ ] T063 [P] Create `core/tests/property/factvalue_u64_roundtrip.rs`: proptest asserting `FactValue::U64(n)` round-trips through CBOR and JSON for arbitrary `n in 0..u64::MAX`
- [ ] T064 [P] Create `buffers/tests/path_canonicalization.rs`: proptest asserting `buffer_entity_ref(canonicalize(p)) == buffer_entity_ref(canonicalize(p))` across arbitrary paths; reserved-bit invariants (bit 61 set, bits 62/63 cleared) hold universally. Covers `data-model.md` validation rules 1–3

### CHANGELOG body

- [ ] T065 {surface:bus} {surface:cli} {schema-migration} Fill in the `CHANGELOG.md` `[Unreleased] — slice 003 Phase 2` body: enumerate the bus-protocol changes (event removals, event addition, FactValue::U64, Hello bump), fact-family additions with initial schemas 0.1.0, CLI MAJOR removals (simulate-edit, simulate-clean), new `weaver-buffers` binary at 0.1.0. Match the slice-002 CHANGELOG entry style. Include a migration note for the slice-001 test transformation

### Documentation

- [ ] T066 [P] Update `docs/repository-layout.md` (if present) to mention the new `buffers/` workspace member; confirm `docs/01-system-model.md §2.4` cross-references this slice as the first component-authority instantiation (no edit required if the cross-reference is symmetric); confirm `docs/07-open-questions.md §26` still reads as intended

### Agent / assistant context

- [ ] T067 [P] Verify `CLAUDE.md` SPECKIT block points at slice 003's plan (already done during plan phase; this task is a sanity check after all edits have landed)

### Manual validation

- [ ] T068 Run the `quickstart.md` walkthrough by hand end-to-end; verify SC-301 through SC-307 each pass their described criteria. Cross-check the pre-commit hook runs green on any ad-hoc uncommitted state. Report any drift between what the walkthrough claims and what the running system does in a follow-up commit

### Version-timing check

- [ ] T069 [P] Ensure `weaver-buffers --version` median wall-clock time is ≤ 50 ms (matching slice 001's `weaver --version` budget from T075 of slice 001). Add a `core/tests/cli/weaver_buffers_version_timing.rs` benchmark that runs 5 `--version` invocations and asserts median ≤ 50 ms. Prints min/median/max to stderr for diagnostic visibility

### CI verification

- [ ] T070 Run `scripts/ci.sh` end-to-end on the 003-buffer-service branch: clippy green on all crates including `buffers/`; rustfmt check green; full workspace test suite green; slice-001 transformed e2e tests green; all new slice-003 e2e tests green. This is the final gate before PR

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies. Can start immediately.
- **Foundational (Phase 2)**: Depends on Phase 1. Blocks all user stories — the wire protocol must be on v0x03 and slice-001 artifacts must be gone before the service can publish anything.
- **User Story 1 (Phase 3)**: Depends on Phase 2. MVP.
- **User Story 2 (Phase 4)**: Depends on Phase 2 (CLI removal + behavior removal are prerequisites for the "it's really gone" tests). The inspection/F23 tests depend on the service being buildable but not necessarily on US1's e2e tests passing — US2 can overlap with US1's testing phase.
- **User Story 3 (Phase 5)**: Depends on Phase 3 (US1's publisher is the substrate for multi-buffer N>1; the code mostly already handles it, but e2e tests need the US1 publisher working).
- **Polish (Phase 6)**: Depends on all of US1, US2, US3 being green; Polish is where property tests, CHANGELOG body, manual walkthrough, and CI validation land.

### Within each User Story

- Models / types before services / CLI.
- Observer before publisher (the publisher calls the observer).
- Publisher before TUI rendering (the TUI observes facts the publisher emits).
- e2e tests LAST within each story — they depend on the feature being testable end-to-end.
- For tasks that assert facts: a `{retraction}` task MUST exist somewhere in the same phase (P20). T037 and T048 cover US1; T053 covers US2; T060–T061 cover US3.
- For tasks tagged `{schema-migration}`: the migration task (T025) lands in Phase 2 before consumer updates; individual fact-family-introduction tasks in Phase 3 ride the schema established in Phase 2.
- For tasks tagged `{surface:bus}` / `{surface:cli}` / `{surface:fact}`: each PR touching the surface MUST land with its `CHANGELOG.md` entry (P8); T025 scaffolds the surface list, T065 fills in the body.

### Parallel Opportunities

- **Phase 1 setup**: T002-T006 are independent file scaffolds — parallelizable.
- **Phase 2 foundational**: within the wire changes group, T009 (FactValue tests), T012 (Event tests) can run in parallel with the enum edits themselves (T008, T010, T011). T015 (dirty-tracking test removal) is parallel with the removal (T014). T017 (simulate-* test removal) is parallel with T016 (but both touch `core/src/cli/` — order sequentially if the same file). T018-T020 (version-string bumps in three crates) are all parallel. T021-T024 (model-module building blocks) are parallel.
- **Phase 3 US1**: T026 (observer core function) and T027 (error categorization) are parallel; T040-T042 (CLI extras) are parallel; T045 (TUI badge rendering) is parallel with other TUI work.
- **Phase 5 US3**: T057 (unit test) and T058 (TUI verification) are parallel.
- **Phase 6 Polish**: T062-T064 (property tests), T066 (docs), T067 (CLAUDE.md check), T069 (version-timing) are all parallel.

### Implementation Strategy

#### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001–T007).
2. Complete Phase 2: Foundational (T008–T025) — **CRITICAL**, blocks all stories.
3. Complete Phase 3: User Story 1 (T026–T048).
4. **STOP and VALIDATE**: Run the quickstart's SC-301 / SC-302 / SC-303 walkthrough; all three pass.
5. MVP ready for review; deploy/demo if ready.

#### Incremental Delivery

1. Setup + Foundational → Foundation ready.
2. US1 → MVP; deploy.
3. US2 → authority-handoff verification; deploy.
4. US3 → multi-buffer; deploy.
5. Polish → property tests, CHANGELOG, CI gate → PR.

#### Parallel Team Strategy

- Team completes Phase 1 + Phase 2 together (wire changes need coordination).
- Once Phase 2 is done:
  - Developer A: US1 (T026–T048).
  - Developer B: US2 (T049–T054) — runs in parallel; depends on US1's service binary existing but not necessarily on US1's tests passing.
  - Developer C: US3 (T055–T061) — starts once US1's publisher is connecting end-to-end.
- Polish runs last, coordinated across the team.

---

## Notes

- `[P]` tasks = different files, no incomplete-dependency conflicts.
- `[USN]` maps task to specific user story for traceability.
- Each user story should be independently completable and testable per the independence rule.
- Weaver markers (`{retraction}`, `{schema-migration}`, `{surface:...}`, `{latency:...}`) are review notes; they do NOT affect parallel-execution semantics.
- Commit after each task or logical group; Conventional Commits per Amendment 1. Breaking surfaces get `BREAKING CHANGE:` footers: bus protocol bump (T013), event removal (T010), CLI subcommand removal (T016).
- Pre-commit hook MUST stay green at every commit (Amendment 6).
- Slice-002 F31 follow-up (identity-drift / invalid-identity reader-loop reclassification) is OUT OF SCOPE for slice 003 per `research.md §9` — batched for a future soundness slice.
- Slice-002 open debt #2 (Events lack conn-bound identity) and #5 (service-id squatting) are OUT OF SCOPE; spec's FR-021 / FR-022 document them as known gaps with review triggers before slice 006.

---

## Parallel Example: User Story 1

```bash
# Launch observer + reader-loop in parallel (different files, no dependency):
Task: "Implement observer::observe_buffer in buffers/src/observer.rs"
Task: "Implement publisher::reader_loop in buffers/src/publisher.rs"

# Launch CLI extras in parallel (different modules):
Task: "Implement weaver-buffers --version output"
Task: "Implement miette::Diagnostic error rendering"
Task: "Integrate tracing spans"
```

---

## Total Task Count

| Phase | Tasks | Notes |
|---|---|---|
| Phase 1: Setup | 7 | T001–T007 |
| Phase 2: Foundational | 18 | T008–T025 |
| Phase 3: User Story 1 (MVP) | 23 | T026–T048 |
| Phase 4: User Story 2 | 6 | T049–T054 |
| Phase 5: User Story 3 | 7 | T055–T061 |
| Phase 6: Polish | 9 | T062–T070 |
| **Total** | **70** | |
