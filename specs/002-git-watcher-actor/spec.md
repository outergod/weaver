# Feature Specification: Git-Watcher Actor (Slice 002)

**Feature Branch**: `002-git-watcher-actor`
**Created**: 2026-04-21
**Status**: Draft
**Input**: User description: "Slice 002 — Git-Watcher Actor (first non-editor actor; structured ActorId). Introduce a separate `weaver-git-watcher` process that publishes authoritative `repo/*` facts over the existing bus, and give provenance a structured actor identity replacing the opaque `SourceId::External(String)`."

## Clarifications

### Session 2026-04-21

- Q: Which shape does the structured `ActorIdentity` take on the wire and in code? → A: Single enum, one variant per kind, payload per variant (Option A). A closed, Rust-idiomatic taxonomy matching `docs/01-system-model.md §6`; future kinds are added as new variants under additive-evolution rules. Closes the *shape* sub-question of `docs/07-open-questions.md §25`.
- Q: How does the slice migrate off `SourceId::External(String)`? → A: Replace entirely (Option A). No parallel opaque-string support and no deprecation shim; project is pre-1.0 and all bus clients are in-tree. Breaking at the wire; protocol version bumps (L2 Principle 8). Closes the *migration* sub-question of `docs/07-open-questions.md §25`.
- Q: How is the watcher's instance identifier generated? → A: Random UUID v4 per invocation (Option B). 122 bits of randomness; opaque semantics by design (prevents code from depending on implicit temporal ordering); v7 rejected because `Provenance.timestamp_ns` already carries authoritative time on every message and baking ordering into instance IDs creates an invariant that breaks silently under clock skew.
- Q: How does the slice model working-copy state (branch / detached / unborn / etc.)? → A: Discriminated-union-by-naming under `repo/state/*` (Option A′). This slice lands `repo/state/on-branch <name>`, `repo/state/detached <commit>`, and `repo/state/unborn <intended-name>`. Transient operations (rebase/merge/cherry-pick/revert/bisect) deferred. Watcher enforces the mutex invariant (at most one `repo/state/*` asserted per repo at a time). `repo/branch` as a standalone attribute is dropped — its role is subsumed by `repo/state/on-branch`. Acknowledged stopgap: the right long-term home is components (`docs/01-system-model.md §2.4`). Deferral tracked as `docs/07-open-questions.md §26` (Discriminated-Union Facts), which records the revisit triggers and candidate long-term resolutions.
- Q: What does `repo/dirty true` mean, precisely? → A: Working tree OR index differs from HEAD; untracked files excluded (Option A). Equivalent to `git diff HEAD --quiet` returning non-zero. Matches editor-style "modified indicator" intuition and composes with a future `repo/untracked-*` family if needed.

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Observe a repository's state through a non-editor actor (Priority: P1)

An operator runs `weaver-git-watcher /path/to/repo` alongside the running core. Within the interactive latency class, the TUI surfaces the repository's current state (modification status, branch, head commit). When the operator changes the repository — stages a file, switches branches, commits — the TUI reflects the change without restart.

**Why this priority**: This is the primary assertion of the coordination-substrate pivot at the code level. Before this slice, every fact the TUI renders originates from the core itself or from an in-core behavior reacting to buffer events. After this slice, the TUI has observed at least one fact family published by an actor Weaver has never shipped before — a non-editor, out-of-process service. If this story fails, the pivot remains doc-only.

**Independent Test**: An operator starts the core, starts the watcher against a freshly-cloned repository, and observes repository state in the TUI within the interactive latency class. Mutating the repository externally (git CLI) updates the TUI state without operator intervention. Tested end-to-end with a three-process scenario (core + watcher + TUI-or-equivalent client).

**Acceptance Scenarios**:

