//! Rendering hand-made render worlds on a real device, from freshly cooked shaders (ADR-0019
//! §5).
//!
//! These need a Vulkan 1.3 device with validation layers, the pinned Slang and `spirv-val`, so
//! they're ignored by default. CI runs them on Lavapipe. Run them with
//! `cargo test -p oxreal-renderer -- --ignored`.

mod common;

use std::sync::Arc;

use oxreal_math::{LinearRgba, Mat4, Transform, Vec3};
use oxreal_renderer::{
    Camera, CookedShaders, DrawItem, Indices, MeshData, Projection, RenderError, RenderTarget,
    RenderWorld, Renderer, Vertex,
};
use oxreal_rhi::{
    Device, DeviceDesc, GpuSelection, TextureDesc, TextureFormat, TextureHandle, TextureUsage,
    Validation,
};

const SIZE: u32 = 64;
const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

fn device() -> Arc<dyn Device> {
    let _ = oxreal_diagnostics::init_application_logging();
    let device = oxreal_rhi_vulkan::create_device(&DeviceDesc {
        app_name: "oxreal-renderer tests".to_owned(),
        validation: Validation::Enabled,
        gpu: GpuSelection::Automatic,
    })
    .expect("device creation");
    assert!(
        device.validation_enabled(),
        "validation layers must be installed"
    );
    Arc::new(device)
}

/// A renderer on a new device, with shaders cooked into `out` for this test. `None` where
/// `spirv-val` isn't installed and isn't required.
fn renderer(out: &common::TempDir) -> Option<Renderer> {
    let validator = common::validator()?;
    let cooked = common::cook_renderer(&validator, &common::workspace(), &out.0);
    let shaders = CookedShaders::load(&cooked).expect("the cooked shaders load");
    Some(Renderer::new(device(), shaders).expect("renderer"))
}

fn target(device: &dyn Device, size: u32) -> RenderTarget {
    let texture = device
        .create_texture(&TextureDesc {
            label: "target",
            width: size,
            height: size,
            format: FORMAT,
            usage: TextureUsage::RENDER_TARGET | TextureUsage::COPY_SOURCE,
        })
        .expect("target");
    RenderTarget {
        texture,
        format: FORMAT,
        size: (size, size),
    }
}

/// A world seen from 3 m in front of the origin, cleared to pure blue.
fn world(draws: Vec<DrawItem>) -> RenderWorld {
    let mut world = RenderWorld::default();
    world.camera = Camera::new(
        Transform::from_translation(Vec3::new(0.0, 0.0, 3.0)),
        Projection::default(),
    );
    world.background = LinearRgba::new(0.0, 0.0, 1.0, 1.0);
    world.draws = draws;
    world
}

/// Renders `world` into a new target and reads it back.
fn render(renderer: &mut Renderer, world: &RenderWorld, size: u32) -> (Vec<u8>, TextureHandle) {
    let device = Arc::clone(renderer.device());
    let target = target(device.as_ref(), size);
    let mut encoder = device.create_encoder("render test").expect("encoder");
    renderer
        .render(encoder.as_mut(), &target, world)
        .expect("render");
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");
    (
        device.read_texture(target.texture).expect("readback"),
        target.texture,
    )
}

fn pixel(pixels: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * size + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

const BLUE: [u8; 4] = [0, 0, 255, 255];

fn assert_no_validation_errors(device: &dyn Device) {
    device.wait_idle().expect("wait idle");
    assert_eq!(
        device.validation_error_count(),
        0,
        "validation reported errors (see log)"
    );
}

#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers, the pinned Slang and spirv-val"]
fn draws_registered_meshes_and_skips_stale_handles() {
    let out = common::TempDir::new("gpu-draw");
    let Some(mut renderer) = renderer(&out) else {
        return;
    };
    let cube = renderer.register_mesh(&MeshData::cube()).expect("cube");
    let gone = renderer.register_mesh(&MeshData::cube()).expect("second");
    renderer.unregister_mesh(gone).expect("unregister");
    assert!(!renderer.has_mesh(gone));
    assert_eq!(renderer.mesh_count(), 1);

    let draws = vec![
        DrawItem::new(cube, Mat4::IDENTITY),
        DrawItem::new(gone, Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0))),
    ];
    let device = Arc::clone(renderer.device());
    let target = target(device.as_ref(), SIZE);
    let mut encoder = device.create_encoder("render test").expect("encoder");
    let stats = renderer
        .render(encoder.as_mut(), &target, &world(draws))
        .expect("render");
    assert_eq!((stats.drawn, stats.skipped), (1, 1));
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");
    let pixels = device.read_texture(target.texture).expect("readback");

    // The cube covers the middle; the background shows in the corners.
    let middle = pixel(&pixels, SIZE, SIZE / 2 + 3, SIZE / 2 + 3);
    assert_ne!(middle, BLUE, "the cube must cover the middle");
    assert!(
        middle[0] > middle[2],
        "checkerboard colors, not the background: {middle:?}"
    );
    for (x, y) in [(0, 0), (SIZE - 1, 0), (0, SIZE - 1), (SIZE - 1, SIZE - 1)] {
        assert_eq!(pixel(&pixels, SIZE, x, y), BLUE, "corner ({x}, {y})");
    }
    device
        .destroy_texture(target.texture)
        .expect("destroy target");
    drop(renderer);
    assert_no_validation_errors(device.as_ref());
}

