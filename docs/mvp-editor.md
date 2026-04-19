## Weaver Editor MVP — Definition

> **Milestone naming.** This document defines the **Editor MVP** — the first Weaver milestone usable as a real editor for daily work. It builds on the **Ontology Prototype** ([`mvp-ontology.md`](mvp-ontology.md)) by layering the gates that prevent the prototype from being used as an editor in production. This naming follows the framing decision in [`mvp-review-hacker-triage.md`](mvp-review-hacker-triage.md).

### Goal

Prove that Weaver can serve as a primary editor for a willing first customer — not because every feature exists, but because the *editor-shaped concerns* that prevent daily use are resolved.

The Editor MVP is successful if a competent Emacs user can:

- type real text without losing work to a missing undo,
- run a long-running task without the editor freezing,
- author a Steel behavior without fear that a runaway loop hangs the session, and
- complete a real Git workflow (stage, split, commit hunks) entirely within Weaver.

If those four conditions hold, daily use is possible. If any one fails, it does not.

---

## Prerequisites

The Editor MVP **requires** the Ontology Prototype to be complete and accepted. Specifically:

- The bus, fact space, behavior engine, composition runtime, applicability model, and reflective loop pass the Ontology Prototype's acceptance criteria ([`mvp-ontology.md`](mvp-ontology.md) §Acceptance Criteria).
- The architectural commitments from Vidvik triage Batch 1 are implemented:
  - Bus delivery classes (architecture §3.1) — authoritative messages with snapshot-plus-deltas reconnect.
  - Steel resource limits (architecture §9.4.1) — host-enforced CPU budget, recursion depth, fact-write quota with cooperative cancellation.
  - Trace retention (architecture §10.2) — snapshot-and-truncate with declared `why?` horizon.

The Editor MVP does **not** repeat these commitments; it depends on them.

---

## Gates

Four gates must hold before the Editor MVP is considered shipped. Each is testable; none is partial.

### Gate 1 — Undo / Redo

A committed undo design, implemented end-to-end, that recovers from edits without losing work.

Per the resolution of [`07-open-questions.md` §18](07-open-questions.md), the committed shape is **(c) + (d)**: content components carry lightweight version tags as part of their update model, and a **governed history service** reads them to implement undo/redo.

Acceptance:

- Every text-editing command produces a versioned snapshot of the affected content component, with provenance.
- The history service authoritatively owns history facts; `undo` and `redo` are requests addressed to it.
- Behavior effects that are pure derivations (e.g., `dirty`) re-derive correctly on revert.
- Behavior effects authored elsewhere (asserted facts in response to edits) are explicit concerns of the history service — not silently lost, not silently kept.
- Undo/redo work across reflective-loop reloads of behaviors that touched the affected content.

### Gate 2 — Async Tasks with Cancellation

A task model for long-running work (compile, test, search, indexing) that does not freeze the editor and surfaces failure as facts.

Acceptance:

- Tasks are first-class entities (`task/start`, `task/running`, `task/output-stream`, `task/exit-code`, `task/cancelled`).
- Task output arrives over the bus as `stream-item` (lossy delivery class is acceptable for output; `task/exit-code` is authoritative).
- Cancellation is universal — every running task is cancellable from a known action; cancellation propagates to the underlying process.
- Failure surfaces as `task/exit-code` plus structured error facts; behaviors can react to either.
- A reference task service ships with the Editor MVP; it covers shell execution and a wrapped compile/test runner.

### Gate 3 — Runaway-Steel Containment in Production Conditions

The host-enforced limits from architecture §9.4.1 hold under realistic load, not just in unit tests.

Acceptance:

