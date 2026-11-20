//! The cube example's scene as ECS data and systems (ADR-0014, ADR-0005).
//!
//! - **Entities:** the camera (a [`FlyCamera`]), the spinning cube with two small moons as its
//!   children, and the floor tiles and pillars. Each mesh has a `LocalTransform`.
//! - **Resources:** the frame's `InputState` (fed by the window events), [`FrameTime`], and the
//!   [`DrawList`] the renderer reads.
//! - **Systems:**
//!   - `PrePhysics`: gameplay moves the camera and spins the cube.
//!   - `PostUpdate`: `oxreal-hierarchy` propagates transforms, so the moons orbit with the
//!     cube.
//!   - `Extract`: copies the world transforms into the [`DrawList`], so the renderer never reads
//!     the scene itself (CLAUDE.md rule 3).

use oxreal_ecs::{Component, Resource, Schedule, Stage, System, With, World};
use oxreal_hierarchy::{ChildOf, LocalTransform, WorldTransform};
use oxreal_input::InputState;
use oxreal_math::{Mat4, Quat, Transform, Vec3};

use crate::fly_camera::FlyCamera;

/// Turns an object about two axes over time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Spin {
    /// Seconds spun so far.
    elapsed: f32,
}
impl Component for Spin {}

/// A mesh to draw at the entity's world transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Mesh;
impl Component for Mesh {}

impl Component for FlyCamera {}

/// Seconds since the previous frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FrameTime(pub(crate) f32);
impl Resource for FrameTime {}

/// What the renderer draws this frame: the camera, and one object-to-world matrix per mesh.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct DrawList {
    pub(crate) camera: Transform,
    pub(crate) objects: Vec<Mat4>,
}
impl Resource for DrawList {}

/// Creates the world: resources, the camera, the spinning cube and its two moons, a 9 × 9 floor
/// of 1.8 m tiles with 0.2 m gaps 1 m below the cube, and four 3 m pillars.
pub(crate) fn world() -> World {
    let mut world = World::new();
    world.insert_resource(InputState::new());
    world.insert_resource(FrameTime(0.0));
    world.insert_resource(DrawList::default());

    // Above and behind the cube, looking at it, at walking pace.
    world.spawn(FlyCamera::looking_at(
        Vec3::new(0.0, 1.2, 4.0),
        Vec3::ZERO,
        3.0,
    ));
    let cube = world.spawn((LocalTransform::default(), Mesh, Spin { elapsed: 0.0 }));
    // Children of the cube, and parts of it: they turn with it, through transform propagation
    // alone, and go when it does.
    for (offset, scale) in [
        (Vec3::new(1.4, 0.0, 0.0), 0.3),
        (Vec3::new(-1.2, 0.5, 0.3), 0.2),
    ] {
        world.spawn((
            LocalTransform(Transform::from_translation(offset).with_scale(Vec3::splat(scale))),
            Mesh,
            ChildOf::new(cube).despawn_with_parent(),
        ));
    }
    for x in -4_i8..=4 {
        for z in -4_i8..=4 {
            let tile = Transform::from_translation(Vec3::new(
                f32::from(x) * 2.0,
                -1.05,
                f32::from(z) * 2.0,
            ))
            .with_scale(Vec3::new(1.8, 0.1, 1.8));
            world.spawn((LocalTransform(tile), Mesh));
        }
    }
    for (x, z) in [(-3.0, -3.0), (3.0, -3.0), (-3.0, 3.0), (3.0, 3.0)] {
        let pillar =
            Transform::from_translation(Vec3::new(x, 0.5, z)).with_scale(Vec3::new(0.4, 3.0, 0.4));
        world.spawn((LocalTransform(pillar), Mesh));
    }
    world
}