1. **Given** the core is running and no watcher is attached, **When** the operator launches `weaver-git-watcher` against a clean repository currently on a named branch, **Then** the TUI begins surfacing current `repo/dirty`, `repo/head-commit`, and `repo/state/on-branch` facts for that repository within the interactive latency class.
2. **Given** the watcher is attached to a clean repository, **When** the operator modifies a tracked file outside Weaver, **Then** the TUI's reported dirty-state transitions from clean to dirty within the interactive latency class (per FR-006b: dirty-state reflects working-tree-or-index-differs-from-HEAD; a tracked-file modification crosses that threshold).
3. **Given** the watcher is attached and reporting `repo/state/on-branch`, **When** the operator switches to a different branch via external tooling, **Then** the TUI reflects the new branch name (via the updated `repo/state/on-branch` value) and the new head commit within the interactive latency class.
4. **Given** the watcher is attached and reporting `repo/state/on-branch`, **When** the operator checks out a commit directly (`git checkout <sha>`), **Then** `repo/state/on-branch` retracts and `repo/state/detached` asserts atomically with a shared causal parent, and the TUI reflects the transition.
5. **Given** the watcher has been attached and then disconnects (clean shutdown or crash), **When** the operator inspects the repository's facts, **Then** the system presents them as stale or retracted with visible provenance — the TUI does not falsely assert current state.

---

### User Story 2 — Trace any fact back to its originating actor by structured identity (Priority: P2)

An operator uses `weaver inspect <entity>` on any fact — produced by the core, by an in-core behavior, or by an external watcher — and sees the originating actor rendered as a structured, human-readable identity (actor kind + service identifier + instance identifier), not an opaque tag.

**Why this priority**: Constitution §17 (Multi-Actor Coherence) requires every fact, event, and action to record its originating actor. The current opaque `SourceId::External(String)` variant collapses every out-of-process producer into an unstructured string, breaking the invariant at the wire and inspection boundaries. This slice materializes the structured identity; it is the provenance-level counterpart to Story 1 and must hold whether any external watcher is attached or not.

**Independent Test**: Run the core in isolation (no external actor attached), trigger a buffer-edit event, and run `weaver inspect` on the resulting `buffer/*` fact. Confirm the output renders the originating actor as a structured identity rather than a raw string. No external process required for this test.

**Acceptance Scenarios**:

1. **Given** the core is running and the dirty-tracking behavior has fired, **When** the operator runs `weaver inspect` on the `buffer/dirty` fact, **Then** the output attributes the fact to the dirty-tracking behavior by structured identity (actor kind, identifier), not by raw string.
2. **Given** the watcher from Story 1 is attached and has published repository facts, **When** the operator runs `weaver inspect` on a `repo/*` fact, **Then** the output attributes the fact to the watcher by structured identity including the watcher's service identifier and instance identifier, and distinguishes it unambiguously from core- or behavior-authored facts.
3. **Given** the trace has recorded events, fact assertions, and causal parents involving multiple actors, **When** the operator requests a causal walk-back via `why?`, **Then** each step in the chain carries its actor as a structured identity, readable without needing to interpret an opaque string.

---

### Edge Cases

- **Repository becomes inaccessible after watcher attachment** (permissions change, filesystem unmount, repository deleted): watcher must publish a degraded-lifecycle signal and retract or mark stale the facts it authored, rather than reporting last-known state indefinitely.
- **Repository is not a git repository** when the watcher is pointed at it: watcher must fail fast at startup with a structured error on the bus, not silently publish empty state.
- **Repository transitions into an invalid on-disk state** mid-session (corrupt index, interrupted rebase): watcher must surface the condition as a lifecycle signal rather than publishing fabricated facts.
- **Core restarts while watcher is attached**: watcher must reconnect and re-announce itself; prior facts the watcher authored must be re-asserted on reconnect (or a prior-session snapshot resumed), and inspection must not attribute new facts to a stale instance identifier.
- **Operator runs two watcher instances against the same repository**: authority is single-writer per fact family (architecture §5); the second instance must be rejected or must fail to claim authority, never publish competing authoritative facts.
- **Repository is extremely large or produces high-frequency state changes**: watcher must not starve the bus or the TUI; updates must remain within the interactive latency class under a reasonable repository size, and the watcher must back off gracefully under extreme churn.

