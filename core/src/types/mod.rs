//! Domain types — entities, facts, events, bus messages.
//!
//! The types here are the contract surface for everything that moves on
//! the bus. Storage, transport, and rendering are built around these
//! shapes; changing a type here is a public-surface change per L2 P7.

pub mod buffer_entity;
pub mod edit;
pub mod entity_ref;
pub mod event;
pub mod fact;
pub mod ids;
pub mod message;
