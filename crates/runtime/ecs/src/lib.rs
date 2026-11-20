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
//! for (mut position, velocity) in world.query_mut::<(&mut Position, &Velocity)>() {
//!     position.0 += velocity.0;
//!     position.1 += velocity.1;
//! }
//!
//! // Only players: a filter, like a role test (ADR-0020 §2).
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
//!   [`EcsError::NoSuchEntity`]). A handle is only meaningful in the world that created it: in
//!   another world it may name an unrelated entity.
//! - **Mutable access goes through [`Mut`]**, a smart pointer used like `&mut T`. Writing through
//!   it marks the value changed; fetching it doesn't.
//! - **Queries are checked for aliasing** when created: `(&mut A, &A)` panics instead of
//!   producing two references to the same value.
//! - Programming errors panic with a message that names the type involved. Conditions that
//!   depend on runtime state, such as a stale handle, return errors or `None` (ADR-0010).
//!
//! # Systems and schedules
//!
//! Frame logic runs as [`System`]s in a [`Schedule`], stage by stage ([`Stage`]: input, then
//! Unreal's tick groups, then extraction). A system **declares** the components and resources it
//! reads and writes, and touching anything else panics, naming the missing declaration. Inside a
//! system, [`SystemContext`] offers queries (`ctx.world`), resources (`ctx.resources`) and
//! [`Commands`] (`ctx.commands`): structural changes recorded now and applied at the end of the
//! stage, in order. See [`Commands`] for an example.
//!
//! # Lifecycle hooks
//!
//! A component type can declare [hooks](ComponentHooks) that run when a component is added,
//! inserted, replaced or removed, to keep engine invariants such as relation inverses. Hooks
//! read and write data through a [`HookWorld`] and queue commands, which run right after the
//! change that triggered them.
//!
//! # Change detection
//!
//! Every component value and resource records when it was added and last changed. A system's
//! [`Added`] and [`Changed`] filters see what happened since its previous run: others' writes,
//! never its own, and everything on its first run. [`SystemResources::is_added`] and
//! [`SystemResources::is_changed`] do the same for resources. Queries made on the [`World`]
//! directly have no previous run, so there everything counts.
//!
//! Relations (`ChildOf` and `Children`) live in `oxreal-hierarchy`, built on hooks. Dynamic
//! (reflection-driven) queries follow.
//!
//! [ADR-0014]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0014-ecs-architecture-and-storage.md

#![forbid(unsafe_code)]

mod bundle;
mod change;
mod command;
mod component;
mod entity;
mod hook;
mod query;
mod resource;
mod schedule;
mod storage;
mod type_map;
mod world;

pub use bundle::Bundle;
pub use change::Mut;
pub use command::Commands;
pub use component::{Component, ComponentHooks, Hook, HookCause, HookContext};
pub use entity::Entity;
pub use hook::HookWorld;
pub use query::{
    Added, Changed, QueryData, QueryFilter, QueryIter, ReadOnlyQueryData, With, Without,
};
pub use resource::Resource;
pub use schedule::{
    Schedule, ScheduleError, Stage, System, SystemContext, SystemResources, SystemWorld,
};
pub use world::{EcsError, World};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod schedule_tests;

#[cfg(test)]
mod hook_tests;

#[cfg(test)]
mod change_tests;
