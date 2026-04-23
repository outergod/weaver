# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/).
Per-public-surface versioning is per L2 constitution Principle 8 (`.specify/memory/constitution.md`).

## Public surfaces tracked

Per L2 Principle 7, each public surface carries its own version.

- **Bus protocol** v0.2.0 (was v0.1.0) — message categories, delivery classes (lossy / authoritative), CBOR tag scheme entries 1000 (entity-ref), 1001 (keyword), **1002 (structured actor identity, slice 002)**. See `specs/002-git-watcher-actor/contracts/bus-messages.md`.
- **Fact-family schema `buffer/dirty`** v0.1.0 — unchanged since slice 001.
- **Fact-family schema `repo/dirty`** v0.1.0 (slice 002, new) — `FactValue::Bool`; asserted by `weaver-git-watcher` per Clarification Q5 (index-or-working-tree differs from HEAD; untracked-only is clean). See `specs/002-git-watcher-actor/data-model.md`.
- **Fact-family schema `repo/head-commit`** v0.1.0 (slice 002, new) — `FactValue::String` (40-char hex SHA). Retracted in the `Unborn` state.
- **Fact-family schema `repo/state/on-branch`** v0.1.0 (slice 002, new) — `FactValue::String` (branch name). Asserted iff HEAD points at `refs/heads/<name>`.
- **Fact-family schema `repo/state/detached`** v0.1.0 (slice 002, new) — `FactValue::String` (detached HEAD commit SHA).
- **Fact-family schema `repo/state/unborn`** v0.1.0 (slice 002, new) — `FactValue::String` (intended branch name for an empty repository).
- **Fact-family schema `repo/observable`** v0.1.0 (slice 002, new) — `FactValue::Bool`. `false` during watcher-`Degraded`; flips `true` on recovery. Suppresses dirty rendering in the TUI when `false` per `contracts/cli-surfaces.md`.
- **Fact-family schema `repo/path`** v0.1.0 (slice 002, new) — `FactValue::String` (canonical working-tree root). The three `repo/state/*` attributes obey a mutex invariant: at most one asserted per repository entity at any trace prefix (`docs/07-open-questions.md §26`).
- **Fact-family schema `watcher/status`** v0.1.0 (slice 002, new) — `FactValue::String` mirroring `LifecycleSignal` (`started` / `ready` / `degraded` / …). Keyed by the watcher's per-invocation instance-UUID entity, not the repository.
- **CLI surface** v0.1.0 — shape unchanged; `weaver inspect --output=json` gains an always-present `asserting_kind` discriminator (MINOR additive per cli-surfaces.md §wire compatibility). Slice 002 also adds the new `weaver-git-watcher` binary — its own versioning tracks the crate's `Cargo.toml` (`0.1.0`).
- **Configuration schema** v0.1.0 — unchanged.

## [Unreleased] — slice 002 Phase 2 — Foundational (ActorIdentity migration)

**Breaking bus-protocol change** — version bumps `0.1.0 → 0.2.0`. Slice 001 clients cannot connect to a v0.2.0 core; all in-tree bus clients (core, TUI, CLI, e2e test harness, test client) rebuild together.

### Changed — bus protocol (MAJOR)

- **Provenance `source` shape** changed from opaque `SourceId::External(String)` to structured `ActorIdentity` — one closed enum per actor kind in `docs/01-system-model.md §6`. Variants: `Core`, `Behavior { id }`, `Tui`, `Service { service-id, instance-id }`, `User { id }`, `Host { host-id, hosted-origin }`, `Agent { agent-id, on-behalf-of }`. Wire shape: internally-tagged CBOR/JSON with kebab-case `type` discriminator and kebab-case field names. Closes `docs/07-open-questions.md §25` sub-questions: *shape* (single closed enum) and *migration* (replace, not extend). See `specs/002-git-watcher-actor/` Clarifications Q1, Q2.
- **New CBOR tag 1002** reserved for structured actor identity (adjacent to the slice-001 tags 1000 and 1001).
- **`LifecycleSignal`** extended with `Degraded`, `Unavailable`, `Restarting` variants per `docs/05-protocols.md §5`. Slice-001 core continues to emit only `Started` / `Ready` / `Stopped`; the richer states are intended for services that can degrade without exiting.
- **`Hello.protocol_version`** advances `0x01 → 0x02`. Mismatched clients receive `Error { category: "version-mismatch", ... }` and connection close (unchanged handshake logic; bumped constant).

