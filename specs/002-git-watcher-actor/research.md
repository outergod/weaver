# Research: Git-Watcher Actor

Resolves library, cadence, and dependency decisions referenced from `plan.md`. Each entry follows the **Decision / Rationale / Alternatives** shape required by the plan template.

## 1. Git observation library

**Decision**: `gix` (gitoxide — pure-Rust git), pinned to a current minor version with transitive deps pinned via `Cargo.lock`. Used through a narrow `observer` module in `git-watcher/` that exposes only the operations the slice needs (HEAD kind, current branch name, head-commit SHA, dirty check).

**Rationale**:
- **License cleanliness under AGPL-3.0-or-later.** `gix` and its core transitive deps are Apache-2.0 OR MIT. Both are explicitly listed as acceptable in L2 Additional Constraints (License clause). No C-library linking-exception review surface.
- **Build simplicity.** Pure Rust. No C compiler, no vendored libgit2. Nix flake stays lean; CI cache behaviour is well-understood.
- **Correct cost model for polling.** In-process reads are <1 ms per operation. At the chosen 250 ms cadence (§2 below) and ~3–4 ops per poll, CPU cost is negligible (~16 ops/sec × <1 ms ≈ <1 % of one core). Shell-out to `git` would pay ~20–50 ms of process-spawn per command and scale badly when multi-repo watching eventually lands.
- **API surface covers the slice's needs without gaps.** Specifically:
  - `gix::open` — opens the repository once; the handle amortizes across polls.
  - `repo.head()` returns a `Head` whose `kind` enum variants (`Symbolic { name, .. }`, `Detached { .. }`, `Unborn { .. }`) align directly with the `repo/state/*` discriminated-union spec (Clarification Q4).
  - `repo.status()` exposes index-vs-HEAD and worktree-vs-HEAD queries; a configuration that excludes untracked files matches the Q5 dirty definition exactly.
  - `repo.rev_parse_single("HEAD")` resolves the head commit for `repo/head-commit`.
- **External validation.** Cargo itself is migrating read-heavy paths from `git2` to `gix`. For the narrow set of operations this slice uses, production-readiness is established.

**Alternatives considered**:
- *`git2` (libgit2 FFI)*: mature and broad, but carries a C build dependency (vendored libgit2) that increases first-build time, CI-cache churn, and cross-platform build complexity. The libgit2 licence is GPL-2.0-with-linking-exception — probably compatible with AGPL-3.0-or-later, but adds a review surface we can skip entirely by picking `gix`. Rejected for this slice; revisit only if `gix` is discovered to have a coverage gap for an operation we genuinely need.
- *Shell out to `git` CLI*: simplest code (e.g., `Command::new("git").arg("diff").arg("HEAD").arg("--quiet")`) and inherits the user's install, but process-spawn overhead at polling cadence is wasteful (~400–800 ms/sec at 4 polls/sec vs. negligible for in-process). Also parses `git`'s plumbing output, which is stable but still a parsing surface for bugs to hide in. Rejected; keep as a mental fallback if `gix` proves inadequate during implementation.

**Licence note (confirmed at implementation time)**: spot-check transitive deps (`gix-hash`, `gix-object`, `gix-traverse`, etc.) before merge to confirm none of them pull in a licence incompatible with AGPL-3.0-or-later. No known exceptions expected; the `gix` workspace is uniform.

## 2. Polling cadence

**Decision**: **250 ms default**, exposed as `--poll-interval <duration>` CLI flag on `weaver-git-watcher` (parses via `humantime`-style or `Duration::from_millis` input).

**Rationale**:

End-to-end latency budget decomposition for SC-002 (≤ 500 ms operator-perceived):

| Step | Expected cost |
|---|---|
| Poll interval (worst case: mutation occurs immediately after a poll fired) | X ms |
| `gix` observation ops | 1–5 ms |
| Bus round-trip (watcher → core → subscriber) | 5–10 ms |
| TUI re-render (crossterm) | ~5 ms |

- **X = 100 ms**: worst case ~120 ms; comfortable. CPU ~10 polls/sec × ~2 ms = <2 % of one core — negligible.
- **X = 250 ms**: worst case ~270 ms; well under 500 ms budget. CPU ~4 polls/sec × ~2 ms = <1 % of one core. **Sweet spot.**
- **X = 500 ms**: worst case ~520 ms — **exceeds** SC-002's budget. Rejected as a default.

