# Feature Specification: Buffer Service (Slice 003)

**Feature Branch**: `003-buffer-service`
**Created**: 2026-04-23
**Status**: Draft
**Input**: User description: "Slice 003 — Buffer Service. Stand up the first content-backed service: a new binary `weaver-buffers` connects to the core as an `ActorIdentity::Service` and holds single-writer authority over the `buffer/*` fact family for a set of opened files."

## Clarifications

### Session 2026-04-23

- Q: What is the scalability policy for number of concurrently open buffers in a single `weaver-buffers` invocation? → A: No scalability commitment in slice 003. FR-007 proves N>1 only; quantitative limits land in a later slice when real editor use reveals the actual constraint.
- Q: What should the service do when its positional argument list contains two paths that canonicalize to the same buffer entity? → A: De-duplicate at CLI parse time; emit one `buffer/open` per unique canonical path. Log a debug message if duplicates were removed.
- Q: Should slice 003 commit now to the wire-level invariant that `BufferOpen` for an already-owned buffer is a no-op at the fact level, or defer to slice 004? → A: Lock now. `BufferOpen` is event-idempotent at the fact level; slice 004 layers focus semantics on top without changing the wire.
- Q: Is `watcher/status` a service-level signal orthogonal to per-buffer health, or does it aggregate per-buffer state? → A: Service-level, orthogonal. `watcher/status` tracks service liveness (once-Ready after all initial bootstraps; Degraded only when the service itself cannot serve). Per-buffer transient file trouble is reported exclusively via `buffer/observable`; `watcher/status=degraded` fires only if *all* currently-open buffers become unobservable (or the service itself is impaired).

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Observe a file's state through a buffer service (Priority: P1)

An operator runs `weaver-buffers <file>` alongside the running core. Within the interactive latency class, the TUI surfaces the file's current state — path, byte size, and dirty indicator. When the operator modifies the file externally, the TUI reflects the change without restart. When the operator shuts the service down, the facts retract cleanly and the TUI shows the buffer disappearing.

**Why this priority**: This is the slice's primary assertion. It stands up the first **content-backed service** on the bus — a service that conceptually owns a `:content` component (per `docs/01-system-model.md §2.4`) for each opened file, with every observable fact being a *derivation* of that content rather than the content itself. Every later slice in the dogfooding ladder (004 editing, 005 project entity, 006 agent skeleton, 007 agent tool use, 008 dogfooded loop) requires content to already live behind a service boundary. If this story fails, dogfooding cannot begin.

**Independent Test**: An operator starts the core, starts the buffer service against a freshly-prepared file, and observes `buffer/path`, `buffer/byte-size`, and `buffer/dirty` in the TUI within the interactive latency class. Mutating the file externally (a shell `echo >> file`) flips the dirty state; sending SIGTERM to the service retracts every `buffer/*` fact. Tested end-to-end with a three-process scenario (core + buffer service + TUI-or-equivalent client).

**Acceptance Scenarios**:

1. **Given** the core is running and no buffer service is attached, **When** the operator launches `weaver-buffers ./file.txt` against an existing readable file, **Then** the TUI begins surfacing current `buffer/path`, `buffer/byte-size`, and `buffer/dirty=false` facts for that buffer within the interactive latency class.
2. **Given** the buffer service is attached to a clean file, **When** the operator modifies the file outside Weaver (e.g., `echo new >> file.txt`), **Then** the TUI's reported `buffer/dirty` transitions from `false` to `true` within the interactive latency class.
3. **Given** the service has been publishing facts, **When** the operator sends SIGTERM to the service process, **Then** every `buffer/*` fact authored by that service retracts, the service's `watcher/status` transitions `unavailable` → `stopped`, and the TUI's Buffers section no longer shows the buffer.
4. **Given** the service is running at least one buffer and the file of one buffer is deleted mid-session, **When** the next observation tick fires, **Then** the service emits `buffer/observable=false` for the affected entity exactly once on the healthy→unobservable transition (edge-triggered, not per-tick); the TUI renders that buffer with an `[observability lost]` badge; other buffers are unaffected; `watcher/status` stays `ready` so long as at least one buffer remains observable.
5. **Given** the service disconnects uncleanly (SIGKILL), **When** the operator inspects the fact space, **Then** the core has retracted every fact the service's connection owned and the TUI presents the buffer as gone — never a stale last-known-state reading.

