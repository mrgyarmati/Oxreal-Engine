//! The M1 renderer: draws a plain-data [`RenderWorld`] through the RHI ([ADR-0019] §1).
//!
//! **Layer:** rendering. It depends on the RHI, never on a backend: it holds the device as
//! `Arc<dyn oxreal_rhi::Device>` and records through `&mut dyn CommandEncoder` (ADR-0017 §1).
//! It doesn't know the ECS: the game fills a [`RenderWorld`] each frame (brief rule 3).
//!
//! - **Shaders** come from a cooked directory, never a compiler ([`CookedShaders`], ADR-0013
//!   §4). The loader refuses a cache cooked from other sources than this build's.
//! - **Meshes** are validated and uploaded by [`Renderer::register_mesh`] and drawn by naming
//!   their generational [`MeshHandle`] in a [`DrawItem`].
//! - **The application** owns surfaces, the frame loop and the color target; the renderer
//!   records one render pass into the encoder it's given ([`Renderer::render`]), so an
//!   offscreen editor viewport can use it unchanged.
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use oxreal_renderer::{CookedShaders, DrawItem, MeshData, RenderTarget, RenderWorld, Renderer};
//! # fn frame(device: Arc<dyn oxreal_rhi::Device>, surface: oxreal_rhi::SurfaceHandle)
//! #     -> Result<(), Box<dyn std::error::Error>> {
//! let shaders = CookedShaders::load("target/debug/cooked".as_ref())?;
//! let mut renderer = Renderer::new(device.clone(), shaders)?;
//! let cube = renderer.register_mesh(&MeshData::cube())?;
//!
//! let mut world = RenderWorld::default();
//! world.draws.push(DrawItem::new(cube, oxreal_math::Mat4::IDENTITY));
//! if let Some(mut frame) = device.begin_frame(surface)? {
//!     let target = RenderTarget::backbuffer(&frame);
//!     renderer.render(frame.encoder(), &target, &world)?;
//!     device.end_frame(frame)?;
//! }
//! # Ok(()) }
//! ```
//!
//! [ADR-0019]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0019-player-renderer-and-cooked-shaders.md

mod cache;
mod mesh;
mod renderer;
mod world;

pub use cache::{CookedShaders, ShaderCacheError};
pub use mesh::{Indices, MAX_VERTICES, MeshData, MeshError, MeshHandle, Vertex};
pub use renderer::{RenderError, RenderStats, RenderTarget, Renderer};
pub use world::{Camera, DrawItem, Projection, RenderWorld};

/// The build script's source-key code, compiled here too so its golden test checks the code
/// the build runs.
#[cfg(test)]
#[path = "../build/source_key.rs"]
mod build_source_key;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::build_source_key::{hex, parse_recipe, source_key};

    /// The golden key of `oxreal-shader-compiler`'s cook fixture with Slang version `0.0`. The
    /// cook's own implementation is tested against the same value (its `cook/key.rs`); if they
    /// disagree, a cache cooked for this build would be refused as stale.
    const GOLDEN: &str = "a48e9c1c4f9726634f69e55fb5e33605a5688f1aafba879f1a0ef139bf90f05c";

    #[test]
    fn the_build_scripts_key_matches_the_cooks_golden_value() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/shader-compiler/tests/cook");
        let recipe = std::fs::read_to_string(fixture.join("shaders/cook.json")).unwrap();
        let shaders = parse_recipe(&recipe).unwrap();
        let contents: Vec<Vec<u8>> = shaders[0]
            .inputs
            .iter()
            .map(|input| std::fs::read(fixture.join(input)).unwrap())
            .collect();
        assert_eq!(hex(&source_key("0.0", &shaders[0], &contents)), GOLDEN);
    }

    #[test]
    fn the_build_scripts_constants_equal_the_cooks() {
        use oxreal_shader_compiler::cook;
        assert_eq!(
            super::build_source_key::SOURCE_KEY_FORMAT,
            cook::SOURCE_KEY_FORMAT
        );
        assert_eq!(super::build_source_key::TARGET, cook::TARGET);
        assert_eq!(
            super::build_source_key::COMPILE_OPTIONS,
            oxreal_shader_compiler::compile_options(),
            "the cook's slangc flags changed: update COMPILE_OPTIONS and both golden values"
        );
    }

    #[test]
    fn the_build_script_refuses_inputs_that_leave_the_workspace() {
        for input in [
            "../outside.slang",
            "/abs.slang",
            "a\\b.slang",
            "C:/x.slang",
            "a//b",
        ] {
            let recipe = format!(
                r#"{{"format": 1, "shaders": [{{"output": "X", "source": "x", "entry_points": [],
                "include_dirs": [], "inputs": ["{}"]}}]}}"#,
                input.replace('\\', "\\\\")
            );
            assert!(parse_recipe(&recipe).is_err(), "`{input}` was accepted");
        }
    }
}
