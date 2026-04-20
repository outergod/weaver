<!--
SYNC IMPACT REPORT — 2026-04-19

Version change: (template stub) → 0.1.0  [INITIAL RATIFICATION]
Bump rationale: First concrete content. No prior semantic version exists.

Modified principles:
  - All template placeholders ([PRINCIPLE_1_NAME] … [PRINCIPLE_5_NAME]) replaced with
    21 concrete L2 engineering principles (see Core Principles section).

Added sections:
  - Preamble (L1 / L2 relationship statement)
  - Core Principles (21 numbered principles)
  - Additional Constraints
  - Development Workflow
  - Governance

Removed sections:
  - None. Template placeholder section names ([SECTION_2_NAME], [SECTION_3_NAME])
    resolved to "Additional Constraints" and "Development Workflow".

Templates requiring updates:
  - ✅ .specify/templates/plan-template.md
        Constitution Check placeholder ("[Gates determined based on constitution
        file]") is already generic; /speckit.plan will populate gates from this
        constitution at plan time. No template edit required.
  - ✅ .specify/templates/spec-template.md
        Added "Affected Public Surfaces" section (Fact Families & Authorities,
        Other Public Surfaces, Failure Modes) per Development Workflow and
        Principles 5, 7, 8, 14, 15, 16. Mandatory if feature touches services,
        fact-space, Steel, or CLI.
  - ✅ .specify/templates/tasks-template.md
        Added "Weaver-Specific Task Categories" section with markers
        {retraction}, {host-primitive}, {schema-migration},
        {latency:immediate|interactive|async}, and {surface:...} per
        Principles 7, 8, 14, 15, 18, 20. Within-story rules updated to enforce
        retraction tasks, host-primitive review, and migration ordering.
  - ✅ .claude/skills/speckit-constitution/SKILL.md
        Generic; no agent-specific references requiring update.
  - ✅ docs/00-constitution.md (L1)
        Read-only authoritative source; cited from L2 preamble. No edit.

Runtime guidance docs:
  - README.md — no constitution references; no update required.
  - CLAUDE.md — currently a SpecKit stub; no changes required.

Follow-up TODOs:
  - None deferred. All placeholders resolved with concrete values.

----------------------------------------------------------------------
AMENDMENT 1 — 2026-04-19 — version 0.1.0 → 0.2.0

Added:
  - Additional Constraints: Conventional Commits 1.0 with hybrid scope
    vocabulary (public-surface names from Principle 7 when applicable;
    workspace/area names otherwise). Breaking public-surface changes
    require a `BREAKING CHANGE:` footer. Feeds Principle 8.

Templates / dependent files updated:
  - ✅ AGENTS.md — added "Commit conventions" section with examples.
  - ✅ .specify/extensions/git/git-config.yml — migrated all 16 hook
        messages to Conventional Commits style (`docs(specify): …` for
        documentation-artifact commits; `chore(specify): …` for progress
        checkpoints and housekeeping).

Bump rationale: MINOR — materially expanded guidance via a new binding
convention; not backward-incompatible (existing commits are not rewritten).

Out of scope (deferred):
  - commitlint or git-hook enforcement of the convention.
  - Historical commit-message rewriting.

----------------------------------------------------------------------
AMENDMENT 2 — 2026-04-19 — version 0.2.0 → 0.3.0

Origin: docs/mvp-review-hacker-triage.md Batch 1 (Vidvik review findings
DB2, DB3, UQ3).

Modified principles:
  - P3 (Defensive Host, Fault-Tolerant Guest) — strengthened with explicit
    requirement for host-enforced resource limits (CPU budget, recursion
    depth, per-firing fact-write quota) such that no Steel computation can
    hang the composition lane indefinitely. Cooperation by authors is
    preferred but not relied upon for safety. Closes the L2/L1 spirit gap
    identified in the Vidvik review (DB2).

L1 / architecture documents updated in lockstep:
  - ✅ docs/02-architecture.md §3.1 — restructured as Delivery Classes
        (lossy vs authoritative); §3.2 retains subscriber-level overrides.
        Resolves DB3 / SA2 / AC2.
  - ✅ docs/02-architecture.md §9.4 — host-enforced budget/cancellation
        added via new §9.4.1 (Resource Limits and Cooperative Cancellation).
        Aligns with strengthened P3.
  - ✅ docs/02-architecture.md §10.2 (new) — promoted snapshot-and-truncate
        retention from open question to architectural commitment; declared
        `why?` horizon as a system property. Resolves UQ3.
  - ✅ docs/05-protocols.md §1 — annotated message categories with default
        delivery class; cross-references architecture §3.1.
  - ✅ docs/07-open-questions.md §17 — marked RESOLVED with pointer to §10.2.
  - ✅ docs/07-open-questions.md §22 — marked RESOLVED with pointer to §3.1.

