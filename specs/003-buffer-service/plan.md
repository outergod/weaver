# Implementation Plan: Buffer Service

**Branch**: `003-buffer-service` | **Date**: 2026-04-23 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/003-buffer-service/spec.md`

## Summary

A fourth Rust binary — `weaver-buffers` — joins the existing `weaver` (core), `weaver-tui` (TUI), and `weaver-git-watcher` (git-repo watcher) as the first **content-backed service** on the bus. It opens one or more files named at the CLI, holds a `:content` component (per `docs/01-system-model.md §2.4`) for each in its in-memory byte store, and publishes a small set of *derived* authoritative facts over the bus: `buffer/path`, `buffer/byte-size`, `buffer/dirty` (memory-vs-disk), and `buffer/observable`, plus its own service-level `watcher/status` lifecycle. Per the slice-003 clarifications, `watcher/status` is service-level and orthogonal to per-buffer health; `EventPayload::BufferOpen` is event-idempotent at the fact level; duplicate positional paths de-duplicate at parse time.

Alongside, the slice 001 `core/dirty-tracking` behavior is deleted; `EventPayload::BufferEdited` / `BufferCleaned` and the `weaver simulate-edit` / `simulate-clean` CLI subcommands leave the protocol. `FactValue` grows a `U64` variant (additive, lands under the bus-protocol MAJOR bump). The bus protocol advances `0x02 → 0x03`. The TUI gains a Buffers render section below the existing Repositories section.

This slice is the first code-level instantiation of a **component authority** — a service that conceptually owns a non-proposition-shaped component per L1 §2.4, even while component-primitive infrastructure itself is deferred. It is load-bearing for slice 004 (editing), 005 (project entity), 006 (agent skeleton), 007 (agent tool use), 008 (dogfooded loop): every later slice requires content to live behind a service boundary.

## Technical Context

**Language/Version**: Rust 2024 edition; resolver = "3" (workspace-level); toolchain pinned to 1.94.0 via `fenix` per `flake.nix` + `rust-toolchain.toml` (unchanged from slice 002).
**Primary Dependencies**: existing — `tokio`, `ciborium`, `serde` + `serde_json`, `clap` derive, `miette`, `thiserror`, `tracing` + `tracing-subscriber`, `proptest`, `vergen`, `crossterm`, `uuid` (v4), `humantime`; **new** — `sha2` (content digesting — see `research.md` §1), `tempfile` (dev-dep for e2e fixtures; already transitively via `proptest` — see `research.md` §2).
**Storage**: In-memory only (service's per-buffer byte store) + filesystem read (regular files opened at startup). No persistence change from slices 001/002; trace remains in-process.
**Testing**: `cargo test` (unit + scenario); `proptest` (CBOR round-trip for new `FactValue::U64` variant; SC-306 component-discipline property asserting no fact value carries buffer content); workspace-level e2e tests extend from three-process (slice 002) to four-process (core + git-watcher + buffer-service + test-client).
**Target Platform**: Linux + macOS desktop. Single machine. Bus over Unix-domain socket as in slices 001/002.
**Project Type**: Rust workspace; **adds one new member crate** `buffers/` (binary target: `weaver-buffers`). Modifies `core/` (event vocabulary, behavior removal, `FactValue::U64`, bus protocol version) and `tui/` (new render region + subscription). `git-watcher/` and `ui/` untouched.
**Performance Goals**: SC-301 — cold start (single buffer) to first TUI render ≤ 1 s; SC-302 — external mutation to TUI render ≤ 500 ms (interactive latency class per `docs/02-architecture.md §7.1`, accounting for polling overhead); SC-303 — SIGKILL to full retraction ≤ 5 s; SC-304 — authority-conflict-exit ≤ 1 s from first failed claim.
**Constraints**: One bus connection per invocation (FR-007); polling-based observation (default 250 ms, matching slice 002); single-writer authority per buffer entity (FR-005); content never on the wire (FR-002a, constitutional); `EventPayload::BufferEdited` / `BufferCleaned` removed (FR-010, breaking); bus protocol v0.2 → v0.3 (MAJOR).
**Scale/Scope**: ~500–800 new LOC across `core`, `tui`, `buffers`, and e2e tests; 4 new fact attributes (`buffer/path`, `buffer/byte-size`, `buffer/dirty`, `buffer/observable`); 1 new `FactValue` variant (`U64`); 1 new `EventPayload` variant (`BufferOpen`) + 2 removed (`BufferEdited`, `BufferCleaned`); 3 new scenario tests (F23 isolated overwrite, component-discipline property, bootstrap-causal-parent); 4 new e2e tests (bootstrap, mutation, sigkill, authority-conflict); 2 slice-001 e2e tests transformed (`hello_fact`, `disconnect`).

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Gates derived from `.specify/memory/constitution.md` v0.7.0. Each principle is named with a slice-specific gate. Principles not exercised by this slice are listed with forward-looking triggers.

### Applicable principles (PLANNED — must hold by `/speckit.implement` exit)

- **P1 — Domain modeling without type hierarchy.** Buffer state is a flat attribute set (no `buffer/state/*` discriminated union). The `:content` component is a conceptual authority realized this slice as the service's own in-memory byte store + on-disk file; *no in-code `Component` trait* is introduced (component infrastructure is deferred per `docs/07-open-questions.md §26`). No trait hierarchy is added to `core/` or `buffers/`.
- **P2 — Purity at edges, transactional state at core.** The buffer service's per-buffer observation (content read → digest → compare-to-memory) is a pure function of filesystem state + memory state at poll time. The publish step (bus send of changed facts) is the sole side effect per poll per buffer. Bootstrap and state-recovery transitions share a per-buffer causal parent so `why?` walks each buffer's bootstrap as one transaction (per Clarification 2026-04-23).
- **P4 — Simplicity in implementation.** One new crate reusing the slice 002 service-client skeleton (connect → handshake → authority claim per entity → poll loop → edge-triggered lifecycle → retract-on-disconnect); no new abstraction layers. The publisher is conn-bound per slice 002 F14, which we inherit unchanged.
- **P5 — Serialization and open standards.** Bus: CBOR via `ciborium`; **no new Weaver CBOR tag** — `FactValue` uses an existing adjacent-tag structure and the `U64` variant lands as an additive enum entry under the MAJOR wire bump. CLI: `--output=json` continues via `serde_json`; adds JSON shape for the new `buffer/*` families. All wire-visible identifiers are kebab-case per Amendment 5: `buffer/path`, `buffer/byte-size`, `buffer/dirty`, `buffer/observable`, `weaver-buffers` (service-id).
- **P6 — Humane shell.** `clap` derive for `weaver-buffers` CLI. Errors reference fact-space state: `buffer not openable at <path>: <reason> — no fact (entity:<derived>, attribute:buffer/path) will be asserted.`
- **P7 — Public-surface enumeration.** Three surfaces touched:
  - **Bus protocol** — MAJOR wire-incompatible change. `Hello.protocol_version` advances 0x02 → 0x03; `EventPayload::BufferEdited`/`BufferCleaned` removed; `EventPayload::BufferOpen { path }` added; `FactValue::U64(u64)` added. Enumerated in `contracts/bus-messages.md`.
  - **Fact-family schemas** — new families `buffer/path`, `buffer/byte-size`, `buffer/dirty`, `buffer/observable`. All v0.1.0; additive. Authority transfer of `buffer/dirty` from behavior to service is a schema-level event documented in `CHANGELOG.md`.
  - **CLI + structured output** — new `weaver-buffers` binary; `simulate-edit`/`simulate-clean` removed (MAJOR within the `weaver` CLI surface); `bus_protocol` field value advances `0.2.0 → 0.3.0` in all three binaries' `--version` output. Enumerated in `contracts/cli-surfaces.md`.
- **P8 — SemVer + Keep a Changelog per surface.** Bus protocol bumps MAJOR (0.2 → 0.3); buffer fact-family schemas start at 0.1 each; `weaver` CLI surface carries a MAJOR bump for `simulate-edit`/`simulate-clean` removal; `weaver-buffers` ships at 0.1.0. `CHANGELOG.md` gains entries per surface. Every bus message on the new protocol carries `protocol_version: 0x03`.
- **P9 — Scenario + property-based testing.** Scenario tests: buffer service attaches → publishes facts → TUI observes; external mutation flips dirty; SIGKILL retracts. Property tests: CBOR round-trip over `FactValue::U64`; component-discipline invariant (SC-306 — no fact value across randomly-generated observation sequences carries buffer content). F23 isolated scenario test (spec FR-013): behavior-authored + service-authored `buffer/dirty` on same key → inspect attributes to service.
- **P10 — Regressions captured as scenario tests before fix.** Convention continues.
- **P11 — Provenance everywhere.** Every `buffer/*` fact carries `Provenance { source: ActorIdentity::Service { service_id: "weaver-buffers", instance_id: <uuid-v4> }, timestamp_ns, causal_parent }`. `weaver --version`, `weaver-tui --version`, `weaver-git-watcher --version`, and new `weaver-buffers --version` all emit per-P11 fields and report `bus_protocol: "0.3.0"`.
- **P12 — Determinism and single-VM concurrency discipline.** Buffer service's poll loop is a single Tokio task; observations are sequential per poll (iterated over the N buffers in CLI-declared order); publishing is serialized through the shared bus writer handle. No shared mutable state across tasks.
- **P13 — Observability for operators.** `tracing` spans wrap each poll tick, each per-buffer mutation detection, each publish, each lifecycle transition. `tracing-subscriber` JSON layer respected via `--output=json`.
- **P15 — Schema evolution and trace-store migration.** Each new fact family declares an initial schema version (0.1.0). `buffer/dirty`'s authority-origin transfer is documented as an entry under the fact-family's wire-shape history. The bus protocol's MAJOR bump requires a wire-incompatibility entry in `CHANGELOG.md`; trace-store migration is N/A while traces are in-memory only.
- **P16 — Failure modes are public contract.** `weaver-buffers` publishes lifecycle (`started`, `ready`, `degraded`, `unavailable`, `stopped`) per `docs/05-protocols.md §5`. Degradation taxonomy is **orthogonal** per Clarification 2026-04-23: `watcher/status` is service-level; `buffer/observable` is per-buffer. Enumerated in `contracts/bus-messages.md` failure-modes table.
- **P17 — Documentation in lockstep.** This slice's design directly engages `docs/01-system-model.md §2.4` (Components vs Facts) — it is the first code-level instantiation of a component authority without shipping component primitives. Cross-references landed in spec's Architectural alignment; plan and contracts reinforce. No L1 change required.
- **P18 — Performance budgets per latency class.** Publish path declared interactive (≤ 100 ms bus-level). Polling cadence declared at 250 ms default (matching slice 002). External-mutation-to-render budget 500 ms (SC-302).
- **P19 — Reproducible builds.** `Cargo.lock` committed (existing). New dep `sha2` pinned to minor version; `tempfile` (dev-dep) already in tree.
- **P20 — Retraction is first-class.** `buffer/observable` retracts-and-reasserts on recovery per FR-016; every `buffer/*` fact retracts on SIGTERM per FR-020; on SIGKILL the core's `release_connection` path (slice 002 F1) retracts every fact owned by the dropped connection. Tests cover all three retraction paths.
- **P21 — AI agent conduct.** Continues — Conventional Commits per Amendment 1; regression-tests-before-fix (P10); commits `Co-Authored-By`.

### Principles not exercised by this slice (justified)

- **P3 — Defensive Host, Fault-Tolerant Guest.** No Steel host primitive added; the L2/arch §9.4.1 contract has no implementation surface this slice.
- **P14 — Steel sandbox discipline.** No Steel host primitives.

### Additional constraints (must hold by implementation exit)

- **License.** New crate `buffers/` declares `license = "AGPL-3.0-or-later"` via `license.workspace = true`. Inbound dependency `sha2` (MIT / Apache-2.0) is compatible.
- **Wire vocabulary naming (Amendment 5).** All new identifier values are kebab-case: `buffer/path`, `buffer/byte-size`, `buffer/dirty`, `buffer/observable`, `weaver-buffers` (service-id), `buffer-open` (event variant tag).
- **Code quality gates (Amendment 6).** New crate passes `cargo clippy --all-targets --workspace -- -D warnings` and `cargo fmt --all -- --check`. `scripts/ci.sh` runs green before commit. Pre-commit hook runs the full gate chain.
- **Conventional Commits (Amendment 1).** Per-finding or per-task commits use `feat(bus):`, `feat(buffers):`, `fix(buffers):`, `docs(specify):`, `test(buffers):`, etc. Breaking public-surface changes carry `BREAKING CHANGE:` footers: bus protocol bump, `EventPayload` variant removal, CLI subcommand removal.

**Result**: PASS. No principle violated. No Complexity Tracking entries required. The three surfaces that trigger per-surface versioning (bus protocol, fact-family schemas, CLI + output) are enumerated in the Phase 1 contracts documents.

### Post-design re-check (after Phase 1 artifacts)

Re-evaluated after `research.md`, `data-model.md`, `contracts/bus-messages.md`, `contracts/cli-surfaces.md`, `quickstart.md` landed:

- All 19 applicable principles still hold.
- **P7/P8** — bus protocol v0.2 → v0.3 (MAJOR), new fact-family schemas at v0.1.0 each, and the `FactValue::U64` additive variant are now enumerated with concrete wire shapes in `contracts/bus-messages.md`. `CHANGELOG.md` updates deferred to `/speckit.implement` (the actual wire change lands with code).
- **P16** — service-level vs per-buffer degradation taxonomy is mapped to concrete lifecycle events and failure facts in `contracts/bus-messages.md` §Failure modes, matching Clarification 2026-04-23.
- **P20** — retraction paths are stated as scenario tests in `quickstart.md` (bootstrap→SIGTERM, bootstrap→degraded-recovery→SIGTERM, bootstrap→SIGKILL→core release).
- **P9** — F23 isolated scenario test, component-discipline property, and CBOR `U64` round-trip are itemized in `data-model.md` validation rules and `quickstart.md` verification steps.

**Re-check result**: PASS. The design phase tightened P7/P8/P16/P20/P9 coverage without introducing constitutional tension. No Complexity Tracking entries needed.

## Project Structure

### Documentation (this feature)

```text
specs/003-buffer-service/
├── plan.md              # This file
├── spec.md              # Phase 0 input — feature specification (with 4 clarifications)
├── research.md          # Phase 0 — content-digest lib, e2e-fixture approach, FactValue::U64 landing
├── data-model.md        # Phase 1 — buffer entity, :content component (conceptual), buffer/* family, lifecycle mapping
├── quickstart.md        # Phase 1 — four-process walkthrough + SC-301..SC-307 verification
├── contracts/
│   ├── bus-messages.md  # Phase 1 — v0.3 wire: new FactValue::U64, new EventPayload::BufferOpen, removed BufferEdited/Cleaned, new buffer/* families
│   └── cli-surfaces.md  # Phase 1 — weaver-buffers CLI + weaver simulate-* removals + weaver-tui Buffers section
├── checklists/
│   └── requirements.md  # Spec quality checklist (passing)
└── tasks.md             # Phase 2 output (/speckit.tasks — NOT created here)
```

### Source Code (repository root)

```text
core/
├── Cargo.toml                    # bumps internal version if per-crate versioning engages; bus protocol v0.3 noted in --version output
├── build.rs                      # vergen — unchanged
└── src/
    ├── lib.rs                    # MODIFIED — re-exports unchanged shape; EventPayload enum updated
    ├── main.rs                   # MODIFIED — simulate-edit/simulate-clean subcommand removal; --version bus_protocol bumped
    ├── bus/                      # MODIFIED — handshake accepts protocol_version 0x03; rejects 0x02
    ├── behavior/                 # MODIFIED — dirty_tracking.rs REMOVED; mod.rs updated; dispatcher.rs unchanged (inherits slice 002 conn-bound identity, lock-order, release_connection)
    ├── fact_space/               # UNCHANGED
    ├── trace/                    # UNCHANGED
    ├── inspect/                  # UNCHANGED (slice 002's structured-identity rendering already covers ActorIdentity::Service)
    ├── types/
    │   ├── event.rs              # MODIFIED — EventPayload::BufferEdited + BufferCleaned REMOVED; EventPayload::BufferOpen { path: String } ADDED
    │   └── fact.rs               # MODIFIED — FactValue::U64(u64) ADDED
    └── cli/
        ├── simulate.rs           # REMOVED — simulate-edit / simulate-clean gone
        ├── mod.rs                # MODIFIED — unregisters the simulate subcommands
        └── output.rs             # MODIFIED — JSON rendering handles FactValue::U64
tui/
├── Cargo.toml                    # unchanged
└── src/
    ├── main.rs                   # unchanged
    ├── client.rs                 # MODIFIED — subscription pattern adds `buffer/` prefix handling
    ├── render.rs                 # MODIFIED — new Buffers render section below Repositories; renders path/size/dirty per open buffer with the `by service weaver-buffers (inst ...)` annotation
    └── commands.rs               # unchanged (no new keystrokes; slice 004)
buffers/                          # NEW crate (binary: `weaver-buffers`)
├── Cargo.toml                    # license.workspace = true; depends on core for types + sha2 + tokio + clap + miette + thiserror + tracing + uuid
├── README.md                     # short — role, usage, trim
└── src/
    ├── main.rs                   # CLI entry (clap derive): positional `<PATH>...`, --poll-interval, --socket, --output, -v
    ├── lib.rs                    # re-export for testability
    ├── observer.rs               # per-buffer file observation: read content → sha2 digest → compare memory digest → return (path, byte_size, dirty) tuple
    ├── publisher.rs              # bus client; handshake; per-buffer bootstrap with shared causal parent; poll loop; lifecycle (service-level watcher/status + per-buffer buffer/observable); retract-on-shutdown
    └── model.rs                  # BufferObservation struct + BufferState (in-memory byte store + on-disk digest); path dedup helper
git-watcher/                      # UNCHANGED (slice 002)
ui/                               # UNCHANGED
tests/e2e/
├── hello_fact.rs                 # TRANSFORMED — drives buffer/open (not simulate-edit) + external mutation; SC-307 coverage
├── disconnect.rs                 # TRANSFORMED — drives buffer service disconnect (not simulate-edit); SC-307 coverage
├── subscribe_snapshot.rs         # UNCHANGED
├── git_watcher_*.rs              # UNCHANGED (slice 002)
├── buffer_open_bootstrap.rs      # NEW — four-process: core + git-watcher + buffer-service + test client; SC-301 coverage
├── buffer_external_mutation.rs   # NEW — open a buffer, mutate file externally, observe dirty flip; SC-302 coverage
├── buffer_sigkill.rs             # NEW — SIGKILL buffer service, observe release_connection retractions; SC-303 coverage
└── buffer_authority_conflict.rs  # NEW — launch two instances on overlapping paths; second exits 3; SC-304 coverage
CHANGELOG.md                      # MODIFIED — bus-protocol MAJOR bump to 0.3.0; new fact families at v0.1.0; CLI MAJOR entry for simulate-edit/clean removal; new weaver-buffers binary at 0.1.0
Cargo.toml                        # workspace — adds `buffers` to members
```

**Structure Decision**: `buffers` becomes a top-level workspace member alongside `core`, `ui`, `tui`, `git-watcher`. Top-level placement matches the existing flat workspace convention and asserts the buffer service as an architectural peer — a content-backed service on the bus, per `docs/02-architecture.md §2`, not an auxiliary tool. Alternatives considered and rejected: nesting under a hypothetical `services/` directory would suggest hierarchy not yet present in the workspace today; the first introduction of such hierarchy belongs to a slice where directory grouping is justified by multi-service cohesion beyond two peers. The buffer service imports shared types from `core`'s library face exactly as git-watcher does. See `research.md` for the content-digest library choice (`sha2`) and for the content-on-disk observation strategy.

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

No violations. Section intentionally empty.

---

*Plan complete. Phase 0 (research.md), Phase 1 (data-model.md, contracts/, quickstart.md), and CLAUDE.md SPECKIT-block update follow as separate artifacts.*
