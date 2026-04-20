---

description: "Task list for Hello, fact (slice 001)"
---

# Tasks: Hello, fact

**Input**: Design documents from `/specs/001-hello-fact/`
**Prerequisites**: plan.md ✓, spec.md ✓, research.md ✓, data-model.md ✓, contracts/ ✓

**Tests**: Tests are REQUIRED for this slice per L2 P9 (scenario + property-based) and L2 P10 (regressions captured as scenario tests). Each non-trivial implementation task has a corresponding test task that lands FIRST.

**Organization**: Tasks are grouped by user story (US1 P1, US2 P2, US3 P2 from `spec.md`) so each story can be implemented and validated independently.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Maps to user story in `spec.md` (US1, US2, US3)
- File paths are repository-relative

## Weaver-Specific Task Categories

Per the L2 Constitution and the Vidvik review tasks-template additions, certain tasks carry markers in addition to `[P]` and `[Story]`:

- `{retraction}` — exercises a fact retraction path (P20). REQUIRED whenever a task asserts facts.
- `{latency:interactive}` — declares the latency class for the operation (per arch §7.1).
- `{surface:bus|fact|cli|config}` — task changes a public surface from P7. Pair with a CHANGELOG entry per P8.
- (Skipped in this slice: `{host-primitive}` — no Steel; `{schema-migration}` — initial schemas only.)

## Path Conventions

- **Workspace root**: `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`, `CHANGELOG.md`
- **Core crate**: `core/`
- **TUI crate**: `tui/`
- **End-to-end tests**: `tests/e2e/` at workspace root

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Workspace skeleton — make `cargo build --workspace` succeed (even with empty `fn main`).

- [X] T001 Update workspace `Cargo.toml` with `[workspace.package]` (edition = "2024", license, authors) and `[workspace.dependencies]` for shared deps (tokio, serde, serde_json, ciborium, clap, miette, thiserror, tracing, tracing-subscriber, proptest, vergen, crossterm)
- [X] T002 [P] Create `rust-toolchain.toml` pinning the stable channel (per L2 P19)
- [X] T003 [P] Update `.gitignore` to add `target/`, `**/*.rs.bk`, `*.sock` (Rust + bus socket file). Do **not** add `Cargo.lock` to `.gitignore` — workspace Cargo.lock MUST be tracked per L2 P19 (reproducible builds). Cargo's defaults are correct; this clause is for defensive clarity.
- [X] T004 [P] Create `CHANGELOG.md` at workspace root with initial entries: `bus protocol v0.1.0`, `buffer/dirty fact-family schema v0.1.0`, `CLI surface v0.1.0`, `configuration schema v0.1.0` (per L2 P7 / P8)
- [X] T005 Create `core/Cargo.toml` with both `[lib]` and `[[bin]]` targets, declaring workspace dependencies it needs and a `[build-dependencies]` entry for `vergen`
- [X] T006 Create `core/build.rs` invoking `vergen` to emit `VERGEN_GIT_SHA`, `VERGEN_GIT_DIRTY`, `VERGEN_BUILD_TIMESTAMP`, `VERGEN_CARGO_DEBUG` (per L2 P11)
- [X] T007 Create `core/src/lib.rs` with empty module declarations (`pub mod provenance; pub mod types; pub mod fact_space; pub mod bus; pub mod trace; pub mod behavior; pub mod inspect; pub mod cli;`)
- [X] T008 Create `core/src/main.rs` with a minimal `fn main()` that calls into `core::cli::run()` (returning `Result<(), miette::Report>`)
- [X] T009 [P] Create `tui/Cargo.toml` declaring a dependency on `core` (path = "../core") and workspace deps it needs (crossterm, tokio, ciborium, miette)
- [X] T010 [P] Create `tui/src/main.rs` with a minimal `fn main()` that calls into `tui::run()` plus `tui/src/lib.rs` with `pub mod client; pub mod render; pub mod commands;` and empty modules

**Checkpoint**: `cargo build --workspace` succeeds; both binaries exist (do nothing yet); `weaver --version` does not work yet (Phase 2).

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Domain types, FactStore trait + impl, bus codec/listener/dispatcher, CLI scaffolding, `weaver --version`. Required before any user story can begin because every story depends on bus + types + behavior dispatcher.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