Bump rationale: MINOR — materially expanded guidance via strengthened P3
and three new architectural commitments. Not backward-incompatible since
no implementation exists to break.

Out of scope (deferred to later batches or sessions):
  - DB1 (undo) — Batch 2: milestone split (Ontology Prototype + Editor MVP).
  - SA4 (hunk-staging workflow) — Batch 3.
  - AC1 (core as semantic bottleneck) — re-evaluate after Batch 3.
  - AC3 (service-promotion ergonomics) — post-MVP engineering work.
  - UQ2 (cursor promotion API) — post-Editor-MVP.

----------------------------------------------------------------------
AMENDMENT 3 — 2026-04-19 — version 0.3.0 → 0.4.0

Origin: spec-review during /speckit.specify for the Hello-fact slice
(specs/001-hello-fact/spec.md FR-007 / FR-008 framing tension surfaced
the missing CLI-vs-bus boundary).

Modified principles:
  - P5 (Serialization and Open Standards) — added an explicit boundary
    between CLI structured output (one-shot scripting; snapshot interface)
    and continuous machine integration (bus-subscriber concern delivered
    via services). Clarifies that agents, monitoring, introspection
    clients, and language hosts MUST be designed as bus participants —
    not as CLI parsers — to inherit delivery-class guarantees,
    provenance, retraction, lifecycle signals, and reconnect semantics.

L1 / architecture documents updated in lockstep:
  - None this amendment. The boundary is a constitutional articulation
    of an architectural shape already implied by docs/02-architecture.md
    §3.1 (delivery classes) and the overall services-on-the-bus model;
    no architectural document required new commitments.

Bump rationale: MINOR — materially expanded guidance via a new
positive constraint about where continuous-integration consumers live.
Not backward-incompatible (no implementation exists; no consumer is
relying on the old framing).

Forward-looking implication:
  - A future "introspection / agent service" slice is anticipated. It
    will participate as a service on the bus, expose introspection
    requests/responses, and be the primary surface for continuous
    machine consumers. Hello-fact (specs/001-hello-fact) is being
    aligned now so its bus-level inspection capability (FR-008) is
    forward-compatible without rework.

----------------------------------------------------------------------
AMENDMENT 4 — 2026-04-20 — version 0.4.0 → 0.5.0

Origin: drift caught during /speckit.implement Phase 1. The scaffold's
Cargo.toml declared `license = "MIT OR Apache-2.0"` (Rust-ecosystem
default) while LICENSE clearly specifies AGPL-3.0-or-later. No
constitutional commitment existed to prevent this drift.

Added:
  - Additional Constraints: License clause. Weaver is AGPL-3.0-or-later;
    all workspace members MUST declare that license; inbound
    dependencies MUST carry AGPL-3.0-or-later-compatible licenses
    (MIT, Apache-2.0, BSD-*, MPL-2.0, ISC acceptable; GPL-2.0-only,
    proprietary-only, and other non-reciprocal-incompatible licenses
    are not). Licensing is treated as a compatibility surface per P7.

L1 / architecture documents updated in lockstep:
  - None this amendment. Licensing is orthogonal to architecture; it
    lives at the engineering-discipline layer (L2).

Bump rationale: MINOR — materially expanded guidance. Adds a new
binding rule about inbound-dependency license compatibility that
agents and reviewers MUST check on every new dependency. Not
backward-incompatible since no prior license commitment existed in L2
to override.

Forward-looking implication:
  - Phase 1 scaffolding (commit 9924d0d) used the wrong license
    declaration. The correction lands in a follow-up commit that updates
    Cargo.toml (workspace root) and amends CHANGELOG.md Phase 1 notes.
  - Future dependency additions and future member crates MUST honor
    this clause. Agent contributions in particular (per P21) MUST
    verify license alignment as part of scaffolding any new crate or
    proposing any new dependency.

----------------------------------------------------------------------
AMENDMENT 5 — 2026-04-20 — version 0.5.0 → 0.6.0

