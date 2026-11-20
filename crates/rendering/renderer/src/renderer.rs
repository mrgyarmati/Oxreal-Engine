//! The renderer: owns the mesh pass's GPU objects and records a [`RenderWorld`] into a target.

use std::sync::Arc;

use oxreal_diagnostics::{debug, warn};
use oxreal_math::Mat4;
use oxreal_math::projection::{
    ProjectionError, REVERSED_DEPTH_CLEAR, perspective_infinite_reversed, view_matrix,
};
use oxreal_rhi::{
    AddressMode, BufferDesc, BufferUsage, ColorAttachment, CommandEncoder, CullMode,
    DepthAttachment, DepthLoadOp, DepthState, Device, Filter, Frame, LoadOp, PipelineHandle,
    RenderPassDesc, RenderPipelineDesc, RhiError, SamplerDesc, SamplerHandle, ShaderCode,
    ShaderEntry, StoreOp, TextureDesc, TextureFormat, TextureHandle, TextureUsage,
};

use crate::mesh::{GpuMesh, MeshPool};
use crate::{CookedShaders, MeshData, MeshError, MeshHandle, RenderWorld};

/// The mesh shader's push constants: object-to-clip matrix, vertex buffer address, texture and
/// sampler indices (`shaders/Mesh.slang`, checked by tests/shader_layout.rs).
const CONSTANT_BYTES: usize = 80;
/// The default material's checkerboard: its size in texels, and squares per side.
const BOARD_TEXELS: u32 = 64;
const BOARD_SQUARES: u32 = 4;

/// Why the renderer couldn't do something.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RenderError {
    /// The encoder wasn't created by the renderer's device.
    #[error("the command encoder belongs to another device than the renderer's")]
    ForeignEncoder,
    /// The target has no area, or a format the mesh pass can't render to.
    #[error(
        "can't render to a {width}x{height} {format:?} target: it needs an area and a color format"
    )]
    InvalidTarget {
        /// Its width.
        width: u32,
        /// Its height.
        height: u32,
        /// Its format.
        format: TextureFormat,
    },
    /// The camera's settings make no projection.
    #[error("invalid camera")]
    Camera(#[source] ProjectionError),
    /// A mesh can't be registered.
    #[error("invalid mesh")]
    Mesh(#[source] MeshError),
    /// The handle names no registered mesh.
    #[error("{0:?} names no registered mesh")]
    UnknownMesh(MeshHandle),
    /// Every mesh slot is taken.
    #[error("too many meshes are registered")]
    TooManyMeshes,
    /// The cooked shaders lack one the renderer needs. A bug: loading checks it.
    #[error("the cooked shaders lack `{0}`")]
    MissingShader(&'static str),
    /// The device failed.
    #[error("{action} failed")]
    Device {
        /// What was being done.
        action: &'static str,
        /// The device's error.
        #[source]
        source: RhiError,
    },
}

fn device_error(action: &'static str) -> impl FnOnce(RhiError) -> RenderError {
    move |source| RenderError::Device { action, source }
}

/// Where a frame is rendered: a color texture of the renderer's device, with
/// [`TextureUsage::RENDER_TARGET`]. The application owns it (ADR-0019 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderTarget {
    /// The texture.
    pub texture: TextureHandle,
    /// Its format.
    pub format: TextureFormat,
    /// Its size in pixels.
    pub size: (u32, u32),
}

impl RenderTarget {
    /// A frame's backbuffer.
    #[must_use]
    pub fn backbuffer(frame: &Frame) -> Self {
        Self {
            texture: frame.backbuffer(),
            format: frame.backbuffer_format(),
            size: frame.backbuffer_size(),
        }
    }
}

/// What a [`Renderer::render`] call drew.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct RenderStats {
    /// Draws recorded.
    pub drawn: u32,
    /// Draws skipped because their mesh handle names no registered mesh.
    pub skipped: u32,
}

/// The default material: a checkerboard every mesh is drawn with until materials exist.
#[derive(Debug)]
struct Material {
    texture: TextureHandle,
    sampler: SamplerHandle,
    texture_index: u32,
    sampler_index: u32,
}

