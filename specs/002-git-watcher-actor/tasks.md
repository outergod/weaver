---

description: "Task list for Git-Watcher Actor (slice 002)"
---

# Tasks: Git-Watcher Actor

**Input**: Design documents from `/specs/002-git-watcher-actor/`
**Prerequisites**: plan.md ✓, spec.md ✓, research.md ✓, data-model.md ✓, contracts/ ✓

**Tests**: Tests are REQUIRED for this slice per L2 P9 (scenario + property-based) and L2 P10 (regressions as scenario tests). Each non-trivial implementation task has a corresponding test task that lands FIRST. Non-regression of slice 001's acceptance (SC-006) is enforced by keeping its existing tests green throughout the migration.

**Organization**: Tasks are grouped by user story (US1 P1, US2 P2 from `spec.md`) so each story can be implemented and validated independently.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Maps to user story in `spec.md` (US1, US2)
- File paths are repository-relative

## Weaver-Specific Task Categories

Markers apply in addition to `[P]` and `[Story]`:

- `{retraction}` — exercises a fact retraction path (P20). REQUIRED whenever a task asserts facts.
- `{schema-migration}` — touches a fact-family schema (P15). Breaking changes require explicit migration; this slice's bus protocol bump (0x01 → 0x02) is the relevant migration.
- `{latency:immediate|interactive|async}` — latency class per arch §7.1.
- `{surface:bus|fact|cli|config}` — touches a public surface (P7). CHANGELOG entry required per P8.
- (Skipped: `{host-primitive}` — no Steel this slice.)

## Path Conventions

- **Workspace root**: `Cargo.toml`, `CHANGELOG.md`.
- **Core crate**: `core/`.
- **TUI crate**: `tui/`.
- **Watcher crate (new)**: `git-watcher/`.
- **End-to-end tests**: `tests/e2e/` at workspace root.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Add the `git-watcher` crate to the workspace and wire in new dependencies. Make `cargo build --workspace` succeed with a stubbed `fn main()`.

- [X] T001 Update workspace `Cargo.toml` to add `git-watcher` as a new workspace member; add `uuid = "1"` (default-features = false, features = `["v4"]`) and `gix = "0.6x"` (pin current minor version per research §1) to `[workspace.dependencies]`; add `humantime = "2"` for `--poll-interval` parsing
- [X] T002 [P] Create `git-watcher/Cargo.toml` with `[package]` declaring `name = "weaver-git-watcher"`, `edition.workspace = true`, `license = "AGPL-3.0-or-later"`; declare `[[bin]]` target named `weaver-git-watcher`; depend on `core` (for shared types) + `tokio` + `clap` + `miette` + `thiserror` + `tracing` + `tracing-subscriber` + `ciborium` + `gix` + `uuid` + `humantime`
- [X] T003 [P] Create `git-watcher/build.rs` invoking `vergen` (shared workspace dep) to emit `VERGEN_GIT_SHA`, `VERGEN_GIT_DIRTY`, `VERGEN_BUILD_TIMESTAMP`, `VERGEN_CARGO_DEBUG` per L2 P11
- [X] T004 [P] Create `git-watcher/src/lib.rs` with empty module declarations (`pub mod model; pub mod observer; pub mod publisher; pub mod cli;`) so integration tests can import watcher types
- [X] T005 [P] Create `git-watcher/src/main.rs` with minimal `fn main() -> Result<(), miette::Report>` that calls `weaver_git_watcher::cli::run()`
- [X] T006 [P] Create `git-watcher/README.md` briefly describing role, usage, and a pointer to `specs/002-git-watcher-actor/`
- [X] T007 Verify `cargo build --workspace` succeeds after T001–T006; `cargo lint` and `cargo fmt-check` are clean

**Checkpoint**: The workspace compiles with a stub watcher binary that does nothing. Ready for Phase 2.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Replace `SourceId` with `ActorIdentity`, bump the bus protocol to `0x02`, extend `LifecycleSignal`, migrate all in-tree call sites and tests. This phase MUST complete before either user story can begin because both depend on the new provenance shape and handshake.

**⚠️ CRITICAL**: No US1 / US2 work can begin until this phase is complete.

### Type migration (core)