Origin: noticed during /speckit.implement Phase 2. Serde derive
produced snake_case variant-tag values (`fact_assert`, `buffer_edited`,
`fact_not_found`) while fact attribute strings are kebab-ish
(`buffer/dirty`). The inconsistency grates in a Lisp-heritage project
whose composition language is Scheme (Steel). Without a constitutional
commitment, agents default to Rust's snake_case convention and the
drift spreads.

Added to Additional Constraints:
  - Wire vocabulary naming: identifier VALUES on the bus and in CLI
    structured output follow Lisp/Scheme convention (kebab-case).
    Applies to event names, fact attributes, action types, behavior
    identifiers, message-kind discriminators, enum tag values,
    lifecycle states, error categories, subscription pattern names,
    output-format names. Behavior identifiers use `/` as namespace
    separator (e.g., `core/dirty-tracking`, not `core::dirty_tracking`).
    Struct FIELD NAMES inside structured messages follow the
    implementing language's idiom (snake_case in Rust, camelCase in
    JavaScript). Line: protocol-visible values are kebab-case;
    in-language field names are language-idiomatic.

L1 / architecture documents updated in lockstep:
  - None this amendment. Naming is orthogonal to architecture; lives at
    the engineering-discipline layer (L2).

Bump rationale: MINOR — materially expanded guidance. Adds a new
binding rule about wire-identifier casing that agents and reviewers
MUST apply to every new fact family, event, action, and behavior ID.
Not backward-incompatible since slice 001 is unshipped; the follow-up
commit updates the existing types to honor the convention before any
external consumer exists.

Forward-looking implication:
  - Phase 2 scaffold (commit f212bdc) used serde defaults (snake_case
    variant tags). Follow-up commit adds `#[serde(rename_all =
    "kebab-case")]` to BusMessage, SubscribePattern, EventPayload,
    InspectionError, SourceId, OutputFormat — plus updates the two
    contracts/ docs to reflect kebab-case wire-format examples.
  - Phase 3 behavior registration (T042) uses `core/dirty-tracking`
    as the behavior ID, not `core::dirty_tracking`.
  - Future agents authoring new fact families, actions, or behaviors
    MUST use kebab-case for all wire-visible names.
-->

# Weaver Constitution (Engineering — L2)

These engineering practices serve the architectural commitments in `docs/00-constitution.md` (L1).
L2 binds *how Weaver is built*; L1 binds *what Weaver is*.
L2 MUST NOT contradict L1; conflicts are resolved in L1's favor.
Every plan and spec MUST cite the L2 principles it touches in its Constitution Check.

## Core Principles

### 1. Domain Modeling Without Type Hierarchy

The domain model MUST NOT introduce type taxonomies. Capability emerges from fact-pattern matching, not from base-class extension. In Rust, prefer trait-bounded generics and small composable types over deep trait hierarchies.

**Rationale:** Open-ended editing systems cannot be enumerated as a closed type hierarchy. This is the engineering manifestation of L1 §3 (Entities Are Untyped) and L1 §4 (Facts Over Objects).

### 2. Purity at Edges, Transactional State at Core

Predicates, projections, and derivations MUST be pure functions of fact-space state. Mutating operations MUST be transactional and trace-logged. Behaviors SHOULD be deterministic given `(snapshot, event)`.

**Rationale:** Purity at the edges keeps reasoning local and composable; transactionality at the core keeps the fact space coherent under concurrent input.

### 3. Defensive Host, Fault-Tolerant Guest

Host primitives exposed to Steel MUST validate inputs at the boundary, MUST NOT panic on guest input, and MUST recover from behavior errors without corrupting fact-space or trace integrity. The host MUST enforce composition resource limits — per-firing CPU budget, recursion depth, and per-firing fact-write quota — such that no Steel computation can hang the composition lane indefinitely. The host MUST survive at minimum: Steel infinite loop, malformed fact assertion, bus timeout, mid-request service crash. Behavior authors SHOULD cooperate with these limits via host primitives that yield (continuations, async I/O); cooperation is preferred but not relied upon for safety.

**Rationale:** Steel is dynamically typed and user-editable; the Reflective Loop's promise of "safe to experiment" depends on a host that cannot be killed by guest mistakes *and* a composition lane that cannot be hung by them. Host survival without lane responsiveness is Emacs's old wound with better nouns. See `docs/02-architecture.md §9.4.1` (Resource Limits and Cooperative Cancellation) and `docs/04-composition-model.md §12` (Event-Loop Stability).