---

### User Story 2 — Authority over `buffer/dirty` has moved from behavior to service (Priority: P2)

An operator uses `weaver inspect` on any `buffer/dirty` fact and sees it attributed to the **buffer service** by structured identity — `service weaver-buffers (instance <UUID>)` — never to the slice-001 `core/dirty-tracking` behavior. The behavior is gone from the shipped core; the service is the sole authority for the fact.

**Why this priority**: This slice is the first time a fact family's authority migrates between actor kinds (behavior → service). L1 constitution §5 and architecture §5 require single-writer authority per canonical fact family; slice 001 shipped a *behavior-authored* `buffer/dirty` and slice 003 must cleanly transfer that ownership. The user-visible consequence is that the trace, inspection, and TUI all attribute the fact to the service — there is no lingering behavior variant, no provenance-drift surface, and no "which producer owns this?" ambiguity. If this story fails, slice 001's contract is violated and the coordination-substrate pivot leaks back to an embedded-behavior world.

**Independent Test**: Run the core in isolation (no buffer service attached); confirm no `core/dirty-tracking` behavior is registered and no `buffer/edited` / `buffer/cleaned` events can be accepted. Start the buffer service; confirm `weaver inspect <entity>:buffer/dirty` renders `asserting_service: "weaver-buffers"`, `asserting_instance: "<UUID>"`, and never `asserting_behavior`. An isolated scenario test constructs *both* a behavior-authored and a service-authored `buffer/dirty` on the same key and verifies inspect returns the live-fact's (service) provenance — the F23 live-fact-provenance invariant from slice 002.

**Acceptance Scenarios**:

1. **Given** the shipped core is running (no slice-001 behavior registered), **When** the operator runs `weaver simulate-edit 1`, **Then** the command does not exist (CLI surface removed) and no `buffer/edited` event is accepted on the bus.
2. **Given** the buffer service is attached and has published `buffer/dirty=false` for an opened file, **When** the operator runs `weaver inspect <entity>:buffer/dirty`, **Then** the output's authoring-actor line reads "service weaver-buffers (instance …)," the JSON form carries `asserting_service: "weaver-buffers"` + `asserting_instance: "<UUID>"`, and the `asserting_behavior` field is absent.
3. **Given** an isolated scenario test injects a behavior-authored `buffer/dirty=true` into the fact store and then has the buffer service assert `buffer/dirty=false` on the same key via the bus, **When** `weaver inspect` is called on the resulting fact, **Then** inspection attributes the live fact to the service — not to the stale behavior index entry — per the F23 live-fact-provenance contract.

---

### User Story 3 — Multi-buffer within one invocation (Priority: P3)

An operator runs `weaver-buffers ./a.txt ./b.txt ./c.txt` and sees three independent buffer entities in the TUI, each with its own path / size / dirty state. One operator-perceived process, one bus connection, N authority claims.

**Why this priority**: The editor shape envisioned by the dogfooding north-star (slice 008) requires a single service process to own many buffers simultaneously — any other arrangement forces an O(N) process-fan-out per opened file, which contradicts the slice-002-established single-service-per-role convention. Shipping N-in-one in slice 003 exercises the slice-002 invariant that **authority is per-`(family, entity)`, identity is conn-bound**: one connection carries one `ActorIdentity`, but can claim many entities under it. This is the slice-002 architectural commitment under a non-trivial load, proving the model scales to editor-grade use before slice 004 layers editing on top.

**Independent Test**: Launch the buffer service with three positional paths; confirm the TUI Buffers section renders three rows, one per file, each attributed to the same service instance. Confirm `weaver inspect` on any fact renders the shared instance id. Confirm an externally-mutated `b.txt` flips only that entity's dirty bit, not `a.txt`'s or `c.txt`'s. Confirm SIGTERM retracts all three buffers' facts together.

