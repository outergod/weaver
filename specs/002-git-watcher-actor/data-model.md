# Data Model: Git-Watcher Actor

Domain types for the git-watcher slice. Types live in `core/src/lib.rs` (shared) and in `git-watcher/src/` (watcher-internal). All shared types derive `serde::Serialize + Deserialize` for CBOR (bus) and JSON (CLI output) round-trips per L2 P5.

> **Scope note.** This slice *replaces* the opaque `SourceId::External(String)` with a closed `ActorIdentity` enum (Clarification Q1; open-questions §25 shape + migration sub-questions). The replacement is a **breaking wire change** — `Hello.protocol_version` advances 0x01 → 0x02, paired with a `CHANGELOG.md` entry per L2 P8. Working-copy state is modeled as a discriminated-union-by-naming under `repo/state/*` (Clarification Q4; open-questions §26), with the watcher enforcing the mutex invariant.

## Type catalog

### ActorIdentity (REPLACES `SourceId`)

Every fact, event, action, and trace entry carries an originating actor. One closed enum per actor kind in `docs/01-system-model.md §6`; each variant carries the identity components appropriate to that kind.

```text
ActorIdentity = enum {
    Core,                              // the core process itself
    Behavior(BehaviorId),              // in-core reactive unit
    Tui,                               // the TUI process
    Service {                          // a governed service on the bus
        service_id: String,            //   e.g., "git-watcher"
        instance_id: Uuid              //   v4 per process invocation (Clarification Q3)
    },
    User(UserId),                      // reserved; not emitted this slice
    Host {                             // reserved; language-host proxying user code
        host_id: String,
        hosted_origin: HostedOrigin
    },
    Agent {                            // reserved; delegated actor
        agent_id: String,
        on_behalf_of: Option<Box<ActorIdentity>>
    }
}

HostedOrigin {                         // defined for completeness; emitted when Host actor lands
    file: String,
    location: Option<String>,
    runtime_version: String
}

UserId(String)
```

**CBOR wire form (tag 1002 — Weaver structured-actor-identity)**: adjacent-tagged per contracts/bus-messages.md naming convention — `{ "type": "service", "service-id": "git-watcher", "instance-id": "<uuid>" }`; unit variants (`Core`, `Tui`) serialize as `{ "type": "core" }` / `{ "type": "tui" }`.

**Invariants**:
- `Service.service_id` MUST be non-empty and kebab-case (Amendment 5).
- `Service.instance_id` MUST be a v4 UUID (non-nil).
- `Agent.on_behalf_of`, when `Some`, MUST NOT form a cycle (reserved; not exercised this slice).

### Repository (entity)

The watcher's unit of observation. A git working tree on the local filesystem, addressed by the canonicalized absolute path of its worktree root.

```text
Repository entity key: an EntityRef assigned by the core on first assertion of any repo/* fact for a given path.
```

The watcher publishes a bootstrap fact `repo/path <canonical-path>` as part of its first `repo/*` assertion, so subscribers can map `EntityRef ↔ on-disk path` without depending on the wire-visible entity id. The core's entity-assignment scheme is unchanged from slice 001.

### Working-copy state (`repo/state/*`)

A discriminated union materialized across a family of fact attributes. Exactly one variant is asserted per repository entity at any moment; the watcher enforces the invariant (open-questions §26).

```text
repo/state/on-branch  <branch-name: String>    -- HEAD → refs/heads/<name>, ≥1 commit
repo/state/detached   <commit-sha: String>     -- HEAD → commit directly
repo/state/unborn     <intended-name: String>  -- HEAD → nonexistent ref (new repo, no commits)
```

**Transitions** happen as a single atomic retract-then-assert pair sharing a common `causal_parent` EventId. A subscriber observing the retract and assert in sequence sees the transition; the trace's reverse causal index pairs them.

**Deferred variants** (out of slice 002 scope, reserved as follow-up work):

