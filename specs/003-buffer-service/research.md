# Research — Slice 003 (Buffer Service)

Phase 0 decisions. Each entry resolves an implementation-level question that the plan depended on. Rationale + alternatives preserved so post-slice reviewers understand *why*, not just *what*.

## 1. Content-digest library

**Decision**: `sha2` crate (`Sha256`), pinned to minor version. Used to hash both the in-memory buffer content and the on-disk content on each poll tick; compare digests to decide whether `buffer/dirty` transitioned.

**Rationale**:

- `buffer/dirty` must mean *memory bytes ≠ disk bytes* (FR-002b). A byte-for-byte comparison is correct but requires holding both payloads in memory simultaneously; a digest-based comparison needs only the digests after the read completes and lets the on-disk read stream through a hasher without an intermediate `Vec<u8>`.
- `sha2` is widely audited, MIT/Apache-2.0 licensed (AGPL-compatible per L2 Amendment 4), and already transitively present via other RustCrypto crates in the workspace dependency tree. Adopting it direct costs no new license review.
- Digest cost is immaterial at slice-003 file sizes (typical source files ≤ 1 MiB; SHA-256 runs at ~500 MiB/s on commodity hardware, so per-poll cost is bounded by disk read, not hashing).
- Collision concerns are non-material for this use: two distinct byte strings hashing identically would be a cryptographic break, which is a far worse problem than a stale `buffer/dirty` flag.

**Alternatives considered**:

- **`blake3`** — faster, but another crate in the workspace dep tree. No speed benefit at the file sizes we operate on.
- **`xxhash-rust` / `fxhash`** — fast non-cryptographic hashes. Rejected: collision probability under operator-facing input is non-zero; a collision silently masks a real edit. Engineering-honest hashing uses cryptographic digests even when speed isn't the bottleneck.
- **Byte-for-byte `Vec<u8>` comparison** — rejected: holds 2× file size in memory on every poll; scales poorly with multi-MB files; unnecessary given digests let us hold the authoritative copy once.
- **mtime + size heuristic** — explicitly rejected in the spec's assumptions (same-size edits within mtime resolution can slip past); digest is required for correctness of SC-302.

## 2. Observation strategy (what happens in each poll tick)

**Decision**: per poll tick, for each open buffer, the service (a) opens the file on-disk fresh, (b) streams its content through a SHA-256 hasher to produce a *disk digest*, (c) compares to the *memory digest* cached since open/last-mutation, (d) if they differ, flips `buffer/dirty=true`; if they agree and the buffer was previously dirty, flips it back to `false`. The service does NOT re-read the file into memory on each tick — the memory content is established at open time and only mutated in slice 004+.

**Rationale**:

- The *memory digest* is cheap: computed once at open (full content load), then cached. No re-hashing of memory on every tick.
- The *disk digest* is the per-tick cost. For typical source files this is well under 1 ms of CPU, plus filesystem read time. Well inside the 500 ms external-mutation latency budget (SC-302).
- This strategy is content-accurate (correctness first) and forward-compatible with slice 004: when in-memory content mutates, the service updates the memory digest at write time, and the poll loop's logic is unchanged.
- "Degraded" reading (permission denied, deleted mid-read, I/O error) is handled per-buffer: the service flips `buffer/observable=false` for that specific entity, edge-triggered. Other buffers' ticks proceed unaffected.

**Alternatives considered**:

- **`notify` crate / `inotify` / `fsevents`** — true filesystem-event-driven observation. Rejected for slice 003: consistent with slice 002's polling decision; introduces platform-specific code and event-queue ordering concerns; polling is simpler and within budget. Revisit in a dedicated observability slice alongside git-watcher.
- **Read full content on each tick** — rejected: unbounded memory churn under large files; no benefit over digest comparison.
- **Hash-on-write + watch mtime** — possible optimization (skip digest if mtime unchanged). Rejected for slice 003: premature optimization without measurement; correctness proof is simpler with unconditional digest; mtime-based fast-path can land later if SC-302 becomes tight at scale.

