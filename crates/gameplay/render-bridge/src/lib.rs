//! Oxreal render bridge: what makes an entity visible, and the extraction that hands it to the
//! renderer (ADR-0019 §2, TAD §5.2).
//!
//! Layer: **gameplay**. It joins the ECS world (`oxreal-ecs`, `oxreal-hierarchy`) to the
//! renderer (`oxreal-renderer`), so neither has to know the other: the renderer takes plain data,
//! and the ECS knows nothing of rendering.
//!
//! - **Components:** [`Camera`] makes an entity the point of view, and [`MeshInstance`] draws a
//!   registered mesh. Both are placed by the entity's [`WorldTransform`], so give such entities a
//!   [`LocalTransform`](oxreal_hierarchy::LocalTransform). They can be children like any other
//!   entity.
//! - **Resource:** [`ExtractedWorld`], the [`RenderWorld`] the application hands to
//!   [`oxreal_renderer::Renderer::render`]. The application inserts it, and sets its background.
//! - **System:** [`extract_render_world`], in [`Stage::Extract`], after transform propagation
//!   ([`Stage::PostUpdate`]). [`add_systems`] adds it.
//!
//! It never touches the GPU, owns no GPU resources, and doesn't name a graphics backend.

use oxreal_diagnostics::{info, warn};
use oxreal_ecs::{Component, Entity, Resource, Schedule, Stage, System};
use oxreal_hierarchy::WorldTransform;
use oxreal_math::Transform;
use oxreal_renderer::{DrawItem, MeshHandle, Projection, RenderWorld};

/// Makes an entity a camera: the world is seen from its [`WorldTransform`], looking along its
/// local −Z (ADR-0011). Its scale is ignored, since cameras are rigid.
///
/// When several entities have one, the one with the smallest [`Entity`] is used, so the choice
/// stays the same from frame to frame. Choosing between cameras deliberately comes with the
/// gameplay camera (ADR-0020).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct Camera {
    /// How it projects the world.
    pub projection: Projection,
}

impl Camera {
    /// A camera projecting with `projection`.
    #[must_use]
    pub const fn new(projection: Projection) -> Self {
        Self { projection }
    }
}

impl Component for Camera {}

/// Draws a mesh, placed by the entity's [`WorldTransform`].
///
/// The handle comes from [`oxreal_renderer::Renderer::register_mesh`]. If the mesh has been
/// unregistered, the renderer skips the draw and counts it; nothing is drawn wrongly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct MeshInstance {
    /// The mesh to draw.
    pub mesh: MeshHandle,
}

impl MeshInstance {
    /// Draws `mesh`.
    #[must_use]
    pub const fn new(mesh: MeshHandle) -> Self {
        Self { mesh }
    }
}

impl Component for MeshInstance {}

/// What the renderer draws this frame, filled by [`extract_render_world`] in
/// [`Stage::Extract`]. The renderer reads only this, never the ECS world (brief rule 3).
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct ExtractedWorld {
    /// The frame to draw. Extraction sets its camera and draws. The application sets its
    /// background, which extraction leaves alone.
    pub render_world: RenderWorld,
    /// The entity the frame is seen from. `None` when no entity has a usable [`Camera`]; then
    /// nothing is drawn, and the target is only cleared.
    pub camera: Option<Entity>,
}

impl ExtractedWorld {
    /// An empty extraction that clears to `render_world`'s background.
    #[must_use]
    pub fn new(render_world: RenderWorld) -> Self {
        Self {
            render_world,
            camera: None,
        }
    }
}

impl Resource for ExtractedWorld {}

/// Adds the bridge's systems to `schedule`: [`extract_render_world`] in [`Stage::Extract`].
/// Insert an [`ExtractedWorld`] resource into the world too.
pub fn add_systems(schedule: &mut Schedule) {
    schedule.add(Stage::Extract, extract_render_world());
}