/// Draws [`RenderWorld`]s through the RHI (ADR-0019 §1).
///
/// It holds the device as `Arc<dyn Device>` (ADR-0017 §1) and owns the mesh pass: its pipeline,
/// depth target, meshes and default material. The application owns surfaces, the frame loop
/// and the color target, and hands [`Renderer::render`] an encoder to record into.
///
/// **Safety by construction.** The mesh shader reads vertices through buffer addresses and
/// samples through bindless indices, which `ShaderCode::from_spirv_unchecked` makes the
/// program's responsibility (ADR-0016 §2). The renderer takes all of it on: no pipeline,
/// shader code, address or index appears in its API; meshes are validated when registered;
/// stale handles are skipped; and it records only into its own device's encoders.
#[derive(Debug)]
pub struct Renderer {
    device: Arc<dyn Device>,
    shaders: CookedShaders,
    /// The pipeline and the color format it was created for.
    pipeline: Option<(PipelineHandle, TextureFormat)>,
    /// The depth target and the size it was created for.
    depth: Option<(TextureHandle, (u32, u32))>,
    meshes: MeshPool,
    material: Material,
}

impl Renderer {
    /// Creates a renderer drawing with `shaders` on `device`, and uploads its default material.
    ///
    /// # Errors
    ///
    /// [`RenderError::Device`] if the material can't be created.
    pub fn new(device: Arc<dyn Device>, shaders: CookedShaders) -> Result<Self, RenderError> {
        let material = create_material(device.as_ref())?;
        Ok(Self {
            device,
            shaders,
            pipeline: None,
            depth: None,
            meshes: MeshPool::default(),
            material,
        })
    }

    /// The device the renderer draws with.
    #[must_use]
    pub fn device(&self) -> &Arc<dyn Device> {
        &self.device
    }

    /// Validates `mesh` and uploads it. Draw it by naming the returned handle in a
    /// [`RenderWorld`].
    ///
    /// # Errors
    ///
    /// [`RenderError::Mesh`] if it's invalid (nothing is uploaded),
    /// [`RenderError::TooManyMeshes`], or [`RenderError::Device`].
    pub fn register_mesh(&mut self, mesh: &MeshData) -> Result<MeshHandle, RenderError> {
        let index_count = mesh.validate().map_err(RenderError::Mesh)?;
        let gpu = self.upload(mesh, index_count)?;
        match self.meshes.insert(gpu) {
            Some(handle) => Ok(handle),
            None => Err(RenderError::TooManyMeshes),
        }
    }

    fn upload(&self, mesh: &MeshData, index_count: u32) -> Result<GpuMesh, RenderError> {
        let device = self.device.as_ref();
        let vertex_bytes = mesh.vertex_bytes();
        let index_bytes = mesh.indices.bytes();
        let vertices = device
            .create_buffer(&BufferDesc {
                label: "mesh vertices",
                size: vertex_bytes.len() as u64,
                usage: BufferUsage::SHADER_ADDRESS | BufferUsage::COPY_DESTINATION,
            })
            .map_err(device_error("creating a vertex buffer"))?;
        let rest = (|| {
            device
                .write_buffer(vertices, 0, &vertex_bytes)
                .map_err(device_error("uploading vertices"))?;
            let address = device
                .buffer_address(vertices)
                .map_err(device_error("reading a vertex buffer's address"))?;
            let indices = device
                .create_buffer(&BufferDesc {
                    label: "mesh indices",
                    size: index_bytes.len() as u64,
                    usage: BufferUsage::INDEX | BufferUsage::COPY_DESTINATION,
                })
                .map_err(device_error("creating an index buffer"))?;
            if let Err(e) = device.write_buffer(indices, 0, &index_bytes) {
                destroy_logged("index buffer", device.destroy_buffer(indices));
                return Err(device_error("uploading indices")(e));
            }
            Ok((address, indices))
        })();
        match rest {
            Ok((address, indices)) => Ok(GpuMesh {
                vertices,
                address,
                indices,
                index_count,
                index_format: mesh.indices.format(),
            }),
            Err(e) => {
                destroy_logged("vertex buffer", device.destroy_buffer(vertices));
                Err(e)
            }
        }
    }