## 3. Poll cadence default

**Decision**: 250 ms, exposed via `--poll-interval` (humantime duration parsing). Matches slice 002's git-watcher default exactly.

**Rationale**:

- Keeps SC-302 (external-mutation ≤ 500 ms operator-perceived) comfortably in budget: worst-case poll-to-publish-to-render round-trip is bounded by 250 ms (poll interval) + 100 ms (interactive bus latency) + render; leaves headroom.
- Consistency across the two watcher-style services means operators have one number to reason about, not two.
- `--poll-interval=0ms` rejected at parse time (mirrors slice 002 F33 follow-up) to prevent `tokio::time::interval` panic.

## 4. `FactValue::U64` landing

**Decision**: Add a `U64(u64)` variant to `FactValue` as part of slice 003's bus-protocol MAJOR bump (v0.2 → v0.3). The variant is additive at the enum level; the wire form reuses the existing adjacent-tagging convention (`#[serde(tag = "type", content = "value", rename_all = "kebab-case")]`) producing `{"type":"u64","value":<number>}`.

**Rationale**:

- `buffer/byte-size` has natural type `u64` (file sizes can legitimately exceed `u32::MAX` on modern systems). Encoding as `FactValue::String` decimal was considered and rejected in the spec assumptions.
- Additive enum variants under adjacent-tagged encoding are forward-compatible *in principle*, but CBOR/serde's default behavior on unknown variants is *error*, not *skip-and-log*. Slice 002 already documented this as a compatibility constraint in `contracts/bus-messages.md`: subscribers MUST rebuild for MAJOR protocol bumps. Slice 003 inherits that constraint; the MAJOR bump is the cleanly-documented channel.
- Landing `FactValue::U64` on a MINOR bump *would* be acceptable under L2 P7/P8 if the protocol weren't already breaking on the `EventPayload` vocabulary change. Since it is, the variant rides the MAJOR for free.

**Alternatives considered**:

- **Encode byte-size as decimal `FactValue::String`** — rejected: loses type safety, invites subscriber-side parsing bugs, inconsistent with the fact-value-is-a-typed-value architecture.
- **Introduce a new fact-value kind specifically for byte counts** — over-designed; `u64` is the primitive and is reusable.

## 5. E2e fixture approach

**Decision**: Use the `tempfile` crate (already dev-transitive via `proptest`) to create temporary directories and files per e2e test. Each test owns its `TempDir`; fixture lifetime is bound to the test's `ChildGuard` scope (inherited pattern from slice 002). Paths pass through a `tempdir.path().join("<name>")` and get canonicalized by the service at startup.

**Rationale**:

- `tempfile::TempDir` provides RAII cleanup even on panic, matching the slice 002 pattern where `ChildGuard` owns the service process and the test owns the fixture dir.
- No need for a dedicated fixture harness this slice; the existing `tests/e2e/` structure scales from three-process to four-process by adding one more `ChildGuard`.
- Tests can mutate file content mid-test (for SC-302) by re-writing through `std::fs::write(tempdir.path().join("file"), new_content)`; the buffer service's polling observer picks up the change.

**Alternatives considered**:

- **Shared fixture directory under `target/`** — rejected: concurrent `cargo test` runs would collide; cleanup is manual; tempfile's per-test isolation is strictly better.
- **In-repo fixture files** — rejected: tests need to mutate content mid-test, which pollutes the repo.

## 6. New crate lint/format stance

**Decision**: `buffers/` crate adopts the workspace-level clippy/format gate with no per-crate deviation. The crate's `Cargo.toml` does NOT disable any lints; the workspace root's `[workspace.lints]` (if present) applies; `scripts/ci.sh` runs `cargo clippy --all-targets --workspace -- -D warnings` and includes the new crate automatically.

**Rationale**:

