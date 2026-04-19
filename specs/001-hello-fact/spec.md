# Feature Specification: Hello, fact

**Feature Branch**: `001-hello-fact`
**Created**: 2026-04-19
**Status**: Draft
**Input**: User description: "smallest end-to-end vertical slice exercising bus + fact-space + one embedded behavior + TUI subscription"

## User Scenarios & Testing *(mandatory)*

<!--
  These scenarios describe the developer experience of evaluating Weaver as
  an architecture. They are intentionally narrow — this slice's customer is
  a developer validating that the architectural commitments hold under real
  code, not an end-user editing files.
-->

### User Story 1 - Trigger a fact through Weaver and observe it propagate (Priority: P1)

A developer starts the Weaver core and a separate terminal interface (TUI). From the TUI, they trigger an action that simulates editing a synthetic buffer. The TUI's view of the buffer's "dirty" state changes — without the TUI itself having decided so. The change arrived through Weaver's bus and fact space.

**Why this priority**: This is the slice's reason to exist. Without this loop closing, every other architectural commitment is unverified.

**Independent Test**: Can be tested by starting both processes, triggering the simulated edit from the TUI, and observing the dirty-state indicator appear in the same TUI within the interactive latency budget.

**Acceptance Scenarios**:

1. **Given** the core is running and the TUI is connected with no active facts, **When** the developer invokes `simulate-edit` from the TUI, **Then** the TUI displays the affected buffer as dirty within 100 ms.
2. **Given** the buffer is currently displayed as dirty, **When** the developer invokes `simulate-clean` from the TUI, **Then** the TUI removes the dirty indicator within 100 ms (retraction path per L2 Principle 20).
3. **Given** the TUI is started before the core, **When** the core later becomes available, **Then** the TUI begins receiving facts without requiring a restart.

---

### User Story 2 - Inspect provenance of any displayed fact (Priority: P2)

The developer wants to confirm that the dirty state shown in the TUI was actually produced by the system's behavior chain — not by the TUI making it up. They invoke an inspection command and see which event triggered the behavior, which behavior asserted the fact, and a timestamp.

**Why this priority**: Provenance is constitutional (L1 §15, L2 P11). If it isn't observable in the smallest slice, the principle is decorative.

**Independent Test**: Can be tested by triggering an edit, then issuing the provenance-inspect command on the resulting fact, and verifying the response names a real event identifier and a real behavior identifier.

**Acceptance Scenarios**:

1. **Given** a `dirty` fact is currently asserted, **When** the developer queries its provenance, **Then** the response includes the source event, the asserting behavior's identifier, and a timestamp.
2. **Given** the version command is invoked from the CLI, **When** the developer reads the output, **Then** it includes the build's git commit identifier (with dirty-tree marker), build timestamp, and build profile.

---

### User Story 3 - Run the slice with structured machine output (Priority: P2)

The developer (or an LLM tool the developer is integrating) wants to consume the slice's status as structured data, not as human-formatted text. They pass an output-format flag and receive a parseable structured response that mirrors the bus vocabulary.

**Why this priority**: Structured machine output is the L2 P5 commitment. The smallest slice should exercise it on at least one CLI surface so the discipline is established before code accretes.

**Independent Test**: Can be tested by invoking the status command with `--output=json`, piping the output to a parser, and confirming the parsed structure includes the same fact and entity identifiers used over the bus.

**Acceptance Scenarios**:

1. **Given** the core is running with one or more facts asserted, **When** the developer runs `status --output=json`, **Then** the output is valid JSON whose field names mirror the bus's fact-message schema.

---

### Edge Cases

- The core is not running when the TUI starts → the TUI surfaces the unavailability as a visible state and retries; it does not crash or display stale fictional data.
- The core stops while the TUI is connected → the TUI marks subscribed facts as stale within 5 seconds and indicates the cause.
- Rapid repeated `simulate-edit` invocations → each produces a corresponding event; the dirty fact remains asserted; no event is silently dropped (events are lossy class but a small burst should not exceed the queue).
- A behavior raises an error during firing → the system records the failure in the trace; the fact space is unaffected; the lane remains responsive (per L2 P3).
- A `simulate-clean` is invoked while no `dirty` fact exists → the action is a no-op; an explanatory message indicates why.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST publish a `buffer/edited` event on the bus when a developer invokes `simulate-edit` from the TUI.
- **FR-002**: A registered behavior MUST react to `buffer/edited` events by asserting a fact `(buffer/dirty true)` on the affected buffer entity.
- **FR-003**: The TUI MUST subscribe to `buffer/dirty` facts and render them; the TUI MUST NOT compute dirtiness locally.
- **FR-004**: System MUST publish a `buffer/cleaned` event when a developer invokes `simulate-clean` and a behavior MUST react by retracting the `buffer/dirty` fact.
- **FR-005**: Every event and fact published on the bus MUST carry provenance metadata including source identifier, monotonic sequence or timestamp, and causal-parent reference where applicable.
- **FR-006**: The CLI MUST expose a version command whose output includes: crate version, git commit identifier with dirty-tree marker, build timestamp, build profile.
- **FR-007**: The CLI MUST expose a status command supporting an output-format flag (`--output=<format>`); `json` MUST be supported on Day 1; the output structure MUST mirror the bus fact/event vocabulary.
- **FR-008**: The system MUST expose an inspection capability allowing the developer to query, for any currently-asserted fact, the source event, the asserting behavior's identifier, and a timestamp.
- **FR-009**: When the core is unavailable, the TUI MUST display the unavailability as a visible state and MUST NOT display fictional fact values.
- **FR-010**: When the core becomes unavailable mid-session, the TUI MUST mark previously-subscribed facts as stale within 5 seconds.
- **FR-011**: A behavior firing error MUST be recorded in the trace; the fact-space MUST be unaffected; the lane MUST remain responsive to subsequent events.
- **FR-012**: Every change to behavior code that fixes a regression MUST be preceded by a failing test capturing the prior failure as a fact-space scenario (L2 P10).

