//! Local and world transforms, and their propagation down the hierarchy (ADR-0005).

use oxreal_ecs::{
    Component, ComponentHooks, Entity, HookContext, HookWorld, Schedule, Stage, System,
    SystemWorld, With, Without,
};
use oxreal_math::{Affine3A, Mat4, Transform, Vec3};

use crate::relation::{ChildOf, Children};

/// An entity's placement relative to its parent, or to the world if it has none. This is what
/// gameplay and the editor write.
///
/// Adding it also adds a [`WorldTransform`], which [`propagate_transforms`] keeps up to date.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalTransform(pub Transform);

impl Component for LocalTransform {
    const HOOKS: ComponentHooks = ComponentHooks::NONE.on_add(local_transform_added);
}

impl Default for LocalTransform {
    fn default() -> Self {
        Self(Transform::IDENTITY)
    }
}

/// An entity's placement in the world: its parent's world transform times its
/// [`LocalTransform`] (`world = parent * local`). Computed by [`propagate_transforms`] in
/// [`Stage::PostUpdate`], so it's current from then until gameplay changes local transforms
/// again. Read it in `Extract`, for rendering.
///
/// It's an affine matrix rather than a [`Transform`], so non-uniform scale under rotation stays
/// exact. Only the propagation writes it.
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

fn local_transform_added(world: &mut HookWorld<'_>, context: HookContext) {
    let entity = context.entity;
    if !world.has::<WorldTransform>(entity) {
        world.commands().run(move |world| {
            if world.contains(entity) && !world.has::<WorldTransform>(entity) {
                // Ignore the error: the entity started despawning, so it needs no transform.
                let _ = world.insert(entity, WorldTransform::default());
            }
        });
    }
}

fn affine(transform: &Transform) -> Affine3A {
    Affine3A::from_scale_rotation_translation(
        transform.scale,
        transform.rotation,
        transform.translation,
    )
}

/// The system that computes every [`WorldTransform`] from the [`LocalTransform`]s down the
/// hierarchy. It walks each tree from its root without recursion, so any depth is fine.
///
/// An entity without a `LocalTransform` passes its parent's world transform on unchanged.
/// Add it with [`add_systems`], or to [`Stage::PostUpdate`] directly.
#[must_use]
pub fn propagate_transforms() -> System {
    let mut stack: Vec<(Entity, Affine3A)> = Vec::new();
    System::new("propagate transforms", move |ctx| {
        stack.clear();
        // Every tree starts at an entity without a parent.
        let roots = ctx
            .world
            .query_filtered::<Entity, (Without<ChildOf>, With<LocalTransform>)>()
            .chain(
                ctx.world
                    .query_filtered::<Entity, (Without<ChildOf>, Without<LocalTransform>, With<Children>)>(),
            );
        stack.extend(roots.map(|root| (root, Affine3A::IDENTITY)));
        while let Some((entity, parent)) = stack.pop() {
            let world = propagate_one(&mut ctx.world, entity, parent);
            if let Some(children) = ctx.world.get::<Children>(entity) {
                // Reversed, so the stack visits children in order.
                stack.extend(children.iter().rev().map(|&child| (child, world)));
            }
        }
    })
    .reads::<LocalTransform>()
    .reads::<Children>()
    .writes::<WorldTransform>()
}

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
