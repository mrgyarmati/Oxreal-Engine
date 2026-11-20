//! Local and world transforms, and their propagation down the hierarchy (ADR-0005).

use oxreal_ecs::{
    Component, ComponentHooks, Entity, HookContext, HookWorld, Schedule, Stage, System,
    SystemWorld, With, Without, World,
};
use oxreal_math::{Affine3A, Mat4, Transform, Vec3};

use crate::relation::{ChildOf, Children, MAX_DEPTH};

/// An entity's placement relative to its parent, or to the world if it has none. This is what
/// gameplay and the editor write.
///
/// Adding it also adds a [`WorldTransform`], which [`propagate_transforms`] keeps up to date.
/// Removing it removes the `WorldTransform` too, so nothing is drawn at a stale place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalTransform(pub Transform);

impl Component for LocalTransform {
    const HOOKS: ComponentHooks = ComponentHooks::NONE
        .on_add(local_transform_added)
        .on_remove(local_transform_removed);
}

impl Default for LocalTransform {
    fn default() -> Self {
        Self(Transform::IDENTITY)
    }
}

/// An entity's placement in the world: its parent's world transform times its
/// [`LocalTransform`] (`world = parent * local`).
///
/// It's computed by [`propagate_transforms`] in [`Stage::PostUpdate`]. So it's current from
/// there through `Extract` (read it there, for rendering), and stale from the moment gameplay
/// changes a `LocalTransform` until the next propagation. Code that needs an up-to-date world
/// placement mid-frame computes it from the local transforms instead, as
/// [`TransformRule::KeepWorld`] does.
///
/// It's an affine matrix rather than a [`Transform`], so non-uniform scale under rotation stays
/// exact. Only the propagation writes it. To place an entity in the world, set its
/// `LocalTransform` relative to its parent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldTransform(Affine3A);

impl Component for WorldTransform {}

impl Default for WorldTransform {
    fn default() -> Self {
        Self(Affine3A::IDENTITY)
    }
}

impl WorldTransform {
    /// The affine transform from the entity's space to world space.
    #[must_use]
    pub fn affine(&self) -> Affine3A {
        self.0
    }

    /// The same as a 4 × 4 matrix, for shaders.
    #[must_use]
    pub fn matrix(&self) -> Mat4 {
        Mat4::from(self.0)
    }

    /// The entity's origin in world space.
    #[must_use]
    pub fn translation(&self) -> Vec3 {
        self.0.translation.into()
    }
}

/// What an entity keeps when it's attached or detached (Unreal's attachment rules).
///
/// [`crate::set_parent`] and [`crate::remove_parent`] take one explicitly. There's no default,
/// because keeping the wrong one makes objects jump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TransformRule {
    /// Keep the `LocalTransform` as it is. Its meaning changes with the parent, so the entity
    /// may move in the world.
    KeepLocal,
    /// Keep the entity where it is in the world: its `LocalTransform` is recomputed relative to
    /// the new parent (or to the world, when detaching).
    ///
    /// It's computed from the local transforms up the chain, so it's exact at any point in the
    /// frame. Non-uniform scale under rotation can't always be kept exactly: a `Transform` holds
    /// no shear, which Unreal's `FTransform` can't either.
    KeepWorld,
}

fn local_transform_added(world: &mut HookWorld<'_>, context: HookContext) {
    let entity = context.entity;
    if !world.has::<WorldTransform>(entity) {
        world.commands().run(move |world| {
            if world.has::<LocalTransform>(entity) && !world.has::<WorldTransform>(entity) {
                // An entity that started despawning needs no transform: ignore its refusal.
                let _ = world.insert(entity, WorldTransform::default());
            }
        });
    }
}

fn local_transform_removed(world: &mut HookWorld<'_>, context: HookContext) {
    let entity = context.entity;
    world.commands().run(move |world| {
        if !world.has::<LocalTransform>(entity) {
            world.remove::<WorldTransform>(entity);
        }
    });
}

fn affine(transform: &Transform) -> Affine3A {
    Affine3A::from_scale_rotation_translation(
        transform.scale,
        transform.rotation,
        transform.translation,
    )
}