### Key Entities

- **Buffer**: an opaque, addressable reference identifying a synthetic editable surface. No filesystem path, no content; serves only as the target of edit-related events and dirty-state facts.
- **Buffer-edited event**: a transient message indicating an edit occurred on a specified buffer.
- **Buffer-cleaned event**: a transient message indicating an explicit clean (analog to "save", without actual persistence).
- **Buffer-dirty fact**: an asserted fact `(buffer/dirty true)` carrying provenance back to the originating event and behavior.
- **Behavior firing**: a record in the trace of one behavior reacting to one event, including matched preconditions, asserted/retracted facts, and any error.

## Affected Public Surfaces *(mandatory)*

<!--
  Per L2 Constitution Principle 7, public surfaces have explicit evolution
  policies. This slice introduces the initial versions of several surfaces.
-->

### Fact Families & Authorities

- **Authority**: the **core** is authoritative over the `buffer/*` fact family in this slice (no separate buffer-management service yet).
- **Fact families touched**: `buffer/dirty` — *added* (initial schema). No other families touched.
- **Schema impact**: additive (initial creation; no prior schema to migrate from). See L2 Principle 15.

### Other Public Surfaces

- **Bus protocol**: initial version. Message categories `event`, `fact-assert`, `fact-retract` exercised. Both delivery classes from `docs/02-architecture.md` §3.1 are covered: `event` (lossy) and `fact-assert`/`fact-retract` (authoritative). The CBOR tag scheme registry is established at version 0 (entity-ref, keyword); other tags deferred to slices that need them.
- **Steel host primitive ABI**: not exercised in this slice (no Steel; embedded Rust behavior). Future slice scope.
- **Action-type identifiers**: none defined in this slice (no action entities; `simulate-edit` and `simulate-clean` are TUI-side commands that publish events directly).
- **CLI flags + structured output shape**: `weaver --version`, `weaver status --output=<format>`, `weaver simulate-edit <buffer-id>`, `weaver simulate-clean <buffer-id>` (or equivalent TUI-side equivalents), `weaver inspect <fact-ref>`. Initial version.
- **Configuration schema**: minimal — bus address (default to a well-known local socket or in-process channel) and log verbosity. Initial version.

### Failure Modes

- **Degradation taxonomy**: lifecycle states `started`, `ready`, `stopped` are exercised in this slice. `degraded`, `unavailable`, `restarting` defined in protocol but not exercised by failures within scope.
- **Failure facts**: a behavior firing error is recorded as a trace entry; no public fact family is asserted for the failure in this slice (deferred to later slices that need it).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A developer can trigger a `simulate-edit` from the TUI and see the resulting dirty indicator appear in the same TUI within 100 milliseconds (interactive latency class per `docs/02-architecture.md` §7.1).
- **SC-002**: For 100% of currently-asserted facts visible in the TUI, the developer can issue an inspection command and receive a non-empty provenance response naming the source event, asserting behavior, and timestamp.
- **SC-003**: The `status --output=json` command produces output that an external script can parse and extract the same entity identifiers used over the bus, with no schema reconciliation step required.
- **SC-004**: Killing the core process while the TUI is connected does not crash the TUI; the TUI displays the connection loss within 5 seconds and stops rendering fact values until reconnect.
- **SC-005**: The slice's automated test suite covers the happy path (edit → dirty → clean → retract), at least one failure mode (core unavailable), and one provenance assertion (fact carries non-empty source).
- **SC-006**: Running the slice's executable surfaces, `weaver --version` returns within 50 milliseconds and includes all required provenance fields.

## Assumptions

- **Synthetic buffer**: the buffer entity is purely synthetic; no actual filesystem buffer or content is involved in this slice. Real file-backed buffers are deferred.
- **Single-machine, single-process pair**: both `weaver` (core) and the TUI run on the same machine. Distribution is architecturally supported but not exercised here.
- **In-memory state only**: no persistence between runs. The trace, the fact space, and any subscriptions reset on core restart.
- **Embedded Rust behavior**: the dirty-tracking behavior is written in Rust and registered statically at core startup. Steel integration is deferred to a later slice.
- **TUI commands as event publishers**: `simulate-edit` and `simulate-clean` are TUI-side commands that publish events to the bus. They are not action-entity invocations (action entities are deferred to a later slice).
- **No reflective loop in this slice**: behaviors are static; reload semantics are deferred.
- **No `why?` channel walk-back yet**: the provenance inspection in FR-008 returns the immediate causal parent only — full walk-back is a later slice once the trace has more shape.
- **Interactive latency budget**: the 100 ms target in SC-001 is the interactive class per architecture §7.1. The slice does not yet measure or enforce this in CI; budgets are a P18 concern that becomes mandatory when host primitives accumulate.
