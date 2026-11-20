//! The Oxreal entity component system ([ADR-0014]).
//!
//! **Layer:** runtime. A [`World`] holds **entities** (generational [`Entity`] handles), their
//! **components** (plain data types implementing [`Component`]) and **resources** (typed
//! singletons implementing [`Resource`]). **Queries** iterate every entity that has a set of
//! components.
//!
//! ```
//! use oxreal_ecs::{Component, With, World};
//!
//! struct Position(f32, f32);
//! impl Component for Position {}
//! struct Velocity(f32, f32);
//! impl Component for Velocity {}
//! struct Player;
//! impl Component for Player {}
//!
//! let mut world = World::new();
//! let player = world.spawn((Position(0.0, 0.0), Velocity(1.0, 0.5), Player));
//! world.spawn((Position(5.0, 5.0), Velocity(0.0, -1.0)));
//!
//! // Move everything that has a velocity.
//! for (position, velocity) in world.query_mut::<(&mut Position, &Velocity)>() {
//!     position.0 += velocity.0;
//!     position.1 += velocity.1;
//! }
//!
//! // Only players: a filter, like a role test (ADR-0008).
//! let players = world.query_filtered::<&Position, With<Player>>().count();
//! assert_eq!(players, 1);
//! assert!(world.has::<Player>(player));
//! ```
//!
//! # Model
//!
//! - **Storage is private** (ADR-0014 §1). Entities with the same component types share an
//!   archetype table, one column per type, so queries walk dense arrays. No storage type appears
//!   in this API, so storage can change on benchmark evidence without breaking callers.
//! - **Entity handles are generational.** A despawned entity's handle never refers to a later
//!   entity. Using it just fails ([`World::get`] returns `None`, [`World::insert`] returns
//!   [`EcsError::NoSuchEntity`]).
//! - **Queries are checked for aliasing** when created: `(&mut A, &A)` panics instead of
//!   producing two references to the same value.
//! - Programming errors panic with a message that names the type involved. Conditions that
//!   depend on runtime state, such as a stale handle, return errors or `None` (ADR-0010).
//!
//! This is the first slice of ADR-0014. Deferred commands and schedule stages come next, then
//! relations and hooks, then change detection and dynamic (reflection-driven) queries.
//!
//! [ADR-0014]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0014-ecs-architecture-and-storage.md

#![forbid(unsafe_code)]

mod bundle;
mod component;
mod entity;
mod query;
mod resource;
mod storage;
mod world;

pub use bundle::Bundle;
pub use component::Component;
pub use entity::Entity;
pub use query::{QueryData, QueryFilter, QueryIter, ReadOnlyQueryData, With, Without};
pub use resource::Resource;
pub use world::{EcsError, World};

#[cfg(test)]
mod tests;
