//! Message bus — Unix-socket listener, CBOR codec, delivery-class
//! enforcement per `docs/02-architecture.md` §3.1 and L2 P5.

pub mod client;
pub mod codec;
pub mod delivery;
pub mod event_subscriptions;
pub mod listener;