/// The system that fills [`ExtractedWorld`] from the cameras and mesh instances.
///
/// - **Camera:** the [`Camera`] entity with the smallest [`Entity`] among those whose world
///   transform has an orientation (a zero scale has none). It's logged when the number of
///   cameras changes, and warned about when there is none.
/// - **Draws:** one per [`MeshInstance`] with a [`WorldTransform`], with its world matrix. With
///   no camera there are none.
///
/// If the [`ExtractedWorld`] resource is missing, it does nothing and warns once.
#[must_use]
pub fn extract_render_world() -> System {
    // What was last reported, so the log says each change once rather than every frame.
    let mut reported_cameras: Option<usize> = None;
    let mut reported_missing = false;
    System::new("extract render world", move |ctx| {
        let mut cameras = 0_usize;
        let mut chosen: Option<(Entity, Camera, Transform)> = None;
        for (entity, camera, world) in ctx.world.query::<(Entity, &Camera, &WorldTransform)>() {
            cameras += 1;
            let Some(placement) = camera_placement(world) else {
                continue;
            };
            if chosen.as_ref().is_none_or(|(best, _, _)| entity < *best) {
                chosen = Some((entity, *camera, placement));
            }
        }
        if reported_cameras != Some(cameras) {
            match (cameras, chosen.as_ref()) {
                (_, None) => warn!(cameras, "no usable camera: nothing is drawn"),
                (1, Some(_)) => {}
                (_, Some((entity, _, _))) => {
                    info!(cameras, using = ?entity, "several cameras: using the first");
                }
            }
            reported_cameras = Some(cameras);
        }

        let draws: Vec<DrawItem> = if chosen.is_some() {
            ctx.world
                .query::<(&MeshInstance, &WorldTransform)>()
                .map(|(instance, world)| DrawItem::new(instance.mesh, world.matrix()))
                .collect()
        } else {
            Vec::new()
        };

        let Some(mut extracted) = ctx.resources.get_mut::<ExtractedWorld>() else {
            if !reported_missing {
                warn!("the ExtractedWorld resource is missing: nothing is extracted");
                reported_missing = true;
            }
            return;
        };
        reported_missing = false;
        extracted.camera = chosen.as_ref().map(|(entity, _, _)| *entity);
        if let Some((_, camera, placement)) = chosen {
            extracted.render_world.camera =
                oxreal_renderer::Camera::new(placement, camera.projection);
        }
        extracted.render_world.draws = draws;
    })
    .reads::<Camera>()
    .reads::<MeshInstance>()
    .reads::<WorldTransform>()
    .writes_resource::<ExtractedWorld>()
}

/// A camera's rigid placement: its world transform's rotation and translation, without scale.
/// `None` if the transform has no orientation (a zero scale).
fn camera_placement(world: &WorldTransform) -> Option<Transform> {
    let (_, rotation, translation) = world.affine().to_scale_rotation_translation();
    (rotation.is_finite() && translation.is_finite())
        .then(|| Transform::from_translation(translation).with_rotation(rotation.normalize()))
}

#[cfg(test)]
mod tests {
    use oxreal_ecs::World;
    use oxreal_hierarchy::{ChildOf, LocalTransform};
    use oxreal_math::{LinearRgba, Quat, Vec3};

    use super::*;

    fn schedule() -> Schedule {
        let mut schedule = Schedule::new();
        oxreal_hierarchy::add_systems(&mut schedule);
        add_systems(&mut schedule);
        schedule
    }

    fn world() -> World {
        let mut world = World::new();
        world.insert_resource(ExtractedWorld::default());
        world
    }

    fn extract(world: &mut World) -> ExtractedWorld {
        schedule().run(world).expect("the schedule is valid");
        world
            .resource::<ExtractedWorld>()
            .cloned()
            .expect("the resource stays")
    }

    fn at(position: Vec3) -> LocalTransform {
        LocalTransform(Transform::from_translation(position))
    }

