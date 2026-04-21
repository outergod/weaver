# Reference Workflows

These workflows exist to keep Weaver grounded in real use.

## 1. Project Navigation

### Scenario
The user opens a file in a repository-backed project and asks what can be done here.

### Expected progression
1. Core emits an event indicating a buffer was opened.
2. Core asserts basic facts about the entity representing that open buffer.
3. Project service recognizes project membership from path facts.
4. Git service recognizes repository participation from project-related facts.
5. Relevant behaviors make project and git actions applicable.
6. Leader menu derives current actions from context.

### Properties tested
- contextual applicability
- fact propagation
- service cooperation
- explainable leader menu generation

---

## 2. Cross-Workspace Comparison

### Scenario
The user wants to compare two open buffers associated with different workspaces or projects.

### Expected progression
1. Two entities are marked as compare candidates.
2. Facts express that these entities are selected for comparison.
3. A comparison behavior recognizes a valid compare context.
4. A compare action becomes applicable.
5. The user can inspect why comparison is available.

### Properties tested
- workspaces as lenses rather than containers
- cross-context action derivation
- non-hierarchical applicability

---

## 3. Git-Related Action Projection

### Scenario
The user is focused on a buffer that belongs to a project associated with a git repository.

### Expected progression
1. Project membership facts already hold.
2. Git service publishes repository-related facts.
3. Behaviors infer applicability of git-related actions.
4. Leader menu exposes git operations in context.
5. The user can inspect which facts and services contributed.

### Properties tested
- multi-service context assembly
- action projection without object methods
- provenance and explainability

---

## 4. Degraded Service Experience

### Scenario
A service becomes unavailable while the core remains active.

### Expected progression
1. Lifecycle message signals service degradation or loss.
2. Related facts become stale, unavailable, or explicitly degraded.
3. Dependent actions disappear or are marked unavailable.
4. The user can inspect the reason.

### Properties tested
- graceful degradation
- explicit lifecycle representation
- trustworthy interaction model

---

## 5. Multi-Actor Contribution Review

### Scenario
An external actor — an agent acting on a user instruction, or an autonomous service operating under user delegation — proposes a non-authoritative contribution to the shared semantic state. The user reviews the contribution through the standard inspection channel before accepting, refusing, or refining it.

### Expected progression
1. An actor A publishes a contribution (a proposed fact assertion, a proposed action invocation, or a derived view) carrying `source: A` in provenance and, where applicable, `on-behalf-of: user` in the delegation subfield (protocols §3.4).
2. The fact space admits the contribution under its authority rules: if A holds authority over the relevant family, the assertion is authoritative; otherwise it lands as user-scratch or as a marked speculative derivation (see `07-open-questions.md §24`).
3. A behavior derives applicability of inspection actions on the contribution entity: `inspect-contribution`, `accept-contribution`, `refuse-contribution`.
4. The user invokes `inspect-contribution`. `why?` returns the full delegation chain: the contribution, the actor that published it, the `on-behalf-of` user if any, the causal event that triggered A's action, and any derivation context.
5. The user invokes `refuse-contribution`. The authority owning the contested fact family retracts the contribution; facts that depended on it retract via causal provenance.
6. The trace records the contribution, the inspection, and the refusal as first-class entries with full provenance.

### Properties tested
- actor identity in provenance across actor kinds (constitution §17)
- delegation chain inspection (`on-behalf-of`; protocols §3.4)
- user sovereignty as a mechanism — refusal propagates through the provenance graph (constitution §17 user-sovereignty clause)
- reversibility of non-user contributions (§17)
- reconcilable convergence without silent coalescence (constitution §11)

### Failure modes worth surfacing
- A's authority claim conflicts with an existing authority — does the system surface the conflict (per §11) or accept silently?
- Delegation chain cannot be validated (A claims `on-behalf-of` but the claimed delegator never authorized) — does the contribution land as user-scratch, as rejected, or as flagged?
- User refuses a contribution that other behaviors have already acted on — do downstream effects retract along the causal chain (§11 reconcilability)?

### Why this workflow exists
This workflow makes constitution §17 (Multi-Actor Coherence) and the user-sovereignty clause operational. Without it, §17 remains a framing-level invariant with no scenario showing how refusal, delegation inspection, and conflict surfacing manifest. It is explicitly *not* tied to the editor projection MVP — it gates the first non-editor / multi-actor slice (see the pivot follow-up in `07-open-questions.md §23–§25`).

---

## 6. Hunk Staging to Commit

### Scenario
The user has uncommitted changes in a Git-backed project. They want to stage some hunks, split a multi-purpose hunk, discard one, and commit only the staged work — entirely within Weaver, at editor speed.

This workflow is the diagnostic for the *core orchestrates multi-authority actions* rule (architecture §11) under a real, daily-use multi-authority load. It is also a Gate of the Editor-Projection MVP (`mvp-editor-projection.md` Gate 4).

### Expected progression
1. Git service publishes facts about the working tree: per-file modification status, per-hunk diff content, staged vs. unstaged status. Hunk entities have stable IDs derived from `(file-path, hunk-anchor)` so they survive re-derivation.
2. Behaviors derive applicability of `stage-hunk`, `unstage-hunk`, `split-hunk`, `discard-hunk`, and `commit-staged` actions on the relevant entities.
3. The user invokes `stage-hunk` on hunk H1. The action request flows to the **core**, which orchestrates: validates applicability against current facts, issues a `git/stage-hunk` request to the git service, observes the resulting fact updates (`hunk/staged: true`), and publishes the action's completion in the trace.
4. The user invokes `split-hunk` on hunk H2. The git service performs the split (an internal mutation of its own fact family); new hunk entities materialize with stable IDs derived from the split anchors. The user stages one of the resulting sub-hunks; the other remains unstaged.
5. The user invokes `discard-hunk` on hunk H3. Same orchestration shape; the git service retracts the hunk's facts; the action entity ceases.
6. The user invokes `commit-staged`, providing a message. The core orchestrates: validates that at least one hunk is staged, issues `git/commit` to the git service, observes commit-id facts in response, and publishes the action's completion.
7. `why?` on any action invocation in the chain returns: triggering event, contributing behaviors, fact predicates that matched, the request issued, the response received, and the resulting fact deltas.
8. Throughout, no UI calls reach the git service directly — every state-changing operation flows through the core (architecture §11).

### Properties tested
- multi-authority action orchestration through the core
- stable entity identity for hunks across split/stage/discard re-derivation
- action provenance under a real workload (not a toy "open file" scenario)
- interactive latency class (`docs/02-architecture.md §7.1`) under realistic Git operations
- the rejection of UI-to-service shortcuts (architecture §11 last line)
- the bus delivery class boundary: hunk-fact updates are authoritative; per-character output streams (e.g., from a long `git log`) are lossy

### Failure modes worth surfacing
- Git service slow to respond on a large repository — does the action remain interruptible? Does its applicability fact go stale visibly?
- Concurrent edit to the working tree during staging — do hunk entity IDs survive? If anchors shift, what's the user-visible behavior?
- Core crash mid-orchestration — on restart, does the trace recover the in-flight action's state, or does the action entity reappear as not-yet-invoked?

### Why this workflow exists
Workflow 3 (Git-Related Action Projection) proves that git actions can be *exposed* contextually. This workflow proves they can be *executed* — through the core, under real load, with the orchestration rule intact. If this workflow cannot be made to feel like Magit, the core-orchestrates-always rule must be revisited (Vidvik triage AC1).