- [X] T008 [P] Replace `SourceId` in `core/src/provenance.rs` with a new `ActorIdentity` enum carrying variants `Core`, `Behavior(BehaviorId)`, `Tui`, `Service { service_id: String, instance_id: Uuid }`, `User(UserId)`, `Host { host_id: String, hosted_origin: HostedOrigin }`, `Agent { agent_id: String, on_behalf_of: Option<Box<ActorIdentity>> }` — see `data-model.md` **{surface:bus} {schema-migration}**
- [X] T009 [P] Define `UserId(String)`, `HostedOrigin { file, location, runtime_version }` helper types in `core/src/provenance.rs` alongside `ActorIdentity`
- [X] T010 [P] Update `Provenance` struct in `core/src/provenance.rs` to use `source: ActorIdentity` (replacing `source: SourceId`); preserve `timestamp_ns` and `causal_parent` fields unchanged
- [X] T011 Derive `serde::Serialize + Deserialize` with `#[serde(tag = "type", rename_all = "kebab-case")]` on `ActorIdentity` so CBOR + JSON produce the adjacent-tagged form documented in `contracts/bus-messages.md` **{surface:bus}**
- [X] T012 [P] [P] Validate constructor: `ActorIdentity::service(id: &str, instance: Uuid)` rejects empty or non-kebab-case `service_id` with a structured error (per Amendment 5); add in `core/src/provenance.rs`
- [X] T013 Wire the new CBOR tag 1002 for `ActorIdentity` in `core/src/bus/codec.rs` (or wherever the Weaver tag registry lives); see `contracts/bus-messages.md` **{surface:bus}**

### Bus protocol bump

- [X] T014 [P] Update `Hello.protocol_version` constant in `core/src/types/` from `0x01` to `0x02`; verify exactly one such constant exists in the codebase **{surface:bus}**
- [X] T015 Update handshake in `core/src/bus/listener.rs` to reject `Hello.protocol_version != 0x02` with `Error { category: "version-mismatch", detail: "bus protocol 0x02 required; received {n}" }` followed by connection close **{surface:bus}**

### LifecycleSignal extension

- [X] T016 [P] Extend `LifecycleSignal` enum in `core/src/types/lifecycle.rs` with three additional variants: `Degraded`, `Unavailable`, `Restarting` (kebab-case on the wire per Amendment 5); preserve existing `Started`, `Ready`, `Stopped` **{surface:bus}**
- [X] T017 [P] Update any exhaustive `match` on `LifecycleSignal` in core, TUI, and tests to handle the new variants (default: pass through / ignore; only the watcher will emit the new ones)

### Trace + inspection integration

- [X] T018 Update `core/src/trace/store.rs` so every `TraceEntry` carries `ActorIdentity` in provenance; reverse causal indexes unchanged in shape
- [X] T019 Update `core/src/inspect/` so `InspectionDetail` carries enough information to reconstruct the originating actor's identity: either `asserting_behavior: BehaviorId` OR `asserting_service: String` + `asserting_instance: Uuid` — not both. See `contracts/cli-surfaces.md` **{surface:cli}** — **deferred to the Phase 3 boundary**: load-bearing once the watcher publishes service-authored facts (Phase 3 T041/T055). Slice-001 behavior-authored facts continue to work with the existing `InspectionDetail` shape, so Phase 2 checkpoint is not blocked.

### Property tests for the migration

- [X] T020 [P] Property test in `core/tests/property/actor_identity_wire.rs`: round-trip every `ActorIdentity` variant through CBOR; assert equality on decode **{surface:bus}**
- [X] T021 [P] Property test in `core/tests/property/actor_identity_json.rs`: round-trip every `ActorIdentity` variant through `serde_json`; assert JSON field names are kebab-case (`service-id`, `instance-id`, `hosted-origin`)
- [X] T022 [P] Property test in `core/tests/property/provenance_wire.rs` (existing file): extend to cover `ActorIdentity::Service` variants alongside `Core` / `Behavior` / `Tui`

### Slice 001 non-regression

- [X] T023 Update slice 001 call sites in `core/src/behavior/dirty_tracking.rs` (and similar) that constructed `SourceId::Behavior(...)` to construct `ActorIdentity::Behavior(...)`; no behavioural change
- [X] T024 Update all test fixtures in `core/tests/` and `tests/e2e/` that reference `SourceId::*` to use the corresponding `ActorIdentity::*` variant; no semantic change
- [X] T025 Verify slice 001 tests still pass: `cargo test --workspace` — any regression here is a migration bug, not a new-feature failure
- [X] T026 Verify `weaver inspect 1:buffer/dirty` still returns a valid result against a running slice-001-style core (ad hoc smoke; no new test file needed)

