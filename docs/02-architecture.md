# Architecture

Weaver is composed of a small local core and a set of cooperating services.

## 1. Core Responsibilities

The core is responsible for:

- entity and fact management
- event processing
- behavior execution
- action invocation
- service coordination
- authority enforcement
- trace and provenance tracking
- maintaining a local semantic model of the system

The core is not responsible for rendering UI.

---

## 2. Services

Capabilities beyond the editing core are provided by services.

Typical services may include:

- project service
- git service
- search/index service
- language service bridge
- task/test service
- file watcher service
- language hosts (Python, Node.js) — see §9.1.1

Services:

- run independently
- communicate explicitly
- declare capabilities
- publish facts and events
- participate in a shared behavioral system

No service is entitled to become a replacement monolith.

### 2.1 Registration and Discovery

Services register with the core at startup. For the MVP and early iterations, registration is **static configuration** — services are listed in a config file or compiled in.

**Dynamic discovery** — services advertising themselves over the bus and attaching at runtime — is deferred. It becomes relevant alongside the distribution story (§6 failure model) and remains an open follow-up; see 07-open-questions §15.

---

## 3. Message Bus

The system communicates through an explicit bus or transport layer carrying:

- events
- fact assertions
- fact retractions
- requests
- responses
- streams
- health and lifecycle signals

The bus must support:

- asynchronous operation
- cancellation
- structured errors
- streaming where appropriate
- provenance preservation

---

## 4. Fact Space

The core maintains a local, queryable view of shared semantic state.

This includes:

- authoritative facts
- derived semantic relations
- service-provided state
- interaction-relevant projections

This model represents meaning, not presentation.

### 4.1 Index Maintenance

Behavior preconditions are fact patterns. Without indexing, evaluating N behaviors against M entities on each event is quadratic; with a rich fact space and a broad behavior set, the naïve path does not scale.

The core maintains incremental indexes keyed by the predicate shapes that registered behaviors actually reference:

- **lazy creation** — an index is materialized on first use of its predicate shape, not pre-built, not speculative
- **incremental maintenance** — indexes update on fact assertion and retraction, never full recomputation
- **shared across behaviors** — multiple behaviors referencing the same predicate shape share one index

This borrows directly from archetype-based ECS implementations (Bevy ECS, Flecs). Behavior-precondition registration is the equivalent of query registration; fact assertion and retraction are archetype transitions for the affected entity.

The fact space is therefore not a generic triplestore — it is a specialized structure optimized for the evaluation pattern that behaviors actually produce.

### 4.2 Derived-View Materialization

Derived views (leader menus, workspace projections, compareable sets, entities relevant to a search result) are **recomputed on demand** for the MVP and early iterations.

Predicate-shape indexing (§4.1) already handles the behavior-evaluation hot path. Other derived views are computed at query time against the current fact space plus any cached index.

Incremental maintenance, caching with invalidation, and partial delegation to services remain available optimizations — deferred until measurement demonstrates the on-demand cost is unacceptable for a specific view kind. This is a simplicity concession, not an ontological commitment.

---

## 5. Authority and Ownership

Each authoritative fact family must have a declared owner.

Examples:

- buffer-open state → core
- project root mapping → project service
- repository branch and hunk state → git service
- diagnostics → language service bridge

This avoids contradictory canonical claims. **Authority is single-writer per family**: competing authoritative claims are rejected; multiple sources may produce derived or speculative facts, which must be marked as such.

### 5.1 Entity Lifetime

The authority owning an entity's primary fact family decides when the entity ceases. Entity retraction cascades: action entities targeting the entity cease with it (see system-model §7.1), derived facts about it are invalidated, and subscriptions receive explicit retraction events with provenance.

Derived or speculative facts may still exist alongside authoritative facts, but must be marked accordingly.

---

## 6. Failure Model

Weaver must degrade gracefully.

If a service fails:

- the core remains responsive
- dependent capabilities become unavailable or stale in an explicit way
- traces preserve what happened
- restart or reattachment is possible

Failure must not become silent semantic corruption.

---

## 7. Latency Model

The system must acknowledge that not all actions are local or immediate.

Therefore:

- editing actions may be local and immediate
- service-dependent actions may be asynchronous
- partial results may appear before complete results
- pending and stale states must be representable

Latency is part of the architecture, not an implementation embarrassment.

### 7.1 Latency Classes

The architecture commits to three named latency classes. Every request schema declares which class its response targets; breaches are observable in traces.

- **Immediate (≤16ms)** — interactive operations that must feel instantaneous: keystroke echo, cursor motion, local edit commit. Runs entirely within the core and the client render path.
- **Interactive (≤100ms)** — operations involving the bus, local services, and fact-space updates: applicability recomputation, composed behavior firing, local service requests. Clients may show micro-feedback but should not surface pending state.
- **Asynchronous (unbounded)** — operations involving external systems or long-running computation: network calls, indexing, remote services. Pending state is surfaced explicitly; partial results may appear before completion.