### 4. Simplicity in Implementation, Not in Architecture

No abstraction without a second concrete consumer. No speculative extension beyond L1 commitments. YAGNI applies to *code*, not to architectural commitments — the architecture is unavoidably sophisticated and MUST NOT be simplified by abandoning it.

**Rationale:** L1 §16 (Explainability Over Cleverness) prefers transparency at the user-facing edge; this principle prefers minimalism in the implementation that delivers it.

### 5. Serialization and Open Standards

Bus wire format (core ↔ services) is **CBOR** with a Weaver tag scheme (entity-ref, keyword, symbol, authority-qualified provenance). Tag numbers are a public surface per Principle 7 — they become part of the bus protocol version.

The outer shell (CLI) MUST support `--output=<format>` with **JSON** as the Day-1 minimum, exposed via a pluggable serde-style serializer. **TOON** is a v1.x roadmap aspiration, not a Day-1 gate. Output shape MUST mirror the bus fact/event/intent vocabulary. Tests MUST assert on deserialized structures, not on output strings.

**CLI structured output is for one-shot scripting and human-adjacent tooling.** It is a snapshot interface: it cannot represent streaming state, sequence guarantees, snapshot-plus-deltas reconnect, or causal continuity through the trace. It MUST NOT be treated as the primary integration surface for continuous machine consumers.

**Continuous machine integration — agents, monitoring tools, introspection clients, language hosts — is a bus-subscriber concern.** Such consumers participate as services (or service-like clients) on the bus, where they receive the full delivery-class guarantees of Principle 5 and Principle 7 and can walk the causal chain via the `why?` channel. New continuous-integration use cases MUST be designed as bus participants, not as CLI parsers.

S-expressions belong to Steel source and the REPL only — never on the bus.

Ecosystem standards (LSP, XDG base directories, OS-native file watching) are adopted where applicable. Weaver-original protocols are versioned per Principle 8.

**Rationale:** Serialization frontiers are independent. The fact tuple is the canonical semantic shape; serializers are per-frontier views. Pluggable serializers make format evolution a dependency change, not a rewrite. Conflating "structured CLI output" with "agent integration surface" is a category error: STDOUT is a snapshot, the bus is a stream. Designing agents around STDOUT JSON forecloses provenance, retraction, lifecycle signals, and reconnect — exactly the guarantees Weaver exists to provide.

### 6. Humane Shell

CLI surfaces use `clap` (derive macros). Errors use `miette` / `thiserror` and reference fact-space state and provenance, not just code-level state. Example: not `Error: missing path`, but `Action :save is unavailable on entity #b3a — no fact (entity:#b3a, attribute:path, …) is asserted by any authority.`

**Rationale:** L1 §2 (Everything Is Introspectable) extends to the shell. Errors that name the fact-space state surface the `why?` channel at the operator's terminal.

### 7. Public-Surface Enumeration

The project MUST maintain an explicit list of public surfaces, each with an evolution policy:

- **Bus protocol** (message shapes, including the CBOR tag scheme from Principle 5)
- **Steel host primitive ABI** (every function exposed to Scheme)
- **Fact-family schemas** (per service, per `docs/05-protocols.md §7`)
- **Action-type identifiers** — *stricter than SemVer*: changing an Action-type ID breaks historical `why?` traces, because Action entity IDs are derived from `(action-type, target)`
- **CLI flags + structured output shape**
- **Configuration schema**

**Rationale:** "Public" means different things to different consumers (services, agents, scripts, traces). Each surface needs its own compatibility regime.

### 8. SemVer + Keep a Changelog Per Surface

Each public surface from Principle 7 carries its own version number. That version MUST travel in provenance (e.g., bus messages carry the bus protocol version; facts carry the fact-family schema version). The whole-binary version is necessary but not sufficient. CI flags public-surface diffs that lack a changelog entry.

**Rationale:** A single binary version cannot express that the bus protocol is backward-compatible while a fact-family schema is breaking. Per-surface versioning lets agents and tooling reason about compatibility precisely.

### 9. Scenario + Property-Based Testing

- **Pure helpers** (predicates, projections, derivations): classic TDD red/green/refactor with unit tests.
- **Behaviors and host primitives**: scenario tests written test-first as `(initial fact-space, event sequence) → (expected fact deltas, emitted intents)`.
- **Fact-space invariants**: property-based tests (e.g., "every action with `:applicability-reason` fact is reachable from the leader-key tree").

The refactor pass remains a discipline regardless of test style.