    /// Unregisters a mesh. Its handle names nothing from now on, and its buffers are destroyed
    /// once the GPU and every encoder already recording are done with them (ADR-0016 §1).
    ///
    /// # Errors
    ///
    /// [`RenderError::UnknownMesh`] if the handle names no registered mesh, or
    /// [`RenderError::Device`].
    pub fn unregister_mesh(&mut self, handle: MeshHandle) -> Result<(), RenderError> {
        let mesh = self
            .meshes
            .remove(handle)
            .ok_or(RenderError::UnknownMesh(handle))?;
        destroy_mesh(self.device.as_ref(), &mesh)
    }

    /// Whether `handle` names a registered mesh.
    #[must_use]
    pub fn has_mesh(&self, handle: MeshHandle) -> bool {
        self.meshes.get(handle).is_some()
    }

    /// How many meshes are registered.
    #[must_use]
    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// Records `world` into `encoder`, as one render pass that clears `target` to the world's
    /// background and draws its meshes with depth testing.
    ///
    /// The encoder must be the renderer's device's: from [`Device::create_encoder`] or a frame
    /// of [`Device::begin_frame`]. Draws whose mesh handle is stale are skipped, counted and
    /// logged. Recording itself can't fail: the device reports misuse when the encoder is
    /// submitted (ADR-0017 §3).
    ///
    /// # Errors
    ///
    /// [`RenderError::ForeignEncoder`], [`RenderError::InvalidTarget`],
    /// [`RenderError::Camera`], or [`RenderError::Device`] if the pipeline or depth target
    /// can't be created. Nothing is recorded then.
    pub fn render(
        &mut self,
        encoder: &mut dyn CommandEncoder,
        target: &RenderTarget,
        world: &RenderWorld,
    ) -> Result<RenderStats, RenderError> {
        // Recording looks handles up in the encoder's device. In another device's encoder our
        // handles could name unrelated objects, and the shader would read our addresses.
        if !self.device.owns_encoder(encoder) {
            return Err(RenderError::ForeignEncoder);
        }
        let (width, height) = target.size;
        if width == 0 || height == 0 || target.format.is_depth() {
            return Err(RenderError::InvalidTarget {
                width,
                height,
                format: target.format,
            });
        }
        // Sizes are far below 2^24, where f32 would lose precision.
        let aspect = width as f32 / height as f32;
        let projection =
            perspective_infinite_reversed(world.camera.vertical_fov, aspect, world.camera.near)
                .map_err(RenderError::Camera)?;
        let clip_from_world = projection * view_matrix(&world.camera.transform);
        let pipeline = self.pipeline_for(target.format)?;
        let depth = self.depth_for(target.size)?;

        let mut stats = RenderStats::default();
        let attachments = [ColorAttachment {
            texture: target.texture,
            load: LoadOp::Clear(world.background),
            store: StoreOp::Store,
        }];
        let mut pass = encoder.begin_render_pass(&RenderPassDesc {
            label: "mesh pass",
            color_attachments: &attachments,
            depth_attachment: Some(DepthAttachment {
                texture: depth,
                load: DepthLoadOp::Clear(REVERSED_DEPTH_CLEAR),
                store: StoreOp::DontCare, // Only this pass reads it.
            }),
        });
        pass.set_pipeline(pipeline);
        for draw in &world.draws {
            let Some(mesh) = self.meshes.get(draw.mesh) else {
                stats.skipped += 1;
                continue;
            };
            pass.set_index_buffer(mesh.indices, 0, mesh.index_format);
            pass.set_push_constants(&self.constants(clip_from_world, &draw.object_to_world, mesh));
            // The whole index buffer, with vertex offset 0: registration checked every index.
            pass.draw_indexed(0..mesh.index_count, 0, 1);
            stats.drawn += 1;
        }
        drop(pass);
        if stats.skipped > 0 {
            warn!(
                skipped = stats.skipped,
                "draws named unregistered meshes and were skipped"
            );
        }
        Ok(stats)
    }

