//! Meshes: their data, its validation, and the generational handles a [`crate::RenderWorld`]
//! names them by.

use oxreal_math::{Vec2, Vec3};
use oxreal_rhi::{BufferHandle, IndexFormat};

/// The bytes one [`Vertex`] takes in a vertex buffer: 8 little-endian `f32`s, the layout
/// `shaders/Mesh.slang` reads (tests/shader_layout.rs checks it).
pub(crate) const VERTEX_BYTES: usize = 32;

/// The most vertices a mesh may have. The vertex shader sees each index (plus a vertex offset
/// of 0) as its vertex ID, which must lie in `0..=i32::MAX` (`ShaderCode::from_spirv_unchecked`).
pub const MAX_VERTICES: usize = 1 << 31;

/// A mesh vertex.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vertex {
    /// Position in object space, metres (ADR-0011).
    pub position: Vec3,
    /// Normal in object space, unit length.
    pub normal: Vec3,
    /// Texture coordinates: (0, 0) is the texture's first texel.
    pub uv: Vec2,
}

/// A mesh's triangle indices, three per triangle, each naming a vertex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Indices {
    /// 16-bit indices, for meshes of up to 65,536 vertices.
    U16(Vec<u16>),
    /// 32-bit indices.
    U32(Vec<u32>),
}

impl Indices {
    fn len(&self) -> usize {
        match self {
            Self::U16(i) => i.len(),
            Self::U32(i) => i.len(),
        }
    }

    /// The position and value of the first index that names no vertex, if any.
    fn first_out_of_range(&self, vertex_count: usize) -> Option<(usize, u32)> {
        let too_large = |(position, index): (usize, u32)| {
            (usize::try_from(index).map_or(true, |i| i >= vertex_count))
                .then_some((position, index))
        };
        match self {
            Self::U16(i) => i
                .iter()
                .map(|&x| u32::from(x))
                .enumerate()
                .find_map(too_large),
            Self::U32(i) => i.iter().copied().enumerate().find_map(too_large),
        }
    }

    pub(crate) fn format(&self) -> IndexFormat {
        match self {
            Self::U16(_) => IndexFormat::Uint16,
            Self::U32(_) => IndexFormat::Uint32,
        }
    }

    pub(crate) fn bytes(&self) -> Vec<u8> {
        match self {
            Self::U16(i) => i.iter().flat_map(|x| x.to_le_bytes()).collect(),
            Self::U32(i) => i.iter().flat_map(|x| x.to_le_bytes()).collect(),
        }
    }
}

/// A triangle mesh to upload with [`crate::Renderer::register_mesh`].
#[derive(Debug, Clone, PartialEq)]
pub struct MeshData {
    /// The vertices.
    pub vertices: Vec<Vertex>,
    /// The triangles' indices into `vertices`. Triangles wind counter-clockwise seen from their
    /// front (ADR-0011 §4); back faces are culled.
    pub indices: Indices,
}

/// Why a mesh can't be registered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MeshError {
    /// It has no vertices or no indices.
    #[error("a mesh needs at least one vertex and one triangle")]
    Empty,
    /// Its index count isn't a multiple of 3.
    #[error("{count} indices don't make whole triangles (3 indices each)")]
    NotTriangles {
        /// The index count.
        count: usize,
    },
    /// An index names a vertex the mesh doesn't have.
    #[error("index {index} at position {position} names no vertex: the mesh has {vertex_count}")]
    IndexOutOfRange {
        /// Where the index is.
        position: usize,
        /// Its value.
        index: u32,
        /// How many vertices there are.
        vertex_count: usize,
    },
    /// It has more indices than a draw can name.
    #[error("{count} indices is more than a draw can name ({})", u32::MAX)]
    TooManyIndices {
        /// The index count.
        count: usize,
    },
    /// It has more vertices than a vertex ID can address.
    #[error("{count} vertices is more than the {MAX_VERTICES} a mesh may have")]
    TooManyVertices {
        /// The vertex count.
        count: usize,
    },
}

impl MeshData {
    /// Checks everything the mesh shader's safety contract relies on: every index names a
    /// vertex, and vertex IDs stay in `0..=i32::MAX`. Also that it holds whole triangles.
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn validate(&self) -> Result<(), MeshError> {
        self.checked_index_count().map(|_| ())
    }

