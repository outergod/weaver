# Composition Model

## Behaviors

Defined as:
- event triggers
- fact predicates
- actions

Example:

```lisp
(behavior format-on-save
  (on buffer/saved
    (when (language $buffer :rust)
      (emit formatter/request))))
```

---

## Properties

- composable
- introspectable
- debuggable

---

## Debugging

Users can inspect:
- why a behavior fired
- which facts matched
- resulting actions

