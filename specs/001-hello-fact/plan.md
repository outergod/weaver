# Implementation Plan: Hello, fact

**Branch**: `001-hello-fact` | **Date**: 2026-04-19 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/001-hello-fact/spec.md`

## Summary

Two Rust processes — `weaver` (core) and `weaver-tui` (terminal UI) — communicate over a local-IPC bus using a typed message protocol. The core registers one embedded Rust behavior that asserts a `buffer/dirty` fact in response to a `buffer/edited` event. The TUI subscribes to the fact stream and renders the dirty state. A bus-level inspection request (used by both TUI and CLI) returns the source event, asserting behavior, and timestamp for any asserted fact. The CLI exposes one-shot snapshot commands (`--version`, `status --output=json`, `inspect <fact-ref>`) — the bus is the integration surface for continuous consumers per L2 P5.

This is the smallest end-to-end exercise of: bus delivery classes (lossy `event` + authoritative `fact-assert`/`fact-retract`), the fact space (assert + retract + subscribe), one embedded behavior, provenance-everywhere, and a TUI that derives state from the fact space rather than from local knowledge.

## Technical Context

**Language/Version**: Rust 2024 edition; resolver = "3" (already declared in workspace `Cargo.toml`); pin via `rust-toolchain.toml` to current stable.
**Primary Dependencies**: `tokio` (async runtime), `ciborium` (CBOR for the bus per L2 P5 + arch §3.1), `serde` + `serde_json` (CLI `--output=json` per L2 P5), `clap` derive (L2 P6), `miette` + `thiserror` (L2 P6), `tracing` + `tracing-subscriber` (L2 P13), `proptest` (L2 P9 property tests), `vergen` (build-time provenance per L2 P11), `crossterm` (TUI rendering — minimal; `ratatui` deferred until needed).
**Storage**: N/A. In-memory only per spec Assumptions; no persistence between runs.
**Testing**: `cargo test` for unit + scenario tests; `proptest` for property-based tests on fact-space invariants. Scenario tests express behavior firing as `(initial fact-space, event sequence) → (expected fact deltas)`.
**Target Platform**: Linux + macOS desktop. Single-machine, two-process pair via Unix domain socket.
**Project Type**: Rust workspace with two binary crates that share types via the core crate's library face. (Workspace members `core`, `ui`, `tui` are declared; `ui` is untouched in this slice.)
**Performance Goals**: SC-001 — `simulate-edit` to TUI render ≤ 100 ms (interactive latency class per `docs/02-architecture.md` §7.1). SC-006 — `weaver --version` returns ≤ 50 ms.
**Constraints**: Single-machine, in-memory only, single behavior, single fact family, single buffer entity. Two processes; local IPC only; no network. No Steel integration.
**Scale/Scope**: ~1000 LOC across `core` and `tui` combined; 1 behavior; 1 fact family; 6 CLI subcommands; 5 bus message variants exercised.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Gates derived from `.specify/memory/constitution.md` v0.4.0. Each applicable principle is named with a slice-specific gate. Principles not exercised by this slice are listed below with justification per L2 governance.

### Applicable principles (PLANNED — must hold by `/speckit.implement` exit)

- **P1 — Domain modeling without type hierarchy.** Domain types are small composable structs / enums (`EntityRef`, `FactKey`, `Provenance`, `Fact`, `Event`, `BusMessage`). No trait inheritance trees; trait bounds used for generic constraint, not for polymorphism.
- **P2 — Purity at edges, transactional state at core.** Fact predicates, trace renderers, message serializers are pure functions. Bus enqueue and fact-space mutation are the only mutating operations; both are transactional and produce trace entries.
- **P4 — Simplicity in implementation.** Two crates only (`core`, `tui`); shared types live in `core`'s library face. No abstraction layers beyond what the bus's message-dispatch genuinely requires (one trait + one enum). No speculative extension points.
- **P5 — Serialization and open standards.** Bus uses CBOR via `ciborium` with the Weaver tag registry's initial entries (entity-ref, keyword). CLI `--output=json` uses `serde_json`. Tests assert on deserialized structures (P5 mandate). The CLI surfaces are explicitly one-shot per amended P5; FR-008 inspection is a bus request/response, not a CLI-only feature.
- **P6 — Humane shell.** `clap` derive for both binaries; `miette`/`thiserror` for error types. Errors that refer to fact-space state name the fact key and authority, not just the call site.
- **P9 — Scenario + property-based testing.** Behaviors and bus dispatch tested as scenarios `(initial fact-space, event seq) → (expected deltas + intents)`. Property tests cover fact-space invariants (assert/retract round-trip preserves identity; provenance is always non-empty; sequence numbers are monotonic per publisher). Pure helpers (predicates, serializers) get unit tests in classic TDD style.
- **P10 — Regressions captured as scenario tests before fix.** Convention; the scenario-test scaffolding lands in this slice so the discipline is enforceable from the next bug onward.
- **P11 — Provenance everywhere.** `weaver --version` output via `vergen` includes: crate version, git commit SHA, dirty-tree marker, build timestamp, build profile. Bus protocol version is included once a stable version is fixed. Every `BusMessage` carries `Provenance { source, timestamp_ns, causal_parent: Option<EventId> }`. Every `TraceEntry` includes the firing's behavior identifier.
- **P12 — Determinism and single-VM concurrency discipline.** Single Tokio runtime in `core`. Behavior firings serialize through one `mpsc` consumer; behavior body sees `&FactSpaceSnapshot`, mutates only via the dispatcher's commit step. No `Arc<Mutex<...>>` outside the fact-space boundary.
- **P13 — Observability for operators.** `tracing` crate with structured spans wrapping every behavior firing, every bus publish, every fact mutation. `tracing-subscriber` writes structured logs to stderr. OTel deferred (P13 says "where applicable"; not applicable at this scale).
- **P19 — Reproducible builds.** `Cargo.lock` is committed. `rust-toolchain.toml` pins the stable channel. `build.rs` (via `vergen`) embeds git SHA + dirty bit + timestamp. No `--features` gates that change semantics.
- **P20 — Retraction is first-class.** `simulate-clean` triggers retraction symmetric to assert. Fact-space API exposes `retract` alongside `assert`. The scenario test suite includes assert→retract round-trip; PR template will note retraction as a checklist item from this slice forward.

### Principles not exercised by this slice (justified)

- **P3 — Defensive host, fault-tolerant guest.** No Steel integration. Becomes mandatory the first time a Steel host primitive lands. Until then, the L2/arch §9.4.1 contract has no implementation surface to enforce.
- **P7 — Public-surface enumeration.** This slice introduces *initial* versions of: bus protocol (v0.1.0), `buffer/dirty` fact-family schema (v0.1.0), CLI flags + structured output, configuration schema (minimal: bus socket path, log level). The surfaces are enumerated in `contracts/cli-surfaces.md` and `contracts/bus-messages.md`. Per-surface evolution policy becomes operative on the first breaking change.
- **P8 — SemVer + Keep a Changelog per surface.** A `CHANGELOG.md` is created at the repo root in this slice (initial entry only). Per-surface version bumping engages on the first change to any of the surfaces in P7.
- **P14 — Steel sandbox discipline.** No Steel host primitives in this slice.
- **P15 — Schema evolution and trace-store migration.** `buffer/dirty` schema is v0.1.0 (initial). Trace store is in-memory only — migration policy engages with the first persistent trace.
- **P16 — Failure modes are public contract.** Single-process pair; the only degradation is "core unavailable" handled by FR-009/FR-010. Multi-service degradation taxonomy engages when the second service exists.
- **P17 — Documentation in lockstep.** Applies to this work meta-level: spec, plan, and any code commits reference each other; the L2/L1 sync CI check is itself unimplemented but the discipline is followed by hand.
- **P18 — Performance budgets per latency class.** SC-001 names the interactive class budget. CI enforcement (per-primitive bound checks) is deferred — only one primitive (the bus publish/subscribe path) exists; per-primitive enforcement makes sense once the count grows.
- **P21 — AI agent conduct.** Followed throughout: Conventional Commits per Amendment 1, regression-tests-before-fix per P10, public-surface changes documented per P7/P8, attributable commits via `Co-Authored-By` trailer. No code-level enforcement needed.

**Result**: PASS. No principle is violated. All 12 applicable principles have concrete gates; the 9 not exercised are individually justified with a forward-looking trigger.

**Note on the gating mechanism itself**: this Constitution Check is the first run of L2 against real code. The gates above are non-vacuous and slice-specific — the L2 principles produced useful, testable text without requiring template editing. This is itself a positive datapoint for the constitution's design.

### Post-design re-check (after Phase 1 artifacts)

Re-evaluating after `data-model.md`, `contracts/bus-messages.md`, `contracts/cli-surfaces.md`, `quickstart.md`:

- All 12 originally-applicable principles still hold; design did not introduce violations.
- **P7 (Public-surface enumeration)** — promoted from "Partially N/A" to **Applied**. Surfaces are now enumerated in detail across the two contract documents (bus protocol with tag registry, CLI flags + structured output, configuration schema). Per-surface evolution policy is stated in each contract.
- **P8 (SemVer + Keep a Changelog per surface)** — promoted from "Partially N/A" to **Applied**. Initial `CHANGELOG.md` is part of the slice deliverable; bump policies stated per surface.
- **P16 (Failure modes are public contract)** — promoted from "Partially N/A" to **Applied**. The bus-connection lifecycle and degradation taxonomy are documented in `bus-messages.md` (Failure modes table) and the TUI's `UNAVAILABLE` rendering is specified in `cli-surfaces.md`. FR-009/FR-010 now have concrete implementation surfaces.
- All other "Partially N/A" entries (P15, P18) and "N/A" entries (P3, P14, P17, P21) remain at their pre-design status; design did not change their applicability.

**Re-check result**: PASS. The design phase strengthened the principle coverage rather than introducing complexity that would require Complexity Tracking. The constitution's gating mechanism is producing the intended effect — surfaces created by the design phase activate principles that the spec alone could not.

## Project Structure

### Documentation (this feature)

```text
specs/001-hello-fact/
├── plan.md              # This file
├── research.md          # Phase 0 — local-IPC choice, library selections, build-info pattern
├── data-model.md        # Phase 1 — domain types and their relationships
├── quickstart.md        # Phase 1 — build, run, verify
├── contracts/
│   ├── bus-messages.md  # CBOR wire schemas; Weaver tag registry (initial)
│   └── cli-surfaces.md  # CLI flag + output shape contracts
├── checklists/
│   └── requirements.md  # Spec quality checklist (already passing)
└── tasks.md             # Phase 2 output (/speckit.tasks — NOT created here)
```

### Source Code (repository root)

```text
core/
├── Cargo.toml             # both library and binary targets
├── build.rs               # vergen — git SHA, dirty bit, timestamp
└── src/
    ├── lib.rs             # public types: EntityRef, FactKey, Fact, Event, BusMessage, Provenance
    ├── main.rs            # `weaver` binary: bus listener, behavior registry, CLI dispatch
    ├── bus/               # listener, codec (CBOR), delivery classes, sequence numbers
    ├── fact_space/        # narrow `FactStore` trait + initial `HashMap`-backed impl (see research §13: ECS-library decision deferred)
    ├── behavior/          # registry, dispatcher (single mpsc consumer), one shipped behavior
    ├── trace/             # append-only log, entry types
    ├── inspect/           # bus request/response handler for FR-008
    └── cli/               # clap derive, --version, status, inspect, simulate-edit, simulate-clean