## Requirements *(mandatory)*

### Functional Requirements

**Structured actor identity:**

- **FR-001**: The system MUST represent every actor that originates a fact, event, or action by a structured identity carrying at minimum: an actor kind, a service identifier, and an instance identifier.
- **FR-002**: The system MUST replace the opaque `External(String)` representation of out-of-process actor identity with a structured form; any legacy opaque-string identity that cannot be expressed as structured form MUST be either rejected or surfaced as a well-defined fallback, never silently admitted as authoritative.
- **FR-003**: Provenance carried on every bus message MUST be expressible under the structured identity scheme; the bus wire contract MUST be updated accordingly and versioned.
- **FR-004**: The trace store and the inspection surface MUST be able to present actor identity in its structured form without loss.

**Git-watcher service:**

- **FR-005**: The system MUST provide a standalone binary that, when pointed at a single git repository, connects to the running core as a service actor with a structured identity.
- **FR-006**: The watcher MUST publish authoritative facts about the watched repository covering: modification status (`repo/dirty <bool>`), current head commit (`repo/head-commit <sha>`), and working-copy state under the `repo/state/*` family. Initial `repo/state/*` variants required: `repo/state/on-branch <branch-name>`, `repo/state/detached <commit-sha>`, and `repo/state/unborn <intended-branch-name>`. Transient operation states (rebase, merge, cherry-pick, revert, bisect) are out of scope for this slice. Hunks, stash, remote state, and index detail are out of scope.
- **FR-006b**: `repo/dirty true` MUST mean: working tree OR index differs from HEAD, excluding untracked files (per Clarification Q5). Operationally equivalent to `git diff HEAD --quiet` returning a non-zero exit code. Untracked files are explicitly not part of the dirty signal; a future fact family may surface them separately.
- **FR-006a**: The watcher MUST enforce the invariant that exactly one `repo/state/*` fact is asserted per watched repository at any time (single-variant mutex). State transitions MUST be published as an atomic retract-then-assert pair sharing a common causal parent.
- **FR-007**: The watcher MUST re-assert or retract its facts in response to repository state changes within the interactive latency class.
- **FR-008**: The watcher MUST announce its lifecycle (started, ready, degraded, unavailable, stopped) through the standard lifecycle channel so other bus participants can observe its availability.
- **FR-009**: The watcher MUST reject a second instance attempting to claim authority over the same repository's fact family, preserving single-writer authority (architecture §5).
- **FR-010**: The watcher MUST be invoked via command-line argument naming the repository path; dynamic discovery is out of scope.

**Observation and inspection:**

- **FR-011**: The TUI MUST subscribe to the new `repo/*` fact family and render the current dirty/branch/head-commit state for each watched repository alongside existing rendering.
- **FR-012**: The system's inspection surface (`weaver inspect` CLI) MUST render structured actor identity for any inspected fact in a form that distinguishes actor kind and identity components without exposing wire-level representation detail.
- **FR-013**: The causal walk-back (`why?`) MUST preserve structured actor identity along every step of the chain.

**Failure and degradation:**

- **FR-014**: When the watcher loses access to the repository or detects an invalid repository state, the system MUST surface the degradation as explicit lifecycle and error facts; it MUST NOT leave stale facts asserted without an observable indication of staleness.
- **FR-015**: When the core restarts with the watcher attached, the system MUST reconnect and re-establish the watcher's fact authority without attributing new facts to a stale instance identifier from a prior session.

### Key Entities

