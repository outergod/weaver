# Weaver Technology Considerations

## Core

Being the obvious choice for this kind of endeavor, Rust is the main candidate for the Core because of the language and runtime traits:

- Cross-platform
- Highly performant
- Largely memory-safe

## Composition Language

**Steel** — a Scheme implementation in Rust, embedded in the core process. Rationale and commitments are detailed in composition-model §7. Key properties:

- Homoiconic, live-evaluable, macro-capable — preserves the reflective loop
- Ergonomic Rust interop (`register_fn` one-liners; `steel-derive` for types)
- Sandboxable via host-controlled capability exposure
- Available today; an existing proof-of-concept demonstrates the Rust-primitive / Scheme-composition split in practice

Steel is version-pinned at the project level; upstream changes are adopted deliberately.

## Services

Services are independent processes (or in-core sandboxes) that speak the bus protocol. Weaver distinguishes two shapes of first-party language support.

### Bus SDKs

First-party libraries that implement the bus protocol for a target language, so users can write standalone services in that language. Rust is the reference implementation. Additional SDKs (Go, Python, Node.js, others) follow as demand justifies. A bus SDK has no host runtime — it is the wire protocol rendered as a library.

Compiled languages (Go, C, C++, Rust itself) use the SDK path exclusively.

### Language Hosts

First-party services that run user code inside an interpreted or embeddable runtime and proxy it onto the bus. Initial targets: **Python** and **Node.js**.

The implementation language of a language host is the language it hosts (Python host in Python, Node host in Node). This keeps semantic fidelity, stack legibility, and ecosystem packaging (pip, npm) natural for contributors and users.

**Go is explicitly not a language-host target.** AOT-compiled languages do not fit the host pattern; Go users write standalone services via the Go SDK.

Language hosts are authority proxies for the code they host; see architecture §9.1.1 for the responsibility and provenance model.

### Scaffolding

A service scaffolding path is a first-class project concern: writing a minimum-viable service must approach `defun`-level ceremony (templates, trivial registration, hot reload) to preserve extension velocity when composed behaviors must be promoted.

## UI

Web-based, but OS-native browser; probably Tauri to get the best of all worlds (full control without Electron bloat). CodeMirror, unless a better alternative exists.