    /// [`Self::validate`], returning the index count.
    pub(crate) fn checked_index_count(&self) -> Result<u32, MeshError> {
        let vertex_count = self.vertices.len();
        let index_count = self.indices.len();
        if vertex_count == 0 || index_count == 0 {
            return Err(MeshError::Empty);
        }
        if vertex_count > MAX_VERTICES {
            return Err(MeshError::TooManyVertices {
                count: vertex_count,
            });
        }
        let Ok(count) = u32::try_from(index_count) else {
            return Err(MeshError::TooManyIndices { count: index_count });
        };
        if !index_count.is_multiple_of(3) {
            return Err(MeshError::NotTriangles { count: index_count });
        }
        match self.indices.first_out_of_range(vertex_count) {
            Some((position, index)) => Err(MeshError::IndexOutOfRange {
                position,
                index,
                vertex_count,
            }),
            None => Ok(count),
        }
    }

    /// The vertices in the vertex buffer's layout.
    pub(crate) fn vertex_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.vertices.len() * VERTEX_BYTES);
        for v in &self.vertices {
            let floats = v
                .position
                .to_array()
                .into_iter()
                .chain(v.normal.to_array())
                .chain(v.uv.to_array());
            bytes.extend(floats.flat_map(f32::to_le_bytes));
        }
        bytes
    }

    /// A unit cube: [-0.5, 0.5] on each axis, four vertices per face so each face has its own
    /// normal and shows the whole texture upright (its first row along the face's top edge).
    #[must_use]
    pub fn cube() -> Self {
        // Each face's outward normal, and the direction of its top edge seen from outside.
        let faces = [
            (Vec3::X, Vec3::Y),
            (Vec3::NEG_X, Vec3::Y),
            (Vec3::Y, Vec3::NEG_Z),
            (Vec3::NEG_Y, Vec3::Z),
            (Vec3::Z, Vec3::Y),
            (Vec3::NEG_Z, Vec3::Y),
        ];
        // Texture coordinates of each face's corners, counter-clockwise seen from outside:
        // top left, bottom left, bottom right, top right.
        let corners = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
        let mut vertices = Vec::with_capacity(faces.len() * corners.len());
        let mut indices = Vec::with_capacity(faces.len() * 6);
        for (face, (normal, up)) in (0_u16..).zip(faces) {
            // Seen from outside, right × up points out of the face.
            let right = up.cross(normal);
            for [u, v] in corners {
                vertices.push(Vertex {
                    position: 0.5 * normal + (u - 0.5) * right - (v - 0.5) * up,
                    normal,
                    uv: Vec2::new(u, v),
                });
            }
            let first = face * 4;
            indices.extend([0, 1, 2, 0, 2, 3].map(|corner| first + corner));
        }
        Self {
            vertices,
            indices: Indices::U16(indices),
        }
    }
}

/// A registered mesh. Handles are generational: once the mesh is unregistered, its handle
/// names nothing, even after the slot is reused. A handle names a mesh of the renderer that
/// registered it; another renderer may have a different mesh under the same handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeshHandle {
    index: u32,
    generation: u32,
}

impl MeshHandle {
    /// A handle that never names a mesh, in any renderer: its slot index is reserved and never
    /// issued. Draws with it are skipped and counted like any stale handle. Use it where a
    /// handle is needed before a mesh exists, such as in tests that run without a GPU.
    ///
    /// It isn't a "no mesh" value: every draw with it is skipped, counted and logged. For a
    /// mesh that may be absent, use `Option<MeshHandle>`.
    pub const DANGLING: Self = Self {
        index: RESERVED_INDEX,
        generation: 0,
    };
}

/// The slot index [`MeshHandle::DANGLING`] uses. The pool never issues it.
const RESERVED_INDEX: u32 = u32::MAX;

/// A mesh's GPU buffers.
#[derive(Debug)]
pub(crate) struct GpuMesh {
    pub(crate) vertices: BufferHandle,
    /// The vertex buffer's address, for the shader.
    pub(crate) address: u64,
    pub(crate) indices: BufferHandle,
    pub(crate) index_count: u32,
    pub(crate) index_format: IndexFormat,
}

#[derive(Debug)]
struct Slot {
    generation: u32,
    mesh: Option<GpuMesh>,
}

/// Registered meshes, by generational handle.
#[derive(Debug)]
pub(crate) struct MeshPool {
    slots: Vec<Slot>,
    free: Vec<u32>,
    /// How many slots may exist: indices `0..slot_limit` are issued. At most
    /// [`RESERVED_INDEX`], so the reserved index never is. Tests lower it to reach the limit.
    slot_limit: u32,
}

impl Default for MeshPool {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            slot_limit: RESERVED_INDEX,
        }
    }
}