**Acceptance Scenarios**:

1. **Given** three valid, readable files, **When** the operator runs `weaver-buffers ./a.txt ./b.txt ./c.txt`, **Then** the TUI shows three distinct buffer rows within the interactive latency class, each with its own path / size / dirty.
2. **Given** three buffers open under one service, **When** the operator modifies only `./b.txt` externally, **Then** only `./b.txt`'s `buffer/dirty` transitions to `true`; the other two remain `false`.
3. **Given** the service is owning three buffers, **When** `weaver inspect` is called on any of the three `buffer/path` facts, **Then** all three facts share the same `asserting_service` ("weaver-buffers") and same `asserting_instance` UUID.

---

### Edge Cases

- **File path is a directory, symlink to a directory, or does not exist at startup**: service exits with a structured error and exit code 1 *before* asserting any facts. No partial-open state.
- **File is not readable by the invoking user (permission denied)**: same as above — fail-fast, exit code 1, structured error on stderr.
- **File is unreadable *mid-session* (permissions change, filesystem unmount, file deleted)**: service emits `buffer/observable=false` for the affected entity *exactly once* on the healthy→unobservable transition (edge-triggered per slice 002 F21). If this leaves the service with *zero* currently-observable buffers, the service additionally emits `watcher/status=degraded`; if at least one other buffer remains observable, `watcher/status` stays `ready`. Subsequent failed observations remain silent until recovery.
- **File is extremely large (multi-gigabyte)**: the service reads the file once at open time into memory. If reading exceeds available memory, the service fails with a structured error at startup (exit code 1). Slice 003 does not commit to a streaming-open path; this is an editor-grade size limit, tunable in a later slice.
- **Core restarts while the service is attached**: the service observes bus EOF and exits (per slice-002 FR-015 convention; auto-reconnect out of scope). Operator restart obtains a fresh instance UUID, which trivially satisfies the stale-identity prohibition.
- **Two `weaver-buffers` instances target the same path**: the second instance's first authority claim on the shared buffer entity fails; the second instance exits with code 3 *without* disrupting the first. No competing authoritative facts are ever asserted.
- **A buffer service also attempts to publish a non-`buffer/*` fact family**: core rejects with a wire-level authority error (a service's first-publish binds its identity per F14, but each subsequent `(family, entity)` claim is checked; a service claiming `repo/*` — already owned by a different git-watcher connection — is rejected with `authority-conflict`).
- **File is replaced atomically mid-session (e.g., editor save-via-rename)**: service's mutation observer detects the change (content differs byte-for-byte) and flips `buffer/dirty` to `true`. The buffer identity is keyed by *path* (not inode), so rename-over-same-path preserves the buffer entity.

## Requirements *(mandatory)*

### Functional Requirements

**Buffer service and content boundary:**

- **FR-001**: The system MUST provide a standalone binary that, when invoked with one or more path arguments identifying regular files, connects to the running core as a service actor with a structured identity (`ActorIdentity::Service { service_id: "weaver-buffers", instance_id: <UUID v4 per invocation> }`).
- **FR-002**: The service MUST publish authoritative facts for each opened buffer covering: canonical absolute path (`buffer/path`), current in-memory byte size (`buffer/byte-size`), dirty state (`buffer/dirty`), and observability (`buffer/observable`). Buffer state is a flat attribute set; no `buffer/state/*`-style discriminated union is introduced.
- **FR-002a**: Buffer content — the bytes of the opened file — MUST NEVER be emitted as a fact value, directly or indirectly. No preview, no digest, no range fetch, no stream subscription. Content lives in the service's in-memory byte store and on-disk only; only derivations of content (size, dirty flag) cross the bus. This commitment is constitutional (§2.4 Components vs Facts) and is the slice's defining invariant.
- **FR-002b**: `buffer/dirty true` MUST mean: in-memory bytes ≠ on-disk bytes at the last observation. Operationally equivalent to a byte-for-byte comparison between the service's memory and the file's current disk content. External mutation of the file is the only trigger that can flip dirty to `true` in this slice (no in-process mutation exists).
- **FR-003**: The service MUST re-assert or retract its facts in response to observed file state changes within the interactive latency class.
- **FR-004**: The service MUST announce its lifecycle (`started`, `ready`, `degraded`, `unavailable`, `stopped`) through the `watcher/status` fact family, keyed by the service's per-invocation instance entity. The vocabulary matches slice 002 exactly. `watcher/status` tracks **service-level** liveness and is **orthogonal** to per-buffer health (Clarification 2026-04-23):
  - `started` — once at process start, before any bootstrap.
  - `ready` — once, after every initial-bootstrap buffer has published its facts successfully. Never fires per-buffer.
  - `degraded` — only when the service itself cannot serve: inability to reach the bus, OR all currently-open buffers are simultaneously unobservable. Single-buffer transient failures are NOT reported here (see `buffer/observable`).
  - `unavailable` — once, at the start of shutdown.
  - `stopped` — once, at process exit.
- **FR-005**: The service MUST enforce single-writer authority per `(family, buffer-entity)` pair. A second `weaver-buffers` instance whose positional paths include any path whose entity is already claimed MUST fail its authority claim and exit non-zero, preserving single-writer authority (architecture §5).
- **FR-006**: The service MUST be invoked via command-line positional arguments naming one or more file paths; dynamic discovery, TUI-driven open, or service-side file picker are out of scope.
- **FR-006a**: Positional paths MUST be de-duplicated at CLI parse time after canonicalization (Clarification 2026-04-23). Two argv entries that canonicalize to the same absolute path produce exactly one `EventPayload::BufferOpen`; a debug-level log records the de-duplication. The observable effect is identical to the operator having passed the canonical path once.
- **FR-007**: The service MUST maintain exactly one bus connection for the lifetime of the process, regardless of how many buffers are open. All authority claims ride on that single connection — identity is conn-bound per slice 002 F14; authority is per-entity per F10. **No quantitative scale target is committed in slice 003** (Clarification 2026-04-23): the requirement proves the pattern works for N>1; concrete caps or tested-up-to claims are out of scope and land in a later slice.

**Entity identity:**

- **FR-008**: Each opened buffer MUST have a stable entity reference derived from the canonicalized absolute path of the file, so that opening the same path twice (from the same or different service invocation) refers to the same entity. The derivation MUST reserve bit 61 of the entity reference as the buffer-namespace marker — distinct from slice 002's bit 63 (repo) and bit 62 (watcher instance) — so trace inspection can distinguish buffer entities from repo and watcher-instance entities at a glance.

**Authority handoff (removal of slice-001 behavior):**

- **FR-009**: The slice-001 `core/dirty-tracking` behavior MUST be removed from the shipped core. `buffer/dirty` is service-authored only; no behavior-authored variant can be asserted under normal operation.
- **FR-010**: `EventPayload::BufferEdited` and `EventPayload::BufferCleaned` MUST be removed from the bus event vocabulary. No producer, no consumer. The wire variants cease to exist.
- **FR-011**: A new event variant `EventPayload::BufferOpen { path: String }` MUST exist on the wire. In slice 003, the only producer is the buffer service's own startup (translating positional CLI paths to internal open events); no external client emits this event. Future slices (004+) will add the TUI-side producer.
- **FR-011a**: Receiving `EventPayload::BufferOpen` whose canonicalized path corresponds to a buffer entity already owned by the running service instance MUST be a **no-op at the fact level** (Clarification 2026-04-23). No `buffer/*` facts are re-published, no content is re-read from disk, no bootstrap sequence is replayed. This invariant preserves event-level idempotence (constitution §3, events commute) and forecloses destructive silent-revert semantics when slices 004+ add editor- and agent-driven open producers. Reverting a buffer's in-memory content to on-disk content is an explicit, separate action introduced by a later slice, not a side effect of `BufferOpen`.
- **FR-012**: The `weaver` CLI subcommands `simulate-edit` and `simulate-clean` MUST be removed (the event variants they produced are gone).
- **FR-013**: A dedicated scenario test MUST construct both a behavior-authored and a service-authored `buffer/dirty` on the same key and verify `weaver inspect` attributes the fact to the service — exercising slice 002's F23 live-fact-provenance path as an isolated unit, in lieu of a runtime migration overlap.

**Observation and inspection:**

- **FR-014**: The TUI MUST subscribe to the `buffer/*` fact family and render a dedicated Buffers section below the existing Repositories section, one row per open buffer (path, byte size, dirty badge). Rendering rules mirror the slice 002 Repositories section: `by service weaver-buffers (inst <short-uuid>), event <id>, <t>s ago` line under each buffer; `[observability lost]` badge when `buffer/observable=false`; `[stale]` marker when the TUI loses its core subscription.
- **FR-015**: `weaver inspect` on any `buffer/*` fact MUST render the authoring actor as `service weaver-buffers (instance <UUID>)` in both human and JSON forms, without shape changes to the existing slice-002 inspection surface.

**Failure and degradation:**

- **FR-016**: When a specific buffer's file becomes unreadable mid-session (permissions change, file deleted, filesystem unmount), the service MUST publish `buffer/observable=false` for that buffer entity exactly once on the healthy→unobservable transition (edge-triggered, not per-tick). Recovery (file becomes readable again) MUST publish `buffer/observable=true` for that entity on the first successful observation. Per-buffer transient failures do NOT flip `watcher/status` by themselves.
- **FR-016a**: Service-level `watcher/status=degraded` MUST fire only when the *service itself* cannot serve — concretely: the bus becomes unreachable (and the service has not yet exited), OR every currently-open buffer's `buffer/observable` is simultaneously `false`. Recovery (bus regained, or at least one buffer becomes observable again) re-publishes `watcher/status=ready`. Transitions are edge-triggered per slice 002 F21.
- **FR-017**: When the service cannot open a requested path at startup (file does not exist, is a directory, is unreadable, exceeds available memory), it MUST exit with code 1 after emitting a structured error diagnostic. No `buffer/*` fact MUST be asserted for a failed open.
- **FR-018**: When the bus is unavailable at startup (socket missing, handshake failed), the service MUST exit with code 2 without emitting any facts.
- **FR-019**: When an authority claim is rejected (duplicate path against a prior instance), the service MUST exit with code 3 within 1 second of the rejection; any facts it successfully asserted before the rejection MUST be retracted server-side via the core's `release_connection` path.
- **FR-020**: On clean shutdown (SIGTERM / SIGINT), the service MUST retract every `buffer/*` fact it authored, publish `watcher/status` transitions (`unavailable` → `stopped`), close the bus connection, and exit with code 0.

**Known gaps (documented, not fixed this slice):**

- **FR-021**: The slice-002 open debt item "Events lack per-connection identity binding" MUST NOT be silently carried: the spec explicitly declares that `EventPayload::BufferOpen` is not subject to conn-bound identity enforcement (slice 002 F14 applies to `FactAssert` only). In slice 003 this is harmless — the only producer is the buffer service's own startup, which is trusted by construction. The gap becomes hazardous in slice 006 (agent skeleton) and MUST be closed before any agent can emit events. Recorded as forward-facing follow-up.
- **FR-022**: The slice-002 open debt item "First-to-claim `service_id` squatting" is equally unaddressed. A malicious service could connect before the real `weaver-buffers` and claim `service_id = "weaver-buffers"`. No trust root for service identities exists yet. Deferred to a capability-model slice.

### Key Entities

- **Buffer entity**: A file opened by the buffer service. Addressed by an `EntityRef` derived from the file's canonicalized absolute path, with bit 61 reserved to distinguish it from repo (bit 63) and watcher-instance (bit 62) namespaces. Opaque by construction (constitution §3); interpretation comes from its asserted facts and its implicit `:content` component.
- **`:content` component (conceptual)**: The bytes of the opened file, held in the buffer service's memory and mirrored on disk. *Not implemented as an in-code `Component` type this slice* — component infrastructure is deferred (`docs/07-open-questions.md §26`). The buffer service is the conceptual authority; every derived fact (`buffer/byte-size`, `buffer/dirty`) is a projection of this component. Slice 003 ships the authority and the derivations; a later slice ships the primitive.
- **Buffer service instance**: A running `weaver-buffers` process. Has an instance identifier — a random UUID v4 generated at process start — distinct across restarts, and distinct across any two concurrent instances even if they watch different files. Holds single-writer authority over the `buffer/*` fact family for every buffer it has opened. The service-side entity corresponding to this instance is derived from the UUID (mirroring slice 002's watcher-instance entity) and hosts the `watcher/status` lifecycle fact.
- **Buffer fact family** (`buffer/*`): The namespace of derived facts authored by the buffer service. Slice 003 covers `buffer/path` (String), `buffer/byte-size` (U64), `buffer/dirty` (Bool), `buffer/observable` (Bool). Content-bearing families (`buffer/preview`, `buffer/content-digest`, byte-range subscription) are explicitly *not introduced* in this slice and would require component infrastructure.
- **Watcher-status fact family** (`watcher/status`): Reused from slice 002 unchanged. Keyed by the buffer service's per-invocation instance entity, not by any buffer entity. Value mirrors `LifecycleSignal`.

## Affected Public Surfaces *(mandatory)*

### Fact Families & Authorities

- **Authority**: `weaver-buffers` (service) holds single-writer authority over the `buffer/*` fact family for each buffer entity it has opened. Per-entity, per-connection per slice 002 F10; never identity-keyed.
- **Fact families touched**:
  - `buffer/path`, `buffer/byte-size`, `buffer/dirty`, `buffer/observable` — **added**, authored by the buffer service.
  - `buffer/dirty` specifically: **authority transferred** from the slice-001 `core/dirty-tracking` behavior to the service. The behavior is removed from the shipped core. Wire shape of the fact is unchanged; authority origin shifts from `ActorIdentity::Behavior` to `ActorIdentity::Service`.
  - `watcher/status` — **read-only** (no change to authority or shape from slice 002; the buffer service reuses the fact family to publish its own lifecycle).
  - `repo/*` — **read-only** (no change; slice 002 semantics intact).
- **Schema impact**: **Breaking** at the bus-event-vocabulary level. `EventPayload::BufferEdited` / `BufferCleaned` are removed from the wire; a new `EventPayload::BufferOpen { path: String }` is added. The bus protocol bumps MAJOR `0x02 → 0x03`. All in-tree clients (core, TUI, git-watcher, buffer service, e2e harness) rebuild together. No provenance-shape change; no `FactValue` enum-variant removal (only additive: `U64` introduced).

### Other Public Surfaces

- **Bus protocol**: bumps from `0x02` to `0x03`. Enumerated changes:
  - `EventPayload::BufferEdited` removed.
  - `EventPayload::BufferCleaned` removed.
  - `EventPayload::BufferOpen { path: String }` added.
  - `FactValue::U64(u64)` added (additive; lands under the MAJOR bump).
  - `Hello.protocol_version` advances `0x02 → 0x03`; mismatched clients receive `Error { category: "version-mismatch" }` and connection close.
- **CBOR tag scheme**: no new tags required. Tag 1002 (`ActorIdentity`) reused unchanged.
- **Action-type identifiers**: not affected (no actions introduced this slice).
- **CLI flags + structured output shape**:
  - New binary `weaver-buffers`. CLI surface: `weaver-buffers <PATH>... [--socket=<path>] [--poll-interval=<duration>] [--output=human|json] [-v/-vv/-vvv] [--version]`. Positional paths variadic (one or more required). Exit codes: `0` clean, `1` startup failure, `2` bus unavailable, `3` authority conflict, `10` internal.
  - Existing `weaver` CLI: `simulate-edit <buffer-id>` and `simulate-clean <buffer-id>` **removed** (the events they emitted are gone). `weaver --version` JSON field `bus_protocol` advances `0.2.0 → 0.3.0`.
  - Existing `weaver-tui`: `--version` JSON field `bus_protocol` advances `0.2.0 → 0.3.0`. Render gains a **Buffers** section below the Repositories section; no new keybindings.
  - `weaver inspect`: shape unchanged; new fact families render under the existing structured-identity surface.
- **Configuration schema**: no changes. `weaver-buffers` respects the same `$XDG_CONFIG_HOME/weaver/config.toml` scaffold and the same `WEAVER_SOCKET` / `RUST_LOG` environment variables as `weaver` and `weaver-git-watcher`. `--poll-interval` is a CLI-only tuneable, not persisted.
- **Steel host primitive ABI**: not affected.

### Failure Modes *(mandatory)*

- **Degradation taxonomy**: Service-level lifecycle (`started` → `ready` ↔ `degraded` → `unavailable` → `stopped`; `restarting` reserved) tracks the service's own ability to serve, **orthogonal to per-buffer health** (Clarification 2026-04-23). `Ready` fires exactly once after all initial-bootstrap buffers publish their facts. `Degraded` applies only when the service cannot reach the bus OR every currently-open buffer is simultaneously unobservable; single-buffer transient file trouble with at least one other observable buffer remaining reports via `buffer/observable` only and leaves `watcher/status=ready`. `Unavailable` applies at the start of shutdown.
- **Failure facts**:
  - `watcher/status <lifecycle-state>` — published by the service on its own instance entity. Transitions are edge-triggered (slice 002 F21): `Degraded` fires once on the healthy→degraded boundary, not every failed poll.
  - `buffer/observable <bool>` — published per-buffer-entity. Asserted `false` when the buffer's file becomes unreadable; flipped back to `true` on the first successful recovery observation.
  - Structured error diagnostics on stderr when the service encounters an unrecoverable condition at startup (not a regular file, permission denied, etc.). Exit codes per FR-017 through FR-020.
- **Authority-conflict surfacing** (mirrors slice 002): when a second `weaver-buffers` instance's first `FactAssert` on `buffer/*` for a duplicate entity is rejected, the core emits `Error { category: "authority-conflict", context: "buffer/<path> already claimed by <first-instance-uuid>" }`; the second instance's reader loop classifies this as fatal and exits code 3. The first instance is unaffected. Slice-002 F31 follow-up item (reclassify `identity-drift` / `invalid-identity` as fatal) may be relevant during implementation but is **not in scope for this slice**.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-301**: From a cold start (core + TUI running, service not yet attached), an operator launches `weaver-buffers ./file.txt` and sees the file's state (path, byte count, clean indicator) reflected in the TUI within **one second** of the service's process start.
- **SC-302**: Following an external mutation of an open buffer's file (content appended, content replaced, file rewritten), the TUI reflects the new dirty state within an operator-perceived **500 ms** budget (interactive latency class per `docs/02-architecture.md §7.1`, accounting for polling overhead, matching slice-002 SC-002).
- **SC-303**: Following SIGKILL of the buffer service, every `buffer/*` fact authored by that service instance is retracted from the fact space within **5 seconds**, and the TUI's Buffers section shows no stale rows (matching slice-001 SC-004's disconnect budget).
- **SC-304**: Two `weaver-buffers` instances launched with overlapping positional paths converge on a state where exactly one instance continues to publish authoritative facts; the other exits with code 3 within **1 second** of its first failed authority claim.
- **SC-305**: An operator inspecting any `buffer/*` fact through `weaver inspect` receives an actor-identity rendering that names the service kind, service identifier `"weaver-buffers"`, and the per-invocation instance identifier, without any reference to an in-core behavior.
- **SC-306**: No fact value asserted by the buffer service across any trace this slice can produce carries buffer content as its value. Every `buffer/*` fact value is a path (string), a byte count (u64), or a boolean. This is asserted as a property test over randomly-generated observation sequences, not only as a scenario test.
- **SC-307**: The slice-001 end-to-end test skeleton (publish → observe → retract) continues to pass in its transformed form: the `hello_fact` and `disconnect` e2e suites no longer drive `simulate-edit` / `simulate-clean`, but instead drive `buffer/open` (via the service's startup) + external-mutation scenarios. Both suites green end-to-end with the new protocol version.

## Assumptions

*Commitments made when the feature description did not specify certain details; revisited in `/speckit.clarify` if any prove load-bearing.*

- **Polling is the observation mechanism for slice 003.** The service polls each open file's on-disk content against its in-memory byte store at a fixed cadence (default 250 ms, matching slice 002's `--poll-interval`). Filesystem-level watches (`inotify`, `kqueue`, `FSEvents`) are deferred to a later slice, consistent with slice-002 research §1.
- **Memory-vs-disk dirty check uses a content-digest approach** (hash in memory, hash on disk on each poll, compare). An mtime-plus-size heuristic is rejected because it can miss same-size edits within the mtime resolution; correctness of SC-302 requires the byte-equality reading.
- **One buffer service per invocation opens N files at startup.** The service CLI accepts one or more positional paths. Multi-buffer within one process is the committed shape; a one-process-per-buffer alternative is rejected because it contradicts the editor-shape the dogfooding ladder requires.
- **Path-based entity derivation.** The buffer entity is keyed by the file's canonicalized absolute path, with bit 61 reserved in the 64-bit entity reference. Inode-keyed derivation is rejected because it would fail save-via-rename (common in editors), where the inode changes but the path does not — the authority-claim invariant would then break on every save.
- **Fail-fast on unopenable paths at startup.** A path that does not exist, is a directory, is a symlink to a non-regular target, is unreadable, or exceeds available memory when read causes the service to exit with code 1 *before* asserting any facts. Keep-alive-with-`buffer/observable=false` is rejected for startup because it muddies the "did the operator typo the path?" signal into "is the file intermittently unavailable?" — a distinction the slice-004 dynamic-open story will need.
- **Bootstrap publication order.** Service lifecycle is separated from per-buffer bootstrap (Clarification 2026-04-23): the service publishes `watcher/status=started` once at process start → then, for each buffer, `buffer/path` → `buffer/byte-size` → `buffer/dirty=false` → `buffer/observable=true` (the four facts for one buffer share a per-buffer synthesized bootstrap-tick event id as `causal_parent`, matching slice 002's per-repo bootstrap pattern) → finally `watcher/status=ready` once after all buffers have bootstrapped. `why?` on any buffer fact walks to that buffer's bootstrap tick; `why?` on `watcher/status=ready` walks the service-level transition.
- **The `:content` component has no in-code `Component` type this slice.** Component infrastructure (a `Component` trait, `get-component` host primitive, update-in-place semantics) is deferred per `docs/07-open-questions.md §26`. The buffer service's internal byte store plus the on-disk file are the proto-component; a future infrastructure slice formalizes the boundary.
- **Service-id is `"weaver-buffers"`** (kebab-case per Amendment 5). Per-invocation UUID v4 carries instance identity, per slice 002 Clarification Q3. No in-band negotiation of service identity (slice-002 open debt #5); first-to-claim squatting remains an unguarded failure surface.
- **Four-process e2e testing.** The existing `tests/e2e/` scaffolding extends from three processes (core + git-watcher + test-client) to four (core + git-watcher + buffer-service + test-client) without a new test-harness primitive. Slice-003 e2e tests add fixtures for buffer files (temp-dir-managed) but reuse the `ChildGuard`-style process ownership pattern.
- **`EventPayload::BufferOpen` is sender-untrusted but consumer-trusted this slice.** Since the only producer is the buffer service's own startup (and the only consumer is the same service's internal initialization path), identity-forgery is a structural non-issue in slice 003. The gap becomes structural in slice 006 and MUST be closed before any agent can emit events — tracked under FR-021.
- **No slice-001/002 e2e scenario is silently dropped.** Every `hello_fact` / `disconnect` / `subscribe_snapshot` e2e scenario is transformed to the new `buffer/open` → external-mutation path, or explicitly retired with justification in the CHANGELOG. "Simulate-edit" and "simulate-clean" CLI invocations are the only retirements; their semantic analogs live in the new tests.
