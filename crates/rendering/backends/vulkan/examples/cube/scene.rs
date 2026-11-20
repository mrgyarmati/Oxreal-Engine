//! The cube example's scene as ECS data and systems (ADR-0014).
//!
//! - **Entities:** the camera (a [`FlyCamera`]), the spinning cube, and the floor tiles and
//!   pillars, each with a [`Placement`].
//! - **Resources:** the frame's `InputState` (fed by the window events), [`FrameTime`], and the
//!   [`DrawList`] the renderer reads.
//! - **Systems:** gameplay in `PrePhysics` moves the camera and spins the cube. `Extract` copies
//!   what the renderer needs into the [`DrawList`], so the renderer never reads the scene itself
//!   (CLAUDE.md rule 3).

use oxreal_ecs::{Component, Resource, Schedule, Stage, System, World};
use oxreal_input::InputState;
use oxreal_math::{Mat4, Quat, Transform, Vec3};

use crate::fly_camera::FlyCamera;

/// Where an object stands: object space to world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placement(pub(crate) Transform);
impl Component for Placement {}

/// Turns an object about two axes over time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Spin {
    /// Seconds spun so far.
    elapsed: f32,
}
impl Component for Spin {}

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

/// Creates the world: resources, the camera, the spinning cube, a 9 × 9 floor of 1.8 m tiles
/// with 0.2 m gaps 1 m below the cube, and four 3 m pillars.
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
    world.spawn((Placement(Transform::IDENTITY), Spin { elapsed: 0.0 }));
    for x in -4_i8..=4 {
        for z in -4_i8..=4 {
            let tile = Transform::from_translation(Vec3::new(
                f32::from(x) * 2.0,
                -1.05,
                f32::from(z) * 2.0,
            ))
            .with_scale(Vec3::new(1.8, 0.1, 1.8));
            world.spawn(Placement(tile));
        }
    }
    for (x, z) in [(-3.0, -3.0), (3.0, -3.0), (-3.0, 3.0), (3.0, 3.0)] {
        let pillar =
            Transform::from_translation(Vec3::new(x, 0.5, z)).with_scale(Vec3::new(0.4, 3.0, 0.4));
        world.spawn(Placement(pillar));
    }
    world
}

/// The example's systems.
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
                for (mut placement, mut spin) in
                    ctx.world.query_mut::<(&mut Placement, &mut Spin)>()
                {
                    spin.elapsed += seconds;
                    placement.0.rotation = Quat::from_rotation_y(spin.elapsed * 0.7)
                        * Quat::from_rotation_x(spin.elapsed * 0.45);
                }
            })
            .writes::<Placement>()
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
                list.objects
                    .extend(ctx.world.query::<&Placement>().map(|p| p.0.to_matrix()));
            })
            .reads::<FlyCamera>()
            .reads::<Placement>()
            .writes_resource::<DrawList>(),
        );
    schedule
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn the_schedule_is_valid_and_extracts_every_object() {
        let mut world = world();
        let mut schedule = schedule();
        let list = frame(&mut world, &mut schedule, 0.0);
        assert_eq!(list.objects.len(), 1 + 81 + 4);
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
    fn the_cube_spins_with_frame_time() {
        let mut world = world();
        let mut schedule = schedule();
        let first = frame(&mut world, &mut schedule, 0.0).objects;
        let later = frame(&mut world, &mut schedule, 0.25).objects;
        assert_ne!(first[0], later[0], "the spinning cube turned");
        assert_eq!(first[1..], later[1..], "the scenery stands still");
    }
}