```text
repo/state/rebasing    <target-branch>         -- rebase state files present
repo/state/merging     <from-commit>
repo/state/cherry-pick <target-commit>
repo/state/revert      <...>
repo/state/bisect      <...>
```

### Repository-level facts

```text
repo/dirty       <bool>      -- working tree OR index differs from HEAD; untracked files excluded (Clarification Q5)
repo/head-commit <String>    -- full SHA-1 (or SHA-256 when the repo is configured accordingly) of HEAD
repo/path        <String>    -- canonical absolute path to the worktree root (bootstrap fact)
repo/observable  <bool>      -- false when the watcher is degraded/unavailable; retracted when the watcher observes the repo again
```

### Watcher-instance facts

```text
watcher/status <LifecycleSignal>    -- service-lifecycle state; per-instance
```

`watcher/status` is keyed by a watcher-instance entity (entity identity = `Service.instance_id`); distinct from the repository entity. Subscribers correlating the two use the `Provenance.source.instance_id` field.

### LifecycleSignal (EXTENDED from slice 001)

```text
LifecycleSignal = enum {
    Started,
    Ready,
    Degraded,
    Unavailable,
    Restarting,
    Stopped
}
```

Three new variants (`Degraded`, `Unavailable`, `Restarting`) align the enum with `docs/05-protocols.md §5`. Slice 001's core emits only `Started`/`Ready`/`Stopped`; the watcher uses the full vocabulary.

### WorkingCopyState (watcher-internal, `git-watcher/src/model.rs`)

```text
WorkingCopyState = enum {
    OnBranch { name: String },
    Detached { commit: String },
    Unborn { intended_branch_name: String }
}
```

This is the watcher's internal observation type, constructed from `gix::head::Kind` during observation. It converts 1:1 into the matching `repo/state/*` family assertion at publish time. Keeping the type internal to `git-watcher` avoids coupling core/TUI code to a git-specific enum; the wire is the fact-family naming.

## Extended BusMessage variants

`BusMessage` gains nothing new in this slice — the existing `FactAssert`, `FactRetract`, `Event`, `Subscribe`, `SubscribeAck`, `InspectRequest`, `InspectResponse`, `Lifecycle`, `StatusRequest`, `StatusResponse`, `Error`, `Hello` set is sufficient. What changes is the inner shape of `Provenance.source` (now `ActorIdentity` per above) and the set of `LifecycleSignal` variants.

No `BusMessage` variant is added; no variant is removed. The MAJOR bus-protocol bump is driven by the `ActorIdentity` shape change and the `LifecycleSignal` enum growth, both of which land under the new CBOR wire tag for the provenance field.

## Relationships

```text
Repository entity  <--has-->  repo/path <canonical-path>        (bootstrap fact)
Repository entity  <--has-->  repo/dirty <bool>
Repository entity  <--has-->  repo/head-commit <sha>
Repository entity  <--has-->  repo/state/on-branch <name>        (exactly one of the
Repository entity  <--has-->  repo/state/detached <sha>          three asserted at any
Repository entity  <--has-->  repo/state/unborn <name>           moment, per mutex)
Repository entity  <--has-->  repo/observable <bool>             (published by watcher under degradation)

Watcher-instance entity  <--has-->  watcher/status <LifecycleSignal>

Every fact above:
    .provenance.source  = ActorIdentity::Service {
                              service_id: "git-watcher",
                              instance_id: <uuid-v4 per invocation>
                          }
    .provenance.timestamp_ns = monotonic per bus message
    .provenance.causal_parent = Some(event_id) for publishes that reflect a transition;
                                None for initial bootstrap publishes
```

## Validation rules