### Added — core

- `ActorIdentity::service(service_id, instance_id)` constructor with kebab-case validation (L2 Amendment 5); rejects empty identifiers and identifiers containing uppercase, underscores, whitespace, leading/trailing/consecutive hyphens.
- `ActorIdentity::behavior(id)` / `ActorIdentity::user(id)` convenience constructors.
- `UserId`, `HostedOrigin` placeholder types — reserved for forward-compat, not emitted this slice.
- `kind_label()` method on `ActorIdentity` for diagnostic rendering.
- `uuid` workspace dependency (`v4` feature) per Clarification Q3.

### Added — watcher crate scaffold

- New workspace member `git-watcher/` — produces the `weaver-git-watcher` binary. Phase 1 scaffold only: CLI prints a Phase-1 marker and exits. Real implementation lands in Phase 3 (US1).
- Workspace deps: `gix = "0.66"` (pure-Rust git; research §1), `humantime = "2"` (for `--poll-interval` in Phase 3).

### Added — Phase 3: `weaver-git-watcher` end-to-end (US1)

- **Observer**: `RepoObserver` opens a repository via `gix::discover`, keys the watcher by the **discovered working-tree root** (never the user-typed input path — prevents two watchers on different subpaths from bypassing the authority mutex). Bare repositories are rejected at `open()` with a dedicated `BareRepositoryUnsupported` variant; in-progress transient operations (rebase / merge / cherry-pick / revert / bisect) surface as `UnsupportedTransientState` so the watcher flips to `Degraded` rather than misreporting branch state. Symbolic HEAD outside `refs/heads/` (e.g. pointing at a tag) surfaces as `UnsupportedHeadShape`. HEAD-resolve failures on an `OnBranch` / `Detached` state propagate as `ObserverError::Observation`. Dirty check uses `git diff HEAD --quiet` via shell-out (documented deviation from research §1); SHA resolution uses `gix`.
- **Publisher**: one `weaver-git-watcher` process asserts authority over `repo/*` for a single repo entity via an `ActorIdentity::Service` with a fresh UUID v4 per invocation (Clarification Q3). The publisher splits its bus stream post-handshake so a reader task can surface server-sent `Error` frames (`authority-conflict`, `identity-drift`, `not-owner`, `invalid-identity`) to the main loop, exiting with the documented code path (`2` bus-unavailable, `3` authority-conflict, `10` internal). Degraded-state emission is **edge-triggered**: the `Lifecycle(Degraded)` + `repo/observable=false` pair fires only on the healthy→degraded transition, not every failed poll.
- **Authority-conflict mechanism** (core): new `AuthorityMap` + `ServicePublishOutcome` + `ServiceRetractOutcome` in `core/src/behavior/dispatcher.rs`. Claims are **conn-keyed** (identity alone is client-forgeable on the wire) and a connection binds its `ActorIdentity` on first publish — any subsequent publish under a different identity returns `ServicePublishOutcome::IdentityDrift`, surfaced over the bus as `Error { category: "identity-drift" }`. Retract attribution is synthesized server-side (client-supplied `source` and `timestamp_ns` are ignored; only `causal_parent` survives as a correlation hint).
- **Connection-owned fact tracking**: every service-asserted fact is recorded against its owning connection; `release_connection` retracts everything the connection published when the stream closes, so SIGKILL of a watcher leaves no stale `repo/*` facts in the store.
- **CLI surface (new binary)** — `weaver-git-watcher <REPOSITORY-PATH> [--poll-interval=250ms] [--socket=<path>] [--output=json|human] [-v/-vv/-vvv] [--version]`. `--socket` folds `WEAVER_SOCKET` env var (parity with `weaver`). `--output=json` switches both `--version` rendering AND runtime tracing to JSON. `--poll-interval=0ms` is rejected at parse time (would panic `tokio::time::interval`). Documented exit codes: 0 clean, 1 startup failure (including bootstrap `observe()` errors), 2 bus unavailable, 3 authority conflict, 10 internal.

### Added — Phase 3: TUI Repositories section