**Rationale:** Pure TDD assumes deterministic input → deterministic output. Weaver's behaviors are reactive over fact-space; scenario tests express their semantics natively, and property tests guard invariants that scenarios cannot enumerate.

### 10. Regressions Captured as Scenario Tests Before Fix

Every bug fix MUST be preceded by a failing test that captures `(fact-space state, event sequence, expected behavior)`. "Function X returned wrong value" is insufficient — the test must reproduce the fact-space conditions that triggered the bug.

**Rationale:** Regression tests are living documentation of edge cases. In a fact-space system, the edge case is the fact-space configuration, not the function call.

### 11. Provenance Everywhere

`weaver --version` MUST emit: crate version, git SHA (with dirty bit), Steel version, bus protocol version, build timestamp, build profile. Every fact MUST carry authority + version. Every trace entry MUST carry behavior version.

**Rationale:** L1 §15 (Provenance Is Mandatory) at the architectural level; this principle binds the engineering manifestations — binary identity, fact metadata, trace metadata.

### 12. Determinism and Single-VM Concurrency Discipline

Behaviors MUST be deterministic given `(fact-space snapshot, event)`. Long-running pure work MUST yield via async continuations. There is no shared mutable state outside the fact space.

**Rationale:** `docs/02-architecture.md §9.4` commits to single-VM, single-threaded fact-space semantics. The engineering implication is that any non-determinism in a behavior is a bug, and any blocking work outside an async continuation jeopardizes the immediate latency class (Principle 18).

### 13. Observability for Operators

Structured logs and spans via the `tracing` crate (with OpenTelemetry export where applicable). Operator observability MUST integrate with the trace model — not duplicate it. Logs may reference trace entries by ID; trace entries MUST NOT depend on logs.

**Rationale:** The L1 trace model serves the user's `why?` channel. Operators (and CI) need a separate, parallel observability surface for performance, errors, and lifecycle. The two MUST be reconciled, not conflated.

### 14. Steel Sandbox Discipline

Every new Steel host primitive MUST ship with: a rationale, a threat model, and a resource budget (CPU per call, recursion depth limits, fact-assertion authority). New primitives are reviewed; they MUST NOT be merged by accretion.

**Rationale:** Each host primitive is an enlargement of the trusted surface between guest Scheme and the Rust core. Without explicit review, the sandbox erodes. See `docs/04-composition-model.md §7` (Composition Language: Steel).

### 15. Schema Evolution and Trace-Store Migration

Fact-family schemas are files under version control, versioned per Principles 7 and 8. Changes MUST be additive by default per `docs/05-protocols.md §7`. The trace store has a documented migration policy so historical traces remain readable across versions.

**Rationale:** A trace that cannot be read across versions is no trace at all. Schema evolution discipline preserves the long-term value of provenance.

### 16. Failure Modes Are Public Contract

Every service MUST document its degradation taxonomy, matching the lifecycle states in `docs/05-protocols.md §5` (`started`, `ready`, `degraded`, `unavailable`, `restarting`, `stopped`). Every action MUST document its failure facts.

**Rationale:** Graceful degradation (L1 §6) is observable only if the failure vocabulary is known. Undocumented failures become silent regressions.

### 17. Documentation in Lockstep with Implementation

Architectural changes require an Architecture Decision Record (ADR). Doc tests are used where feasible. CI enforces sync between `docs/00-constitution.md` (L1) and `.specify/memory/constitution.md` (L2): no contradictions; cited sections must exist.

**Rationale:** Weaver is documentation-first today. As code arrives, the documentation MUST remain authoritative. CI prevents silent drift.

### 18. Performance Budgets Per Latency Class

Every host primitive declares its latency class — *immediate* (≤16ms), *interactive* (≤100ms), *async* (unbounded) — per `docs/02-architecture.md §7.1`. CI tracks regression bounds where feasible. Breaches surface in traces.

**Rationale:** Latency classes are an architectural commitment. Without per-primitive budgets, the commitment is aspirational. With them, regressions are observable.

### 19. Reproducible Builds

`Cargo.lock` is committed. Steel version is pinned. Bus protocol version is pinned. Build info (per Principle 11) is embedded in every binary. Dev and prod use the same Steel and bus protocol versions.

**Rationale:** Reproducibility is itself a form of provenance. Without it, "this fact came from Weaver v0.3.1" is an unverifiable claim.

### 20. Retraction Is First-Class