### Domain types (parallel — all in different files)

- [X] T011 [P] Define `EntityRef(u64)` newtype + CBOR tag 1000 serialization in `core/src/types/entity_ref.rs`; add `pub mod entity_ref` to `core/src/types/mod.rs`
- [X] T012 [P] Define `Provenance { source, timestamp_ns, causal_parent }` and `SourceId` enum in `core/src/provenance.rs`; constructor enforces non-empty source
- [X] T013 [P] Define `EventId(u64)` and `BehaviorId(String)` newtypes in `core/src/types/ids.rs`
- [X] T014 [P] Define `FactKey { entity, attribute }`, `FactValue` enum (Bool/String/Int/Null), `Fact { key, value, provenance }` in `core/src/types/fact.rs`; CBOR tag 1001 for keyword serialization
- [X] T015 [P] Define `Event { id, name, target, payload, provenance }` and `EventPayload` enum (BufferEdited, BufferCleaned) in `core/src/types/event.rs`
- [X] T016 [P] Define `BusMessage` enum (Hello, Event, FactAssert, FactRetract, Subscribe, SubscribeAck, InspectRequest, InspectResponse, Lifecycle, Error) and supporting types (HelloMsg, SubscribePattern, LifecycleSignal, ErrorMsg, InspectionDetail, InspectionError) in `core/src/types/message.rs`
- [X] T017 [P] Define `TraceEntry { sequence, timestamp_ns, payload }` and `TracePayload` enum (Event, FactAsserted, FactRetracted, BehaviorFired, Lifecycle) in `core/src/trace/entry.rs`

### Foundational invariant tests (parallel — proptest-based)

- [X] T018 [P] Property test: `Provenance::new` rejects empty `External("")` source; timestamps are monotonic per SourceId in `core/src/provenance.rs` `#[cfg(test)] mod tests`
- [X] T019 [P] Property test: `EventId` round-trips through CBOR encode/decode preserving identity in `core/src/types/event.rs` `#[cfg(test)] mod tests`
- [X] T020 [P] Property test: `BusMessage` round-trips through CBOR for every variant (smoke-level coverage) in `core/src/types/message.rs` `#[cfg(test)] mod tests`

### Storage and codec layers (depend on types above)

- [X] T021 Define `FactStore` trait (`assert / retract / query / subscribe / snapshot`) AND `FactSpaceSnapshot` type (immutable view of all currently-asserted facts; cheaply-cloneable, e.g., `Arc<HashMap<FactKey, Fact>>`) in `core/src/fact_space/mod.rs`; reference `research.md` §13 in module docs noting the ECS-library decision is deferred
- [X] T022 Implement `HashMap`-backed `FactStore` (`InMemoryFactStore`) with subscription channels in `core/src/fact_space/in_memory.rs`; unit tests for assert→query, assert→retract→query, subscription receives event
- [X] T023 [P] {retraction} Property test: `assert(f) → retract(f.key) → query(f.key) == None`; `assert(f) → assert(f') → query(key) == Some(f')` (latest wins) in `core/src/fact_space/in_memory.rs` `#[cfg(test)] mod tests`
- [X] T024 Implement TraceStore (`append`, reverse causal index by EventId and FactKey, monotonic sequence) in `core/src/trace/store.rs`; unit tests for append + reverse-lookup
- [X] T025 Implement bus codec (length-prefixed CBOR framing via `ciborium`) in `core/src/bus/codec.rs`; unit tests for round-trip + frame-too-large rejection (>64 KiB)

### Bus listener and dispatcher (depend on storage + codec)

- [X] T026 Implement bus listener: Unix domain socket bind, accept loop, per-connection task; handshake validates `Hello { protocol_version: 0x01 }` in `core/src/bus/listener.rs`
- [X] T027 {surface:bus} Implement delivery class enforcement: lossy (drop-oldest) channels for `Event`/`stream-item`, authoritative (block-with-timeout, sequence numbers) for `FactAssert`/`FactRetract`/`Lifecycle`/`Error` in `core/src/bus/delivery.rs`
- [X] T028 Implement behavior dispatcher: single mpsc consumer over inbound events; calls registered behaviors with `&FactSpaceSnapshot`; commits behavior outputs (asserted/retracted facts + intents) atomically; emits `TraceEntry::BehaviorFired` in `core/src/behavior/dispatcher.rs`
- [X] T029 [P] Property test: per-publisher sequence numbers in delivery layer are strictly monotonic in `core/src/bus/delivery.rs` `#[cfg(test)] mod tests`