- `tui/src/render.rs` renders a dedicated **Repositories** section below the existing Facts section. State badges: `[on <name>]`, `[detached <sha>]`, `[unborn <name>]`, or `[state unknown]`. The `[observability lost]` badge replaces the dirty indicator when `repo/observable = false`; `[stale]` is appended per-row when the TUI loses its core subscription. Authoring-actor line reuses the shared `annotation` helper to render `by service <id> (inst <short-uuid>), event <id>, <t>s ago`. Facts and Repositories sections both order facts deterministically by `(entity, attribute)` so `[i]nspect` always targets the visually-first fact.

### Added — Phase 4: structured-identity inspection

- `InspectionDetail` gains an always-present `asserting_kind: String` discriminator — `"behavior" | "service" | "core" | "tui" | "user" | "host" | "agent"` (see `ActorIdentity::kind_label`). Identifier fields are populated only for the slice's emitted kinds (`behavior`, `service`). Core / Tui / reserved variants carry the kind alone. Additive per cli-surfaces.md §wire compatibility.
- Backward-compatible deserialization via `InspectionDetailRepr` + `#[serde(from = ...)]`: mixed-version deployments continue to work — a new client decoding a pre-slice response infers `asserting_kind` from the populated identifier fields (`behavior` / `service` / fallback `core`).
- Inspection routes through the **live fact's provenance**, not the `TraceStore::fact_inspection` index, so a service overwriting a behavior-authored fact is now attributed correctly (the behavior index isn't cleared on overwrite — only on retraction — and the inspect handler no longer relies on it for authoritative attribution).

### Added — Phase 4: wire-edge identity validation

- `ActorIdentity::validate()` is the **single gate for wire-derived provenance**. Called from `Provenance::new` (in-process safety) and listener-side for both `BusMessage::FactAssert` and `BusMessage::Event`. Rejects empty `service-id`, `behavior-id`, `user-id`, `host-id`, `hosted-origin.{file,runtime-version}`, `agent-id`; recursively validates `Agent.on_behalf_of`. `Service` identifiers additionally must be kebab-case (Amendment 5). Malformed wire frames receive `Error { category: "invalid-identity" }`. Non-`Service` provenance on `FactAssert` is rejected with `Error { category: "unauthorized" }` (behaviors publish in-core; only services publish over the bus).

### Changed — bus dispatcher

- Lock-order across the dispatcher standardized: `publish_from_service` and `retract_from_service` now both acquire `fact_store` before `conn_facts`; the retract path releases the `conn_facts` guard before awaiting `fact_store.lock()` (the inverse held prior and admitted a deadlock under concurrent publish + non-owner retract traffic).
- `listener.rs::handle_connection` funnels every post-handshake exit through a single `dispatcher.release_connection(conn_id)` call — a forwarding-write failure on a publisher-subscriber connection no longer leaks authority claims or conn-owned facts.

### Fixed — miscellaneous polish

- `weaver-git-watcher --version` honours `--output=json|human` per the CLI contract; three binaries (`weaver`, `weaver-git-watcher`, `weaver-tui`) all report `bus_protocol: "0.2.0"` from the same constant.
- TUI `short_sha` truncation is UTF-8 safe (char-iterator-based); the repo-view representative fact uses the freshest `asserted_at_wall_ns` so the rendered age reflects the watcher's most recent publication, not a startup-only one.

### Added — Phase 3 & 4 test coverage

- `git-watcher/tests/mutex_invariant.rs` (T060) — property test over 1–20-observation random sequences proves the discriminated-union `repo/state/*` mutex invariant holds at every trace prefix.
- `git-watcher/tests/transition_causal.rs` (T061) — six scenario tests exhausting the variant-pair matrix; retract and assert of every transition share a `causal_parent` EventId equal to the triggering poll tick.
- `core/tests/inspect/behavior_authored.rs` (T065), `tests/e2e/git_watcher_inspect.rs` (T066), `core/tests/inspect/structured_always.rs` (T067), `core/tests/inspect/causal_walkback.rs` (T067a), `core/tests/property/inspect_identity.rs` (T068) — Phase-4 inspection coverage: CLI-level attribution for behavior- and service-authored facts, structured-always invariant across fact families, multi-hop causal-chain identity check, round-trip property for every emitted kind.
- `tests/e2e/{git_watcher, git_watcher_sigkill, git_watcher_authority_conflict, fact_assert_identity_guard}.rs` — end-to-end three-process coverage.

## [0.1.0] — 2026-04-20 — slice 001 "Hello, fact"

Initial public release. Ships the end-to-end skeleton: core + TUI +
one embedded behavior + bus + fact space + trace + inspection + CLI,
together validating L2 P1/P2/P4/P5/P6/P9/P10/P11/P12/P13/P19/P20.

