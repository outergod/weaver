# CLI Surface Contracts — Slice 005

CLI grammars + structured-output shapes for `weaver`, `weaver-buffers`, `weaver-git-watcher`, `weaver-tui`. Per L2 P5 / P7 (CLI + structured output as a public surface) and `docs/02-architecture.md §2`.

**This slice introduces a MINOR additive change to the `weaver` CLI surface** (one new subcommand) plus a constant-driven `--version` field bump on every binary that renders `bus_protocol`. No grammar changes to existing subcommands; no removals.

## `weaver` — MINOR additive

One new subcommand lands on the existing `weaver` binary.

### `weaver save`

```
weaver save <ENTITY> [--socket <PATH>]
```

**Semantics**: dispatch a `BufferSave` event against the buffer entity identified by `<ENTITY>`. The service performs an atomic disk write (tempfile + fsync + rename) gated by ownership, version handshake, and pre-rename inode check. Fire-and-forget: the CLI exits `0` on successful dispatch and does NOT wait for the service to apply.

**Positional arguments**:

- `<ENTITY>` — required. The buffer entity reference. Two accepted forms (matches `weaver inspect` resolver):
  - **Path form**: `./file.txt` — canonicalised in-process; entity derived via `buffer_entity_ref(canonical_path)` (slice-003 derivation).
  - **EntityRef form**: `4611686018427387946` — bare unsigned integer; passes through the resolver verbatim.

  The resolver auto-detects: if the argument parses as a `u64`, treat as `EntityRef`; otherwise as a path. (Same heuristic as `weaver inspect`.)

**Optional flags**:

- `--socket <PATH>` — bus socket path. Defaults to `WEAVER_SOCKET` env or the standard-discovery helper (matches slice-003/004 convention).

**No `--version` flag** (the universal `--version` flag at the binary level prints program-version info; the buffer-version handshake value is sourced via in-process bus inspect-lookup per FR-009).

**No `--json` / `--output=json` variant** (deferred; operator confirmed in spec). The CLI emits no structured stdout — only stderr diagnostics on the error paths.

**Pre-dispatch flow**:

1. Resolve `<ENTITY>` → `entity: EntityRef`. Canonicalise the path (if path form) before derivation; an unresolvable path exits `1` with the existing path-canonicalisation diagnostic (matches slice-003/004 convention).
2. **In-process inspect-lookup** of `<entity>:buffer/version` via the slice-004 library function `weaver_core::cli::inspect::lookup_fact`. (Same mechanism slice-004 introduced for `weaver edit`.)
3. **Buffer-not-opened detection**: `InspectResponse::FactNotFound` → exit `1` with `WEAVER-SAVE-001` diagnostic (`"buffer not opened: <ENTITY> — no fact (entity:<derived>, attribute:buffer/version) is asserted by any authority"`).
4. Buffer found at `version = N`: construct `EventOutbound { payload: EventPayload::BufferSave { entity, version: N }, provenance: Provenance { source: ActorIdentity::User, timestamp_ns: now_ns(), causal_parent: None }, causal_parent: None }`. Note: the producer does NOT mint an `EventId` (the listener stamps; per §28(a) FR-019..FR-021).
5. Send `BusMessageInbound::Event(outbound)` via the existing bus client. Exit `0` on successful send.

**Exit codes**:

| Code | Meaning |
|---|---|
| `0` | Event dispatched successfully (does NOT imply save applied at the service). |
| `1` | CLI parse error / malformed `<ENTITY>` / path canonicalisation failure / pre-dispatch lookup found no `buffer/version` fact (`WEAVER-SAVE-001`). |
| `2` | Bus unavailable (socket missing, handshake failed). Aligns with slice-003/004 convention. |

**No new exit code for "save refused at the service" or "stale-version dropped"** — slice-004's silent-drop posture extends to slice 005 (FR-011). Operators wanting save confirmation subscribe to `buffer/dirty` post-dispatch and observe the flip to `false`.

### `WEAVER-SAVE-NNN` diagnostic taxonomy

Seven structured diagnostic codes covering the slice-005 failure surface. Format mirrors slice-004's `WEAVER-EDIT-NNN` convention (structured `tracing` records, code-specific detail fields, no fact-space surface):

