# Implementation Plan: Git-Watcher Actor

**Branch**: `002-git-watcher-actor` | **Date**: 2026-04-21 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/002-git-watcher-actor/spec.md`

## Summary

A third Rust binary — `weaver-git-watcher` — joins the existing `weaver` (core) and `weaver-tui` (TUI) as the first **non-editor actor** on the bus. It watches one git repository and publishes authoritative facts (`repo/dirty`, `repo/head-commit`, and a discriminated-union-by-naming family `repo/state/on-branch | detached | unborn`) under a structured actor identity. In parallel, `SourceId::External(String)` is replaced by a structured `ActorIdentity` enum whose variants match the `docs/01-system-model.md §6` taxonomy — the change ripples through `core`'s provenance, the CBOR wire contract, trace store, and inspection rendering. The bus protocol bumps to v0.2.0. The TUI gains a new render line showing current repository state; `weaver inspect` renders structured actor identity in both human and JSON forms.

This slice is the first code-level assertion of the coordination-substrate pivot (constitution v0.2, §17): an actor Weaver never shipped before participates on the bus as a peer, with identity and authority that the existing ontology already supports. It deliberately does not land agents, delegation (`on-behalf-of`), action entities, conflict surfacing, or speculative facts.

## Technical Context

**Language/Version**: Rust 2024 edition; resolver = "3" (workspace-level); toolchain pinned to 1.94.0 via `fenix` per `flake.nix` + `rust-toolchain.toml`.
**Primary Dependencies**: existing — `tokio`, `ciborium`, `serde` + `serde_json`, `clap` derive, `miette`, `thiserror`, `tracing` + `tracing-subscriber`, `proptest`, `vergen`, `crossterm`; **new** — `uuid = "1"` with `v4` feature (watcher instance identity), `gix` (pure-Rust git; pinned minor version — see `research.md` §1).
**Storage**: In-memory only. No persistence change from slice 001; trace remains in-process.
**Testing**: `cargo test` (unit + scenario); `proptest` (CBOR round-trip for the new `ActorIdentity` variants; `repo/state/*` mutex invariant as a property); workspace-level e2e tests extended from two-process to three-process scenarios (core + watcher + test-client).
**Target Platform**: Linux + macOS desktop. Single machine. Bus over Unix domain socket as in slice 001.
**Project Type**: Rust workspace; **adds one new member crate** `git-watcher/` (binary target: `weaver-git-watcher`). Modifies `core/` (provenance, wire contract, inspection) and `tui/` (new render region + subscription). `ui/` untouched.
**Performance Goals**: SC-001 — cold start to first TUI render ≤ 1 s; SC-002 — external mutation to TUI render ≤ 500 ms (interactive latency class per `docs/02-architecture.md §7.1`, with polling overhead folded into the operator-perceived budget).
**Constraints**: Single repository per watcher instance; static CLI-argument registration only; polling-based observation (inotify/kqueue deferred); single-writer authority per fact family; `SourceId::External(String)` removed (breaking wire change; bus protocol v0.1 → v0.2).
**Scale/Scope**: ~600–900 new LOC across `core`, `tui`, and `git-watcher`; 5 new fact attributes (`repo/dirty`, `repo/head-commit`, and three `repo/state/*` variants); 1 new actor-identity variant (`Service { service_id, instance_id }`) + rewritten `SourceId` enum; 3 new e2e test files.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Gates derived from `.specify/memory/constitution.md` v0.7.0. Each principle is named with a slice-specific gate. Principles not exercised by this slice are listed with forward-looking triggers.

### Applicable principles (PLANNED — must hold by `/speckit.implement` exit)

- **P1 — Domain modeling without type hierarchy.** `ActorIdentity` is a closed sum type (enum with one variant per kind); no trait hierarchies. `repo/state/*` discriminated-union is expressed by naming convention + watcher-enforced mutex invariant (open-questions §26); no `StateBase` trait.
- **P2 — Purity at edges, transactional state at core.** Watcher's repository observation (`gix` read ops) is a pure function of filesystem state at poll time. The publish step (bus send) is the sole side effect per poll. Mutex-invariant maintenance (retract-then-assert for `repo/state/*` transitions) is a two-message transaction sharing a single `causal_parent`.
- **P4 — Simplicity in implementation.** One new crate; one new actor kind (`Service`); reuses existing bus, codec, trace, and inspection infrastructure. No abstraction layers added: the watcher is a bus client like the TUI, not a subclass of some "service framework" (no such framework exists yet).
- **P5 — Serialization and open standards.** Bus: CBOR via `ciborium`; new Weaver CBOR tag 1002 for structured `ActorIdentity`. CLI: `--output=json` continues via `serde_json`; JSON shape of inspection output grows additive fields for the new actor variants. Kebab-case wire values (e.g., `repo/state/on-branch`) per Amendment 5.
- **P6 — Humane shell.** `clap` derive for `weaver-git-watcher` CLI. Errors reference fact-space state (e.g., `repo not observable at <path>: not a git repository — no fact (entity:<repo-ref>, attribute:repo/state/*) can be asserted`).
- **P7 — Public-surface enumeration.** Three surfaces touched:
  - **Bus protocol** — MAJOR wire-incompatible change (provenance shape). `Hello.protocol_version` advances 0x01 → 0x02. Enumerated in `contracts/bus-messages.md`.
  - **Fact-family schemas** — new families `repo/dirty`, `repo/head-commit`, `repo/state/*`. Additive. Enumerated in `contracts/bus-messages.md`.
  - **CLI + structured output** — new `weaver-git-watcher` binary; `weaver inspect` actor rendering grows. Additive for existing surfaces. Enumerated in `contracts/cli-surfaces.md`.
- **P8 — SemVer + Keep a Changelog per surface.** Bus protocol bumps MAJOR (0.1 → 0.2); fact-family schemas start at 0.1 each. `CHANGELOG.md` gains entries per surface. Every bus message on the new protocol carries `protocol_version: 0x02`.
- **P9 — Scenario + property-based testing.** Scenario tests: watcher attaches → publishes facts → TUI observes; watcher transitions (on-branch ↔ detached ↔ unborn) produce retract-then-assert pairs with shared causal parent. Property tests: CBOR round-trip over all `ActorIdentity` variants; mutex-invariant property (at most one `repo/state/*` asserted per repository entity across any trace prefix).
- **P10 — Regressions captured as scenario tests before fix.** Convention continues.
- **P11 — Provenance everywhere.** Every `repo/*` fact carries `Provenance { source: ActorIdentity::Service { service_id: "git-watcher", instance_id: <uuid-v4> }, timestamp_ns, causal_parent }`. `weaver --version` and `weaver-git-watcher --version` both emit per-P11 fields. New `repo/*` fact-family schema versions are declared per P15.
- **P12 — Determinism and single-VM concurrency discipline.** Watcher's poll loop is a single Tokio task; observation calls are sequential per poll; publishing is serialized through the bus client handle. No shared mutable state across tasks.
- **P13 — Observability for operators.** `tracing` spans wrap each poll, each observed transition, each publish. `tracing-subscriber` structured-formatter to stderr, respecting `RUST_LOG`.
- **P15 — Schema evolution and trace-store migration.** Each new fact family declares an initial schema version (0.1.0). The bus protocol's MAJOR bump requires a wire-incompatibility entry in `CHANGELOG.md`; trace-store migration is N/A while traces are in-memory only.
- **P16 — Failure modes are public contract.** `weaver-git-watcher` publishes lifecycle (`started`, `ready`, `degraded`, `unavailable`, `stopped`) per `docs/05-protocols.md §5`. Degraded: repository temporarily unreadable (permissions, invalid state). Unavailable: repository lost or watcher exiting. Failure facts: `watcher/status <lifecycle>`, `repo/observable <bool>`. Enumerated in `contracts/bus-messages.md` failure-modes table.
- **P17 — Documentation in lockstep.** This slice's design directly engages the pivot work: it closes open-questions §25 (shape + migration sub-questions) and instantiates §26 (discriminated-union-facts stopgap). Cross-references already landed in commit `c0facda`.
- **P18 — Performance budgets per latency class.** Watcher's publish path declared interactive (≤100 ms bus-level). Polling cadence declared at 250 ms default (see `research.md` §2). Violations (e.g., a slow `gix` call exceeding the interactive budget) surface in traces.
- **P19 — Reproducible builds.** `Cargo.lock` committed (existing). New deps (`uuid`, `gix`) pinned to minor versions; upstream churn adopted deliberately per L2 practice.
- **P20 — Retraction is first-class.** The `repo/state/*` mutex invariant is inherently retraction-driven: every variant transition is retract-then-assert with a shared `causal_parent`. Watcher disconnect retracts all facts it authored. Tests exercise retraction paths explicitly (not merely the assertion paths).
- **P21 — AI agent conduct.** Continues — Conventional Commits per Amendment 1; regression-tests-before-fix; commits `Co-Authored-By`.

### Principles not exercised by this slice (justified)

- **P3 — Defensive host, fault-tolerant guest.** No Steel host primitive added; the L2/arch §9.4.1 contract has no implementation surface this slice.
- **P14 — Steel sandbox discipline.** No Steel host primitives.

### Additional constraints (must hold by implementation exit)

- **License.** New crate `git-watcher/` declares `license = "AGPL-3.0-or-later"` (or inherits via `workspace = true`). Inbound dependencies reviewed for AGPL-3.0-or-later compatibility: `uuid` (MIT/Apache-2.0) ✓; `gix` and its transitive deps (Apache-2.0 OR MIT) to be spot-checked at implementation time — no known-incompatible licenses expected.
- **Wire vocabulary naming (Amendment 5).** All new identifier values are kebab-case: `repo/dirty`, `repo/head-commit`, `repo/state/on-branch`, `repo/state/detached`, `repo/state/unborn`, `watcher/status`, `repo/observable`. New CBOR-enum tags for `ActorIdentity` variants use kebab-case (`core`, `behavior`, `tui`, `service`, `user`, `host`, `agent`).
- **Code quality gates (Amendment 6).** New crate passes `cargo clippy --all-targets --workspace -- -D warnings` and `cargo fmt --all -- --check`. `scripts/ci.sh` runs green before commit. Pre-commit hook runs the full gate chain.

**Result**: PASS. No principle violated. No Complexity Tracking entries required. The three surfaces that trigger per-surface versioning (bus protocol, fact-family schemas, CLI+output) are enumerated in the Phase 1 contracts documents.

### Post-design re-check (after Phase 1 artifacts)

Re-evaluating after `data-model.md`, `contracts/bus-messages.md`, `contracts/cli-surfaces.md`, `quickstart.md` land:

- All 18 applicable principles still hold.
- **P7/P8** — bus protocol v0.1 → v0.2 (MAJOR) and new fact-family schemas at v0.1.0 each are now enumerated with concrete wire shapes in `contracts/bus-messages.md`. `CHANGELOG.md` updates deferred to `/speckit.implement` (the actual wire change lands with code).
- **P16** — watcher degradation taxonomy is now mapped to concrete lifecycle events and failure facts in `contracts/bus-messages.md` §Failure modes.
- **P20** — retraction paths for `repo/state/*` mutex invariant are stated as property tests in `data-model.md` Validation rules and as scenario tests in `quickstart.md`.

**Re-check result**: PASS. The design phase tightened P7/P8/P16/P20 coverage without introducing constitutional tension. No Complexity Tracking entries needed.

## Project Structure

### Documentation (this feature)

```text
specs/002-git-watcher-actor/
├── plan.md              # This file
├── research.md          # Phase 0 — git-observation lib, polling cadence, uuid version
├── data-model.md        # Phase 1 — ActorIdentity, repository entity, repo/state/* union, watcher lifecycle
├── quickstart.md        # Phase 1 — three-process walkthrough + SC-001..SC-006 verification
├── contracts/
│   ├── bus-messages.md  # Phase 1 — v0.2 wire: new provenance shape, new fact families, new lifecycle facts
│   └── cli-surfaces.md  # Phase 1 — weaver-git-watcher CLI + updated weaver inspect output
├── checklists/
│   └── requirements.md  # Spec quality checklist (passing)
└── tasks.md             # Phase 2 output (/speckit.tasks — NOT created here)
```

### Source Code (repository root)

```text
core/
├── Cargo.toml             # bumps internal version if per-crate versioning engages; bus protocol v0.2 noted in --version output
├── build.rs               # vergen — unchanged
└── src/
    ├── lib.rs             # MODIFIED — SourceId replaced by ActorIdentity enum per §6 taxonomy
    ├── main.rs            # MODIFIED — inspection rendering updated for structured ActorIdentity
    ├── bus/               # MODIFIED — handshake accepts protocol_version 0x02; rejects 0x01
    ├── fact_space/        # MODIFIED — `repo/*` subscription patterns wired through existing index
    ├── behavior/          # UNCHANGED (no new behaviors in this slice)
    ├── trace/             # MODIFIED — trace entries carry the new ActorIdentity shape
    ├── inspect/           # MODIFIED — InspectionDetail renders structured actor
    └── cli/               # MODIFIED — weaver inspect / weaver status output includes structured actor
tui/
├── Cargo.toml             # depends on core (shared types)
└── src/
    ├── main.rs            # unchanged
    ├── client.rs          # MODIFIED — subscription pattern gains `repo/` prefix handling
    ├── render.rs          # MODIFIED — new render region for current repo state (dirty / branch / head)
    └── commands.rs        # unchanged
git-watcher/               # NEW crate (binary: `weaver-git-watcher`)
├── Cargo.toml             # license = "AGPL-3.0-or-later"; depends on core for types + gix + uuid + tokio + clap + miette + thiserror + tracing
├── README.md              # short — role, usage, trim
└── src/
    ├── main.rs            # CLI entry (clap derive): positional repo path, --poll-interval, --socket
    ├── lib.rs             # re-export for testability
    ├── observer.rs        # gix-backed repository-state reads: HEAD kind, branch, head-commit, dirty
    ├── publisher.rs       # bus client; handshake; maintains repo/state/* mutex (retract+assert with shared causal parent); lifecycle announcements
    └── model.rs           # WorkingCopyState enum (on-branch | detached | unborn) + conversion from gix::head::Kind
ui/                        # untouched
tests/e2e/
├── hello_fact.rs          # UNCHANGED — must still pass under new protocol version
├── disconnect.rs          # UNCHANGED
├── subscribe_snapshot.rs  # UNCHANGED
├── git_watcher_attach.rs         # NEW — three-process: core + git-watcher + test client; initial handshake + repo/* publication
├── git_watcher_transitions.rs    # NEW — on-branch ↔ detached ↔ unborn transitions; retract-then-assert pairs with shared causal parent
└── structured_actor_inspection.rs # NEW — every existing fact family inspected under the new wire shape; round-trip ok
CHANGELOG.md               # MODIFIED — bus-protocol MAJOR bump; new fact families at v0.1.0; CLI additive entries
Cargo.toml                 # workspace — adds `git-watcher` to members
```

**Structure Decision**: `git-watcher` becomes a top-level workspace member alongside `core`, `ui`, `tui`. Top-level placement matches the existing flat workspace convention and asserts the watcher as an architectural peer (a service on the bus, per `docs/02-architecture.md §2`), not an auxiliary tool. Alternatives considered and rejected: nesting under a hypothetical `tools/` or `services/` directory would suggest hierarchy that is not in the workspace today; the first introduction of such hierarchy belongs to a slice where a second service exists to justify the grouping. The watcher imports shared types from `core`'s library face exactly as the TUI does; no `protocol` crate extraction this slice. See `research.md` for the git-observation library choice (`gix`).

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

No violations. Section intentionally empty.

---

*Plan complete. Phase 0 (research.md), Phase 1 (data-model.md, contracts/, quickstart.md), and CLAUDE.md SPECKIT-block update follow as separate artifacts.*