All four public surfaces (bus protocol, `buffer/dirty` fact schema,
CLI, configuration) debut at their initial versions. Spec success
criteria SC-001 through SC-006 are met and covered by automated
tests (54 unit + 13 integration + 2 e2e).

### Added — slice 001 "Hello, fact"

Entries are organised by phase of `specs/001-hello-fact/tasks.md`.

#### Phase 1 — Setup

- Workspace `Cargo.toml` with `[workspace.package]` (edition 2024, rust-version 1.85, license `AGPL-3.0-or-later` matching `LICENSE`) and `[workspace.dependencies]` for tokio, serde + serde_json, ciborium, clap, miette + thiserror, tracing, proptest, vergen, crossterm. The initial scaffold incorrectly defaulted the license to `MIT OR Apache-2.0` (Rust-ecosystem default); aligned to AGPL per L2 Amendment 4.
- `rust-toolchain.toml` pinning the stable Rust channel for reproducible builds (L2 P19).
- `.gitignore` Rust patterns (`target/`, `**/*.rs.bk`, `*.sock`) with explicit guidance to keep `Cargo.lock` tracked (L2 P19).
- `core` crate scaffold with `[lib]` (`weaver_core`) and `[[bin]]` (`weaver`) targets and `build.rs` invoking `vergen` for build-time provenance (L2 P11).
- `tui` crate scaffold (`weaver-tui` binary) depending on `weaver_core` for shared types.
- `ui` crate stub (Tauri UI deferred per Hello-fact slice 001 scope).

#### Phase 3 — User Story 1 (MVP: trigger + propagate)

- **Fact-family `buffer/dirty` (v0.1.0)**: first live producer. `core/dirty-tracking` behavior asserts `buffer/dirty=true` on `buffer/edited` events and retracts it on `buffer/cleaned`.
- **Bus protocol (v0.1.0)**: subscriptions now *forward* `FactAssert`/`FactRetract` messages to subscribers in real time. The Phase 2 listener acked subscriptions but never forwarded; the new listener multiplexes client reads and subscription fan-out via `tokio::select!`. No wire-format change — the behavior completes what v0.1.0 always promised.
- **CLI surface (v0.1.0)**:
  - `weaver simulate-edit <buffer-id>` now publishes `buffer/edited` on the bus (previous Phase 2 stub was a warn log).
  - `weaver simulate-clean <buffer-id>` now publishes `buffer/cleaned` (previous Phase 2 stub).
  - Both commands return a structured submission ack in `--output=human` or `--output=json`.
- **TUI**: crossterm raw-mode event loop with `e`/`c`/`q` keystrokes; live rendering of subscribed facts with `by <behavior>, event <id>, Δs ago` annotation; stale-view rendering with `UNAVAILABLE` status on core disconnect per `contracts/cli-surfaces.md`.
- **Dispatcher**: commit is now atomic with respect to behavior error — when a behavior firing returns `error: Some(_)`, its assertions and retractions are rolled back and the `BehaviorFired` trace entry records empty `asserted`/`retracted` lists. Tightens the implicit contract that Phase 2's docstring already claimed; covered by the new `error_recovery` scenario test.
- **Shared bus-client helper (`core/src/bus/client.rs`)**: consolidates the `Hello`/`Lifecycle(Ready)`/`Subscribe` handshake used by the CLI's one-shot subcommands, the TUI, and the e2e harness. Consolidation paves the way for the inspect client in Phase 4.
- **Workspace member `weaver-e2e`** (`tests/`): workspace-level end-to-end tests spawning the `weaver` binary. Two tests ship with this phase: `hello_fact` (SC-001, happy + retraction round-trip ≤ 100 ms) and `disconnect` (SC-004, SIGKILL survivability within 5 s).

#### Phase 4 — User Story 2 (provenance inspection)

- **Bus protocol (v0.1.0)**: `InspectRequest` now returns a real `InspectionDetail` instead of an always-`FactNotFound` placeholder. The handler walks the trace store's reverse causal index (already built in Phase 2) and is `O(1)` per lookup.
- **CLI surface (v0.1.0)**:
  - `weaver inspect <entity-id>:<attribute>` is now live. Parses the colon-delimited fact key, issues `InspectRequest`, renders human or JSON output matching `contracts/cli-surfaces.md`. Exit code 2 on `FactNotFound`.
  - Input validation — malformed keys (missing colon, empty halves, non-numeric entity id) produce structured errors before touching the bus.