tui/
├── Cargo.toml             # depends on `core` (for shared types)
└── src/
    ├── main.rs            # `weaver-tui` binary
    ├── client.rs          # connects to core's socket; CBOR codec; subscribe + request
    ├── render.rs          # crossterm-based rendering of subscribed facts
    └── commands.rs        # in-TUI commands: simulate-edit, simulate-clean, inspect

ui/                        # untouched in this slice (Tauri; later milestone)

tests/                     # workspace-level integration tests for end-to-end scenarios
└── e2e/
    └── hello_fact.rs      # spawns core + connects a test client; exercises happy + retraction path

CHANGELOG.md               # NEW — initial entry, per L2 P8
rust-toolchain.toml        # NEW — pin stable
.gitignore                 # MODIFIED — add target/, **/*.rs.bk
Cargo.toml                 # MODIFIED at workspace root — add [workspace.package] + [workspace.dependencies]
```

**Structure Decision**: Two-binary Rust workspace (`core`, `tui`) where `core` exposes a library face for shared types so `tui` can deserialize bus messages without depending on `core`'s implementation modules. `ui` is left for a later milestone. A separate `protocol` crate is *not* introduced — per L2 P4 (simplicity in implementation), the second concrete consumer (TUI) imports types directly from `core`'s library; promotion to a standalone crate happens when a third consumer (e.g., a future agent service) materializes. Local IPC uses a Unix domain socket carrying length-prefixed CBOR frames; details in `research.md`.

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

No violations. Section intentionally empty.

---

*Plan complete. Phase 0 (research.md), Phase 1 (data-model.md, contracts/, quickstart.md), and CLAUDE.md update follow as separate artifacts.*