/// The example's systems, plus the hierarchy's transform propagation.
pub(crate) fn schedule() -> Schedule {
    let mut schedule = Schedule::new();
    schedule
        .add(
            Stage::PrePhysics,
            System::new("fly camera", |ctx| {
                let seconds = ctx.resources.get::<FrameTime>().map_or(0.0, |t| t.0);
                let Some(input) = ctx.resources.get::<InputState>() else {
                    return;
                };
                for mut camera in ctx.world.query_mut::<&mut FlyCamera>() {
                    camera.update(input, seconds);
                }
            })
            .writes::<FlyCamera>()
            .reads_resource::<InputState>()
            .reads_resource::<FrameTime>(),
        )
        .add(
            Stage::PrePhysics,
            System::new("spin", |ctx| {
                let seconds = ctx.resources.get::<FrameTime>().map_or(0.0, |t| t.0);
                for (mut local, mut spin) in
                    ctx.world.query_mut::<(&mut LocalTransform, &mut Spin)>()
                {
                    spin.elapsed += seconds;
                    local.0.rotation = Quat::from_rotation_y(spin.elapsed * 0.7)
                        * Quat::from_rotation_x(spin.elapsed * 0.45);
                }
            })
            .writes::<LocalTransform>()
            .writes::<Spin>()
            .reads_resource::<FrameTime>(),
        )
        .add(
            Stage::Extract,
            System::new("extract draw list", |ctx| {
                let camera = ctx
                    .world
                    .query::<&FlyCamera>()
                    .next()
                    .map(FlyCamera::transform);
                let Some(mut list) = ctx.resources.get_mut::<DrawList>() else {
                    return;
                };
                list.camera = camera.unwrap_or(Transform::IDENTITY);
                list.objects.clear();
                list.objects.extend(
                    ctx.world
                        .query_filtered::<&WorldTransform, With<Mesh>>()
                        .map(WorldTransform::matrix),
                );
            })
            .reads::<FlyCamera>()
            .reads::<WorldTransform>()
            .writes_resource::<DrawList>(),
        );
    oxreal_hierarchy::add_systems(&mut schedule);
    schedule
}

#[cfg(test)]
mod tests {
    use oxreal_ecs::{Entity, Without};
    use oxreal_input::{ButtonState, KeyCode, KeyEvent, MouseButton};
    use oxreal_window::WindowEvent;

    use super::*;

    fn frame(world: &mut World, schedule: &mut Schedule, seconds: f32) -> DrawList {
        world.insert_resource(FrameTime(seconds));
        schedule.run(world).expect("the schedule is valid");
        if let Some(mut input) = world.resource_mut::<InputState>() {
            input.end_frame();
        }
        world.resource::<DrawList>().cloned().unwrap_or_default()
    }

    fn world_position(world: &World, entity: Entity) -> Vec3 {
        world
            .get::<WorldTransform>(entity)
            .map(WorldTransform::translation)
            .unwrap_or(Vec3::NAN)
    }

    #[test]
    fn the_schedule_is_valid_and_extracts_every_mesh() {
        let mut world = world();
        let mut schedule = schedule();
        let list = frame(&mut world, &mut schedule, 0.0);
        assert_eq!(list.objects.len(), 1 + 2 + 81 + 4);
        assert_eq!(
            list.camera.translation,
            Vec3::new(0.0, 1.2, 4.0),
            "the camera starts where it was placed"
        );
    }

    #[test]
    fn input_flies_the_camera_through_the_schedule() {
        let mut world = world();
        let mut schedule = schedule();
        let before = frame(&mut world, &mut schedule, 0.0).camera;
        if let Some(mut input) = world.resource_mut::<InputState>() {
            input.handle_window_event(&WindowEvent::MouseButton {
                button: MouseButton::Right,
                state: ButtonState::Pressed,
            });
            input.handle_window_event(&WindowEvent::Key(KeyEvent {
                key: KeyCode::KeyW,
                state: ButtonState::Pressed,
                repeat: false,
            }));
        }
        let after = frame(&mut world, &mut schedule, 0.5).camera;
        // 3 m/s for half a second, toward the cube.
        let moved = after.translation - before.translation;
        assert!((moved.length() - 1.5).abs() < 1e-4, "{moved}");
        assert!(
            moved.dot(before.forward()) > 1.49,
            "along the view: {moved}"
        );
    }

    #[test]
    fn the_moons_orbit_with_the_spinning_cube_and_the_scenery_stands_still() {
        let mut world = world();
        let mut schedule = schedule();
        frame(&mut world, &mut schedule, 0.0);
        let moons: Vec<Entity> = world.query_filtered::<Entity, With<ChildOf>>().collect();
        let scenery: Vec<Entity> = world
            .query_filtered::<Entity, (With<Mesh>, Without<ChildOf>, Without<Spin>)>()
            .collect();
        assert_eq!((moons.len(), scenery.len()), (2, 85));
        let moons_before: Vec<Vec3> = moons.iter().map(|&m| world_position(&world, m)).collect();
        let scenery_before: Vec<Vec3> =
            scenery.iter().map(|&s| world_position(&world, s)).collect();
        assert!((moons_before[0] - Vec3::new(1.4, 0.0, 0.0)).length() < 1e-5);

        frame(&mut world, &mut schedule, 0.5);
        for (&moon, before) in moons.iter().zip(&moons_before) {
            let after = world_position(&world, moon);
            assert!(
                (after - *before).length() > 0.1,
                "the moon moved: {before} -> {after}"
            );
            assert!(
                (after.length() - before.length()).abs() < 1e-4,
                "it keeps its distance from the cube"
            );
        }
        let scenery_after: Vec<Vec3> = scenery.iter().map(|&s| world_position(&world, s)).collect();
        assert_eq!(scenery_before, scenery_after);
    }
}
