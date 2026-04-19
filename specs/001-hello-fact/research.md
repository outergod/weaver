# Research: Hello, fact

Resolves library, transport, and pattern decisions referenced from `plan.md`. Each entry follows the **Decision / Rationale / Alternatives** shape required by the plan template.

## 1. Local IPC transport: core ↔ TUI

**Decision**: Unix domain socket (Linux + macOS) carrying length-prefixed CBOR frames. Socket path is configurable; default `$XDG_RUNTIME_DIR/weaver.sock` per L2 Additional Constraints (XDG base dirs).

**Rationale**:
- Simpler than TCP localhost (no port management, fewer security concerns, OS handles cleanup on socket-file removal).
- `tokio::net::UnixStream` is well-supported and async-native.
- Length-prefixed framing (4-byte big-endian length + payload) is the standard pattern for CBOR-over-stream and avoids ambiguity at the boundary.
- Local-only by design — distribution is a deferred concern (per spec Assumptions).

**Alternatives considered**:
- *TCP localhost*: would work; adds port-binding concerns (12-factor's port-binding was explicitly rejected per L2 Amendment 1 absorption). Reserved for the future when distribution lands.
- *Shared memory / `mio` raw*: too low-level for a slice with no measured perf gap.
- *gRPC / similar RPC frameworks*: overkill; would pull in code generation and a schema language, contradicting P5's "fact tuple is the canonical semantic shape" — gRPC's RPC-call shape is the wrong primitive when the bus is event/fact-driven.

## 2. CBOR library

**Decision**: `ciborium` (and `ciborium-io` for stream framing).

**Rationale**:
- Actively maintained; serde-native; pure Rust; no `unsafe` in the public path.
- Supports the tag mechanism (CBOR major type 6) cleanly, which is required for the Weaver tag scheme (entity-ref, keyword) committed in L2 P5 / arch §3.1.
- Streaming encode/decode via `ciborium::ser::into_writer` / `from_reader` works directly with `tokio::io::AsyncRead` adapters.

**Alternatives considered**:
- *`serde_cbor`*: deprecated and unmaintained.
- *`minicbor`*: smaller surface but requires its own derive macros (no serde compatibility); breaks the "serde everywhere" simplicity.

## 3. Async runtime

**Decision**: `tokio` with the `rt-multi-thread`, `net`, `sync`, `signal`, `time`, and `macros` features. Single runtime per process.

**Rationale**:
- L2 P12 commits to single-VM, single-threaded fact-space semantics. Tokio's multi-thread runtime is fine for I/O scheduling because the *fact-space mutation* path is serialized through a single `mpsc` consumer regardless of how many tasks read from sockets.
- Ecosystem dominance: `ciborium`, `tracing`, `clap`, `crossterm` all integrate cleanly with Tokio.
- `tokio::signal::unix::signal` for graceful shutdown on SIGTERM/SIGINT.

**Alternatives considered**:
- *`async-std`*: smaller ecosystem, less momentum.
- *`smol`*: minimal but trades ecosystem for size; not justified for a slice already pulling in tracing/clap.

## 4. TUI rendering

**Decision**: `crossterm` for raw terminal control. Manual rendering of a small dirty-fact list. **`ratatui` deferred** until the TUI has more than ~50 lines of render logic.

**Rationale**:
- Hello-fact's TUI shows: a connection status line, a list of currently-asserted `dirty` facts, and a command prompt. ~30 lines of render code at most.
- `ratatui` is the right answer for richer TUIs but adds a layout/widget system this slice doesn't need (P4 — no abstraction without a second concrete consumer).
- `crossterm` alone provides cursor positioning, color, raw-mode input — exactly what Hello-fact needs.

**Alternatives considered**:
- *`ratatui`*: the right answer when the TUI grows; introduce in a later slice.
- *`termion`*: less actively maintained than `crossterm`; not multi-platform.

## 5. CLI parser

**Decision**: `clap` v4 with derive macros.

**Rationale**:
- L2 P6 names `clap` derive explicitly.
- v4 derive is mature; integrates with `miette` for error rendering.

**Alternatives considered**: none — the constitution settled this.

## 6. Error types

**Decision**: `thiserror` for error type definitions; `miette` for diagnostic rendering at process boundaries.

**Rationale**:
- L2 P6 names both.
- `thiserror` for library types (no rendering opinions); `miette` for the binary entry point's `Result` -> human/structured output translation.
- `miette` natively supports both human-friendly output and `--output=json` (via its Diagnostic trait + JSON reporter).

**Alternatives considered**: `anyhow` for application-level error pile-up — not used here because we want typed errors that reference fact-space state per L2 P6.

## 7. Tracing / observability

**Decision**: `tracing` for instrumentation; `tracing-subscriber` with `EnvFilter` and a structured-formatter writing to stderr. OTel export deferred.

**Rationale**:
- L2 P13 names `tracing` explicitly. OTel is "where applicable" — not applicable at single-process Hello-fact scale.
- `EnvFilter` allows runtime log-level control via `RUST_LOG`.
- Structured formatter (`fmt::layer()` with `.json()` flag) emits parseable lines if needed for offline analysis.

**Alternatives considered**: `log` — older, less structured; doesn't compose with spans natively.

## 8. Property-based testing

**Decision**: `proptest`.

**Rationale**:
- More expressive shrinking than `quickcheck`; better Rust ergonomics.
- Used to express fact-space invariants: (a) assert/retract round-trip preserves identity, (b) provenance is non-empty on every published message, (c) per-publisher sequence numbers are strictly monotonic.

**Alternatives considered**: `quickcheck` — older, weaker shrinking.

## 9. Build-time provenance (P11)

**Decision**: `vergen` v9 in `core/build.rs`. Emits `VERGEN_GIT_SHA`, `VERGEN_GIT_DIRTY`, `VERGEN_BUILD_TIMESTAMP`, `VERGEN_CARGO_DEBUG`. Read at runtime via `env!()` macros and surfaced through `weaver --version`.

**Rationale**:
- L2 P11 requires `weaver --version` to emit crate version + git SHA + dirty bit + build timestamp + build profile. `vergen` produces all five with one build.rs.
- Pure compile-time; no runtime dependency.

**Alternatives considered**:
- *Custom `build.rs`*: works but reinvents what `vergen` already does correctly (handles dirty-tree detection across submodules; falls back gracefully outside a git checkout).

## 10. Workspace structure: shared types

**Decision**: `core` crate exposes both `lib` and `bin` targets. `tui` depends on `core` as a library dependency for shared types (`EntityRef`, `Fact`, `BusMessage`, etc.). No separate `protocol` crate.

**Rationale**:
- L2 P4 forbids abstractions without a second concrete consumer. `tui` is the second consumer of the types but a third consumer (future agent service) would justify a `protocol` extraction later.
- Lib + bin in one crate is idiomatic Rust.
- Workspace `Cargo.toml` already declares the three members; no change to membership.

**Alternatives considered**:
- *Separate `protocol` crate now*: speculative; defer until justified.
- *Duplicate types in both crates*: no — defeats serialization round-trip.

## 11. Bus protocol versioning

**Decision**: Bus protocol carries a one-byte version prefix on every connection's handshake message. Initial version: `0x01` (interpreted as v0.1.0). Mismatched versions disconnect with a structured error per L2 P16.

**Rationale**:
- L2 P7 enumerates bus protocol as a public surface; per-surface versioning travels in provenance per L2 P11.
- One-byte prefix is the smallest viable mechanism; CBOR-tagged version on every message would be wasteful.

**Alternatives considered**:
- *Per-message version*: too verbose; the connection lifetime is the natural version-change boundary.
- *No version on the wire*: punts the problem; first protocol change becomes a hostile rewrite.

## 12. Inspection capability shape (FR-008)

**Decision**: Inspection is a bus `request`/`response` pair, not a CLI subcommand. The `inspect` request carries a `FactRef` (entity + attribute); the response includes source event ID, asserting behavior ID, timestamp, and the path to the trace entry. The CLI's `weaver inspect <fact-ref>` command is a thin wrapper that connects to the bus, issues the request, and renders the response.

**Rationale**:
- L2 P5 (amended, v0.4.0): continuous machine integration is a bus-subscriber concern. Designing `inspect` as a CLI-only feature would force re-implementation for the future agent service.
- Forward-compatible by construction: any client (TUI today, agent service tomorrow) issues the same request.

**Alternatives considered**:
- *CLI-only with file-based trace dump*: rejected — couples consumers to file format and reading semantics.
- *HTTP endpoint*: rejected — outside the bus protocol; introduces a second integration surface.

## 13. Fact-space implementation: deferred ECS-library decision

**Decision**: For Hello-fact, the fact space is implemented behind a narrow `FactStore` trait (`assert / retract / subscribe / snapshot`) with an initial `HashMap<FactKey, Fact>`-backed implementation. The choice between hand-rolled archetype storage, `bevy_ecs`, `hecs`, `flecs-rs`, or a custom ECS is **explicitly deferred** to a later slice when fact families and behavior counts justify the evaluation.

**Rationale**:
- L2 P4 (simplicity in implementation): one entity, one fact family, one behavior. A HashMap-backed `FactStore` is ~50 lines. A library dependency is unjustified at this scale.
- Provenance, authority, and trace integration must wrap any storage primitive — Bevy ECS components have no native concept of provenance, single-writer authority, or trace-on-mutation. Wrapping a library's API negates much of its ergonomics.
- Behavior reload (future Steel integration) and async continuations (arch §9.4 / §9.4.1) interact poorly with Bevy's static-system-registration and synchronous-system-execution model. Locking that decision in now without measurement is premature.
- Weaver's two-tier model (small propositional **facts** vs large typed **components** per system-model §2.4) does not map cleanly onto Bevy's single-tier "everything is a component" assumption.
- Architecture §4.1 already credits Bevy ECS / Flecs as inspiration for the indexing model — the lineage is acknowledged at the architectural level; the implementation choice is engineering.

**Trait shape (informal sketch — final shape lands in `/speckit.implement`)**:

```rust
trait FactStore {
    fn assert(&mut self, fact: Fact) -> Result<TraceSequence, FactError>;
    fn retract(&mut self, key: FactKey, provenance: Provenance) -> Result<TraceSequence, FactError>;
    fn query(&self, key: &FactKey) -> Option<&Fact>;
    fn subscribe(&mut self, pattern: SubscribePattern) -> SubscriptionHandle;
    fn snapshot(&self) -> FactSpaceSnapshot;
}
```

Provenance, authority, and trace integration live above this trait — they are concerns of the dispatcher and bus layer, not of the storage primitive. Swapping the `HashMap` impl for a library-backed impl later is a localized change.

**Alternatives considered** (for the future revisit, not for this slice):

| Library | Size | Fit notes |
|---|---|---|
| **Hecs** | Tiny (~5 KLOC) | Pure ECS; no scheduler; no game-engine assumptions. Best library candidate if we go that route. |
| **Bevy ECS** (standalone via `bevy_ecs`) | Large | Most mature; pulls in scheduler conventions that conflict with Weaver's reflective-loop / async model. |
| **Flecs (Rust bindings)** | Medium | C library + Rust wrapper; rich query language; cross-language story matters less for a Rust core. |
| **Custom archetype impl** | Whatever we write | Full control over provenance/authority/trace integration; biggest engineering investment. Aligns with arch §4.1's "specialized structure optimized for the evaluation pattern" wording. |

**Revisit triggers**: fact families ≥ 5, behaviors ≥ 10, or measured perf bottleneck on the hand-rolled storage. The next significant fact-space slice is the natural decision point.

---

## Open items deferred to `/speckit.tasks` or later slices

- **Bus protocol versioning negotiation** — what does a v0.1 client see when connecting to a v0.2 core? Slice 1 only has v0.1, so this is genuinely deferred.
- **Reconnect/replay semantics** — arch §3.1 requires snapshot-plus-deltas on reconnect for authoritative messages. Hello-fact has no automatic reconnect path; the TUI's "core unavailable" handling per FR-009/FR-010 is degradation, not reconnect. Reconnect engages in a future slice when long-running sessions matter.
- **Steel integration** — explicitly out of scope; future slice.
- **`ratatui` adoption** — when render code exceeds ~50 lines.
- **OTel export** — when the project moves beyond single-machine.
