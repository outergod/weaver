# Interaction Model

Weaver is an interactive system centered on contextual applicability and discoverability.

## 1. Primary Question

The central interaction question is:

> What can I do here now?

This is answered from shared semantic state, not from UI structure.

---

## 2. Leader-Key Navigation

Leader menus are derived from:

- current entity context
- available facts
- applicable behaviors
- available services

Leader menus are semantic projections.

Each UI may render them differently.

---

## 6. Actions and Availability

Actions do not belong to entities.

They become available when:

- context satisfies behavior conditions
- required services are available
- relevant facts are present

This applicability is part of shared semantic state.

---

## 7. Explainability in Interaction

Users must be able to inspect:

- why an action is available
- which facts contributed
- which behaviors matched
- which services participated

This inspection is independent of how the UI presents it.

---

## 8. UI Independence

Different UIs may:

- render the same context differently
- derive additional views
- emphasize different aspects of state

The system does not require a canonical presentation.

---

## 9. Local Interaction State

Some interaction state may be client-local, including:

- layout
- focus within panes
- transient selections
- visual filters

This state:

- does not participate in global behavior
- does not alter shared semantic truth

---

## 10. Shared Interaction State

Some interaction state is shared because it affects system semantics.

Examples:

- compare targets
- active tasks
- selected entities (optional, policy-dependent)
- workspace context (optional, policy-dependent)

This state is represented as facts and participates in behavior.