- **Actor identity**: The attribution on every fact, event, and action. Realized as a single closed-enum with one variant per actor kind (per Clarification Q1), each variant carrying its own payload: what *kind* of actor the producer is, and the identity components appropriate to that kind (service id + instance id for services; behavior id for in-core behaviors; user id for users; host id + hosted-origin for language hosts; agent id + optional delegation chain for external agents). Replaces an opaque string for non-core producers; refines rendering for core producers. See `docs/01-system-model.md §6` for the actor taxonomy this materializes.
- **Repository**: A git-managed working tree on the local filesystem, addressed by its root path. The unit of observation for the watcher. Each tracked repository has a stable entity reference in the fact space so subsequent git-related slices can attach further facts to the same repository without re-keying.
- **Repository fact family** (`repo/*`): The namespace of facts authored by the watcher. This slice covers dirty status (`repo/dirty`), head commit (`repo/head-commit`), and working-copy state as a discriminated-union-by-naming (`repo/state/on-branch | detached | unborn`). Transient operation states, hunks, stash, and remote state are extension points for later slices.
- **Working-copy state**: A discriminated-union concept represented across the `repo/state/*` family. Per Clarification Q4, exactly one variant is asserted per repo at any moment; the watcher enforces this invariant. Acknowledged as a stopgap for the constitutional component model (`docs/01-system-model.md §2.4`), which is the right long-term home and is deferred to a future architectural slice.
- **Watcher instance**: A running `weaver-git-watcher` process. Has an instance identifier — a random UUID v4 generated at process start (per Clarification Q3) — distinct across restarts and across any two watcher instances whether or not they target the same repository (where only one at a time may hold authority). The identifier is opaque by design: temporal ordering comes from `Provenance.timestamp_ns`, not from the identifier itself.

## Affected Public Surfaces *(mandatory)*

### Fact Families & Authorities

- **Authority**: `weaver-git-watcher` (service) holds single-writer authority over the `repo/*` fact family for the repository it is watching.
- **Fact families touched**:
  - `repo/dirty`, `repo/head-commit`, and the `repo/state/*` family (initial variants: `repo/state/on-branch`, `repo/state/detached`, `repo/state/unborn`) — **added**, authored by the watcher.
  - Existing `buffer/*` family — **read-only** (no change to authority or shape).
- **Schema impact**: **Breaking** at the provenance level. The wire representation of actor identity within provenance changes from an opaque tagged string to a structured record. Per Clarification Q2, the structured form is the only recognized wire shape in this slice — no parallel support for the legacy `External(String)` wire representation, no deprecation shim. The change requires a bus protocol version bump (L2 Principle 8). Legacy producers cannot connect without recompilation; all in-tree bus clients (core, TUI, hello-fact test client) are rebuilt together.

### Other Public Surfaces

- **Bus protocol**: Provenance shape on every `Event`, `FactAssert`, `FactRetract`, `InspectResponse`, and trace-carrying message is updated to carry structured actor identity. CBOR tag scheme gains a new tag for structured actor identity; version-bump of the bus protocol per L2 Principle 8.
- **Action-type identifiers**: Not affected (this slice does not introduce actions).
- **CLI flags + structured output shape**: `weaver inspect` JSON output shape adds actor identity structure; the human-readable shape also changes. Must remain backward-compatible in *field-additive* form per Principle 7 — no removal of existing fields, new fields added.
- **Configuration schema**: `weaver-git-watcher` introduces a minimal CLI surface (positional repository path). If a configuration file is introduced, new keys for watcher registration must be explicitly optional.
- **Steel host primitive ABI**: Not affected.

### Failure Modes *(mandatory)*

