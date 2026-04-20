//! Fact space — `FactStore` trait and in-memory implementation.
//!
//! ECS-library decision (Bevy / Hecs / Flecs / custom archetype) is
//! intentionally deferred — see `specs/001-hello-fact/research.md` §13.
//! For Hello-fact the trait is backed by a `HashMap<FactKey, Fact>`.
//!
//! Trait + `InMemoryFactStore` land in T021 / T022.
