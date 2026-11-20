//! The Oxreal hierarchy: parent/child relations and transform propagation ([ADR-0005],
//! [ADR-0014]).
//!
//! **Layer:** runtime. Built on `oxreal-ecs` hooks and systems. It owns no data outside the ECS:
//! the hierarchy is a set of components.
//!
//! - [`ChildOf`] links an entity to its parent. Insert it, or use [`set_parent`], to attach.
//!   Remove it, or use [`remove_parent`], to detach. Hooks keep the parent's ordered
//!   [`Children`] in step, and refuse links to dead or despawning parents and links that would
//!   form cycles. Both components are immutable, so no change can skip those hooks.
//! - **When a parent is despawned, each child follows its link's [`OnParentDespawn`]:** it's
//!   detached and keeps its place in the world (the default, as in Unreal), or it's despawned
//!   too, before its parent (`ChildOf::new(parent).despawn_with_parent()`, for the parts of one
//!   object).
//! - **Attaching and detaching take a [`TransformRule`]:** keep the local transform, or keep the
//!   place in the world.
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
//!     ChildOf::new(ship).despawn_with_parent(),
//! ));
//!
//! let mut schedule = Schedule::new();
//! add_systems(&mut schedule);
//! schedule.run(&mut world).expect("the schedule is valid");
//!
//! let turret_world = world.get::<WorldTransform>(turret).map(WorldTransform::translation);
//! assert_eq!(turret_world, Some(Vec3::new(10.0, 2.0, 0.0)));
//!
//! world.despawn(ship); // The turret goes too: it was attached with `despawn_with_parent`.
//! assert!(world.is_empty());
//! ```
//!
//! [ADR-0005]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0005-engine-owned-ecs-and-scene-hierarchy.md
//! [ADR-0014]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0014-ecs-architecture-and-storage.md

#![forbid(unsafe_code)]

mod relation;
mod transform;

pub use relation::{
    ChildOf, Children, HierarchyError, MAX_DEPTH, OnParentDespawn, remove_parent, set_parent,
};
pub use transform::{
    LocalTransform, PROPAGATE_TRANSFORMS, TransformRule, WorldTransform, add_systems,
    propagate_transforms,
};

#[cfg(test)]
mod tests;