250 ms is the smallest default that leaves meaningful SC-002 headroom without wasting CPU on idle repositories. Cranking tighter is tunable via the CLI flag (e.g., 50 ms during interactive debugging); relaxing (e.g., 1 s on battery) is also tunable.

**Rationale for exposing the flag**:
- Makes the tradeoff observable — operators who care about tightness know where the knob is.
- Prevents accidental coupling — other code paths can't "just know" the poll interval; the watcher publishes its chosen interval in `watcher/config` provenance (or at minimum logs it on startup).
- Supports the future efficient-observation story: when inotify/kqueue lands in a later slice, the poll interval becomes a backstop rather than the primary trigger; the flag's semantics evolve cleanly ("max interval between checks" rather than "time between checks").

**Alternatives considered**:
- *Hard-code to 250 ms with no override*: tuning during debugging or on constrained hardware becomes a recompile.
- *Default to 100 ms*: faster worst-case latency but 2–4× the idle CPU cost for no user-visible benefit at typical operator pace.
- *Event-driven via `notify` (inotify/kqueue)*: explicitly out of scope per spec Assumptions. Efficient observation is a follow-up slice; adding it now would broaden the slice for no pivot-assertion value.

## 3. `uuid` crate — version and feature flag

**Decision**: `uuid = { version = "1", default-features = false, features = ["v4"] }` in `git-watcher/Cargo.toml`.

**Rationale**:
- Stable since `1.0` (2022). No pinning concerns beyond the workspace-wide `Cargo.lock` commitment (L2 P19).
- `v4` is the only feature needed (per Clarification Q3: random UUIDs, not timestamp-ordered). `default-features = false` drops unneeded features (e.g., `std` formatting pieces we can re-enable if the compiler complains; leave defaults if simpler).
- License: Apache-2.0 OR MIT. Clean for AGPL-3.0-or-later.
- `v4` feature pulls in `getrandom` transitively; `getrandom` is already in the workspace via `rand_core` (from `proptest`), so no meaningful new dependency weight.

**Alternatives considered**:
- *Hand-roll random identifiers via `rand` + `base64`*: reinvention for no benefit; `uuid` is the standard type for this shape and rendering.
- *v7 (timestamp-ordered)*: rejected in Q3 clarification for principled reasons (avoids baking implicit temporal ordering into an identifier; `Provenance.timestamp_ns` already carries time authoritatively).

## 4. Bus protocol version negotiation

**Decision**: Bump `Hello.protocol_version` from `0x01` to `0x02`. Core rejects `0x01` connections with a structured `Error { category: "version-mismatch", detail: "..." }` and closes. No parallel-version support.

**Rationale**:
- Per L2 P7/P8: bus protocol is a public surface; a wire-incompatible change is MAJOR. The provenance shape changes (opaque `SourceId::External(String)` → structured `ActorIdentity` with a new CBOR tag), so any old client will mis-deserialize.
- Per spec Clarification Q2: no parallel-support / no deprecation shim. The only in-tree clients (TUI, e2e test harness, and new `git-watcher`) are rebuilt together.
- `0x02` at the wire is the smallest legal step; the protocol surface itself gets a `0.2.0` entry in `CHANGELOG.md`.

**Alternatives considered**:
- *Keep `0x01` and let unknown-variant tolerance cover the new fields*: rejected — the provenance shape is a **struct** change, not a sum-type variant addition. Existing clients decoding `provenance.source` will hit a CBOR tag they don't know (tag 1002 for structured identity) and fail in the middle of a message, not at a variant boundary.
- *Protocol version per-surface negotiation via handshake ext*: overengineered for a slice that rebuilds all clients in-tree.

## 5. Watcher lifecycle integration with existing `LifecycleSignal`

**Decision**: Extend `LifecycleSignal` with `Degraded`, `Unavailable`, `Restarting` variants so the watcher's lifecycle transitions land in the same channel as the core's. The core emits `Started`/`Ready`/`Stopped` today; `Degraded`/`Unavailable`/`Restarting` become observable states for services.