- **TUI**: `i` keystroke triggers inspection of the first displayed fact. Waiting state rendered explicitly (`(waiting for response…)`) between request send and response; correlation via `request_id` so out-of-order InspectResponses are handled safely.
- **`core/src/inspect/handler.rs`** (new): pure routine `inspect_fact(snapshot, trace, key) -> Result<InspectionDetail, InspectionError>`. Uses `FactSpaceSnapshot` for the current-assertion check (fast `Arc` clone) and `TraceStore::fact_inspection` for the asserting behavior/event lookup.
- **New test coverage**: `inspect_inspection_found`, `inspect_inspection_not_found`, `property_inspection_invariant`, plus fact-key-parser unit tests in `cli::inspect::tests`.

#### Phase 5 — User Story 3 (structured machine output)

- **Bus protocol (v0.1.0 — additive)**: two new `BusMessage` variants — `StatusRequest` (client → core, unit) and `StatusResponse { lifecycle, uptime_ns, facts }` (core → client). Additive surface change per L2 P7. A future slice with a deployed v0.1 client will bump `Hello.protocol_version` if a wire-incompatible change ships; the protocol-level CBOR deserializer does NOT yet handle unknown variants gracefully, so adding variants *today* is only safe because all clients are co-developed in this repo. This caveat is a known gap in the contract — to be tightened in a future slice.
- **CLI surface (v0.1.0)**:
  - `weaver status [-o human|json]` is now live (was a warn-log stub). Connects to the bus, sends `StatusRequest`, renders the response per `contracts/cli-surfaces.md`.
  - On `core-unavailable`: renders the documented `{"lifecycle": "unavailable", "error": "..."}` shape and exits `2`.
  - Exit-code policy centralised in `cli::errors::exit_code` (`OK=0`, `GENERAL=1`, `EXPECTED=2`).
- **Error surface (new)**: `WeaverCliError` in `core/src/cli/errors.rs` with `miette::Diagnostic` derive. Four codes wired up (`WEAVER-002` core-unavailable, `WEAVER-101` parse-error, `WEAVER-201` fact-not-found, `WEAVER-301` protocol-error). JSON envelope matches contract: `{ "error": { "category", "code", "message", "context", "fact_key" } }`. `fact_key` populated for fact-scoped errors per L2 P6.
- **Dispatcher**: tracks `started_at_ns`; exposes `Dispatcher::uptime_ns()` for the status handler.
- **Listener**: handles `StatusRequest` by snapshotting the fact-store, reading dispatcher uptime, and replying with `StatusResponse`.
- **`core/src/cli/output.rs`** (new): `StatusResponse` struct with serde round-trip, `render_status` dispatcher, human and JSON formatters. Unit tests verify (a) round-trip preservation, (b) ready shape omits `error`, (c) unavailable shape omits `uptime_ns` and `facts`.
- **New test coverage**: `cli_status_round_trip`, `cli_status_unavailable` (exit-code 2), `cli_status_human`, plus `cli::output::tests` (4) and `cli::errors::tests` (3).

#### Phase 5.5 — Wire-tagging alignment (pre-1.0 cleanup)

The first slice is the right time to unify serialization strategy across
public wire surfaces. Adopted **adjacent tagging** (`"type"` + variant-specific
content field) uniformly for every sum type with non-unit variants:

- **`SourceId`** — `#[serde(tag = "type", content = "id")]`. Wire form now
  matches `contracts/cli-surfaces.md`'s example literally:
  `{"type":"behavior","id":"core/dirty-tracking"}` (was `{"behavior":"..."}`).
- **`BusMessage`** — `#[serde(tag = "type", content = "payload")]`. Every
  message variant now shares the shape `{"type":"<kind>","payload":<data>}`
  (unit variants omit `payload`). Uniform `.type`-based dispatch for
  consumers (was a rotating outer key per variant).
- **`SubscribePattern`** — `#[serde(tag = "type", content = "pattern")]`.
  `{"type":"family-prefix","pattern":"buffer/"}` (was `{"family-prefix":"..."}`).

`FactValue` already used adjacent tagging (`tag="type", content="value"`) — all
four data-bearing enums now share the pattern. Unit-only enums
(`LifecycleSignal`, `EventPayload`, `InspectionError`) remain bare kebab-case
strings, which is naturally consistent with adjacent-tag content semantics.