Every fact-asserting code path MUST consider retraction. The PR template prompts for it. Tests exercise retraction paths, not just assertion paths.

**Rationale:** Mutable fact-space without disciplined retraction silently accumulates stale state. The introspection promise (L1 §2) erodes as stale facts pollute the `why?` channel.

### 21. AI Agent Conduct

AI contributions bind to this constitution: fact-style commits, doc updates as part of code changes, changelog entries on public-surface changes (Principles 7 and 8), regression tests before fixes (Principle 10). Agent-authored commits MUST be attributable in trace metadata.

**Rationale:** Agents are first-class contributors. The same engineering discipline applies; provenance (Principle 11) makes their work auditable.

## Additional Constraints

- **Composition language**: Steel only, per `docs/04-composition-model.md §7`.
- **Workspace members**: `core`, `ui`, `tui` (Rust crates) per the workspace `Cargo.toml`. Services MAY be polyglot but MUST speak the bus protocol.
- **Configuration**: follows XDG base directories with environment-variable overrides. Secrets MUST NOT live in the repository or in default configuration.
- **Serialization frontiers are independent**: fact tuples are the canonical semantic shape. Serializers (CBOR on the bus, JSON / TOON in the outer shell, native Steel values in-core) are per-frontier views. Steel ↔ wire conversion is defined once in the core so SDK consumers receive idiomatic language types.
- **Constitution sync**: `.specify/memory/constitution.md` (L2) and `docs/00-constitution.md` (L1) MUST stay in sync. CI enforces this per Principle 17.
- **Commit messages**: follow the [Conventional Commits 1.0](https://www.conventionalcommits.org/) specification — `<type>(<scope>): <description>`. Scope vocabulary is *hybrid*: use Principle 7 public-surface names (`bus`, `steel`, `fact`, `action`, `cli`, `config`) when the change touches a public surface; otherwise use workspace/area names (`core`, `ui`, `tui`, `docs`, `specify`). Conventional Commit types feed automated changelog generation and per-surface SemVer derivation under Principle 8. Breaking public-surface changes MUST include a `BREAKING CHANGE:` footer.
- **License**: Weaver is licensed under **AGPL-3.0-or-later** (see `LICENSE`). All workspace member `Cargo.toml` manifests MUST declare `license = "AGPL-3.0-or-later"` (or inherit it via `license.workspace = true`). Inbound dependencies MUST carry licenses compatible with AGPL-3.0-or-later — MIT, Apache-2.0, BSD-*, MPL-2.0, ISC are acceptable; GPL-2.0-only, proprietary-only, and other non-reciprocal-incompatible licenses are not. Dependency additions MUST be reviewed for license compatibility as part of per-PR review. Licensing is treated as a compatibility surface per Principle 7.
- **Wire vocabulary naming**: identifier *values* on the bus and in CLI structured output follow Lisp/Scheme convention — **kebab-case**. This applies to: event names, fact attributes, action types, behavior identifiers, message-kind discriminators, enum tag values, lifecycle states, error categories, subscription pattern names, output-format names. Behavior identifiers use `/` as the namespace separator (e.g., `core/dirty-tracking`, not `core::dirty_tracking`). Struct *field names* inside structured messages follow the implementing language's idiom (`snake_case` in Rust, `camelCase` in JavaScript clients). The line: protocol-visible identifier *values* are kebab-case; in-language *field names* are language-idiomatic. Per Principle 5, both bus (CBOR) and outer-shell (JSON) representations honor this.

## Development Workflow

- Plans MUST cite the L2 principles they touch in `plan-template.md`'s Constitution Check section.
- Specs MUST reference the fact families and authorities affected.
- Tasks MUST include "regression test added" for any bug fix (Principle 10).
- Public-surface changes MUST update the changelog and the relevant version (Principles 7, 8).
- New Steel host primitives MUST include rationale, threat model, and resource budget (Principle 14).

## Governance

- L1 (`docs/00-constitution.md`) supersedes L2 on architectural questions.
- L2 supersedes ad-hoc engineering preferences.
- Amendments are made via PR with rationale and migration notes.
- SemVer applies to L2 itself: MAJOR for backward-incompatible principle changes, MINOR for added principles, PATCH for clarifications.
- All PRs MUST verify compliance with relevant L2 principles. Violations MUST be justified in the plan's Complexity Tracking section.

**Version**: 0.6.0 | **Ratified**: 2026-04-19 | **Last Amended**: 2026-04-20