    /// Push constants for drawing `mesh` with object-to-world matrix `model`.
    fn constants(
        &self,
        clip_from_world: Mat4,
        model: &Mat4,
        mesh: &GpuMesh,
    ) -> [u8; CONSTANT_BYTES] {
        let clip_from_object = clip_from_world * *model;
        let mut bytes = [0; CONSTANT_BYTES];
        let (words, _) = bytes.as_chunks_mut::<4>();
        for (chunk, value) in words.iter_mut().zip(clip_from_object.to_cols_array()) {
            *chunk = value.to_le_bytes();
        }
        bytes[64..72].copy_from_slice(&mesh.address.to_le_bytes());
        bytes[72..76].copy_from_slice(&self.material.texture_index.to_le_bytes());
        bytes[76..80].copy_from_slice(&self.material.sampler_index.to_le_bytes());
        bytes
    }

    /// The pipeline for color `format`, created (or recreated) as needed.
    fn pipeline_for(&mut self, format: TextureFormat) -> Result<PipelineHandle, RenderError> {
        if let Some((pipeline, created_for)) = self.pipeline {
            if created_for == format {
                return Ok(pipeline);
            }
            self.pipeline = None;
            // Destruction is deferred until the GPU and open encoders are done with it.
            self.device
                .destroy_render_pipeline(pipeline)
                .map_err(device_error("destroying the previous pipeline"))?;
        }
        let spirv = self
            .shaders
            .spirv("Mesh")
            .ok_or(RenderError::MissingShader("Mesh"))?;
        // SAFETY: `ShaderCode::from_spirv_unchecked`'s obligations:
        // - Valid SPIR-V within Oxreal's features: `CookedShaders::load` accepted these words
        //   only if their SHA-256 matches a manifest whose source key equals the one this crate
        //   was built with (build.rs). Such manifests are written only by the cook, which uses
        //   exactly the pinned Slang with its capability allowlist and fails unless `spirv-val`
        //   accepts the module (ADR-0019 §4). Tampering with the cooked directory is out of
        //   scope, as for the executable itself.
        // - Layouts: the source key ties the words to exactly the `Mesh.slang` and
        //   `Bindless.slang` this build packs for; tests/shader_layout.rs checks those sources
        //   read `Vertex` with a 32-byte stride (`mesh::VERTEX_BYTES`) and the push constants
        //   at the offsets `constants` writes, and every draw writes all 80 bytes first.
        // - Addresses: the only address the shader gets is a registered mesh's vertex buffer,
        //   looked up by generational handle in the same `render` call that records the draw.
        //   Unregistering makes the handle stale at once and destroys the buffer through the
        //   RHI's deferred destruction, so it outlives every encoder already recording. Only
        //   this device's encoders are recorded into (`owns_encoder`), so the address and
        //   handles mean what we created them as.
        // - Indexed draws: `MeshData::validate` checked at registration that every index names a
        //   vertex and that vertex counts stay within `0..=i32::MAX`; each draw covers exactly
        //   the mesh's whole index buffer (bound at offset 0) with vertex offset 0.
        // - Bindless indices: only the default material's texture and sampler, which live until
        //   the renderer is dropped. The material texture is private, so it's never an
        //   attachment of the pass that samples it.
        let code = unsafe { ShaderCode::from_spirv_unchecked(spirv) };
        let pipeline = self
            .device
            .create_render_pipeline(&RenderPipelineDesc {
                label: "mesh pass",
                vertex: ShaderEntry {
                    code,
                    entry_point: "meshVertex",
                },
                pixel: ShaderEntry {
                    code,
                    entry_point: "meshPixel",
                },
                color_formats: &[format],
                depth: Some(DepthState::SCENE),
                push_constant_bytes: CONSTANT_BYTES as u32,
                cull: CullMode::Back,
            })
            .map_err(device_error("creating the mesh pipeline"))?;
        debug!(?format, "mesh pipeline created");
        self.pipeline = Some((pipeline, format));
        Ok(pipeline)
    }