### CLI scaffolding and version

- [X] T030 {surface:cli} Implement CLI scaffolding (clap derive) with subcommands `run`, `status`, `inspect`, `simulate-edit`, `simulate-clean`, and global flags `-v/--verbose`, `-o/--output=<format>`, `--socket=<path>` in `core/src/cli/args.rs`
- [X] T031 {surface:cli} Implement `weaver --version` rendering all P11 fields (crate version, git SHA, dirty bit, build timestamp, build profile, bus protocol version) in both human and json forms in `core/src/cli/version.rs`
- [X] T032 [P] Test: `weaver --version --output=json` produces valid JSON containing all five P11 fields and the bus protocol version in `core/src/cli/version.rs` `#[cfg(test)] mod tests`
- [X] T033 Implement tracing setup (tracing-subscriber with EnvFilter via RUST_LOG; structured spans to stderr) wired into `core/src/main.rs` startup
- [X] T034 {surface:config} Implement configuration loading (XDG paths + env vars + CLI flag override precedence per L2 Additional Constraints) in `core/src/cli/config.rs`

### TUI client foundation

- [X] T035 Implement TUI bus client (connect to socket, send Hello, await Lifecycle::Ready, send Subscribe(FamilyPrefix("buffer/"))) in `tui/src/client.rs`
- [X] T036 Implement TUI render skeleton (crossterm raw-mode, event loop, basic frame rendering with connection status + facts list + commands footer) in `tui/src/render.rs`
- [X] T037 [P] {surface:cli} Implement TUI CLI args (clap derive) with `--socket=<path>`, `--no-color`, and `--version` in `tui/src/main.rs`

**Checkpoint**: `weaver run` listens on the socket; `weaver-tui` connects and renders an empty facts list with "ready" status; `weaver --version` produces the documented output. No behavior is registered yet, so no facts ever appear. User story implementation can now begin.

---

## Phase 3: User Story 1 - Trigger and propagate (Priority: P1) 🎯 MVP

**Goal**: A developer triggers `simulate-edit` from the TUI; the dirty-tracking behavior asserts `buffer/dirty`; the TUI renders it within 100 ms. Symmetric `simulate-clean` retracts the fact.

**Independent Test**: With both processes running, pressing `e` in the TUI shows `buffer/dirty(EntityRef(1)) = true` within 100 ms; pressing `c` removes it within 100 ms. Validates spec SC-001.

### Tests for User Story 1 (write FIRST, ensure FAIL before implementation)

- [X] T038 [P] [US1] Scenario test: `(empty fact-space, [Event::BufferEdited(EntityRef(1))]) → asserts Fact(buffer/dirty=true) on EntityRef(1) with non-empty Provenance` in `core/tests/behavior/dirty_tracking_assert.rs`
- [X] T039 [P] [US1] {retraction} Scenario test: `(fact-space with buffer/dirty asserted, [Event::BufferCleaned(EntityRef(1))]) → retracts buffer/dirty on EntityRef(1)` in `core/tests/behavior/dirty_tracking_retract.rs`
- [X] T040 [P] [US1] Scenario test: `(empty fact-space, [BufferEdited, BufferCleaned, BufferEdited]) → final state has buffer/dirty asserted (assert/retract/assert sequence)` in `core/tests/behavior/dirty_tracking_sequence.rs`
- [X] T041 [P] [US1] Property test: for any sequence of BufferEdited/BufferCleaned events, the final state of `buffer/dirty` matches the parity of the last event in `core/tests/property/dirty_tracking_invariant.rs`
- [X] T074 [P] [US1] Scenario test: register a fixture behavior that raises an error during firing; publish an event matching its trigger; assert `TraceEntry::BehaviorFired { error: Some(_) }` is recorded; assert fact-space is unchanged; assert dispatcher processes the next matching event normally per spec FR-011 + L2 P3 in `core/tests/behavior/error_recovery.rs`