| Rule | Origin | Test type |
|---|---|---|
| `ActorIdentity::Service { service_id, .. }` has a non-empty kebab-case `service_id` | L2 P11 + Amendment 5 | Property test on `ActorIdentity::service` constructor |
| `ActorIdentity::Service { instance_id, .. }` is a v4 UUID (non-nil) | Clarification Q3 | Property test on constructor |
| CBOR round-trip preserves every `ActorIdentity` variant exactly | L2 P5 | Property test (proptest strategies for each variant) |
| At most one `repo/state/*` variant asserted per `Repository` entity at any time | Clarification Q4 + open-questions §26 | Property test on the trace prefix: for every prefix, at every Repository entity, count of asserted state variants ≤ 1 |
| `repo/state/*` transition emits exactly one retract and exactly one assert sharing a `causal_parent` | Clarification Q4 | Scenario test — simulate HEAD transition, inspect the two-entry trace window |
| `repo/dirty` semantics match `git diff HEAD --quiet` exit code (untracked excluded) | Clarification Q5 | Scenario test — set up worktree in each of: clean / modified-tracked / staged-only / untracked-only / combined; assert `repo/dirty` value |
| Watcher disconnect retracts all facts it authored | L2 P20 + FR-014 | Scenario test on the three-process e2e harness — kill watcher; assert facts transition to retracted or `repo/observable = false` |
| `Hello.protocol_version = 0x01` connection is rejected with structured `Error { category: "version-mismatch", ... }` and closed | Research §4 | Scenario test — test client attempts `0x01` handshake |
| `weaver inspect` renders structured actor identity (no opaque tag string) for every fact family | FR-012 | Scenario test covering `buffer/*` and `repo/*` families |
| Two `weaver-git-watcher` instances against the same repo: second fails to claim authority | FR-009 | Scenario test (exact mechanism finalized at `/speckit.tasks`) |

## State transitions

### Repository working-copy state (the mutex-invariant machine)

```
                   ┌──────────────────────────┐
                   │ (watcher starts)         │
                   └────────────┬─────────────┘
                                │ initial observation
                                ▼
           ┌────────────────────────────────────────────┐
           │ asserts exactly one of:                    │
           │   • repo/state/on-branch <name>            │
           │   • repo/state/detached   <sha>            │
           │   • repo/state/unborn     <intended-name>  │
           └────────────────┬───────────────────────────┘
                            │
                            ▼
       ┌────────────────────────────────────────────────┐
       │ on next poll, if HEAD changed:                 │
       │   1. retract previous repo/state/* fact        │
       │   2. assert new repo/state/* fact              │
       │   both messages share causal_parent = <poll-   │
       │     event id>; published as adjacent frames    │
       └────────────────┬───────────────────────────────┘
                        │
                        ▼
                   (loops until watcher stops;
                    on stop, all repo/* retracted OR
                    repo/observable=false published)
```

### Watcher lifecycle (`watcher/status`)

```
    Started ──→ Ready ──→ Degraded ⇄ Ready
                  │           │
                  │           └──→ Unavailable ──→ (process exit) ──→ Stopped
                  │
                  └──→ Stopped
```

Transitions:

- `Started → Ready`: bus handshake completed successfully; initial observation published.
- `Ready → Degraded`: transient filesystem/git error (permissions flicker, interrupted rebase state file corruption). `repo/observable` retracted or set to `false`; prior `repo/state/*` fact is left as-is (potentially stale; degradation is the honest signal).
- `Degraded → Ready`: observation recovers; `repo/observable` returns to `true`; current `repo/state/*` re-asserted if it changed during degradation.
- `Ready | Degraded → Unavailable`: repository lost (deleted, unmounted) or watcher shutting down. All `repo/*` facts retracted.
- `* → Stopped`: final state before process exit.

## Out of scope (data model)

- **Agent and Host actor variants emit in this slice.** Their enum variants exist (for wire stability across the protocol bump) but are reserved; emitting them is future work.
- **`on-behalf-of` delegation chain.** Schema allows it; population is deferred to the agent slice (open-questions §23).
- **Transient operation states (`rebasing`, `merging`, etc.).** Deferred per Clarification Q4.
- **Multi-repository entities in one watcher process.** Out of spec scope; each watcher instance observes exactly one repository.
- **Components for working-copy state (§2.4 long-term home).** Tracked in open-questions §26; stopgap lives here.