/// `entity`'s world placement, computed now from the local transforms up its chain. An entity
/// without a `LocalTransform` contributes nothing.
pub(crate) fn world_placement(world: &World, entity: Entity) -> Affine3A {
    let mut placement = Affine3A::IDENTITY;
    let mut current = Some(entity);
    for _ in 0..MAX_DEPTH {
        let Some(e) = current else { break };
        if let Some(local) = world.get::<LocalTransform>(e) {
            placement = affine(&local.0) * placement;
        }
        current = world.get::<ChildOf>(e).map(ChildOf::parent);
    }
    placement
}

/// Sets `entity`'s local transform so it's placed at `placement` relative to `parent_placement`.
fn place(world: &mut World, entity: Entity, parent_placement: Affine3A, placement: Affine3A) {
    let local = parent_placement.inverse() * placement;
    let (scale, rotation, translation) = local.to_scale_rotation_translation();
    let transform = Transform {
        translation,
        rotation,
        scale,
    };
    if let Some(mut existing) = world.get_mut::<LocalTransform>(entity) {
        existing.0 = transform;
    } else if !local.abs_diff_eq(Affine3A::IDENTITY, 1e-6) {
        // An entity without a local transform was placed by its parent alone. It needs one now
        // to stay where it is. Ignore a refusal: a despawning entity needs no transform.
        let _ = world.insert(entity, LocalTransform(transform));
    }
}

/// Before `child` is attached to `parent`: keep its place in the world.
pub(crate) fn keep_world_on_attach(world: &mut World, child: Entity, parent: Entity) {
    let placement = world_placement(world, child);
    let parent_placement = world_placement(world, parent);
    place(world, child, parent_placement, placement);
}

/// Before `child` is detached from its parent: keep its place in the world.
pub(crate) fn keep_world_on_detach(world: &mut World, child: Entity) {
    let placement = world_placement(world, child);
    place(world, child, Affine3A::IDENTITY, placement);
}

/// The system that computes every [`WorldTransform`] from the [`LocalTransform`]s down the
/// hierarchy. It walks each tree from its root without recursion, so any depth is fine.
///
/// An entity without a `LocalTransform` passes its parent's world transform on unchanged.
/// Add it with [`add_systems`], or to [`Stage::PostUpdate`] directly. Its name is
/// [`PROPAGATE_TRANSFORMS`], for ordering other `PostUpdate` systems after it.
#[must_use]
pub fn propagate_transforms() -> System {
    let mut stack: Vec<(Entity, Affine3A)> = Vec::new();
    System::new(PROPAGATE_TRANSFORMS, move |ctx| {
        stack.clear();
        // Every tree starts at an entity without a parent.
        let roots = ctx
            .world
            .query_filtered::<Entity, (Without<ChildOf>, With<LocalTransform>)>()
            .chain(ctx.world.query_filtered::<Entity, (
                Without<ChildOf>,
                Without<LocalTransform>,
                With<Children>,
            )>());
        stack.extend(roots.map(|root| (root, Affine3A::IDENTITY)));
        while let Some((entity, parent_transform)) = stack.pop() {
            let world_transform = propagate_one(&mut ctx.world, entity, parent_transform);
            if let Some(children) = ctx.world.get::<Children>(entity) {
                // Reversed, so the stack visits children in order.
                stack.extend(children.iter().rev().map(|&child| (child, world_transform)));
            }
        }
    })
    .reads::<LocalTransform>()
    .reads::<Children>()
    .writes::<WorldTransform>()
}

/// The name of the [`propagate_transforms`] system, for `before`/`after` constraints.
pub const PROPAGATE_TRANSFORMS: &str = "propagate transforms";

/// Computes and stores one entity's world transform. Returns it, for its children.
fn propagate_one(world: &mut SystemWorld<'_>, entity: Entity, parent: Affine3A) -> Affine3A {
    let transform = match world.get::<LocalTransform>(entity) {
        Some(local) => parent * affine(&local.0),
        None => parent,
    };
    if let Some(mut stored) = world.get_mut::<WorldTransform>(entity) {
        stored.0 = transform;
    }
    transform
}

/// Adds the hierarchy's systems to `schedule`: [`propagate_transforms`] in
/// [`Stage::PostUpdate`], after gameplay and physics and before extraction (ADR-0005).
pub fn add_systems(schedule: &mut Schedule) {
    schedule.add(Stage::PostUpdate, propagate_transforms());
}
