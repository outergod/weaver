//! Weaver core library — types, fact space, bus, behavior dispatcher.
//!
//! See `specs/001-hello-fact/plan.md` for the slice this library implements.
//!
//! # Module overview
//!
//! - [`provenance`] — provenance metadata required on every fact, event, and
//!   trace entry per L2 P11.
//! - [`types`] — domain types (entity references, IDs, facts, events, bus
//!   messages).
//! - [`fact_space`] — `FactStore` trait and `InMemoryFactStore`. The
//!   ECS-library decision is deferred per `specs/001-hello-fact/research.md`
//!   §13.
//! - [`bus`] — Unix-socket bus listener, CBOR codec, delivery-class
//!   enforcement.
//! - [`trace`] — append-only trace store with reverse causal index.
//! - [`behavior`] — dispatcher and embedded behaviors.
//! - [`inspect`] — bus-level inspection request/response handler (FR-008).
//! - [`cli`] — clap-based CLI entry points and subcommands.
//!
//! Module bodies are populated by tasks T011 onward in
//! `specs/001-hello-fact/tasks.md`.

pub mod provenance;
pub mod types;
pub mod fact_space;
pub mod bus;
pub mod trace;
pub mod behavior;
pub mod inspect;
pub mod cli;
