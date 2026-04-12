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