### Version + changelog

- [X] T027 Update `weaver --version` output in `core/src/cli/version.rs` to emit `bus_protocol: "0.2.0"` (human + JSON forms); see `contracts/cli-surfaces.md` **{surface:cli}**
- [X] T028 Update `CHANGELOG.md` with entries: `## Bus protocol 0.2.0 - 2026-04-21` (provenance shape change; `ActorIdentity` replaces `SourceId::External`; new CBOR tag 1002; `LifecycleSignal` gains three variants); `## CLI surface - 2026-04-21` (`weaver inspect` JSON output adds `asserting_service`/`asserting_instance` for service-authored facts) **{surface:bus} {surface:cli}**

**Checkpoint**: Foundation ready. `cargo test --workspace` green. Bus protocol is v0.2. All slice-001 scenarios continue to pass under the new wire.

---

## Phase 3: User Story 1 - First non-editor actor observable end-to-end (Priority: P1) 🎯 MVP

**Goal**: Run `weaver-git-watcher /path/to/repo` alongside the core. Within the interactive latency class, the TUI surfaces the repository's dirty state, current branch (or detached/unborn), and head commit. External repository mutations propagate into the fact space and render in the TUI without restart.

**Independent Test**: Three-process scenario (core + watcher + TUI-or-test-client). Operator mutates the watched repo externally; TUI reflects the change within 500 ms. Watcher disconnect retracts all its facts.

### Watcher: data model and observation

- [X] T029 [P] [US1] Implement `WorkingCopyState` enum in `git-watcher/src/model.rs` with variants `OnBranch { name }`, `Detached { commit }`, `Unborn { intended_branch_name }`; derive common traits (`Debug`, `Clone`, `Eq`, `PartialEq`)
- [X] T030 [P] [US1] Implement conversion `TryFrom<gix::head::Head<'_>> for WorkingCopyState` in `git-watcher/src/model.rs`; map `gix::head::Kind::Symbolic` → `OnBranch`, `Kind::Detached` → `Detached`, `Kind::Unborn` → `Unborn`
- [X] T031 [US1] Unit tests in `git-watcher/tests/model.rs` covering all three `WorkingCopyState` variants from synthetic `gix` fixture repositories
- [X] T032 [P] [US1] Implement `git-watcher/src/observer.rs` with a `RepoObserver` struct owning a `gix::Repository` handle and exposing `fn observe(&self) -> Result<Observation, ObserverError>` returning `{ state: WorkingCopyState, dirty: bool, head_commit: Option<String> }` **{latency:interactive}**
- [X] T033 [US1] Dirty-check implementation in `RepoObserver::observe`: uses `gix::status` (or `gix::diff::index::*`) configured to match Clarification Q5 — working tree OR index differs from HEAD, untracked files EXCLUDED
- [X] T034 [US1] Head-commit resolution: `repo.rev_parse_single("HEAD")` → hex string; `None` for `Unborn` state
- [X] T035 [P] [US1] Scenario test in `git-watcher/tests/observer.rs`: build a fresh temp repo via `tempfile::TempDir` + `std::process::Command::new("git")` for setup; assert `observe()` returns expected `Observation` for each of: freshly-initialized (`Unborn`), committed-on-main (`OnBranch`), detached-HEAD, dirty-modified-tracked, dirty-staged-only, dirty-modified-and-staged, dirty-untracked-only (→ dirty=false per Q5)

### Watcher: CLI + startup

- [X] T036 [US1] Implement `git-watcher/src/cli.rs` with `clap` derive: positional `<REPOSITORY-PATH>`, `--poll-interval <duration>` (parsed via `humantime::parse_duration`, default `250ms`), `--socket <path>`, `--output <format>` (`human` | `json`), `-v/--verbose`, `--version` **{surface:cli}**
- [X] T037 [US1] `--version` output in `git-watcher/src/cli.rs` mirroring `weaver --version` plus `service_id: "git-watcher"` field; both human and JSON forms per `contracts/cli-surfaces.md`
- [X] T038 [US1] `cli::run()` in `git-watcher/src/cli.rs`: parse args; canonicalize repo path; fail with exit-code 1 + `WEAVER-GW-001` if path does not exist or is not a git repo; call into `publisher::run`