### Implementation for User Story 1

- [X] T042 [US1] Define dirty-tracking behavior: registers for `buffer/edited` and `buffer/cleaned` event names; on edited asserts `buffer/dirty=true`; on cleaned retracts `buffer/dirty`; behavior_id `core/dirty-tracking` in `core/src/behavior/dirty_tracking.rs`. Also tightens dispatcher commit atomicity — when a behavior returns `error: Some(_)` its assertions/retractions are rolled back before the `BehaviorFired` trace entry is recorded.
- [X] T043 [US1] Wire dirty-tracking behavior registration into core startup in `core/src/cli/mod.rs::run_core`
- [X] T044 [P] [US1] {surface:cli} {latency:interactive} Implement `weaver simulate-edit <buffer-id>` CLI subcommand: connects to bus, publishes `Event::BufferEdited(EntityRef(buffer-id))`, returns submission ack in human or JSON form in `core/src/cli/simulate.rs`. Introduces shared `core/src/bus/client.rs` helper (Hello handshake + send/recv/subscribe) used by both the CLI and the TUI.
- [X] T045 [P] [US1] {surface:cli} {retraction} {latency:interactive} Implement `weaver simulate-clean <buffer-id>` CLI subcommand: connects to bus, publishes `Event::BufferCleaned`, returns submission ack in `core/src/cli/simulate.rs`
- [X] T046 [P] [US1] Implement TUI `e` and `c` keystroke handlers that publish events via the bus client in `tui/src/commands.rs`
- [X] T047 [P] [US1] Implement TUI fact rendering: crossterm raw-mode event loop, subscribe to `buffer/` family (via the shared core client), render asserted facts with `by behavior X, event Y, Δms ago` annotation in `tui/src/render.rs`. Listener (`core/src/bus/listener.rs`) rewritten to multiplex `read_message` and subscription fan-out via `tokio::select!`, forwarding `FactAssert`/`FactRetract` to subscribers.
- [X] T071 [P] [US1] Implement TUI disconnect detection: background reader task forwards inbound frames to an mpsc; read-error or stream EOF emits a `Disconnect` signal to the render layer in `tui/src/client.rs`. No heartbeat needed — Unix-socket EOF on core exit surfaces within milliseconds, well inside the SC-004 5 s budget.
- [X] T072 [P] [US1] Implement TUI stale-fact rendering: on `Disconnect` signal, mark all subscribed facts as stale and switch the connection-status line to `UNAVAILABLE` per `contracts/cli-surfaces.md` in `tui/src/render.rs`
- [X] T048 [US1] {latency:interactive} End-to-end test: spawn `weaver run` as subprocess, connect a test client over the socket, publish BufferEdited, assert FactAssert arrives within 100 ms with correct provenance; then publish BufferCleaned, assert FactRetract arrives in `tests/e2e/hello_fact.rs`. The workspace-level `tests/` crate is now a proper cargo member (`weaver-e2e`) with explicit `[[test]]` entries so the `e2e/` layout matches the plan.
- [X] T073 [US1] End-to-end test: spawn `weaver run` as subprocess, connect a test client, publish BufferEdited, assert FactAssert arrives, then SIGKILL the core process; assert client emits `Disconnect` signal within 5 s, no panic, exit cleanly per spec SC-004 in `tests/e2e/disconnect.rs`

**Checkpoint**: User Story 1 is fully functional and testable independently. The TUI loop closes through the bus and the fact space (no local shortcuts). Spec SC-001 and SC-005 (happy path + retraction coverage) are met.

---

## Phase 4: User Story 2 - Provenance inspection (Priority: P2)

**Goal**: A developer inspects any displayed fact and receives the source event, asserting behavior, and timestamp via a bus request/response (FR-008). The CLI's `inspect` subcommand is a thin wrapper over the same bus call.

**Independent Test**: With a `buffer/dirty` fact asserted, pressing `i` in the TUI returns `core/dirty-tracking, event N, Δns ago`. Same fact-key via `weaver inspect 1:buffer/dirty --output=json` returns the same fields. Validates spec SC-002.

### Tests for User Story 2 (write FIRST)