- L2 Amendment 6 (Code quality gates) binds every crate to the workspace-level floor. New crates inherit discipline by default.
- No slice-local rationale to add pedantic/nursery lints — the slice is the first buffer-service crate; introducing stricter-than-floor lints here would diverge from the git-watcher crate's stance with no compensating benefit.

## 7. Service identity vocabulary

**Decision**: `service_id = "weaver-buffers"` (kebab-case per Amendment 5). Instance identifier is a random UUID v4 per invocation, mirroring slice 002.

**Rationale**:

- Matches the binary name; operators reading traces or `weaver inspect` output see the same token in both places.
- UUID v4 stays consistent with slice 002's Clarification Q3 rationale (opacity by design; temporal ordering comes from `timestamp_ns`, not identifier structure).

## 8. Bootstrap causal-parent shape for N buffers

**Decision**: Each buffer's four bootstrap facts (`buffer/path`, `buffer/byte-size`, `buffer/dirty=false`, `buffer/observable=true`) share a *per-buffer* synthesized `bootstrap-tick` event id as `causal_parent`. Different buffers use different synthesized event ids. The service-level `watcher/status=ready` assertion fires once after all N buffers complete and carries `causal_parent = None` (lifecycle is the originating signal).

**Rationale**:

- Per Clarification 2026-04-23, service-level lifecycle is orthogonal to per-buffer bootstrap. Sharing a causal parent *across* buffers would blur the line: a `why?` walk from buffer B's facts would lead to buffer A's bootstrap, which is not the causal truth.
- Per-buffer shared parent matches slice 002's per-repo bootstrap pattern, preserving the "transaction per observation target" shape.
- Service-level `Ready` with no causal parent follows the slice 002 convention for lifecycle transitions (the lifecycle event is the origin; nothing causes it).

## 9. CLI error-classification stance (slice-002 F31 follow-up)

**Decision**: Slice 003 does NOT attempt to fix slice-002 F31 (reclassifying `identity-drift` / `invalid-identity` as fatal in the reader loop). The `weaver-buffers` reader loop inherits the *same* behavior slice 002 shipped (exit 10 on those categories), so the inconsistency is uniform across services rather than service-specific.

**Rationale**:

- Closing F31 is a cross-service cleanup best done in a dedicated soundness slice that touches both `git-watcher/src/publisher.rs::reader_loop` and `buffers/src/publisher.rs::reader_loop` in one pass, plus updates `contracts/cli-surfaces.md` §Exit codes and the README exit-code tables.
- Doing it in slice 003 would widen scope without advancing the slice's goal (stand up the buffer service), and the fix would land in isolation rather than alongside its natural companion work.
- Keeping the behavior symmetric across services means review-finding triage stays coherent: any reviewer noticing the issue in slice 003 is noticing the same issue as in slice 002, and the fix batches cleanly.

## 10. Performance headroom for multi-buffer polling

**Decision**: No explicit cap on per-poll work. Each poll tick iterates over the N open buffers sequentially, hashing each in turn; total per-tick cost scales linearly with N. No per-buffer parallelism in slice 003.

**Rationale**:

- Per Clarification 2026-04-23, slice 003 makes no scalability commitment. Sequential iteration is the simplest implementation that ships FR-007's N>1 proof.
- Tokio's single-threaded executor sufficies: the publisher task's wall-clock time per poll is the sum of per-buffer I/O + digest, which for typical N ≤ 10 and typical files remains well within the 250 ms cadence.
- If SC-302 becomes tight in practice (e.g., N=50 with large files), a later slice can introduce bounded parallelism or adaptive cadence. No premature optimization.

**Alternatives considered**:

- **`tokio::task::spawn` per buffer per tick** — rejected: overhead dominates at small N; introduces concurrency management complexity; the buffer service has no multi-task invariants to enforce at slice 003 scope.
- **Poll cadence that scales with N** — rejected: introduces observer-visible cadence variation that complicates SC-302 reasoning.

---

*All NEEDS CLARIFICATION markers from the plan's Technical Context have been resolved. No items deferred to Phase 1.*