- **Degradation taxonomy**: The watcher publishes lifecycle transitions via the standard lifecycle messages (`started`, `ready`, `degraded`, `unavailable`, `restarting`, `stopped` — see `docs/05-protocols.md §5`). Degraded applies when the watcher remains connected but cannot observe the repository authoritatively (transient filesystem error, git command failure). Unavailable applies when the watcher has lost the repository entirely or is exiting.
- **Failure facts**:
  - `watcher/status <lifecycle-state>` — published by the watcher.
  - `repo/observable <bool>` — asserted `false` when the watcher enters degraded or unavailable states; retracted on recovery.
  - Structured error messages on the `error` channel when the watcher encounters an unrecoverable condition (not a git repo, permission denied, etc.).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: From a cold start (core running, watcher not yet attached), an operator launches the watcher and sees the watched repository's state reflected in the TUI within **one second** of the watcher's process start.
- **SC-002**: Following an external mutation of the watched repository (file edit, stage, branch switch, commit), the TUI reflects the new state within the interactive latency class defined in `docs/02-architecture.md §7.1` (≤100 ms from the architecture target; an end-to-end operator-perceived budget of **500 ms** is acceptable accounting for filesystem observation overhead).
- **SC-003**: An operator inspecting any fact in the system through the inspection surface receives an actor-identity rendering that lets them distinguish, without referring to source code or wire-level documentation, which actor kind produced the fact and which service/behavior/instance they originated from.
- **SC-004**: The operator can run a full three-process scenario (core, watcher, observer client) end-to-end in a single command or documented short procedure — no manual coordination between the processes beyond invoking them.
- **SC-005**: After the watcher disconnects, no fact authored by that watcher instance remains presented as current without an observable indication of staleness.
- **SC-006**: No existing acceptance scenario from slice 001 (`specs/001-hello-fact/`) regresses — buffer-edit → dirty-state propagation, inspection, status, and version output continue to pass their prior tests under the new structured-identity wire shape.

## Assumptions

- **Polling is acceptable for repository observation in this slice.** The watcher may poll `git` state at a fixed cadence chosen to stay within SC-002. Efficient observation via filesystem-level watches (`inotify`, `kqueue`, `FSEvents`) is explicitly deferred.
- **One repository per watcher invocation.** Multi-repository watching in a single process is out of scope.
- **Static registration only.** Each watcher instance is launched with a repository path on its command line. Dynamic discovery over the bus is deferred per `docs/07-open-questions.md §16`.
- **The structured actor identity is a single enum with variants per kind** (core, behavior, TUI, service, user, language host, external agent). Confirmed by Clarification Q1 (2026-04-21). `docs/07-open-questions.md §25` remains open for longer-term evolution (trait object, per-kind implementations) — this slice commits to the enum form.
- **`SourceId::External(String)` is replaced, not extended.** Confirmed by Clarification Q2 (2026-04-21). No parallel support and no deprecation shim. Breaking at the bus protocol level; paired with a version bump.
- **Git facts are published under repository entities keyed by canonicalized absolute path.** Subsequent slices may refine this keying; the current key scheme is a forward-compatible starting point, not a final commitment.
- **Scope of repository observation is strictly `repo/dirty`, `repo/head-commit`, and the `repo/state/*` discriminated-union (`on-branch` / `detached` / `unborn`).** Transient operation states (`rebasing` / `merging` / `cherry-pick` / `revert` / `bisect`), hunks, stash, remote state, submodules, and index detail are deferred.
- **Working-copy state is modeled as a discriminated-union-by-naming under `repo/state/*`** (Clarification Q4). Mutex invariant is watcher-enforced. The long-term architectural home for this modeling is components (`docs/01-system-model.md §2.4`); component infrastructure does not yet exist in code and is a separate future slice. Deferral tracked in `docs/07-open-questions.md §26`.
- **Three-process testing is tractable.** The existing `tests/e2e/hello_fact.rs` scaffolding (binary under test + child-process ownership via `ChildGuard`) extends to three processes; no new test-harness primitive is required.
- **Conflict surfacing is out of scope.** Two watchers pointed at the same repository produces authority rejection (FR-009) but not a user-visible conflict-review workflow — that is `docs/06-workflows.md §5` (Multi-Actor Contribution Review) territory, reserved for a slice that deliberately introduces competing actors.