### Watcher: publisher (bus client + lifecycle)

- [X] T039 [US1] Implement `git-watcher/src/publisher.rs` with `Publisher` struct owning the Unix-socket stream, an `ActorIdentity::Service { service_id: "git-watcher", instance_id: Uuid::new_v4() }`, and the observer handle
- [X] T040 [US1] Handshake in `Publisher::connect`: send `Hello { protocol_version: 0x02, client_kind: "git-watcher" }`; await `Lifecycle(Started)` then `Lifecycle(Ready)`; exit-code 2 + `WEAVER-GW-002` on version mismatch or connection failure
- [X] T041 [US1] Bootstrap publication in `Publisher::publish_initial`: on `Ready`, assert `repo/path <canonical-path>`, `repo/head-commit <sha>` (if Some), the single `repo/state/*` variant matching `WorkingCopyState`, `repo/dirty <bool>`, `repo/observable true`, `watcher/status ready`. All with `Provenance.source = self.identity`, `causal_parent: None`. **{surface:fact} {schema-migration}**
- [X] T042 [US1] Poll loop in `Publisher::run`: wake every `poll_interval`; call `observer.observe()`; compare against last-known state; on change, publish diffs (see T043 for transition shape)
- [X] T043 [US1] State-transition publishing: when `WorkingCopyState` variant changes, emit exactly one `FactRetract` for the old variant followed by one `FactAssert` for the new variant, both carrying the same `causal_parent` event id (the poll-tick event). Document the two messages MUST be written to the stream before any intervening fact publishes **{retraction} {surface:fact}**
- [X] T044 [US1] Non-variant-change updates (same `repo/state/*` but value changed — e.g., branch head moved): single `FactAssert` replacing the prior value; `causal_parent: Some(poll-tick)`
- [X] T045 [US1] `repo/dirty` + `repo/head-commit` updates: `FactAssert` replacing the prior value on change; no retract needed (those attributes have a single-value-at-a-time semantic without discriminator)
- [X] T046 [US1] Degradation path: on `observer.observe()` error, emit `Lifecycle(Degraded)` + assert `repo/observable false`; retain prior `repo/state/*` fact (stale but observably so); re-attempt next poll. On recovery, assert `repo/observable true` + re-evaluate state via regular path **{retraction}**
- [X] T047 [US1] Shutdown handling: on SIGTERM/SIGINT or bus disconnect, retract every `repo/*` fact authored by this instance (tracked via a `HashSet<FactKey>` maintained alongside publishes); emit `Lifecycle(Unavailable)` then `Lifecycle(Stopped)`; exit 0 **{retraction}**

### Core: authority-conflict mechanism (FR-009)

- [X] T048 [US1] Add authority-claim tracking in `core/src/fact_space/` (or a new `core/src/authority.rs`): a `HashMap<FactFamilyClaim, ActorIdentity>` keyed by `(family_prefix: "repo/", entity: EntityRef)`. First `FactAssert` in a family for a given entity claims authority; subsequent asserts from a different `ActorIdentity` are rejected
- [X] T049 [US1] On rejected assert, core sends `Error { category: "authority-conflict", detail: "repo/{attribute} for entity {eref} already claimed by {existing-identity}" }` to the offending publisher's connection; does not close unless the publisher closes
- [X] T050 [US1] Authority-claim cleanup: when the publishing actor disconnects, its claims are released (claims are tied to connection lifetime for this slice; persistent claims deferred)

### TUI: Repositories section

- [X] T051 [P] [US1] Extend `tui/src/client.rs` subscription to include `FamilyPrefix("repo/")` and `FamilyPrefix("watcher/")` in addition to existing `"buffer/"`
- [ ] T052 [US1] Extend `tui/src/render.rs` with a `render_repositories` function producing the Repositories section per `contracts/cli-surfaces.md`. State badge logic: `[on <name>]` if `repo/state/on-branch` asserted; `[detached <short-sha>]` if `repo/state/detached`; `[unborn <name>]` if `repo/state/unborn`; `[state unknown]` otherwise; append `[observability lost]` if `repo/observable = false`; append `[stale]` when the TUI's subscription has dropped
- [ ] T053 [US1] Dirty-indicator rendering in `tui/src/render.rs`: `clean` / `dirty` next to the state badge; suppressed when `repo/observable = false`
- [ ] T054 [US1] Authoring-actor line in the TUI Repositories section: `by service {service_id} (inst {short-uuid}), event {id}, {elapsed}s ago`