- [X] T049 [P] [US2] Scenario test: `(fact-space with buffer/dirty asserted via behavior firing, InspectRequest{1:buffer/dirty}) → InspectResponse{Ok(InspectionDetail{ source_event, asserting_behavior: "core/dirty-tracking", asserted_at_ns, trace_sequence })}` in `core/tests/inspect/inspection_found.rs`
- [X] T050 [P] [US2] Scenario test: `(empty fact-space, InspectRequest{1:buffer/dirty}) → InspectResponse{Err(InspectionError::FactNotFound)}` in `core/tests/inspect/inspection_not_found.rs`
- [X] T051 [P] [US2] Property test: every InspectionDetail returned for a real fact has non-empty asserting_behavior and a trace_sequence ≥ 0 in `core/tests/property/inspection_invariant.rs`

### Implementation for User Story 2

- [X] T052 [US2] Implement bus inspection handler: receives `InspectRequest`, looks up fact via FactStore, walks reverse causal index in TraceStore, constructs `InspectionDetail` or returns `InspectionError::FactNotFound` in `core/src/inspect/handler.rs`. Returns `InspectionError::NoProvenance` defensively when a fact is asserted without a recorded `BehaviorFired` entry (unreachable in slice 001 — guard for future external producers).
- [X] T053 [US2] Wire inspection handler into bus listener (route InspectRequest messages to the handler; send InspectResponse back on the same connection) in `core/src/bus/listener.rs`
- [X] T054 [P] [US2] {surface:cli} Implement `weaver inspect <fact-key>` CLI subcommand: parses `<entity-id>:<attribute>`, connects to bus, issues InspectRequest, renders response in human or JSON form in `core/src/cli/inspect.rs`. Unit tests for the fact-key parser live in the same file.
- [X] T055 [P] [US2] Implement TUI `i` keystroke handler: issues InspectRequest for the first displayed fact, correlates the response by `request_id`, renders `source_event`/`asserting_behavior`/`asserted_at_ns`/`trace_sequence` below the facts list; shows `(waiting for response…)` between request and reply. Keystroke in `tui/src/commands.rs`; state + render in `tui/src/render.rs`.

**Checkpoint**: User Story 2 works independently. Inspection is a bus-level capability — TUI, CLI, and any future agent service use the same request/response (forward-compatible per L2 P5 amended).

---

## Phase 5: User Story 3 - Structured machine output (Priority: P2)

**Goal**: A developer (or an LLM tool) runs `weaver status -o json` and receives valid JSON whose field names mirror the bus fact/event vocabulary. The CLI is the snapshot interface; continuous integration is the bus's job (per L2 P5 amended).

**Independent Test**: `weaver status -o json | jq .facts` returns an array; field names match `contracts/cli-surfaces.md`; deserializing the output back into typed Rust structures preserves identity. Validates spec SC-003.

### Tests for User Story 3 (write FIRST)

- [X] T056 [P] [US3] Test: `weaver status --output=json` (and `-o json`) produces JSON parseable by `serde_json::from_str::<StatusResponse>`; round-trip preserves all fields in `core/tests/cli/status_round_trip.rs`
- [X] T057 [P] [US3] Test: when core unavailable, `weaver status -o json` returns documented error JSON shape (`{ "lifecycle": "unavailable", "error": "..." }`) and exit code 2 in `core/tests/cli/status_unavailable.rs`
- [X] T058 [P] [US3] Test: `weaver status -o human` produces human-formatted output containing lifecycle state and fact count in `core/tests/cli/status_human.rs`

### Implementation for User Story 3