| Code | Surface | Tracing level | Trigger | Detail fields |
|---|---|---|---|---|
| `WEAVER-SAVE-001` | CLI stderr (CLI-side; pre-dispatch) | `error` | Buffer not opened (entity unknown / no `buffer/version` fact). | `entity`, `path` |
| `WEAVER-SAVE-002` | Service stderr (post-dispatch) | `debug` | Stale version handshake (`event.version != current buffer/version`). | `entity`, `event_version`, `current_version` |
| `WEAVER-SAVE-003` | Service stderr | `error` | Tempfile create / write / fsync I/O failure. Operator-actionable: disk full, permissions. | `entity`, `path`, `errno`, `os_error` |
| `WEAVER-SAVE-004` | Service stderr | `error` | `rename(2)` I/O failure (`EXDEV`, `ENOSPC`, `EACCES`, `EROFS`, etc.). | `entity`, `path`, `errno`, `os_error` |
| `WEAVER-SAVE-005` | Service stderr | `warn` | Refusal: path no longer points to the same inode the buffer opened (concurrent external rename or atomic-replace by another editor). Operator-recoverable: re-open. | `entity`, `path`, `expected_inode`, `actual_inode` |
| `WEAVER-SAVE-006` | Service stderr | `warn` | Refusal: path was deleted on disk between open and save. | `entity`, `path` |
| `WEAVER-SAVE-007` | Service stderr | `info` | Clean-save no-op (`buffer/dirty = false` at dispatch time; no disk I/O performed; idempotent fact re-emission). Operator-informational. | `entity`, `path`, `event_id`, `version` |

All `tracing` records additionally carry the `event_id` field (the core-stamped ID per FR-022; absent on `WEAVER-SAVE-001` because that fires CLI-side before any event is dispatched).

## `weaver-buffers` — UNCHANGED CLI surface

The `weaver-buffers` binary's CLI grammar is unchanged; the `--version` JSON `bus_protocol` field advances `0.4.0 → 0.5.0` (constant-driven inheritance). The new dispatcher arm for `BufferSave` is an internal change; it adds no new flags, no new arguments, no new subcommands.

The `weaver-buffers` startup retains its slice-003/004 lifecycle and adds one new event subscription pattern: `payload-type=buffer-save` (alongside existing `payload-type=buffer-edit` from slice 004 and `payload-type=buffer-open` from slice 003).

## `weaver-git-watcher` — UNCHANGED CLI surface

CLI grammar unchanged; `--version` JSON `bus_protocol` field advances `0.4.0 → 0.5.0` (constant-driven). Internal change: producer-side EventId minting migrated to `EventOutbound` per FR-019 — no observer-visible behavior change beyond the wire bump.

## `weaver-tui` — UNCHANGED CLI surface

CLI grammar unchanged; `--version` JSON `bus_protocol` field advances `0.4.0 → 0.5.0` (constant-driven). The TUI's existing `buffer/*` subscription captures the new `buffer/dirty = false` re-emissions on accepted save without code change.

## `weaver --version` JSON shape (`weaver --version --output=json`)

```json
{
  "crate_version": "0.5.0",
  "git_sha": "<commit-sha>",
  "git_dirty": false,
  "steel_version": "<steel-crate-version>",
  "bus_protocol": "0.5.0",
  "build_timestamp": "<rfc3339-utc>",
  "build_profile": "release"
}
```

The `bus_protocol` field bump from `0.4.0` to `0.5.0` is the only field-value change; the field set is unchanged. All four binaries (`weaver`, `weaver-buffers`, `weaver-git-watcher`, `weaver-tui`) inherit this constant.

## `weaver inspect` — UNCHANGED at the CLI shape

Slice 005 does not change `weaver inspect`'s grammar, flags, or output shape. `weaver inspect --why <entity>:buffer/dirty` walks transparently through `BufferSave` events and stamped `EventId`s — the walkback machinery (slice 004) is unaffected by §28(a)'s wire-shape change because subscribers always receive `Event` (with stamped `id`).

`weaver inspect --why` walkback semantics under §28(a):

- For a `buffer/dirty` fact emitted on accepted save: walkback resolves to the `BufferSave` event with stamped `id`. The event's provenance carries `ActorIdentity::User` (the CLI emitter's identity). Inspect renders this transparently.
- For multi-producer scenarios: stamped IDs are globally unique per trace; walkback never resolves to a foreign producer's event due to wall-clock-ns collision. SC-505 verifies.
- For pre-§28(a) trace entries (only relevant in long-running deployments bridging the upgrade): the slice-004 ZERO-short-circuit on `lookup_event_for_inspect` (FR-024) preserves correctness; pre-fix events at id `0` are returned as `EventNotFound`.

## Slice-004 hygiene carryover

`core/src/cli/edit.rs::handle_edit_json` MUST grow a code comment at the post-parse step explaining why empty `[]` JSON does NOT short-circuit (asymmetric with positional zero-pair). Reference: slice-004 `spec.md §220` reserves wire-level empty `BufferEdit` as a future-tool handshake-probe affordance. The comment is implementation-internal and does not affect the CLI's observable grammar; it is documented here for traceability per FR-025.

---

*Phase 1 — cli-surfaces.md complete. Quickstart follows.*