### End-to-end tests (Phase 3)

- [X] T055 [P] [US1] E2E test in `tests/e2e/git_watcher_attach.rs`: spawn core; spawn watcher against a freshly-initialized temp repo with one commit; spawn a test client; assert the expected initial `repo/*` bootstrap fact set is observed within SC-001 (1s) window **{latency:interactive}**
- [X] T056 [P] [US1] E2E test in `tests/e2e/git_watcher_transitions.rs`: start watcher on a repo on `main`; from outside, `git checkout <sha>`; assert the test client observes `FactRetract(repo/state/on-branch)` and `FactAssert(repo/state/detached)` in sequence with a shared `causal_parent` event id. Repeat for the return path (checkout back to `main`) and for unborn→on-branch (initial commit on a fresh repo) **{retraction}**
- [X] T057 [P] [US1] E2E test in `tests/e2e/git_watcher_dirty.rs`: start watcher on a clean repo; modify a tracked file; assert `repo/dirty=true` observed within SC-002 (500ms). Stage the change; verify `repo/dirty=true` persists. Commit; verify `repo/dirty=false` observed within 500ms. Also verify untracked-only state stays at `repo/dirty=false` per Q5 semantics **{latency:interactive}**
- [X] T058 [P] [US1] E2E test in `tests/e2e/git_watcher_disconnect.rs`: start watcher; kill watcher process; assert the test client sees `Lifecycle(Unavailable)` + `FactRetract` for every prior `repo/*` fact; `weaver status --output=json` confirms no `repo/*` facts remain asserted **{retraction}**
- [ ] T059 [US1] E2E test in `tests/e2e/git_watcher_authority_conflict.rs`: start watcher A on repo R; start watcher B on same repo R; assert watcher B receives `Error { category: "authority-conflict", ... }` and exits with code 3 (`WEAVER-GW-003`)

### Scenario / property tests (Phase 3)

- [ ] T060 [P] [US1] Property test in `git-watcher/tests/mutex_invariant.rs`: for any trace prefix produced by the watcher's poll loop, at any repository entity, the count of asserted `repo/state/*` facts MUST be ≤ 1. Uses `proptest` to generate arbitrary sequences of `WorkingCopyState` transitions **{retraction}**
- [ ] T061 [P] [US1] Scenario test in `git-watcher/tests/transition_causal.rs`: for every synthetic transition, the retract and the subsequent assert carry the same `causal_parent` EventId

**Checkpoint**: US1 independently shippable. Three-process system works end-to-end. Watcher disconnect retracts cleanly. Authority conflict correctly rejects second instance.

---

## Phase 4: User Story 2 - Inspection renders structured actor identity (Priority: P2)

**Goal**: `weaver inspect <fact-key>` renders the authoring actor as structured identity — service-id + instance-id for service-authored facts, behavior-id for behavior-authored facts — never an opaque string.

**Independent Test**: Works without any external actor attached. Trigger an in-core behavior via `weaver simulate-edit 1`; `weaver inspect 1:buffer/dirty` must return a JSON object whose `asserting_behavior` names the behavior by identifier. Attach the watcher; `weaver inspect <eref>:repo/dirty` returns `asserting_service` + `asserting_instance` fields.

### JSON + human rendering

- [X] T062 [US2] Extend `core/src/inspect/render.rs`: `InspectionDetail` JSON serialization writes `asserting_behavior` for `ActorIdentity::Behavior(_)`, and `asserting_service` + `asserting_instance` for `ActorIdentity::Service { .. }`. Only one of these field groups appears per response. Per `contracts/cli-surfaces.md` **{surface:cli}**
- [X] T063 [P] [US2] Extend human rendering in `core/src/inspect/render.rs`: output labelled lines `source: service {service_id} (instance {uuid})` for services, `source: behavior {behavior_id}` for behaviors, `source: core` / `source: tui` for unit variants
- [ ] T064 [US2] Core-authored and TUI-authored fact rendering: for `ActorIdentity::Core` / `ActorIdentity::Tui`, JSON emits `{ "asserting_kind": "core" }` / `{ "asserting_kind": "tui" }` without identifier fields