**Rationale**:
- `docs/05-protocols.md §5` already names the six states (`started`, `ready`, `degraded`, `unavailable`, `restarting`, `stopped`). Slice 001 only implemented three (`started`, `ready`, `stopped`) because only the core had a lifecycle. Adding the other three now keeps the enum honest — existing core code doesn't emit them, but the watcher will.
- Additive enum change on `LifecycleSignal` under the MAJOR bus-protocol bump. Safe under CBOR's unknown-variant semantics for *future* clients, but since we're bumping MAJOR anyway, clean-slate is fine.
- Keeps P16 (Failure modes are public contract) self-consistent: services use one vocabulary, not a parallel taxonomy per service.

**Alternatives considered**:
- *Service-scoped custom lifecycle messages*: fragments the degradation vocabulary per service; agents and operators would need to learn each one.
- *Model lifecycle as facts only*: possible (`watcher/status <lifecycle>` is already a fact per the spec) but duplicates the signalling surface — emit both the fact and the `Lifecycle` message, so subscribers choosing either surface see the same transitions.

## 6. E2E test harness for three-process scenarios

**Decision**: Extend `tests/e2e/`'s existing `ChildGuard` pattern to spawn a third process (`weaver-git-watcher`) alongside the core. Each test creates a temporary directory, runs `git init` + minimal setup via `std::process::Command` (or `gix`), then launches the watcher against it. No new test-harness primitive is introduced.

**Rationale**:
- Slice 001 committed to `ChildGuard` ownership as the pattern; extending it from {core, client} to {core, watcher, client} is straightforward.
- Temporary repositories are cheap and isolated (`tempfile::TempDir`); each test builds exactly the repository state it wants to observe.
- Per-test git state construction (via `gix` or `std::process::Command`) keeps tests deterministic — no shared fixtures that drift.

**Alternatives considered**:
- *Shared test fixture repositories on disk*: faster per-test but introduces shared mutable state across tests. Rejected for flakiness risk.
- *Mock the git-observation layer*: would let tests avoid real git setup but moves the testing boundary above `gix`, losing coverage of the actual observation path. Rejected — the watcher's value in this slice is precisely its real-repository observation.

## 7. `git-watcher` crate placement

**Decision**: New top-level workspace member `git-watcher/` alongside `core/`, `ui/`, `tui/`. Binary target: `weaver-git-watcher`.

**Rationale**:
- Matches the existing flat workspace convention; no new hierarchy.
- Positions the watcher as a service on the bus (arch §2), a peer of core/ui/tui, not a subordinate "tool."
- Crate name `git-watcher` is specific, not generic; binary name `weaver-git-watcher` matches (with the `weaver-` prefix aligning with `weaver-tui`).
- If Mercurial / Jujutsu support ever arrives, factoring into `vcs-watcher-core/` + `git-watcher/` + `hg-watcher/` is a refactor at *that* moment, per L2 P4 (no abstraction without a second concrete consumer). The `repo/*` fact-family namespace observation (noted in the plan) remains the same call.

**Alternatives considered**:
- *Generic `watcher/` crate*: too abstract for a single concrete VCS today; over-promises on what it watches.
- *Nesting under `tools/` or `services/`*: premature hierarchy; no second peer to justify grouping.
- *Vendored as a module inside `core/`*: violates the L1 §1/§9 "independent services" principle and blurs the bus-client vs. core-internal boundary.

---

## Open items deferred to `/speckit.tasks` or later slices

- **Watcher authorization / authentication.** Any process that can connect to the bus today can claim service identity. Acceptable while single-user / localhost; becomes material at the first multi-user or remote-service slice.
- **Efficient repository observation (inotify/kqueue/FSEvents).** Deferred per spec Assumptions; polling satisfies SC-002 with headroom.
- **Authority-rejection mechanism for dual-watcher scenarios.** FR-009 requires the second watcher instance to fail to claim authority over the same repository's fact family. Exact mechanism (core-side single-writer check on `FactAssert`, or handshake-level authority claim) is a plan-phase-2 decision during `/speckit.tasks`.
- **Identity stability across sessions.** The watcher's UUID v4 is per-invocation (Clarification Q3). Cross-session identity persistence remains open (open-questions §25 remaining sub-question), becomes material at the agent-delegation slice.
- **Components as the structural home for `repo/state/*`.** Tracked in open-questions §26 with explicit revisit triggers. Not revisited this slice.