**Why now and not later**: the bus protocol had no deployed external
consumers. The change is wire-breaking for any already-serialized CBOR
payload (none existed outside this repo). Every round-trip test passed
without modification — serde handles both encode and decode through the
same Rust types, so the proof that clients still agree with the core
runs as part of `cargo test`.

`contracts/bus-messages.md` now documents the tagging convention as a
first-class principle.

#### Phase 6 — Polish & cross-cutting concerns

- **`core/README.md`** and **`tui/README.md`** (new): per-crate orientation with module maps, usage snippets, and pointers to the spec.
- **`.github/workflows/ci.yml`** (new): GitHub Actions workflow running `cargo fmt --all -- --check`, `cargo clippy --all-targets --workspace -- -D warnings`, `cargo build --workspace`, `cargo test --workspace` — mirrors `scripts/ci.sh` but cached + pinned. Enforces L2 P19 + L2 P10 + L2 Amendment 6.
- **`core/tests/property/provenance_wire.rs`** (new, T068): proptest that every `BusMessage` variant carrying `Provenance` round-trips through both CBOR and JSON with a non-empty `source`. Two generators per pass.
- **`core/tests/cli/version_timing.rs`** (new, T075): benchmark asserting median wall time of `weaver --version` is ≤ 50 ms (SC-006). Runs a warm-up iteration + 5 samples + median-of-5. Prints min/median/max to stderr for diagnostic visibility.
- **`quickstart.md`**: SC-003 example corrected. The original "edit one buffer repeatedly → `facts.length` grows" claim contradicts the data model (facts are keyed by `(entity, attribute)`; re-assertion refreshes provenance but does not add entries). The walkthrough now uses distinct buffer ids to demonstrate array growth, with a note explaining the fact-space semantics.
- **`tasks.md`**: all T064–T070 + T075 marked `[X]`. CHANGELOG `[Unreleased]` promoted to `[0.1.0] — 2026-04-20`.

#### Fix — `weaver --version` build timestamp stuck at 1980

- **Symptom**: `weaver --version` displayed `built: 1980-01-01T00:00:00.000000000Z` in every `cargo build` run from a nix dev shell.
- **Root cause**: `nixpkgs`' stdenv pre-sets `SOURCE_DATE_EPOCH=315532800` (1980-01-01T00:00:00Z) in every `mkShell` environment as a reproducible-build floor for ZIP-format compatibility (nix issue [#20716](https://github.com/NixOS/nixpkgs/issues/20716)). `vergen` honors `SOURCE_DATE_EPOCH` unconditionally, so the misleading placeholder flowed into the binary.
- **Fix (two-pronged)**:
  - `core/build.rs` — removes `SOURCE_DATE_EPOCH` if and only if it equals the exact nix-stdenv sentinel `315532800`. Any intentional value (e.g., a CI release build setting it to the commit timestamp for bit-reproducibility) is preserved. See [reproducible-builds.org](https://reproducible-builds.org/docs/source-date-epoch/) — `315532800` is documented as a ZIP-compat floor via `max(315532800, real_time)`, not a semantic timestamp; clearing it matches the upstream spec's "fall back to system time when unset" expectation.
  - `flake.nix` — `unset SOURCE_DATE_EPOCH` in `shellHook`, eliminating the sentinel at its source for users of the Weaver flake. The `build.rs` filter is the safety net for devenv/mise/direnv/plain shells that might inherit it from elsewhere.
- **L2 tension**: P11 (informative timestamp) vs P19 (reproducible builds). The resolution preserves both: P11 for dev/CI builds (no SOURCE_DATE_EPOCH, real wall time); P19 for release builds (caller sets SOURCE_DATE_EPOCH to the commit timestamp, which is neither unset nor `315532800`, so it's respected).

### Test summary for v0.1.0

- **54 weaver_core unit tests** — domain types, fact space, bus codec + delivery, trace store, behaviors, CLI parsers.
- **13 integration tests** — `core/tests/{behavior,inspect,property,cli}/*.rs` covering US1, US2, US3 scenario + property + timing.
- **2 workspace-level e2e tests** — `tests/e2e/{hello_fact,disconnect}.rs` spawning the `weaver` binary and driving it over the bus.

Total: 69 tests. `scripts/ci.sh` green end-to-end.