### Scenario tests (Phase 4)

- [ ] T065 [P] [US2] Scenario test in `core/tests/inspect/behavior_authored.rs`: start core; trigger a `BufferEdited` event; run `weaver inspect 1:buffer/dirty --output=json`; assert output contains `asserting_behavior: "core/dirty-tracking"` and NO `asserting_service` field
- [ ] T066 [P] [US2] Scenario test in `tests/e2e/git_watcher_inspect.rs`: start core + watcher; run `weaver inspect <repo-eref>:repo/dirty --output=json`; assert output contains `asserting_service: "git-watcher"` and `asserting_instance` is a valid UUID v4 matching the watcher's startup log
- [ ] T067 [P] [US2] Scenario test in `core/tests/inspect/structured_always.rs`: across ALL fact families (`buffer/*`, `repo/*`, `watcher/*`), `weaver inspect` output NEVER renders an opaque or raw-string actor identity. Enforced at the JSON level (no field value matching `^External\(`)
- [ ] T067a [P] [US2] Scenario test in `core/tests/inspect/causal_walkback.rs`: construct a multi-hop causal chain (buffer-edited Event → core/dirty-tracking Behavior fires → FactAssert of `buffer/dirty`); invoke `why?` on the resulting fact and walk the chain back to its originating Event; assert every step (Event, Behavior firing, Fact assertion) renders structured `ActorIdentity` — never an opaque tag, never a missing actor. Extend with a service-authored step (a `repo/*` fact from a running `weaver-git-watcher`) to verify multi-kind chains preserve structured identity at every hop. Covers FR-013 explicitly (previously only implicitly tested via per-fact scenarios)

### Property test (Phase 4)

- [ ] T068 [P] [US2] Property test in `core/tests/property/inspect_identity.rs`: for any `ActorIdentity` value, round-trip through `weaver inspect`'s JSON emitter then re-parse; the actor identity can be unambiguously reconstructed from the emitted fields

**Checkpoint**: US2 independently shippable. Every inspection across any fact family renders structured actor identity.

---

## Phase 5: Polish & Cross-Cutting Concerns

**Purpose**: Changelog, documentation, quickstart validation.

