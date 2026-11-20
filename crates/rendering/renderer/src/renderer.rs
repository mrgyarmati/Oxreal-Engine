//! The renderer: owns the mesh pass's GPU objects and records a [`RenderWorld`] into a target.

use std::sync::Arc;

use oxreal_diagnostics::{debug, profile_function, warn};
use oxreal_math::Mat4;
use oxreal_math::projection::{ProjectionError, REVERSED_DEPTH_CLEAR, view_matrix};
use oxreal_rhi::{
    AddressMode, BufferDesc, BufferUsage, ColorAttachment, CommandEncoder, CullMode,
    DepthAttachment, DepthLoadOp, DepthState, Device, Filter, Frame, LoadOp, PipelineHandle,
    RenderPassDesc, RenderPipelineDesc, RhiError, SamplerDesc, SamplerHandle, ShaderCode,
    ShaderEntry, StoreOp, TextureDesc, TextureFormat, TextureHandle, TextureUsage,
};

use crate::mesh::{GpuMesh, MeshPool};
use crate::{CookedShaders, MeshData, MeshError, MeshHandle, RenderWorld};

/// The mesh shader's push constants (`shaders/Mesh.slang`'s `MeshConstants`, which
/// tests/shader_layout.rs checks): the object-to-clip matrix at 0, the vertex buffer address,
/// then the texture and sampler indices.
pub(crate) const CONSTANT_BYTES: usize = 80;
pub(crate) const ADDRESS_OFFSET: usize = 64;
pub(crate) const TEXTURE_INDEX_OFFSET: usize = 72;
pub(crate) const SAMPLER_INDEX_OFFSET: usize = 76;
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
    /// The target has no area, a format the mesh pass can't render to, or is a texture the
    /// renderer samples.
    #[error(
        "can't render to {texture:?} ({width}x{height} {format:?}): a target needs an area, a \
         color format, and must not be the renderer's own texture"
    )]
    InvalidTarget {
        /// The texture.
        texture: TextureHandle,
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
///
/// `format` and `size` must describe the texture. If they don't, the device refuses the pass
/// when the encoder is submitted, and nothing is drawn.
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

/// What a [`Renderer::render`] call recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct RenderStats {
    /// Draws recorded. The device can still refuse them at submit (ADR-0017 §3).
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
/// It holds the device as `Arc<dyn Device>` (ADR-0017 §1) and owns the mesh pass: its
/// pipelines, depth target, meshes and default material. The application owns surfaces, the
/// frame loop and the color target, and hands [`Renderer::render`] an encoder to record into.
///
/// **The shader's obligations.** The mesh shader reads vertices through buffer addresses and
/// samples through bindless indices, which `ShaderCode::from_spirv_unchecked` makes the
/// program's responsibility (ADR-0016 §2). The renderer discharges them for everything it
/// records: no pipeline, shader code, address or index appears in its API; meshes are
/// validated when registered; stale handles are skipped; and it records only into its own
/// device's encoders. RHI handles are plain numbers, so code that forges them (`from_raw`)
/// remains the program's responsibility, as ADR-0016 §2 says.
///
/// **Limits.**
/// - One depth target, sized to the last target. Rendering to targets of different sizes
///   recreates it each time (an editor with several viewports will want one renderer each, or
///   a cache keyed by size). Render with one unsubmitted encoder at a time: two open encoders
///   can't share the depth target, and the device refuses the second pass at submit.
/// - Mesh handles name meshes of the renderer that registered them. Another renderer may have a
///   different mesh under the same handle.
#[derive(Debug)]
pub struct Renderer {
    device: Arc<dyn Device>,
    shaders: CookedShaders,
    /// One pipeline per color format rendered to.
    pipelines: Vec<(TextureFormat, PipelineHandle)>,
    /// The depth target and the size it was created for.
    depth: Option<(TextureHandle, (u32, u32))>,
    meshes: MeshPool,
    material: Material,
    /// How many draws the last `render` skipped, so a lasting stale handle is logged once.
    last_skipped: u32,
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
            pipelines: Vec::new(),
            depth: None,
            meshes: MeshPool::default(),
            material,
            last_skipped: 0,
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
    /// [`RenderError::Mesh`] if it's invalid, [`RenderError::TooManyMeshes`], or
    /// [`RenderError::Device`]. Nothing is uploaded then.
    pub fn register_mesh(&mut self, mesh: &MeshData) -> Result<MeshHandle, RenderError> {
        profile_function!();
        let index_count = mesh.checked_index_count().map_err(RenderError::Mesh)?;
        if self.meshes.is_full() {
            return Err(RenderError::TooManyMeshes);
        }
        let gpu = self.upload(mesh, index_count)?;
        self.meshes.insert(gpu).ok_or(RenderError::TooManyMeshes)
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
    /// of [`Device::begin_frame`]. Draws whose mesh handle is stale are skipped and counted, and
    /// logged when their number changes. Recording itself can't fail: the device reports misuse
    /// when the encoder is submitted (ADR-0017 §3).
    ///
    /// # Errors
    ///
    /// [`RenderError::ForeignEncoder`], [`RenderError::InvalidTarget`],
    /// [`RenderError::Camera`], or [`RenderError::Device`] if a pipeline or the depth target
    /// can't be created. Nothing is recorded then.
    pub fn render(
        &mut self,
        encoder: &mut dyn CommandEncoder,
        target: &RenderTarget,
        world: &RenderWorld,
    ) -> Result<RenderStats, RenderError> {
        profile_function!();
        // Recording looks handles up in the encoder's device. In another device's encoder our
        // handles could name unrelated objects, and the shader would read our addresses.
        if !self.device.owns_encoder(encoder) {
            return Err(RenderError::ForeignEncoder);
        }
        let (width, height) = target.size;
        // The material texture is the one the shader samples: it must never be the attachment.
        if width == 0
            || height == 0
            || target.format.is_depth()
            || target.texture == self.material.texture
        {
            return Err(RenderError::InvalidTarget {
                texture: target.texture,
                width,
                height,
                format: target.format,
            });
        }
        // Sizes are far below 2^24, where f32 would lose precision.
        let aspect = width as f32 / height as f32;
        let clip_from_view = world
            .camera
            .projection
            .clip_from_view(aspect)
            .map_err(RenderError::Camera)?;
        let clip_from_world = clip_from_view * view_matrix(&world.camera.transform);
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
            pass.set_push_constants(&pack_constants(
                clip_from_world * draw.object_to_world,
                mesh.address,
                self.material.texture_index,
                self.material.sampler_index,
            ));
            // The whole index buffer, with vertex offset 0: registration checked every index.
            pass.draw_indexed(0..mesh.index_count, 0, 1);
            stats.drawn += 1;
        }
        drop(pass);
        if stats.skipped != self.last_skipped {
            if stats.skipped > 0 {
                warn!(
                    skipped = stats.skipped,
                    "draws name unregistered meshes and are skipped"
                );
            }
            self.last_skipped = stats.skipped;
        }
        Ok(stats)
    }

    /// The pipeline for color `format`, created the first time it's needed.
    fn pipeline_for(&mut self, format: TextureFormat) -> Result<PipelineHandle, RenderError> {
        if let Some(&(_, pipeline)) = self.pipelines.iter().find(|(f, _)| *f == format) {
            return Ok(pipeline);
        }
        // SAFETY: `ShaderCode::from_spirv_unchecked`'s obligations, for every draw this renderer
        // records with the pipeline:
        // - Valid SPIR-V within Oxreal's features: `CookedShaders::load` accepted these words
        //   only if their SHA-256 matches a manifest whose source key equals the one this crate
        //   was built with (build.rs). Such manifests are written only by the cook, which uses
        //   exactly the pinned Slang with its fixed flags and capability allowlist, and fails
        //   unless `spirv-val` accepts the module (ADR-0019 §4). Tampering with the cooked
        //   directory is out of scope, as for the executable itself.
        // - Layouts: the source key covers the Slang version, the fixed compiler flags and the
        //   exact bytes of `Mesh.slang` and `Bindless.slang`. For those sources,
        //   tests/shader_layout.rs checks the shader reads `Vertex` with a 32-byte stride at
        //   offsets 0, 12 and 24 (`mesh::VERTEX_BYTES`) and the push constants at the offsets
        //   `pack_constants` writes; every draw writes all 80 bytes first. This argument holds
        //   for sources that test passes, which CI and `cargo xtask ci` run: edit the shader,
        //   run it.
        // - Addresses: the only address the shader gets is a registered mesh's vertex buffer,
        //   looked up by generational handle in the same `render` call that records the draw.
        //   It's a multiple of `BUFFER_ADDRESS_ALIGNMENT` (16), and vertices are 32 bytes of
        //   4-byte floats, so every read is aligned. Unregistering makes the handle stale at
        //   once and destroys the buffer through deferred destruction, which outlives every
        //   encoder already recording. Only this device's encoders are recorded into
        //   (`owns_encoder`, which `CommandEncoder`'s unsafe contract makes trustworthy), so the
        //   address and handles mean what we created them as.
        // - Indexed draws: `MeshData::checked_index_count` checked at registration that every
        //   index names a vertex and that vertex counts stay within `0..=i32::MAX`; each draw
        //   covers exactly the mesh's whole index buffer (bound at offset 0) with vertex offset 0.
        // - Bindless indices: only the default material's texture and sampler, which live until
        //   the renderer is dropped. The material texture can't be a render target: `render`
        //   refuses it, and it lacks `RENDER_TARGET` usage, so the device would too.
        let code = unsafe { ShaderCode::from_spirv_unchecked(self.shaders.mesh()) };
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
        self.pipelines.push((format, pipeline));
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
        for (_, pipeline) in self.pipelines.drain(..) {
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

/// The mesh shader's push constants: `clip_from_object` column-major, then the vertex buffer
/// address and the material's indices, all little-endian.
pub(crate) fn pack_constants(
    clip_from_object: Mat4,
    address: u64,
    texture_index: u32,
    sampler_index: u32,
) -> [u8; CONSTANT_BYTES] {
    let mut bytes = [0; CONSTANT_BYTES];
    let (words, _) = bytes[..ADDRESS_OFFSET].as_chunks_mut::<4>();
    for (chunk, value) in words.iter_mut().zip(clip_from_object.to_cols_array()) {
        *chunk = value.to_le_bytes();
    }
    bytes[ADDRESS_OFFSET..TEXTURE_INDEX_OFFSET].copy_from_slice(&address.to_le_bytes());
    bytes[TEXTURE_INDEX_OFFSET..SAMPLER_INDEX_OFFSET].copy_from_slice(&texture_index.to_le_bytes());
    bytes[SAMPLER_INDEX_OFFSET..].copy_from_slice(&sampler_index.to_le_bytes());
    bytes
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

#[cfg(test)]
mod tests {
    use oxreal_math::Vec4;

    use super::*;

    #[test]
    fn constants_are_packed_at_the_shaders_offsets() {
        let matrix = Mat4::from_cols(
            Vec4::new(1.0, 2.0, 3.0, 4.0),
            Vec4::new(5.0, 6.0, 7.0, 8.0),
            Vec4::new(9.0, 10.0, 11.0, 12.0),
            Vec4::new(13.0, 14.0, 15.0, 16.0),
        );
        let bytes = pack_constants(matrix, 0x0102_0304_0506_0708, 9, 10);
        // Column-major: the first column's four floats first.
        for (i, value) in (1_u8..=16).enumerate() {
            let float = f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
            assert_eq!(float, f32::from(value), "float {i}");
        }
        assert_eq!(
            u64::from_le_bytes(bytes[64..72].try_into().unwrap()),
            0x0102_0304_0506_0708
        );
        assert_eq!(u32::from_le_bytes(bytes[72..76].try_into().unwrap()), 9);
        assert_eq!(u32::from_le_bytes(bytes[76..80].try_into().unwrap()), 10);
        // The offsets the layout test expects of the shader.
        assert_eq!(
            [
                0,
                ADDRESS_OFFSET,
                TEXTURE_INDEX_OFFSET,
                SAMPLER_INDEX_OFFSET
            ],
            [0, 64, 72, 76]
        );
    }
}