    /// The depth target for `size`, created (or recreated) as needed.
    fn depth_for(&mut self, size: (u32, u32)) -> Result<TextureHandle, RenderError> {
        if let Some((depth, created_for)) = self.depth {
            if created_for == size {
                return Ok(depth);
            }
            self.depth = None;
            // Destruction is deferred until the GPU and open encoders are done with it.
            self.device
                .destroy_texture(depth)
                .map_err(device_error("destroying the previous depth target"))?;
        }
        let depth = self
            .device
            .create_texture(&TextureDesc {
                label: "mesh pass depth",
                width: size.0,
                height: size.1,
                format: TextureFormat::Depth32Float,
                usage: TextureUsage::DEPTH_TARGET,
            })
            .map_err(device_error("creating the depth target"))?;
        debug!(width = size.0, height = size.1, "depth target created");
        self.depth = Some((depth, size));
        Ok(depth)
    }
}

impl Drop for Renderer {
    /// Destroys everything the renderer created. Destruction is deferred, so work already
    /// recorded completes first. Failures are logged.
    fn drop(&mut self) {
        let device = self.device.as_ref();
        if let Some((pipeline, _)) = self.pipeline.take() {
            destroy_logged("pipeline", device.destroy_render_pipeline(pipeline));
        }
        if let Some((depth, _)) = self.depth.take() {
            destroy_logged("depth target", device.destroy_texture(depth));
        }
        for mesh in self.meshes.drain() {
            if let Err(e) = destroy_mesh(device, &mesh) {
                warn!(error = %e, "destroying a mesh failed");
            }
        }
        destroy_logged(
            "material sampler",
            device.destroy_sampler(self.material.sampler),
        );
        destroy_logged(
            "material texture",
            device.destroy_texture(self.material.texture),
        );
    }
}

fn destroy_mesh(device: &dyn Device, mesh: &GpuMesh) -> Result<(), RenderError> {
    let vertices = device.destroy_buffer(mesh.vertices);
    let indices = device.destroy_buffer(mesh.indices);
    vertices
        .and(indices)
        .map_err(device_error("destroying a mesh's buffers"))
}

fn destroy_logged(what: &str, result: Result<(), RhiError>) {
    if let Err(e) = result {
        warn!(error = %e, what, "destroying a renderer object failed");
    }
}

/// Uploads the checkerboard: Oxreal orange and a dark grey, picked in sRGB. The texture is sRGB
/// too, so sampling decodes it to linear color (ADR-0011 §5).
fn create_material(device: &dyn Device) -> Result<Material, RenderError> {
    let square = BOARD_TEXELS / BOARD_SQUARES;
    let pixels: Vec<u8> = (0..BOARD_TEXELS)
        .flat_map(|y| (0..BOARD_TEXELS).map(move |x| (x, y)))
        .flat_map(|(x, y)| {
            if (x / square + y / square).is_multiple_of(2) {
                [0xE8, 0x6A, 0x17, 0xFF]
            } else {
                [0x24, 0x26, 0x2B, 0xFF]
            }
        })
        .collect();
    let texture = device
        .create_texture(&TextureDesc {
            label: "default material",
            width: BOARD_TEXELS,
            height: BOARD_TEXELS,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsage::SAMPLED | TextureUsage::COPY_DESTINATION,
        })
        .map_err(device_error("creating the default material"))?;
    let rest = (|| {
        device
            .write_texture(texture, &pixels)
            .map_err(device_error("uploading the default material"))?;
        let texture_index = device
            .texture_index(texture)
            .map_err(device_error("indexing the default material"))?;
        let sampler = device
            .create_sampler(&SamplerDesc {
                label: "default material",
                mag_filter: Filter::Linear,
                min_filter: Filter::Linear,
                address_mode: AddressMode::ClampToEdge,
                // Surfaces are often seen at steep angles.
                max_anisotropy: Some(8.0),
            })
            .map_err(device_error("creating the default sampler"))?;
        match device.sampler_index(sampler) {
            Ok(sampler_index) => Ok((texture_index, sampler, sampler_index)),
            Err(e) => {
                destroy_logged("material sampler", device.destroy_sampler(sampler));
                Err(device_error("indexing the default sampler")(e))
            }
        }
    })();
    match rest {
        Ok((texture_index, sampler, sampler_index)) => Ok(Material {
            texture,
            sampler,
            texture_index,
            sampler_index,
        }),
        Err(e) => {
            destroy_logged("material texture", device.destroy_texture(texture));
            Err(e)
        }
    }
}
