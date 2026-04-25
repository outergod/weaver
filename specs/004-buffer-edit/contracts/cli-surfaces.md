# CLI Surface Contracts — Slice 004

CLI grammars + structured-output shapes for `weaver`, `weaver-buffers`, `weaver-git-watcher`, `weaver-tui`. Per L2 P5 / P7 (CLI + structured output as a public surface) and `docs/02-architecture.md §2`.

**This slice introduces a MINOR additive change to the `weaver` CLI surface** plus a constant-driven `--version` field bump on every binary that renders `bus_protocol`. No grammar changes to existing subcommands; no removals.

## `weaver` — MINOR additive

Two new subcommands land on the existing `weaver` binary:

### `weaver edit`

```
weaver edit <PATH> [<RANGE> <TEXT>]* [--socket <PATH>]
```

**Semantics**: dispatch one or more `TextEdit` operations as an atomic `EventPayload::BufferEdit` event against the buffer entity derived from `<PATH>`. Fire-and-forget: the CLI exits `0` on successful dispatch and does NOT wait for the service to apply.

**Positional arguments**:

- `<PATH>` — required. The file path identifying the buffer. Canonicalised in-process before entity derivation (matching `weaver-buffers`'s slice-003 canonicalisation).
- `<RANGE> <TEXT>` — zero or more pairs. Each pair is one `TextEdit`. Pairs are positional-positional (no flag interleaving).

**`<RANGE>` grammar**:

```
<start-line>:<start-char>-<end-line>:<end-char>
```

Decimal `u32` integers; zero-based; `start-char` and `end-char` count **UTF-8 bytes within the respective line** (per `bus-messages.md §Position`). Examples:

- `0:0-0:0` — point cursor at start of buffer (insertion).
- `0:0-0:5` — first 5 bytes of line 0.
- `2:10-3:0` — from line 2 byte 10 through end of line 2 (exclusive end at line 3 byte 0).

Parse failure on the range string exits with code `1` and `WEAVER-EDIT-002` diagnostic.

**`<TEXT>` argument**:

The replacement text. Standard shell-quoted UTF-8 string. Empty string (`""`) means delete-only. Non-UTF-8 bytes in argv (rare; locale issue) fail at clap's `OsString → String` conversion.

**Optional flags**:

- `--socket <PATH>` — bus socket path. Defaults to `WEAVER_SOCKET` env or the slice-003 default discovered via the existing helper. Identical semantics to `weaver-buffers --socket`.

**No `--version` flag** (deliberate; the universal `--version` flag at the binary level prints program-version info; the buffer-version handshake value is sourced via in-process bus inspect-lookup per FR-013).

**Pre-dispatch flow**:

1. Canonicalise `<PATH>` → `canonical`. Derive `entity = buffer_entity_ref(canonical)`.
2. Connect to bus at `--socket`. Send `Hello { protocol_version: 0x04, client_kind: "weaver-edit" }`.
3. Send `BusMessage::InspectRequest { request_id: <fresh>, fact: FactKey::new(entity, "buffer/version") }`. Receive `InspectResponse`.
   - **`FactNotFound`** → exit `1` with `WEAVER-EDIT-001 — buffer not opened: <path> — no fact (entity:<derived>, attribute:buffer/version) is asserted by any authority`.
   - **`Found(Fact { value: FactValue::U64(version), .. })`** → use `version` as the dispatch value.
   - **`Found(other-shape)`** → exit `10` with internal-error diagnostic (constitutional invariant violation).
4. Construct `EventPayload::BufferEdit { entity, version, edits: <parsed> }`. Construct envelope `Event { id: <fresh EventId>, name: "buffer/edit", target: Some(entity), payload, provenance: Provenance { source: ActorIdentity::User, timestamp_ns: now_ns(), causal_parent: None } }`.
5. Dispatch via `BusMessage::Event(envelope)`. Connection close on send error.
6. Exit `0`.

**Operator-facing exit codes**:

| Code | Meaning |
|---|---|
| `0` | Event dispatched successfully on the bus. **Does NOT imply the edit was applied** — silent drops at the service (stale-version, validation-failure, unowned-entity) are indistinguishable to the CLI. |
| `1` | CLI parse error (malformed `<RANGE>`, malformed argv, path canonicalisation failure) **OR** pre-dispatch lookup found no `buffer/version` fact for the target entity (buffer not opened). |
| `2` | Bus unavailable (socket missing, handshake rejected). Aligns with `weaver-buffers` exit-code convention. |
| `10` | Internal invariant violation (e.g., `buffer/version` fact has wrong type — should never happen under a conforming `weaver-buffers`). |

**No new exit code for "stale version / edit dropped at the service"** — stale rejection is silent per spec FR-018; the CLI cannot detect it post-dispatch.

**Examples**:

```sh
# Insert "hello " at the start of the file
weaver edit /tmp/foo.txt 0:0-0:0 "hello "

# Delete bytes 5-10 of line 3
weaver edit /tmp/foo.txt 3:5-3:10 ""

# Three-edit atomic batch
weaver edit /tmp/foo.txt 0:0-0:0 "PREFIX " 5:0-5:0 "MIDDLE " 9:0-9:0 "SUFFIX "
```

### `weaver edit-json`

```
weaver edit-json <PATH> [--from <PATH>|-] [--socket <PATH>]
```

**Semantics**: identical to `weaver edit` but reads a JSON-encoded `Vec<TextEdit>` from stdin or a file. Single dispatch; same atomic-batch semantics; same fire-and-forget exit conventions.

**Positional arguments**:

- `<PATH>` — required. The file path identifying the buffer. Same canonicalisation as `weaver edit`.

**Required flag**:

- `--from <PATH>` or `--from -` — source for the JSON `Vec<TextEdit>`. `-` means stdin; otherwise the named file is read fully. (If `--from` is omitted, exit `1` with a usage diagnostic — there is no implicit default since reading stdin without an explicit `-` is ambiguous.)

**Optional flag**:

- `--socket <PATH>` — same as `weaver edit`.

**JSON input format**:

```json
[
  {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "new-text": "hello "},
  {"range": {"start": {"line": 5, "character": 0}, "end": {"line": 5, "character": 0}}, "new-text": "world\n"}
]
```

The JSON top-level is an array of `TextEdit` objects (NOT a wrapped `BufferEdit` event — the CLI wraps the array into an event before dispatch). Field names use kebab-case on JSON (`new-text`); the Rust deserialiser uses `#[serde(rename_all = "kebab-case")]` to bridge.

**Pre-dispatch flow** (extends the `weaver edit` flow):

1. Read JSON from `--from <PATH>` or stdin.
2. Parse to `Vec<TextEdit>`. Parse failure → exit `1` with `WEAVER-EDIT-003 — malformed edit-json input: <serde-json error chain>`.
3. Canonicalise `<PATH>`, derive entity, connect, run inspect-lookup (same as `weaver edit` steps 1–3).
4. Construct the envelope (same as `weaver edit` step 4).
5. **Pre-check serialised wire size**: `ciborium::into_writer(&envelope, &mut buf)`; if `buf.len() > MAX_FRAME_SIZE` (64 KiB) → exit `1` with `WEAVER-EDIT-004 — serialised BufferEdit (<n> bytes) exceeds wire-frame limit (65 536 bytes)`.
6. Dispatch and exit `0`.

**Exit codes** (same table as `weaver edit`, plus):

| Code | Additional meaning |
|---|---|
| `1` | Malformed JSON (`WEAVER-EDIT-003`) **OR** serialised `BufferEdit` exceeds 64 KiB wire-frame limit (`WEAVER-EDIT-004`). |

**Example**:

```sh
echo '[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"new-text":"prefix "}]' \
  | weaver edit-json /tmp/foo.txt --from -
```

## Diagnostic codes (miette)

Per L2 P6 (Humane shell), errors reference fact-space state. The `WEAVER-EDIT-NNN` family is introduced this slice:

- **`WEAVER-EDIT-001`** — buffer not opened. Triggered when pre-dispatch inspect-lookup returns `FactNotFound`. Diagnostic body: `"buffer not opened: <path> — no fact (entity:<u64-derived>, attribute:buffer/version) is asserted by any authority. Run \`weaver-buffers <path>\` to open the buffer."`
- **`WEAVER-EDIT-002`** — invalid `<RANGE>` grammar. Triggered by `weaver edit` range-string parser. Body: `"invalid range \"<arg>\": expected <start-line>:<start-char>-<end-line>:<end-char> with decimal u32 components."`
- **`WEAVER-EDIT-003`** — malformed JSON input. Triggered by `weaver edit-json` JSON parser. Body: serde-json error chain rendered through miette's source-span machinery.
- **`WEAVER-EDIT-004`** — wire-frame too large. Triggered by `weaver edit-json` pre-dispatch size check. Body: `"serialised BufferEdit (<actual-bytes> bytes) exceeds wire-frame limit (65 536 bytes). Reduce the batch size or shorten new-text fields."`

## `weaver-buffers` — `--version` constant bump only

Grammar unchanged from slice 003.

The `bus_protocol` field in `weaver-buffers --version --output=json` advances `0.3.0 → 0.4.0` mechanically (constant-driven via `BUS_PROTOCOL_VERSION_STR` in `core/src/types/message.rs`). This is NOT a CLI-surface schema event — the JSON shape is unchanged; the value of one field changes.

The slice-003 exit-code matrix (0/1/2/3/10) is unchanged. No new exit code for edit handling — edits are processed transparently by the publisher's reader-loop arm; failures are silent (per spec FR-018) and emit `tracing::debug` lines on stderr.

## `weaver-git-watcher` — `--version` constant bump only

Same as `weaver-buffers`: grammar unchanged; `bus_protocol` field advances `0.3.0 → 0.4.0`.

## `weaver-tui` — `--version` constant bump only

Same: grammar unchanged; `bus_protocol` field advances `0.3.0 → 0.4.0`. The Buffers render section already subscribes to `buffer/*` (slice-003 implementation); re-emitted facts on accepted edits flow through transparently. **No new keybindings, no new render regions** — the operator sees `buffer/version` advance and `buffer/dirty=true` flip in real time as edits land.

## Versioning policy (P7 + P8)

- **`weaver` CLI surface** bumps MINOR additive: `0.2.0 → 0.3.0` (additive subcommands `edit` + `edit-json`). Per L2 P8, `CHANGELOG.md` gains a `## Weaver CLI 0.3.0` entry.
- **`weaver-buffers`, `weaver-git-watcher`, `weaver-tui` CLI surfaces** are unchanged at the grammar level; their `bus_protocol` JSON-field value advances in lockstep (constant-driven).
- **Bus protocol** — see `contracts/bus-messages.md`. MAJOR `0.3.0 → 0.4.0`.

## References

- `specs/004-buffer-edit/spec.md` — user stories, FRs, SCs, Clarifications 2026-04-25.
- `specs/004-buffer-edit/contracts/bus-messages.md` — wire contract.
- `specs/004-buffer-edit/research.md §2` — in-process inspect-lookup decision.
- `specs/004-buffer-edit/research.md §5` — wire-frame size pre-check decision.
- `specs/004-buffer-edit/research.md §6` — emitter identity (`ActorIdentity::User`) decision.
- `specs/003-buffer-service/contracts/cli-surfaces.md` — prior CLI contract the slice extends.
- L2 Constitution Amendment 5 — wire-vocabulary kebab-case convention.
- L2 Constitution Amendment 6 — code-quality gates (slice 004 inherits unchanged).