    #[test]
    fn meshes_are_drawn_at_their_world_placement_including_children() {
        let mut world = world();
        world.spawn((Camera::default(), at(Vec3::ZERO)));
        let parent = world.spawn((
            at(Vec3::new(1.0, 0.0, 0.0)),
            MeshInstance::new(MeshHandle::DANGLING),
        ));
        world.spawn((
            at(Vec3::new(0.0, 2.0, 0.0)),
            MeshInstance::new(MeshHandle::DANGLING),
            ChildOf::new(parent),
        ));
        let extracted = extract(&mut world);
        let mut origins: Vec<Vec3> = extracted
            .render_world
            .draws
            .iter()
            .map(|draw| draw.object_to_world.w_axis.truncate())
            .collect();
        origins.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
        assert_eq!(
            origins,
            [Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 2.0, 0.0)]
        );
        assert!(
            extracted
                .render_world
                .draws
                .iter()
                .all(|d| d.mesh == MeshHandle::DANGLING)
        );
    }

    #[test]
    fn the_camera_is_placed_by_its_world_transform_without_scale() {
        let mut world = world();
        let rig = world.spawn(LocalTransform(
            Transform::from_translation(Vec3::new(0.0, 5.0, 0.0)).with_scale(Vec3::splat(3.0)),
        ));
        let turn = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let projection = Projection::Perspective {
            vertical_fov: 1.0,
            near: 0.1,
        };
        let camera = world.spawn((
            Camera::new(projection),
            LocalTransform(
                Transform::from_translation(Vec3::new(1.0, 0.0, 0.0)).with_rotation(turn),
            ),
            ChildOf::new(rig),
        ));
        let extracted = extract(&mut world);
        assert_eq!(extracted.camera, Some(camera));
        let seen = extracted.render_world.camera;
        assert_eq!(seen.projection, projection);
        // The parent's scale moves the child's offset, but the camera itself isn't scaled.
        assert!(
            seen.transform
                .translation
                .abs_diff_eq(Vec3::new(3.0, 5.0, 0.0), 1e-5)
        );
        assert!(seen.transform.rotation.abs_diff_eq(turn, 1e-5));
        assert_eq!(seen.transform.scale, Vec3::ONE);
    }

    #[test]
    fn without_a_camera_nothing_is_drawn_and_the_background_stays() {
        let mut world = World::new();
        let background = LinearRgba::new(0.1, 0.2, 0.3, 1.0);
        let mut render_world = RenderWorld::default();
        render_world.background = background;
        world.insert_resource(ExtractedWorld::new(render_world));
        world.spawn((at(Vec3::ZERO), MeshInstance::new(MeshHandle::DANGLING)));
        let extracted = extract(&mut world);
        assert_eq!(extracted.camera, None);
        assert!(extracted.render_world.draws.is_empty());
        assert_eq!(extracted.render_world.background, background);
    }

    #[test]
    fn the_smallest_entity_wins_among_several_cameras() {
        let mut world = world();
        let cameras: Vec<Entity> = (0_u8..3)
            .map(|i| world.spawn((Camera::default(), at(Vec3::splat(f32::from(i))))))
            .collect();
        let smallest = cameras.iter().copied().min();
        assert_eq!(extract(&mut world).camera, smallest);
        // The same choice every frame.
        assert_eq!(extract(&mut world).camera, smallest);
    }

    #[test]
    fn a_camera_with_zero_scale_has_no_orientation_and_is_passed_over() {
        let mut world = world();
        let flattened = world.spawn((
            Camera::default(),
            LocalTransform(Transform::from_translation(Vec3::ONE).with_scale(Vec3::ZERO)),
        ));
        let extracted = extract(&mut world);
        assert_eq!(extracted.camera, None);
        let usable = world.spawn((Camera::default(), at(Vec3::ZERO)));
        assert!(
            flattened < usable,
            "the unusable one would win if it counted"
        );
        assert_eq!(extract(&mut world).camera, Some(usable));
    }

    #[test]
    fn entities_without_a_world_transform_are_not_drawn() {
        let mut world = world();
        world.spawn((Camera::default(), at(Vec3::ZERO)));
        world.spawn(MeshInstance::new(MeshHandle::DANGLING));
        assert!(extract(&mut world).render_world.draws.is_empty());
    }

    #[test]
    fn a_missing_resource_is_not_an_error() {
        let mut world = World::new();
        world.spawn((Camera::default(), at(Vec3::ZERO)));
        schedule().run(&mut world).expect("the schedule still runs");
        assert!(world.resource::<ExtractedWorld>().is_none());
    }
}