- [ ] T069 [P] Update `CHANGELOG.md` (extend the Phase-2 entries) with fact-family schema entries: `repo/dirty 0.1.0`, `repo/head-commit 0.1.0`, `repo/state/on-branch 0.1.0`, `repo/state/detached 0.1.0`, `repo/state/unborn 0.1.0`, `repo/observable 0.1.0`, `repo/path 0.1.0`, `watcher/status 0.1.0` per L2 P8 **{surface:fact}**
- [ ] T070 [P] Ensure `weaver-git-watcher --version` and `weaver --version` and `weaver-tui --version` all report `bus_protocol: "0.2.0"` consistently
- [ ] T071 Update `git-watcher/README.md` with usage example matching `quickstart.md`
- [ ] T072 Run `cargo lint` + `cargo fmt-check` + `cargo test --workspace` + `scripts/ci.sh` — all green
- [ ] T073 Run the `quickstart.md` procedure manually end-to-end (the three-terminal walkthrough); confirm each SC-001..SC-006 criterion passes; take notes on any friction for future quickstart revisions
- [ ] T074 Grep the codebase to confirm zero remaining references to `SourceId::External(`; confirm no `.to_string()` path on any `ActorIdentity` produces an opaque tag in CLI / JSON / trace output

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies; starts immediately.
- **Foundational (Phase 2)**: Depends on Setup; **BLOCKS both user stories**. Phase 2 is the ActorIdentity migration — it cannot be skipped or parallelized with user-story work because both user stories consume the new provenance shape.
- **User Story 1 (Phase 3)**: Depends on Foundational. Can proceed in parallel with User Story 2 after Phase 2 completes (if team capacity allows), but most work is in US1.
- **User Story 2 (Phase 4)**: Depends on Foundational. Independent from US1 in implementation (different files: core's inspection renderer vs. git-watcher crate + TUI).
- **Polish (Phase 5)**: Depends on both user stories being complete.

### Within Each User Story

- Tests written and failing BEFORE implementation (L2 P10 discipline).
- For tasks that assert facts, a `{retraction}` counterpart MUST exist (Principle 20): T043 / T046 / T047 assert; T056 / T058 / T060 exercise retraction paths.
- For `{schema-migration}` tasks (T008, T041): bus protocol version bump in T014 + CHANGELOG entry in T028 MUST precede consumer updates.

### Parallel Opportunities

- **Phase 1**: T002–T006 run in parallel after T001 completes.
- **Phase 2 type migration**: T008, T009, T010 land in the same file (`core/src/provenance.rs`) and must be serialized; T011, T012 serialize after. T013 (CBOR codec) depends on T008. T016 (LifecycleSignal) is independent — runs parallel to the type migration.
- **Phase 2 test trio**: T020, T021, T022 are in different files and can run in parallel.
- **Phase 3 model layer**: T029, T030 land in `git-watcher/src/model.rs` (serialize); T032 (`observer.rs`) is independent — runs parallel.
- **Phase 3 publisher + TUI**: T039–T047 are in `git-watcher/src/publisher.rs` (serialize); T051–T054 are in `tui/src/` (different crate — parallel with watcher work).
- **Phase 3 e2e tests**: T055–T059 are in distinct files — all parallel.
- **Phase 4 scenarios + property**: T065, T066, T067, T067a, T068 are in distinct files — all parallel.

---

## Parallel Example: Phase 3 e2e tests

After Phase 3 implementation lands (through T054), all five e2e tests can run concurrently — they spawn independent process groups on ephemeral sockets:

```bash
# All four parallel:
Task: "E2E test in tests/e2e/git_watcher_attach.rs — bootstrap fact set within SC-001"
Task: "E2E test in tests/e2e/git_watcher_transitions.rs — state transitions with shared causal parent"
Task: "E2E test in tests/e2e/git_watcher_dirty.rs — dirty transitions within SC-002"
Task: "E2E test in tests/e2e/git_watcher_disconnect.rs — retraction on disconnect"
Task: "E2E test in tests/e2e/git_watcher_authority_conflict.rs — second-instance rejection"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1 (Setup) — workspace compiles with stubbed watcher.
2. Complete Phase 2 (Foundational) — ActorIdentity migration. **CRITICAL**: slice 001 tests remain green throughout.
3. Complete Phase 3 (User Story 1) — watcher publishes; TUI renders.
4. **STOP and VALIDATE**: run the `quickstart.md` three-terminal walkthrough; confirm SC-001 / SC-002 / SC-004 / SC-005.
5. Deploy/demo if ready. The pivot's code-level assertion is live at this point.

### Incremental Delivery

1. Setup + Foundational → `cargo test --workspace` green, slice 001 functionality intact.
2. Add US1 → watcher + TUI Repositories section → three-process system works.
3. Add US2 → inspection output distinguishes service from behavior cleanly.
4. Polish: CHANGELOG, README, quickstart validation.

### Slice 001 non-regression discipline

Treat SC-006 as a gate at the end of each phase, not just the end of the slice:

- After Phase 2 (T025): `cargo test --workspace` — all slice 001 tests pass under the new wire.
- After Phase 3: slice 001 tests still pass while the watcher runs.
- After Phase 4: `weaver inspect 1:buffer/dirty` returns the expected `asserting_behavior` form, unchanged from slice 001.
- After Phase 5: full `quickstart.md` walk + slice 001's quickstart walk both succeed.

If any slice 001 test regresses at any phase boundary, the migration is incomplete. Fix before proceeding.

---

## Notes

- `[P]` tasks = different files, no dependencies on incomplete tasks in the same phase.
- `[Story]` labels appear only in user-story phases (Phase 3: US1, Phase 4: US2). Setup, Foundational, and Polish tasks carry no story label.
- Weaver markers (`{retraction}`, `{schema-migration}`, `{latency:...}`, `{surface:...}`) are review notes that do not change execution semantics; they drive changelog discipline and PR review.
- The authority-conflict mechanism (T048–T050) is the Phase 3 decision committed here to close the plan's "exact mechanism TBD at /speckit.tasks" gap: tracked per `(family_prefix, entity)` pair, tied to connection lifetime. Alternatives (handshake-level claim, persistent claims across reconnect) remain open for future slices.
- The efficient observation via inotify/kqueue remains out of scope; polling satisfies SC-002 with headroom per research §2.
- Commit after each task or logical group; each task's file-path scope means conflicts are rare within a logical phase.