#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers, the pinned Slang and spirv-val"]
fn another_devices_encoder_is_refused() {
    let out = common::TempDir::new("gpu-foreign");
    let Some(mut renderer) = renderer(&out) else {
        return;
    };
    let cube = renderer.register_mesh(&MeshData::cube()).expect("cube");
    let other = device();
    let target = target(other.as_ref(), SIZE);
    let mut encoder = other.create_encoder("foreign").expect("encoder");
    let draws = vec![DrawItem::new(cube, Mat4::IDENTITY)];
    assert!(matches!(
        renderer.render(encoder.as_mut(), &target, &world(draws)),
        Err(RenderError::ForeignEncoder)
    ));
    // Nothing was recorded: the encoder submits cleanly.
    other.submit(encoder).expect("submit");
    other
        .destroy_texture(target.texture)
        .expect("destroy target");
    assert_no_validation_errors(other.as_ref());
    let device = Arc::clone(renderer.device());
    drop(renderer);
    assert_no_validation_errors(device.as_ref());
}

#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers, the pinned Slang and spirv-val"]
fn targets_of_any_size_and_32_bit_indices_render() {
    let out = common::TempDir::new("gpu-sizes");
    let Some(mut renderer) = renderer(&out) else {
        return;
    };
    // One large triangle with 32-bit indices, facing the camera.
    let triangle = MeshData {
        vertices: [(-2.0, -2.0), (2.0, -2.0), (0.0, 2.0)]
            .map(|(x, y)| Vertex {
                position: Vec3::new(x, y, 0.0),
                normal: Vec3::Z,
                ..Vertex::default()
            })
            .to_vec(),
        indices: Indices::U32(vec![0, 1, 2]),
    };
    let mesh = renderer.register_mesh(&triangle).expect("triangle");
    let draws = vec![DrawItem::new(mesh, Mat4::IDENTITY)];
    let world = world(draws);
    // The depth target is recreated for each size.
    let device = Arc::clone(renderer.device());
    for size in [SIZE, 32, SIZE] {
        let (pixels, texture) = render(&mut renderer, &world, size);
        let middle = pixel(&pixels, size, size / 2, size / 2);
        assert_ne!(middle, BLUE, "{size}px: the triangle covers the middle");
        device.destroy_texture(texture).expect("destroy target");
    }
    drop(renderer);
    assert_no_validation_errors(device.as_ref());
}

#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers, the pinned Slang and spirv-val"]
fn invalid_inputs_are_errors_and_record_nothing() {
    let out = common::TempDir::new("gpu-invalid");
    let Some(mut renderer) = renderer(&out) else {
        return;
    };
    let broken = MeshData {
        vertices: vec![Vertex::default(); 3],
        indices: Indices::U16(vec![0, 1, 3]),
    };
    assert!(matches!(
        renderer.register_mesh(&broken),
        Err(RenderError::Mesh(_))
    ));
    assert_eq!(renderer.mesh_count(), 0);
    let cube = renderer.register_mesh(&MeshData::cube()).expect("cube");
    renderer.unregister_mesh(cube).expect("unregister");
    assert!(matches!(
        renderer.unregister_mesh(cube),
        Err(RenderError::UnknownMesh(_))
    ));

    let device = Arc::clone(renderer.device());
    let target = target(device.as_ref(), SIZE);
    let mut bad_camera = world(Vec::new());
    bad_camera.camera.projection = Projection::Perspective {
        vertical_fov: 0.0,
        near: 0.05,
    };
    let mut encoder = device.create_encoder("invalid").expect("encoder");
    assert!(matches!(
        renderer.render(encoder.as_mut(), &target, &bad_camera),
        Err(RenderError::Camera(_))
    ));
    let empty = RenderTarget {
        size: (0, SIZE),
        ..target
    };
    assert!(matches!(
        renderer.render(encoder.as_mut(), &empty, &world(Vec::new())),
        Err(RenderError::InvalidTarget { .. })
    ));
    device
        .submit(encoder)
        .expect("nothing recorded, so a clean submit");
    device
        .destroy_texture(target.texture)
        .expect("destroy target");
    drop(renderer);
    assert_no_validation_errors(device.as_ref());
}