- [X] T059 [US3] {surface:cli} Implement `weaver status` subcommand in `core/src/cli/status.rs`. **Design deviation (documented in CHANGELOG):** instead of `Subscribe(AllFacts)` + snapshot drain, the slice adds a dedicated `BusMessage::StatusRequest` / `BusMessage::StatusResponse { lifecycle, uptime_ns, facts }` pair. Cleaner one-shot semantics, exposes uptime natively, avoids timeout-based snapshot termination.
- [X] T060 [US3] {surface:cli} Format dispatcher `render_status(response, format)` in `core/src/cli/output.rs` switches on `OutputFormat::{Human, Json}`.
- [X] T061 [US3] JSON formatter via `serde_json::to_string_pretty(StatusResponse)`. `StatusResponse` omits `uptime_ns`, `facts`, and `error` fields conditionally so the `ready` and `unavailable` shapes match the contract exactly. Unit tests in `cli::output::tests` verify round-trip and shape conditionals.
- [X] T062 [US3] Human formatter prints `lifecycle:`, `uptime:` (with unit suffix), `facts (N):` list with per-fact provenance summary.
- [X] T063 [US3] {surface:cli} `core/src/cli/errors.rs` — `WeaverCliError` enum (`CoreUnavailable`, `FactNotFound`, `ParseError`, `ProtocolError`) with `miette::Diagnostic` derive. JSON envelope exactly matches `cli-surfaces.md` (category, code, message, context, fact_key). Unit tests verify the envelope shape and that `fact_key` is populated for `FactNotFound`. Exit codes centralised in `errors::exit_code` (`OK=0`, `GENERAL=1`, `EXPECTED=2`).

**Checkpoint**: All three user stories are independently functional. The slice's spec acceptance criteria (SC-001 through SC-006) are met; spec FRs are covered.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, CHANGELOG completeness, CI scaffolding, final verification.

- [ ] T064 [P] Update `CHANGELOG.md` with the actually-shipped initial-version entries for each public surface from P7 (bus protocol, CLI, fact-family schema, configuration) — confirm entries match what was implemented
- [ ] T065 [P] Add `core/README.md` briefly describing the crate's role (core runtime: bus, fact space, behaviors, trace) and link to the slice spec
- [ ] T066 [P] Add `tui/README.md` briefly describing the TUI's role (subscribe, render, simulate commands) and link to the slice spec
- [ ] T067 [P] Add `.github/workflows/ci.yml` running `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --all-targets --workspace -- -D warnings`, and `cargo fmt --all -- --check` (per L2 P19 reproducibility + L2 P10 test-as-gate + L2 Amendment 6 code-quality gates). Equivalent to `scripts/ci.sh` but pinned to specific toolchain/cache configuration in GitHub Actions.
- [ ] T075 [P] Benchmark test: built `weaver --version` binary completes within 50 ms (warm-cache, debug profile) per spec SC-006; uses `std::time::Instant` around `Command::new("./target/debug/weaver").arg("--version").output()` in `core/tests/cli/version_timing.rs`
- [ ] T068 [P] Property test: every `BusMessage` published over the wire carries non-empty Provenance (P11 invariant) in `tests/property/provenance_wire.rs`
- [ ] T069 Run the `quickstart.md` walkthrough end-to-end manually; record any gaps as follow-up tasks in `CHANGELOG.md` notes
- [ ] T070 Verify `git log --oneline` for this branch shows commits in Conventional Commits style (per L2 Additional Constraints / Amendment 1)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately.
- **Foundational (Phase 2)**: Depends on Setup. BLOCKS all user stories.
- **User Stories (Phase 3+)**: All depend on Foundational. Within each:
  - US1 (P1) is the MVP — implement first.
  - US2 (P2) and US3 (P2) can proceed in parallel after US1 (or in priority order if single-developer).
- **Polish (Phase 6)**: Depends on user stories being complete enough to verify.

### User Story Dependencies

- **US1 (P1)**: independent of US2/US3.
- **US2 (P2)**: independent of US3; may rely on US1 having asserted facts to demonstrate against (but its tests can use synthetic fact-space states).
- **US3 (P2)**: independent of US1/US2; may rely on US1's facts existing to make `status` output non-trivial in manual testing.

### Within Each User Story

- Tests written FIRST and FAILING before implementation (per L2 P9 / P10).
- Pure helpers (predicates, formatters): classic TDD.
- Behaviors and host primitives: scenario tests express `(fact-space, event seq) → (deltas, intents)`.
- Property tests cover invariants that scenarios cannot enumerate.
- For tasks tagged `{retraction}`: ensure the retraction path is covered alongside the assertion path (P20).
- For tasks tagged `{surface:...}`: pair with a CHANGELOG entry (P8) — covered en bloc by T064.

### Parallel Opportunities

