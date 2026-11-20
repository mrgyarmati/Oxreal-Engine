//! The demo scene as ECS data and systems (ADR-0014, ADR-0019 §3).
//!
//! It's built in code and **stands in for scene loading** (D-07): once scenes are files, this
//! module goes and the player loads one.
//!
//! - **Entities:** the camera (a [`FlyCamera`] with a render-bridge [`Camera`]), the spinning
//!   cube with two small moons as its children, a floor of tiles and four pillars. Every mesh is
//!   the renderer's cube ([`MeshInstance`]), scaled by its `LocalTransform`.
//! - **Resources:** the frame's `InputState` (fed by the window events), [`FrameTime`], and the
//!   [`ExtractedWorld`] the renderer reads.
//! - **Systems:**
//!   - `PrePhysics`: the fly camera moves its entity, and the cube spins.
//!   - `PostUpdate`: `oxreal-hierarchy` propagates transforms, so the moons orbit with the
//!     cube.
//!   - `Extract`: `oxreal-render-bridge` fills the [`ExtractedWorld`], so the renderer never
//!     reads the scene itself (brief rule 3).

use oxreal_ecs::{Component, Resource, Schedule, Stage, System, World};
use oxreal_hierarchy::{ChildOf, LocalTransform};
use oxreal_input::InputState;
use oxreal_math::{LinearRgba, Quat, Transform, Vec3};
use oxreal_render_bridge::{Camera, ExtractedWorld, MeshInstance};
use oxreal_renderer::MeshHandle;

use crate::fly_camera::FlyCamera;

/// Turns an object about two axes over time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Spin {
    /// Seconds spun so far.
    elapsed: f32,
}
impl Component for Spin {}

/// Seconds since the previous frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FrameTime(pub(crate) f32);
impl Resource for FrameTime {}

/// The color behind the scene, linear: a near-black blue-grey.
const BACKGROUND: LinearRgba = LinearRgba::new(0.02, 0.02, 0.025, 1.0);

/// Creates the world, every mesh drawing `cube`: resources, the camera, the spinning cube and
/// its two moons, a 9 × 9 floor of 1.8 m tiles with 0.2 m gaps 1 m below the cube, and four 3 m
/// pillars.
pub(crate) fn world(cube: MeshHandle) -> World {
    let mut world = World::new();
    world.insert_resource(InputState::new());
    world.insert_resource(FrameTime(0.0));
    world.insert_resource(ExtractedWorld::new(BACKGROUND));

    // Above and behind the cube, looking at it, at walking pace.
    let fly = FlyCamera::looking_at(Vec3::new(0.0, 1.2, 4.0), Vec3::ZERO, 3.0);
    world.spawn((LocalTransform(fly.transform()), fly, Camera::default()));

    let mesh = MeshInstance::new(cube);
    let spinning = world.spawn((LocalTransform::default(), mesh, Spin { elapsed: 0.0 }));
    // Children of the cube, and parts of it: they turn with it, through transform propagation
    // alone, and go when it does.
    for (offset, scale) in [
        (Vec3::new(1.4, 0.0, 0.0), 0.3),
        (Vec3::new(-1.2, 0.5, 0.3), 0.2),
    ] {
        world.spawn((
            LocalTransform(Transform::from_translation(offset).with_scale(Vec3::splat(scale))),
            mesh,
            ChildOf::new(spinning).despawn_with_parent(),
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
            world.spawn((LocalTransform(tile), mesh));
        }
    }
    for (x, z) in [(-3.0, -3.0), (3.0, -3.0), (-3.0, 3.0), (3.0, 3.0)] {
        let pillar =
            Transform::from_translation(Vec3::new(x, 0.5, z)).with_scale(Vec3::new(0.4, 3.0, 0.4));
        world.spawn((LocalTransform(pillar), mesh));
    }
    world
}

/// The scene's systems, plus the hierarchy's transform propagation and the render bridge's
/// extraction.
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
                for (mut camera, mut local) in ctx
                    .world
                    .query_mut::<(&mut FlyCamera, &mut LocalTransform)>()
                {
                    camera.update(input, seconds);
                    // The fly camera is authoritative: it overwrites the entity's placement every
                    // frame, so set its position through `FlyCamera`, not `LocalTransform`.
                    local.0 = camera.transform();
                }
            })
            .writes::<FlyCamera>()
            .writes::<LocalTransform>()
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
        );
    oxreal_hierarchy::add_systems(&mut schedule);
    oxreal_render_bridge::add_systems(&mut schedule);
    schedule
}

#[cfg(test)]
mod tests {
    use oxreal_ecs::{Entity, With, Without};
    use oxreal_hierarchy::WorldTransform;
    use oxreal_input::{ButtonState, KeyCode, KeyEvent, MouseButton};
    use oxreal_window::WindowEvent;

    use super::*;

    fn frame(world: &mut World, schedule: &mut Schedule, seconds: f32) -> ExtractedWorld {
        world.insert_resource(FrameTime(seconds));
        schedule.run(world).expect("the schedule is valid");
        if let Some(mut input) = world.resource_mut::<InputState>() {
            input.end_frame();
        }
        world
            .resource::<ExtractedWorld>()
            .cloned()
            .expect("the scene has an ExtractedWorld")
    }

    fn world_position(world: &World, entity: Entity) -> Vec3 {
        world
            .get::<WorldTransform>(entity)
            .map(WorldTransform::translation)
            .unwrap_or(Vec3::NAN)
    }

    #[test]
    fn the_schedule_is_valid_and_extracts_every_mesh() {
        let mut world = world(MeshHandle::DANGLING);
        let mut schedule = schedule();
        schedule.validate().expect("the systems' order is valid");
        let extracted = frame(&mut world, &mut schedule, 0.0);
        assert_eq!(extracted.render_world().draws.len(), 1 + 2 + 81 + 4);
        assert!(extracted.camera_entity().is_some());
        assert_eq!(
            extracted.render_world().camera.transform.translation,
            Vec3::new(0.0, 1.2, 4.0),
            "the camera starts where it was placed"
        );
        assert_eq!(extracted.background(), BACKGROUND);
    }

    #[test]
    fn input_flies_the_camera_through_the_schedule() {
        let mut world = world(MeshHandle::DANGLING);
        let mut schedule = schedule();
        let before = frame(&mut world, &mut schedule, 0.0)
            .render_world()
            .camera
            .transform;
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
        let after = frame(&mut world, &mut schedule, 0.5)
            .render_world()
            .camera
            .transform;
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
        let mut world = world(MeshHandle::DANGLING);
        let mut schedule = schedule();
        frame(&mut world, &mut schedule, 0.0);
        let moons: Vec<Entity> = world.query_filtered::<Entity, With<ChildOf>>().collect();
        let scenery: Vec<Entity> = world
            .query_filtered::<Entity, (With<MeshInstance>, Without<ChildOf>, Without<Spin>)>()
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

    #[test]
    fn every_draw_uses_the_given_mesh() {
        let mut world = world(MeshHandle::DANGLING);
        let mut schedule = schedule();
        let extracted = frame(&mut world, &mut schedule, 0.0);
        assert!(
            extracted
                .render_world()
                .draws
                .iter()
                .all(|draw| draw.mesh == MeshHandle::DANGLING)
        );
    }
}