- A deliberately runaway Steel behavior (infinite loop, unbounded recursion, fact-assertion storm) is interrupted within its declared latency class without affecting other behaviors' responsiveness.
- Interrupted behaviors produce structured trace entries with cause and partial output.
- Tentative writes from interrupted behaviors are rolled back; the fact space is consistent after interruption.
- A persistent offender (interrupted N times within window W) is automatically quarantined OR surfaces a clear operator action to quarantine it. Threshold and behavior are documented.
- Documentation includes the per-firing CPU budget, recursion depth, and write quota with rationale and tuning guidance.

### Gate 4 — Magit-Grade Hunk Staging

The full hunk-staging-to-commit workflow runs entirely within Weaver. This is the diagnostic for the core-orchestrates-always rule (architecture §11) under a real multi-authority workload.

Acceptance:

- Diff hunks are first-class entities with stable identity across re-derivation.
- Hunk facts include staged/unstaged status, parent file, line range, and content.
- Actions exist for: stage hunk, unstage hunk, split hunk, discard hunk, commit staged.
- Action invocation flows through the core (per architecture §11), not direct UI-to-git-service calls.
- `why?` on any hunk action returns a coherent causal chain.
- The full workflow (open status → stage two hunks → split a third → commit) completes in fewer than ten user actions and matches Magit's responsiveness within the interactive latency class.

This gate is the workflow defined in [`06-workflows.md` §5](06-workflows.md) (added in Vidvik triage Batch 3).

---

## Non-Goals (Editor MVP)

These are deferred beyond the Editor MVP. They are **not** prerequisites for daily use by a willing first customer.

- **Cross-machine distribution** — Editor MVP is single-machine; the distribution story is real architecturally but does not need to be exercised here.
- **Org-mode parity** — agenda, capture, links, export, narrowing in their full form are post-Editor-MVP work.
- **Terminal multiplexing inside Weaver** — task output streams suffice; full terminal emulation is not in scope.
- **Collaborative editing / shared cursor** — cursor remains client-local per [`07-open-questions.md` §19](07-open-questions.md). Promotion API for shared cursor facts is post-Editor-MVP.
- **Project-scale search and indexing as core features** — a search service may exist, but the index lifecycle, cross-project federation, and incremental-index-as-fact-source are post-Editor-MVP.
- **TRAMP-equivalent remote editing** — distributed services architecture supports it; first implementation is post-Editor-MVP.

---

## Acceptance Criteria

The Editor MVP is accepted when **all four gates** pass independently and one continuous-use scenario passes end-to-end:

**Continuous-use scenario.** A first customer (the Vidvik persona, or a real first customer) uses Weaver as their primary editor for a four-hour focused session that includes:

1. Opening and editing files across at least three projects.
2. Running compile/test cycles with output review and error navigation (Gate 2).
3. Performing real Git work including hunk staging and a commit (Gate 4).
4. Authoring or modifying at least one Steel behavior in the same session and observing it take effect via the reflective loop.
5. Recovering from at least one mistake using undo/redo (Gate 1).
6. Encountering a deliberately broken behavior and observing the host contain it (Gate 3) without losing session state.

The session must end with the customer reporting: "I could do this for a real day's work."

---

## Open Risks

These are known unresolved concerns that may surface during Editor MVP work. They do not block the milestone definition; they will be tracked.

- **Service scaffolding ergonomics** ([`07-open-questions.md` §15](07-open-questions.md), Vidvik AC3): if writing a service costs significantly more than `defun`, the promotion path from user-scratch to governed erodes. The Editor MVP makes this concrete by requiring a reference task service (Gate 2) and a hunk-staging diagnostic (Gate 4) — both of which exercise service authoring.
- **Core as semantic bottleneck** (Vidvik AC1): the hunk-staging workflow (Gate 4) is the diagnostic. If core orchestration produces unacceptable latency or coupling for that workflow, architecture §11 must be revisited.
- **Cursor promotion** (Vidvik UQ2): completion, contextual actions, rename, and region operations all want cursor-aware behavior. Editor MVP defers the full promotion API but should surface concrete pain points to inform post-Editor-MVP design.