- All Phase 1 setup tasks marked [P] can run in parallel (T002, T003, T004 alongside T005-T010 once T001 is done).
- All domain-type tasks T011-T017 can run in parallel (different files, no inter-dependencies among the type definitions).
- Foundational invariant tests T018-T020 can run in parallel after their target types exist.
- Within each user story, all test tasks marked [P] can be authored in parallel before implementation begins.
- US2 and US3 phases can be worked on in parallel by different developers (or in two passes by one developer).

---

## Parallel Example: Phase 2 domain types

```bash
# After T001-T010 complete, the domain-type tasks fan out:
Task: T011 EntityRef in core/src/types/entity_ref.rs
Task: T012 Provenance in core/src/provenance.rs
Task: T013 Ids in core/src/types/ids.rs
Task: T014 Fact in core/src/types/fact.rs
Task: T015 Event in core/src/types/event.rs
Task: T016 BusMessage in core/src/types/message.rs
Task: T017 TraceEntry in core/src/trace/entry.rs

# All seven are independent; can land in any order or all at once.
```

## Parallel Example: User Story 1 tests

```bash
# After T037 (foundational) completes, US1 tests fan out:
Task: T038 dirty_tracking_assert.rs
Task: T039 dirty_tracking_retract.rs (with {retraction} marker)
Task: T040 dirty_tracking_sequence.rs
Task: T041 dirty_tracking_invariant.rs (proptest)

# All four authored in parallel; all FAIL before T042-T048 implementation.
```

---

## Implementation Strategy

### MVP First (User Story 1 only)

1. Complete Phase 1: Setup (10 tasks).
2. Complete Phase 2: Foundational (27 tasks; CRITICAL — blocks all stories).
3. Complete Phase 3: User Story 1 (11 tasks).
4. **STOP and VALIDATE**: `cargo test --workspace` passes; manual TUI validation per quickstart.md SC-001.
5. **MVP usable**. Demonstrate to reviewer.

### Incremental Delivery

1. Setup + Foundational → foundation ready (`weaver --version` works; bus listens; TUI connects).
2. + US1 → MVP. Demo. Stop here if scope-bound.
3. + US2 → inspection works. Demo.
4. + US3 → structured CLI output works. Demo.
5. + Polish → CHANGELOG, CI, READMEs. Done.

### Parallel Team Strategy

With multiple developers:

1. Whole team: Phase 1 + Phase 2 together.
2. Once Foundational done:
   - Developer A: US1 (MVP — highest priority)
   - Developer B: US2 (independent; tests stub the fact-space directly)
   - Developer C: US3 (independent; tests use the bus subscription model)
3. Stories integrate at the e2e test level (T048).

---

## Notes

- **Total tasks**: 75 (70 original + 5 added during `/speckit.analyze` remediation: T071–T075).
- **Per phase**: Setup 10, Foundational 27, US1 15 (11 + T071/T072/T073/T074), US2 7, US3 8, Polish 8 (7 + T075).
- **Parallel-marked**: ~45 tasks can run in parallel within their phase windows.
- **Test-first**: every implementation task in US1/US2/US3 has corresponding tests preceding it.
- **Retraction coverage** ({retraction} marker): T023, T039, T045 — assert, scenario test, CLI command.
- **Disconnect / degradation coverage** (FR-009/FR-010 + SC-004): T071 (client detect), T072 (render mark stale), T073 (e2e test).
- **Behavior error path** (FR-011 + L2 P3): T028 (impl) paired with T074 (scenario test).
- **Version timing** (SC-006): T032 (content) paired with T075 (timing benchmark).
- **Latency-class coverage** ({latency:interactive} marker): T044, T045, T048 — the hot path through bus + behavior + subscription.
- **Public-surface markers** ({surface:bus|cli|fact|config}): T027, T030, T031, T034, T037, T044, T045, T054, T059, T060, T063 — paired with CHANGELOG via T064.
- **No `{host-primitive}` tasks**: no Steel in this slice.
- **No `{schema-migration}` tasks**: initial schemas only.
- **Commit cadence**: one commit per logical task (or small bundle). All commits in Conventional Commits style per L2 Additional Constraints.
- **Stop checkpoints** at each phase boundary; validate before proceeding.
