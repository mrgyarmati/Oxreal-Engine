//! The Oxreal hierarchy: parent/child relations and transform propagation ([ADR-0005],
//! [ADR-0014]).
//!
//! **Layer:** runtime. Built on `oxreal-ecs` hooks and systems. It owns no data outside the ECS:
//! the hierarchy is a set of components.
//!
//! - [`ChildOf`] links an entity to its parent. Insert it (or use [`set_parent`]) to attach,
//!   remove it to detach. Hooks keep the parent's ordered [`Children`] in step, and refuse links
//!   to dead parents and cycles.
//! - **Despawning a parent despawns its children first**, recursively. Each child's hooks see
//!   its parent whole (ADR-0014 §4). To keep the children, detach them first.
//! - [`LocalTransform`] places an entity relative to its parent. [`propagate_transforms`]
//!   computes every [`WorldTransform`] (`world = parent * local`) in `PostUpdate`.
//!
//! ```
//! use oxreal_ecs::{Schedule, World};
//! use oxreal_hierarchy::{ChildOf, LocalTransform, WorldTransform, add_systems};
//! use oxreal_math::{Transform, Vec3};
//!
//! let mut world = World::new();
//! let ship = world.spawn(LocalTransform(Transform::from_translation(Vec3::new(10.0, 0.0, 0.0))));
//! let turret = world.spawn((
//!     LocalTransform(Transform::from_translation(Vec3::new(0.0, 2.0, 0.0))),
//!     ChildOf::new(ship),
//! ));
//!
//! let mut schedule = Schedule::new();
//! add_systems(&mut schedule);
//! schedule.run(&mut world).expect("the schedule is valid");
//!
//! let turret_world = world.get::<WorldTransform>(turret).map(WorldTransform::translation);
//! assert_eq!(turret_world, Some(Vec3::new(10.0, 2.0, 0.0)));
//!
//! world.despawn(ship); // The turret goes too.
//! assert!(world.is_empty());
//! ```
//!
//! [ADR-0005]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0005-engine-owned-ecs-and-scene-hierarchy.md
//! [ADR-0014]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0014-ecs-architecture-and-storage.md

#![forbid(unsafe_code)]

mod relation;
mod transform;

pub use relation::{ChildOf, Children, HierarchyError, remove_parent, set_parent};
pub use transform::{LocalTransform, WorldTransform, add_systems, propagate_transforms};

#[cfg(test)]
mod tests;