impl MeshPool {
    /// A pool that issues only indices `0..slot_limit`, to test what happens at the limit.
    #[cfg(test)]
    fn with_slot_limit(slot_limit: u32) -> Self {
        // Every `u32` is at most `RESERVED_INDEX` (`u32::MAX`), so the reserved index stays
        // unissued whatever the limit.
        Self {
            slot_limit,
            ..Self::default()
        }
    }

    /// Stores `mesh`, returning its handle, or gives `mesh` back if every slot is taken.
    pub(crate) fn insert(&mut self, mesh: GpuMesh) -> Result<MeshHandle, GpuMesh> {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.mesh = Some(mesh);
            return Ok(MeshHandle {
                index,
                generation: slot.generation,
            });
        }
        let Some(index) = u32::try_from(self.slots.len())
            .ok()
            .filter(|&index| index < self.slot_limit)
        else {
            return Err(mesh);
        };
        self.slots.push(Slot {
            generation: 0,
            mesh: Some(mesh),
        });
        Ok(MeshHandle {
            index,
            generation: 0,
        })
    }

    pub(crate) fn get(&self, handle: MeshHandle) -> Option<&GpuMesh> {
        self.slots
            .get(handle.index as usize)
            .filter(|slot| slot.generation == handle.generation)
            .and_then(|slot| slot.mesh.as_ref())
    }

    /// Removes the mesh `handle` names. Its slot is reused under a new generation; a slot whose
    /// generation would wrap is retired instead, so no handle can ever name a later mesh.
    pub(crate) fn remove(&mut self, handle: MeshHandle) -> Option<GpuMesh> {
        let slot = self
            .slots
            .get_mut(handle.index as usize)
            .filter(|slot| slot.generation == handle.generation)?;
        let mesh = slot.mesh.take()?;
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.free.push(handle.index);
        }
        Some(mesh)
    }

    /// Removes every mesh, advancing each slot's generation like [`Self::remove`].
    pub(crate) fn drain(&mut self) -> Vec<GpuMesh> {
        let handles: Vec<MeshHandle> = (0_u32..)
            .zip(&self.slots)
            .filter(|(_, slot)| slot.mesh.is_some())
            .map(|(index, slot)| MeshHandle {
                index,
                generation: slot.generation,
            })
            .collect();
        handles.into_iter().filter_map(|h| self.remove(h)).collect()
    }

    /// Whether no more meshes fit: every slot that may exist does, and none is free. It's true
    /// exactly when [`Self::insert`] would return `None`.
    pub(crate) fn is_full(&self) -> bool {
        self.free.is_empty()
            && usize::try_from(self.slot_limit).is_ok_and(|limit| self.slots.len() >= limit)
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.mesh.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu_mesh(raw: u64) -> GpuMesh {
        GpuMesh {
            vertices: BufferHandle::from_raw(raw),
            address: raw,
            indices: BufferHandle::from_raw(raw + 1),
            index_count: 3,
            index_format: IndexFormat::Uint16,
        }
    }

    #[test]
    fn the_cube_is_valid_and_packs_32_bytes_per_vertex() {
        let cube = MeshData::cube();
        assert_eq!(cube.validate(), Ok(()));
        assert_eq!(cube.checked_index_count(), Ok(36));
        assert_eq!(cube.vertices.len(), 24);
        assert_eq!(cube.indices.len(), 36);
        assert_eq!(cube.vertex_bytes().len(), 24 * VERTEX_BYTES);
        // The first vertex: position, then normal, then uv, little-endian.
        let bytes = cube.vertex_bytes();
        let first = cube.vertices[0];
        assert_eq!(&bytes[0..4], &first.position.x.to_le_bytes());
        assert_eq!(&bytes[12..16], &first.normal.x.to_le_bytes());
        assert_eq!(&bytes[24..28], &first.uv.x.to_le_bytes());
    }

    #[test]
    fn the_cube_winds_counter_clockwise_from_outside() {
        let cube = MeshData::cube();
        let Indices::U16(indices) = &cube.indices else {
            panic!("the cube uses 16-bit indices");
        };
        for triangle in indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|i| cube.vertices[usize::from(triangle[i])]);
            let facing = (b.position - a.position).cross(c.position - a.position);
            assert!(facing.dot(a.normal) > 0.0, "{triangle:?} faces inwards");
        }
    }

    #[test]
    fn invalid_meshes_are_refused_with_the_reason() {
        let vertex = Vertex::default();
        let mesh = |vertices: usize, indices: Indices| MeshData {
            vertices: vec![vertex; vertices],
            indices,
        };
        assert_eq!(
            mesh(0, Indices::U16(vec![0, 0, 0])).validate(),
            Err(MeshError::Empty)
        );
        assert_eq!(
            mesh(3, Indices::U32(vec![])).validate(),
            Err(MeshError::Empty)
        );
        assert_eq!(
            mesh(3, Indices::U16(vec![0, 1])).validate(),
            Err(MeshError::NotTriangles { count: 2 })
        );
        assert_eq!(
            mesh(3, Indices::U32(vec![0, 1, 2, 2, 3, 0])).validate(),
            Err(MeshError::IndexOutOfRange {
                position: 4,
                index: 3,
                vertex_count: 3
            })
        );
        assert_eq!(
            mesh(3, Indices::U32(vec![0, 1, u32::MAX])).validate(),
            Err(MeshError::IndexOutOfRange {
                position: 2,
                index: u32::MAX,
                vertex_count: 3
            })
        );
    }

    #[test]
    fn handles_are_generational() {
        let mut pool = MeshPool::default();
        let first = pool.insert(gpu_mesh(1)).unwrap();
        assert!(pool.get(first).is_some());
        assert!(pool.remove(first).is_some());
        assert!(
            pool.get(first).is_none(),
            "a removed mesh's handle names nothing"
        );
        assert!(pool.remove(first).is_none());

        let second = pool.insert(gpu_mesh(3)).unwrap();
        assert_eq!(second.index, first.index, "the slot is reused");
        assert_ne!(second, first);
        assert!(
            pool.get(first).is_none(),
            "an old handle never names the new mesh"
        );
        assert_eq!(pool.get(second).unwrap().address, 3);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn the_pool_is_full_exactly_when_insert_fails() {
        let mut pool = MeshPool::with_slot_limit(3);
        let mut handles = Vec::new();
        for i in 0..3 {
            assert!(!pool.is_full());
            handles.push(pool.insert(gpu_mesh(i)).expect("below the limit"));
        }
        assert!(pool.is_full());
        assert!(pool.insert(gpu_mesh(9)).is_err(), "full means full");
        assert!(
            handles.iter().all(|h| h.index < 3),
            "no index past the limit"
        );

        // A freed slot makes room again.
        pool.remove(handles[1]).unwrap();
        assert!(!pool.is_full());
        handles[1] = pool.insert(gpu_mesh(10)).expect("the freed slot");
        assert!(pool.is_full());

        // A retired slot (its generation would wrap) doesn't make room.
        pool.slots[0].generation = u32::MAX;
        let last = MeshHandle {
            index: 0,
            generation: u32::MAX,
        };
        pool.remove(last).unwrap();
        assert!(pool.is_full(), "the retired slot isn't free");
        assert!(pool.insert(gpu_mesh(11)).is_err());
    }

    #[test]
    fn the_default_limit_leaves_the_reserved_index_unissued() {
        let pool = MeshPool::default();
        assert_eq!(pool.slot_limit, RESERVED_INDEX);
        assert_eq!(MeshHandle::DANGLING.index, RESERVED_INDEX);
    }

    #[test]
    fn the_dangling_handle_names_nothing() {
        let mut pool = MeshPool::default();
        assert!(pool.get(MeshHandle::DANGLING).is_none());
        let handles: Vec<MeshHandle> = (0..4).map(|i| pool.insert(gpu_mesh(i)).unwrap()).collect();
        assert!(!handles.contains(&MeshHandle::DANGLING));
        assert!(pool.get(MeshHandle::DANGLING).is_none());
        assert!(pool.remove(MeshHandle::DANGLING).is_none());
        assert_eq!(pool.len(), 4, "removing it removed nothing");
    }

    #[test]
    fn a_slot_whose_generation_would_wrap_is_retired() {
        let mut pool = MeshPool::default();
        let handle = pool.insert(gpu_mesh(1)).unwrap();
        pool.slots[0].generation = u32::MAX;
        let last = MeshHandle {
            index: handle.index,
            generation: u32::MAX,
        };
        assert!(pool.remove(last).is_some());
        let next = pool.insert(gpu_mesh(2)).unwrap();
        assert_ne!(next.index, handle.index, "the exhausted slot isn't reused");
    }

    #[test]
    fn drain_removes_everything() {
        let mut pool = MeshPool::default();
        let a = pool.insert(gpu_mesh(1)).unwrap();
        pool.insert(gpu_mesh(2)).unwrap();
        assert_eq!(pool.drain().len(), 2);
        assert!(pool.get(a).is_none());
        assert_eq!(pool.len(), 0);
        let later = pool.insert(gpu_mesh(5)).unwrap();
        assert!(
            pool.get(a).is_none(),
            "an old handle never names a later mesh"
        );
        assert!(pool.get(later).is_some());
    }
}
