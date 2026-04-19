# MVP Review Triage: Emacs Hacker (Vidvik)

Triages the findings in [`mvp-review-hacker.md`](mvp-review-hacker.md). Per item: action, target doc(s), rationale.

## Citation verification

All cited line numbers and section references in the review were checked against the source documents:

- `.specify/memory/constitution.md:95` (L2 P3) and `docs/02-architecture.md:252,256` (§9.4) — both accurate. The reviewer's "contradiction" framing is nuanced: P3 commits the *host* to surviving Steel infinite loop; §9.4 says other *behaviors* block until the runaway behavior yields. Spirit is in tension; letter is not strictly violated.
- `docs/02-architecture.md:74` (drop-oldest default) and `docs/07-open-questions.md:258` (§22 unresolved) — accurate.
- `docs/mvp.md:164` (undo excluded) and `docs/07-open-questions.md:170,195` (§18 leans c+d, not committed) — accurate.
- `docs/02-architecture.md:264` (trace append-only) and `docs/07-open-questions.md:147,166` (§17 leans d, not promoted) — accurate.

The reviewer was honest. No misreadings.

## Triage table

| ID | Source | Action | Target doc(s) | Rationale |
|---|---|---|---|---|
| **DB1 / SA1 / UQ1** | Undo deferred from MVP | **APPLY** (framing C resolved below) | `docs/mvp.md`, new `docs/mvp-editor.md`, `docs/07-open-questions.md` §18 | Cannot ship "editor MVP" with no undo. Resolved by milestone split, not by inclusion. |
| **DB2 / SA3** | Steel runaway can hang composition lane | **APPLY** | `docs/02-architecture.md` §9.4, `.specify/memory/constitution.md` P3 | Add CPU budget + cooperative cancellation + VM interruption to composition contract. Closes the L2/L1 spirit gap. |
| **DB3 / SA2 / AC2** | Bus delivery classes | **APPLY** | `docs/02-architecture.md` §3.1, `docs/05-protocols.md`, `docs/07-open-questions.md` §22 | Separate lossy telemetry from authoritative state transitions. Add sequence numbers, gap detection, snapshot/replay for the latter. Resolves §22. Subsumes AC2. |
| **SA4** | Hunk-staging workflow | **APPLY** | new `docs/06-workflows.md` §5 | Stress-tests AC1 (core-orchestrates-always) with a concrete multi-authority scenario. Diagnostic value high. |
| **UQ3** | Trace retention contract | **APPLY-MINOR** | `docs/02-architecture.md` §10, `docs/07-open-questions.md` §17 | Promote §17 lean (snapshot-and-truncate) to architectural commitment; declare `why?` horizon as a system property. |
| **AC1** | Core as semantic bottleneck | **DEFER** | (none yet) | Concern not contradiction; arch §11 explicitly justified the rule. SA4 will test it concretely; don't preemptively change. |
| **AC3** | Service-promotion ergonomics | **DEFER** | (post-MVP engineering note) | Real concern; arch §9.2 already admits "defun-cheap service" requirement. Not actionable until services exist. |
| **UQ2** | Cursor promotion API | **DEFER** | (post-Editor-MVP) | OQ §19 already commits to (b) local-by-default-opt-in; promotion API needs implementation experience. |

## Resolved framing decisions (2026-04-19)

**Undo (DB1) — Option C: milestone split.**

- Current `docs/mvp.md` is renamed/repositioned as the **Ontology Prototype** milestone. Its scope (browse, edit, save, behavior reload, `why?`, single non-trivial behavior end-to-end) is preserved. Honest naming: it proves the conceptual model, not editor-grade usability.
- A new `docs/mvp-editor.md` introduces the **Editor MVP** milestone with explicit gates:
  1. Undo/redo design committed and implemented (per OQ §18 lean: content-component versioning + governed history service).
  2. Steel runaway containment (per DB2: CPU budget + cancellation, not just author responsibility).
  3. Async task model + cancellation (compile, test, long-running queries) — currently a non-goal of the Ontology Prototype.
  4. Magit-grade hunk-staging workflow (per SA4) demonstrates multi-authority orchestration at editor speed.
- Promotion path: Ontology Prototype acceptance criteria stay as-is; Editor MVP layers gates on top.

## Amendment plan

Five amendments, three commit batches.

### Batch 1 — composition lane safety + bus delivery (DB2, DB3, UQ3)

Touches `docs/02-architecture.md` (§3.1, §9.4, §10), `docs/05-protocols.md`, `docs/07-open-questions.md` (§17, §22), `.specify/memory/constitution.md` (P3 + version bump 0.2.0 → 0.3.0).

Single conceptual change ("the protocol is honest about what it delivers and what user code can do to it"); single commit.

### Batch 2 — milestone split (DB1)

Touches `docs/mvp.md` (rename context to "Ontology Prototype"), creates `docs/mvp-editor.md`, updates `docs/07-open-questions.md` §18 to mark the lean as committed-for-Editor-MVP.

Separate commit; reframes scope rather than adding architecture.

### Batch 3 — diagnostic workflow (SA4)

Adds `docs/06-workflows.md` §5 hunk-stage-to-commit. Pure addition; no contracts changed.

Separate commit; this is the diagnostic that will validate or invalidate AC1 over time.

## Out of scope (this round)

- **AC1 core-bottleneck**: re-evaluate after SA4 ships and after Editor MVP scoping work surfaces real multi-authority workflows.
- **AC3 service ergonomics**: revisit when service scaffolding is real.
- **UQ2 cursor promotion API**: revisit when the first cursor-aware behavior is authored.
- **Historical commit rewriting**: out of scope per Conventional Commits amendment (Amendment 1).
- **Second persona**: deferred. Decide after Batch 1–3 land whether org-mode-lifer or ex-Emacs persona surfaces orthogonal concerns or just confirms these.

## Versioning impact

L2 constitution bump 0.2.0 → **0.3.0** (MINOR) for Batch 1: P3 expanded to require host-enforced interruption, not just survival. Materially expanded guidance; not backward-incompatible since no implementation exists to break.

L1 architecture document itself does not carry a SemVer; updates land via PR per its own implicit governance.