---

## 8. UI Boundary

User interfaces are independent clients of the system.

They:

- subscribe to facts and events
- query semantic state
- invoke actions
- derive local views
- render presentation

The core may expose semantic projections (e.g. relations, applicability), but:

- it does not define layout
- it does not define rendering
- it does not enforce a canonical visual structure

---

## 9. Composition and Extension

Weaver separates two kinds of extension.

### 9.1 Services (governed lane)

Services extend the system with new **capabilities**. They own fact families, declare schemas, participate in the protocol with versioning and authority. They may be implemented in any language that speaks the bus protocol; first-party infrastructure services are Rust.

#### 9.1.1 Language Hosts

Some services are **language hosts**: they run user code in an interpreted or embeddable runtime and proxy its bus interactions. Python and Node.js are the initial first-party targets. Compiled languages do not use this pattern; they use bus SDKs and run as standalone services.

A language host is the authoritative bus citizen for the code it hosts. The host's service identity is the `source` on every assertion, event, and response. User code inside the host appears in a **`hosted-origin`** provenance subfield — file, line, hosted-runtime version — carried alongside the authoritative source (see protocols §3.3).

Consequences:

- The host is responsible for sandboxing hosted code and enforcing whatever capability surface it chooses to expose.
- The host is responsible for representing its hosted code honestly: identity, rate, lifecycle, failure containment.
- Authority over a fact family is claimed by the host, not by its hosted users. Single-authority semantics (§5) remain intact; hosted code is observably derived-from the host.

Language hosts are therefore a more demanding service shape than typical — they carry responsibility for everything their users emit, which is why they warrant first-party status.

### 9.2 Composed Behaviors (user-scratch lane)

Composed behaviors extend the system with new **reactive rules**. They are authored in Steel (a Scheme embedded in the core process) and execute in a sandboxed VM adjacent to the core.

The composition runtime exposes host primitives for:

- fact subscription and query
- event emission and reception
- action invocation
- user-scratch fact assertion and retraction (non-authoritative by construction)
- user-scratch fact-family declaration

The composition runtime does NOT expose:

- authoritative fact assertion into governed families
- direct network, filesystem, or process access (these are service-owned)
- unsandboxed Rust interop

Performance-critical primitives live in services (Rust). Composition is glue. When a composed behavior needs capability Steel cannot express, the capability is promoted to a service. Service scaffolding must make "write a tiny service" approach `defun`-level ceremony — otherwise the cost of promotion erodes extension velocity.

### 9.3 Reflective Loop

The composition runtime supports live redefinition of behaviors and user-scratch fact families without core restart. Authoritative fact state is preserved across redefinitions. This is the runtime validation of constitution §13.

---

## 10. Trace Model

The core maintains an **append-only trace** — a log of events, fact assertions, fact retractions, behavior firings, and UI intents, each carrying causal-parent provenance.

The trace is the backing store for the `why?` channel (constitution §15–16): every derived view, every action entity, every applicability state walks back through the trace to the authoritative event or fact that caused it.

The trace:

- is a raw log, not a structured span tree — rendering is a client concern
- retains provenance and causal chain for every recorded item
- is subscribable — clients and services can observe the system's history as it accretes
- is not an analytics product; aggregation and summarization live in derived views, not the core

Structured trace views (span trees, causal DAG visualizations, timing charts) are UI-side derived views over the raw log. The core does not define them.

---

## 11. Action Execution

When an action's consequences span authorities, **the core orchestrates** — regardless of which authorities the action touches, and regardless of whether the core itself is authoritative over any of them.

Rationale:

- **Single source of action semantics.** Applicability is derived in one place (the fact space plus behavior engine); execution is coordinated from the same place.
- **Provenance legibility.** The causal chain from invocation to fact change is a single thread; no service synthesizes the semantics of another service's work.
- **No lead-authority election.** There is no negotiation between services about who drives a multi-authority action.

The core does not perform the authorities' work. Each authority performs its own fact changes and publishes them; the core issues requests, observes responses, and applies its own fact changes (if any) in causal order.

This rule holds in all three topologies:

- **Core owns one side** (e.g., `save`: core owns buffer/dirty state; filesystem service owns path facts). Core orchestrates.
- **Core owns both sides.** Core orchestrates trivially.
- **Core owns neither side** (e.g., `rebase`: git service owns both branch states). Core still orchestrates — it holds the action's applicability and invariants even though it owns no side's fact family.

Services must not expose shortcuts that let clients bypass core orchestration for actions whose semantics the core owns.
