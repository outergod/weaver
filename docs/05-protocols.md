# Protocols

This document describes the conceptual protocol requirements for communication between core and services.

## 1. Message Categories

Weaver communication consists of messages in at least these categories:

- event
- fact-assert
- fact-retract
- request
- response
- stream-item
- lifecycle
- error

---

## 2. Events

An event message represents occurrence or intent.

An event should include:

- event name
- payload
- source
- causal parent if any
- timestamp or sequence identifier

Events are transient and may trigger behavior.

---

## 3. Facts

A fact assertion should include:

- entity reference
- attribute or relation name
- value
- source
- authority status
- derivation metadata if applicable
- freshness metadata

A fact retraction should identify the fact being withdrawn and why.

---

## 4. Requests and Responses

Requests are explicit asks for work or information.

Responses may be:

- immediate
- deferred
- streaming
- partial
- final

All nontrivial requests should be cancellable.

---

## 5. Lifecycle Messages

Services must be able to communicate lifecycle state such as:

- started
- ready
- degraded
- unavailable
- restarting
- stopped

This information must be available to interaction and tracing layers.

---

## 6. Error Requirements

Errors must be structured.

They should include:

- source
- category
- message
- affected request or event if any
- retryability if known

Silent failure is not acceptable protocol behavior.

---

## 7. Schema Requirements

Names, payloads, and semantics should be schema-governed.

At minimum, the protocol must support:

- namespacing
- versioning
- explicit optionality
- backward-compatibility strategy

Typed facts and typed events are desirable.
Untyped entities remain acceptable and preferable.
