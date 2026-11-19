//! Structural checks of SPIR-V before it reaches the driver.
//!
//! [`inspect`] checks what the pipeline needs to be well-defined, and what a driver would
//! otherwise trust blindly:
//! - The header is a SPIR-V 1.0–1.6 header.
//! - Every instruction fits in the module.
//! - Which entry points exist, and for which stage.
//! - How many push-constant bytes the shaders read.
//! - That every declared capability is one Oxreal's required features guarantee.
//! - That every descriptor resource is one of the bindless arrays (set 0, bindings 0 and 1).
//!
//! It isn't a validator. Full validity is the caller's promise when it creates
//! [`oxreal_rhi::ShaderCode`]. Numbers come from the SPIR-V specification (unified, 1.6).

use std::cell::RefCell;
use std::collections::HashMap;

use oxreal_rhi::{BINDLESS_SET, SAMPLERS_BINDING, TEXTURES_BINDING};

const MAGIC: u32 = 0x0723_0203;
/// SPIR-V 1.0 to 1.6, the versions Vulkan 1.3 accepts.
const MIN_VERSION: u32 = 0x0001_0000;
const MAX_VERSION: u32 = 0x0001_0600;
const HEADER_WORDS: usize = 5;
/// Type nesting deeper than this is rejected (also stops malicious recursion).
const MAX_TYPE_DEPTH: u32 = 16;

/// The SPIR-V capabilities Oxreal's required device features guarantee (ADR-0013, ADR-0016 §3).
/// Must match `oxreal-shader-compiler`'s `ALLOWED_CAPABILITIES` (a test checks it): the compiler
/// rejects other capabilities at build time, and this rejects them in code from anywhere else.
const ALLOWED_CAPABILITIES: [u32; 7] = [
    0,    // Matrix
    1,    // Shader
    5301, // ShaderNonUniform
    5302, // RuntimeDescriptorArray
    5307, // SampledImageArrayNonUniformIndexing
    5309, // StorageImageArrayNonUniformIndexing
    5347, // PhysicalStorageBufferAddresses
];

/// Names of capabilities that are easy to use by accident, for error messages.
const KNOWN_CAPABILITY_NAMES: [(u32, &str); 6] = [
    (9, "Float16"),
    (10, "Float64"),
    (11, "Int64"),
    (22, "Int16"),
    (39, "Int8"),
    (4427, "DrawParameters"),
];

mod op {
    pub(super) const CAPABILITY: u32 = 17;
    pub(super) const ENTRY_POINT: u32 = 15;
    pub(super) const TYPE_BOOL: u32 = 20;
    pub(super) const TYPE_INT: u32 = 21;
    pub(super) const TYPE_FLOAT: u32 = 22;
    pub(super) const TYPE_VECTOR: u32 = 23;
    pub(super) const TYPE_MATRIX: u32 = 24;
    pub(super) const TYPE_ARRAY: u32 = 28;
    pub(super) const TYPE_IMAGE: u32 = 25;
    pub(super) const TYPE_SAMPLER: u32 = 26;
    pub(super) const TYPE_RUNTIME_ARRAY: u32 = 29;
    pub(super) const TYPE_STRUCT: u32 = 30;
    pub(super) const TYPE_POINTER: u32 = 32;
    pub(super) const CONSTANT: u32 = 43;
    pub(super) const VARIABLE: u32 = 59;
    pub(super) const DECORATE: u32 = 71;
    pub(super) const MEMBER_DECORATE: u32 = 72;
}

mod decoration {
    pub(super) const ROW_MAJOR: u32 = 4;
    pub(super) const ARRAY_STRIDE: u32 = 6;
    pub(super) const MATRIX_STRIDE: u32 = 7;
    pub(super) const OFFSET: u32 = 35;
    pub(super) const BINDING: u32 = 33;
    pub(super) const DESCRIPTOR_SET: u32 = 34;
}

const STORAGE_PUSH_CONSTANT: u32 = 9;
const STORAGE_PHYSICAL_STORAGE_BUFFER: u32 = 5349;

/// The stage an entry point runs in (SPIR-V execution model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Vertex,
    Fragment,
    Compute,
    Other(u32),
}

impl Stage {
    fn from_model(model: u32) -> Self {
        match model {
            0 => Self::Vertex,
            4 => Self::Fragment,
            5 => Self::Compute,
            other => Self::Other(other),
        }
    }
}

/// What [`inspect`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleInfo {
    pub(crate) entry_points: Vec<(Stage, String)>,
    /// Bytes of the largest push-constant block, 0 without one.
    pub(crate) push_constant_bytes: u32,
}

impl ModuleInfo {
    /// Whether `name` is an entry point for `stage`.
    pub(crate) fn has_entry_point(&self, stage: Stage, name: &str) -> bool {
        self.entry_points
            .iter()
            .any(|(s, n)| *s == stage && n == name)
    }
}

#[derive(Debug, Clone)]
enum Type {
    Scalar(u32),
    Vector {
        component: u32,
        count: u32,
    },
    Matrix {
        column: u32,
        count: u32,
    },
    Array {
        element: u32,
        length_id: u32,
    },
    Struct(Vec<u32>),
    Pointer {
        storage: u32,
        pointee: u32,
    },
    /// `OpTypeImage`: dimensionality, whether arrayed or multisampled, and the "Sampled"
    /// operand (1 = used with a sampler).
    Image {
        dim: u32,
        arrayed: u32,
        multisampled: u32,
        sampled: u32,
    },
    Sampler,
    RuntimeArray(u32),
    Unsized,
}

/// A type, and the struct member (struct id, member index) it belongs to when that matters.
type SizeKey = (u32, Option<(u32, u32)>);

#[derive(Default)]
struct Module {
    types: HashMap<u32, Type>,
    /// Integer type id → width in bits, to tell which constants can be array lengths.
    integer_types: HashMap<u32, u32>,
    /// Integer constants that fit in 32 bits.
    constants: HashMap<u32, u32>,
    /// Computed sizes, keyed by type and (for matrices and arrays) the struct member they
    /// belong to. Keeps the walk linear even for adversarial type graphs.
    sizes: RefCell<HashMap<SizeKey, u32>>,
    array_strides: HashMap<u32, u32>,
    /// (struct id, member index) → offset, matrix stride, row-major.
    member_offsets: HashMap<(u32, u32), u32>,
    member_matrix_strides: HashMap<(u32, u32), u32>,
    member_row_major: HashMap<(u32, u32), bool>,
    push_constant_pointer_types: Vec<u32>,
    capabilities: Vec<u32>,
    /// Variable id → (pointer type, storage class).
    variables: HashMap<u32, (u32, u32)>,
    /// Ids decorated with DescriptorSet and Binding.
    descriptor_sets: HashMap<u32, u32>,
    bindings: HashMap<u32, u32>,
    entry_points: Vec<(Stage, String)>,
}

/// Checks `words` and reports its entry points and push-constant size.
pub(crate) fn inspect(words: &[u32]) -> Result<ModuleInfo, String> {
    if words.len() < HEADER_WORDS {
        return Err(format!(
            "{} words is too short for a SPIR-V header",
            words.len()
        ));
    }
    if words[0] != MAGIC {
        return Err(format!(
            "not SPIR-V: magic number {:#010x}, expected {MAGIC:#010x}",
            words[0]
        ));
    }
    let version = words[1];
    if version & 0xff00_00ff != 0 || !(MIN_VERSION..=MAX_VERSION).contains(&version) {
        return Err(format!(
            "unsupported SPIR-V version word {version:#010x} (1.0 to 1.6 allowed)"
        ));
    }
    if words[3] == 0 {
        return Err("SPIR-V id bound is 0".to_owned());
    }
    if words[4] != 0 {
        return Err(format!("SPIR-V schema word is {}, expected 0", words[4]));
    }

    let mut module = Module::default();
    let mut at = HEADER_WORDS;
    while at < words.len() {
        let count = (words[at] >> 16) as usize;
        let opcode = words[at] & 0xffff;
        if count == 0 || at + count > words.len() {
            return Err(format!(
                "instruction at word {at} (opcode {opcode}) claims {count} words; the module ends at {}",
                words.len()
            ));
        }
        module.read(opcode, &words[at + 1..at + count])?;
        at += count;
    }

    let disallowed: Vec<String> = module
        .capabilities
        .iter()
        .filter(|c| !ALLOWED_CAPABILITIES.contains(c))
        .map(
            |&c| match KNOWN_CAPABILITY_NAMES.iter().find(|(n, _)| *n == c) {
                Some((_, name)) => format!("{name} ({c})"),
                None => format!("capability {c}"),
            },
        )
        .collect();
    if !disallowed.is_empty() {
        return Err(format!(
            "uses SPIR-V capabilities Oxreal's required device features don't guarantee: {}",
            disallowed.join(", ")
        ));
    }
    module.check_descriptor_bindings()?;

    let mut push_constant_bytes = 0;
    for &pointer in &module.push_constant_pointer_types {
        let Some(Type::Pointer {
            storage: STORAGE_PUSH_CONSTANT,
            pointee,
        }) = module.types.get(&pointer)
        else {
            return Err(format!(
                "push-constant variable has type %{pointer}, which isn't a push-constant pointer"
            ));
        };
        let size = module.size_of(*pointee, None, 0)?;
        push_constant_bytes = push_constant_bytes.max(size);
    }
    Ok(ModuleInfo {
        entry_points: module.entry_points,
        push_constant_bytes,
    })
}

/// The literal string starting at `operands[0]`, and how many words it used.
fn literal_string(operands: &[u32]) -> Result<(String, usize), String> {
    let mut bytes = Vec::new();
    for (i, word) in operands.iter().enumerate() {
        for byte in word.to_le_bytes() {
            if byte == 0 {
                let text = String::from_utf8(bytes)
                    .map_err(|_| "an entry-point name is not UTF-8".to_owned())?;
                return Ok((text, i + 1));
            }
            bytes.push(byte);
        }
    }
    Err("an entry-point name is not NUL-terminated".to_owned())
}

fn operand(operands: &[u32], index: usize, what: &str) -> Result<u32, String> {
    operands
        .get(index)
        .copied()
        .ok_or_else(|| format!("{what} is missing operand {index}"))
}

impl Module {
    fn read(&mut self, opcode: u32, operands: &[u32]) -> Result<(), String> {
        let id = |i| operand(operands, i, "an instruction");
        match opcode {
            op::CAPABILITY => {
                self.capabilities.push(id(0)?);
            }
            op::ENTRY_POINT => {
                let model = id(0)?;
                let (name, _) = literal_string(operands.get(2..).unwrap_or_default())?;
                self.entry_points.push((Stage::from_model(model), name));
            }
            op::TYPE_BOOL => {
                self.types.insert(id(0)?, Type::Unsized);
            }
            op::TYPE_INT | op::TYPE_FLOAT => {
                let width = id(1)?;
                if width == 0 || !width.is_multiple_of(8) {
                    return Err(format!("type %{} has a width of {width} bits", id(0)?));
                }
                self.types.insert(id(0)?, Type::Scalar(width / 8));
                if opcode == op::TYPE_INT {
                    self.integer_types.insert(id(0)?, width);
                }
            }
            op::TYPE_VECTOR => {
                self.types.insert(
                    id(0)?,
                    Type::Vector {
                        component: id(1)?,
                        count: id(2)?,
                    },
                );
            }
            op::TYPE_MATRIX => {
                self.types.insert(
                    id(0)?,
                    Type::Matrix {
                        column: id(1)?,
                        count: id(2)?,
                    },
                );
            }
            op::TYPE_ARRAY => {
                self.types.insert(
                    id(0)?,
                    Type::Array {
                        element: id(1)?,
                        length_id: id(2)?,
                    },
                );
            }
            op::TYPE_RUNTIME_ARRAY => {
                self.types.insert(id(0)?, Type::RuntimeArray(id(1)?));
            }
            op::TYPE_IMAGE => {
                self.types.insert(
                    id(0)?,
                    Type::Image {
                        dim: id(2)?,
                        arrayed: id(4)?,
                        multisampled: id(5)?,
                        sampled: id(6)?,
                    },
                );
            }
            op::TYPE_SAMPLER => {
                self.types.insert(id(0)?, Type::Sampler);
            }
            op::TYPE_STRUCT => {
                self.types.insert(
                    id(0)?,
                    Type::Struct(operands.get(1..).unwrap_or_default().to_vec()),
                );
            }
            op::TYPE_POINTER => {
                self.types.insert(
                    id(0)?,
                    Type::Pointer {
                        storage: id(1)?,
                        pointee: id(2)?,
                    },
                );
            }
            op::CONSTANT => {
                // Only integer constants whose value fits in 32 bits are recorded. Anything else
                // used as an array length is rejected as non-constant, never misread.
                let value = match (self.integer_types.get(&id(0)?), operands.get(2..)) {
                    (Some(32), Some(&[low])) => Some(low),
                    (Some(64), Some(&[low, 0])) => Some(low),
                    _ => None,
                };
                if let Some(value) = value {
                    self.constants.insert(id(1)?, value);
                }
            }
            op::VARIABLE => {
                if id(2)? == STORAGE_PUSH_CONSTANT {
                    self.push_constant_pointer_types.push(id(0)?);
                }
                self.variables.insert(id(1)?, (id(0)?, id(2)?));
            }
            op::DECORATE => match id(1)? {
                decoration::ARRAY_STRIDE => {
                    self.array_strides.insert(id(0)?, id(2)?);
                }
                decoration::DESCRIPTOR_SET => {
                    self.descriptor_sets.insert(id(0)?, id(2)?);
                }
                decoration::BINDING => {
                    self.bindings.insert(id(0)?, id(2)?);
                }
                _ => {}
            },
            op::MEMBER_DECORATE => {
                let key = (id(0)?, id(1)?);
                match id(2)? {
                    decoration::OFFSET => {
                        self.member_offsets.insert(key, id(3)?);
                    }
                    decoration::MATRIX_STRIDE => {
                        self.member_matrix_strides.insert(key, id(3)?);
                    }
                    decoration::ROW_MAJOR => {
                        self.member_row_major.insert(key, true);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Checks that every descriptor resource is one of the bindless arrays (ADR-0016 §2). Anything
    /// else wouldn't match the pipeline layout every pipeline shares.
    fn check_descriptor_bindings(&self) -> Result<(), String> {
        let mut ids: Vec<u32> = self
            .descriptor_sets
            .keys()
            .chain(self.bindings.keys())
            .copied()
            .collect();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let (Some(&set), Some(&binding)) =
                (self.descriptor_sets.get(&id), self.bindings.get(&id))
            else {
                return Err(format!(
                    "resource %{id} has a DescriptorSet or Binding decoration without the other"
                ));
            };
            if set != BINDLESS_SET {
                return Err(format!(
                    "resource %{id} is in descriptor set {set}; Oxreal shaders use only the bindless set {BINDLESS_SET} (ADR-0016 §2, Bindless.slang)"
                ));
            }
            match (binding, self.resource_element(id)?) {
                (
                    TEXTURES_BINDING,
                    Type::Image {
                        dim: 1,
                        arrayed: 0,
                        multisampled: 0,
                        sampled: 1,
                    },
                )
                | (SAMPLERS_BINDING, Type::Sampler) => {}
                (TEXTURES_BINDING, _) => {
                    return Err(format!(
                        "resource %{id} at binding {TEXTURES_BINDING} must be an array of sampled 2D textures (Texture2D[])"
                    ));
                }
                (SAMPLERS_BINDING, _) => {
                    return Err(format!(
                        "resource %{id} at binding {SAMPLERS_BINDING} must be an array of samplers (SamplerState[])"
                    ));
                }
                (other, _) => {
                    return Err(format!(
                        "resource %{id} uses binding {other}; the bindless set has only bindings {TEXTURES_BINDING} (textures) and {SAMPLERS_BINDING} (samplers)"
                    ));
                }
            }
        }
        Ok(())
    }

    /// The element type of a descriptor resource: the variable's pointee, or its array's element.
    fn resource_element(&self, id: u32) -> Result<&Type, String> {
        let lookup = |id: u32| {
            self.types
                .get(&id)
                .ok_or_else(|| format!("resource type %{id} is undefined"))
        };
        let &(pointer, _) = self
            .variables
            .get(&id)
            .ok_or_else(|| format!("decorated resource %{id} is not a variable"))?;
        let Type::Pointer { pointee, .. } = lookup(pointer)? else {
            return Err(format!("resource %{id} does not have a pointer type"));
        };
        match lookup(*pointee)? {
            Type::RuntimeArray(element) | Type::Array { element, .. } => lookup(*element),
            other => Ok(other),
        }
    }

    /// Size in bytes of type `id` in an explicit layout, never smaller than the bytes it can
    /// touch. `member` is the struct member it belongs to, for matrix decorations.
    fn size_of(&self, id: u32, member: Option<(u32, u32)>, depth: u32) -> Result<u32, String> {
        if depth > MAX_TYPE_DEPTH {
            return Err(format!("types nest deeper than {MAX_TYPE_DEPTH} levels"));
        }
        let ty = self
            .types
            .get(&id)
            .ok_or_else(|| format!("push constants use undefined type %{id}"))?;
        // Only matrices (and arrays of them) depend on the member's decorations.
        let key = match ty {
            Type::Matrix { .. } | Type::Array { .. } => (id, member),
            _ => (id, None),
        };
        if let Some(&size) = self.sizes.borrow().get(&key) {
            return Ok(size);
        }
        let size = self.compute_size(id, ty, member, depth)?;
        self.sizes.borrow_mut().insert(key, size);
        Ok(size)
    }

    fn compute_size(
        &self,
        id: u32,
        ty: &Type,
        member: Option<(u32, u32)>,
        depth: u32,
    ) -> Result<u32, String> {
        let overflow = || format!("the size of type %{id} overflows");
        // `count` elements `stride` apart, each `element` bytes: the larger of the strided span
        // and where the last element ends, so a stride below the element size can't hide
        // bytes.
        let extent = |stride: u32, count: u32, element: u32| -> Result<u32, String> {
            if count == 0 {
                return Err(format!("type %{id} has zero elements"));
            }
            let span = stride.checked_mul(count).ok_or_else(overflow)?;
            let last_end = stride
                .checked_mul(count - 1)
                .and_then(|start| start.checked_add(element))
                .ok_or_else(overflow)?;
            Ok(span.max(last_end))
        };
        match ty {
            Type::Scalar(bytes) => Ok(*bytes),
            Type::Vector { component, count } => self
                .size_of(*component, None, depth + 1)?
                .checked_mul(*count)
                .ok_or_else(overflow),
            Type::Matrix { column, count } => {
                let stride = member
                    .and_then(|key| self.member_matrix_strides.get(&key))
                    .copied()
                    .ok_or_else(|| format!("matrix %{id} in push constants has no MatrixStride"))?;
                let row_major = member
                    .and_then(|key| self.member_row_major.get(&key))
                    .copied()
                    .unwrap_or(false);
                let Some(&Type::Vector {
                    component,
                    count: rows,
                }) = self.types.get(column)
                else {
                    return Err(format!("matrix %{id} has a non-vector column type"));
                };
                let component_size = self.size_of(component, None, depth + 1)?;
                if row_major {
                    // One stride per row; a row holds one component of every column.
                    let row_size = component_size.checked_mul(*count).ok_or_else(overflow)?;
                    extent(stride, rows, row_size)
                } else {
                    let column_size = component_size.checked_mul(rows).ok_or_else(overflow)?;
                    extent(stride, *count, column_size)
                }
            }
            Type::Array { element, length_id } => {
                let length = self
                    .constants
                    .get(length_id)
                    .copied()
                    .ok_or_else(|| format!("array %{id} has a non-constant length"))?;
                let Some(&stride) = self.array_strides.get(&id) else {
                    return Err(format!("array %{id} in push constants has no ArrayStride"));
                };
                let element_size = self.size_of(*element, member, depth + 1)?;
                extent(stride, length, element_size)
            }
            Type::Struct(members) => {
                let mut end = 0_u32;
                for (index, &member_type) in members.iter().enumerate() {
                    let index = u32::try_from(index).map_err(|_| overflow())?;
                    let offset = self
                        .member_offsets
                        .get(&(id, index))
                        .copied()
                        .ok_or_else(|| format!("struct %{id} member {index} has no Offset"))?;
                    let size = self.size_of(member_type, Some((id, index)), depth + 1)?;
                    end = end.max(offset.checked_add(size).ok_or_else(overflow)?);
                }
                Ok(end)
            }
            Type::Pointer {
                storage: STORAGE_PHYSICAL_STORAGE_BUFFER,
                ..
            } => Ok(8),
            Type::Pointer { .. }
            | Type::Unsized
            | Type::RuntimeArray(_)
            | Type::Image { .. }
            | Type::Sampler => Err(format!(
                "push constants contain type %{id}, which has no fixed size in an explicit layout"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny SPIR-V assembler for tests.
    struct Asm(Vec<u32>);

    impl Asm {
        fn new() -> Self {
            Self(vec![MAGIC, 0x0001_0600, 0, 100, 0])
        }
        fn op(mut self, opcode: u32, operands: &[u32]) -> Self {
            let count = u32::try_from(operands.len() + 1).unwrap();
            self.0.push((count << 16) | opcode);
            self.0.extend_from_slice(operands);
            self
        }
        fn entry(self, model: u32, function: u32, name: &str) -> Self {
            let mut operands = vec![model, function];
            let mut bytes = name.as_bytes().to_vec();
            bytes.push(0);
            while !bytes.len().is_multiple_of(4) {
                bytes.push(0);
            }
            let (words, _) = bytes.as_chunks::<4>();
            operands.extend(words.iter().map(|w| u32::from_le_bytes(*w)));
            self.op(op::ENTRY_POINT, &operands)
        }
    }

    /// `struct { float rotation; float aspect; }` as push constants, with a vertex and a pixel
    /// entry point. The same shape as Triangle.slang.
    fn triangle_like() -> Asm {
        Asm::new()
            .entry(0, 1, "triangleVertex")
            .entry(4, 2, "trianglePixel")
            .op(op::MEMBER_DECORATE, &[10, 0, decoration::OFFSET, 0])
            .op(op::MEMBER_DECORATE, &[10, 1, decoration::OFFSET, 4])
            .op(op::TYPE_FLOAT, &[5, 32])
            .op(op::TYPE_STRUCT, &[10, 5, 5])
            .op(op::TYPE_POINTER, &[11, STORAGE_PUSH_CONSTANT, 10])
            .op(op::VARIABLE, &[11, 12, STORAGE_PUSH_CONSTANT])
    }

    #[test]
    fn finds_entry_points_and_push_constant_size() {
        let info = inspect(&triangle_like().0).unwrap();
        assert!(info.has_entry_point(Stage::Vertex, "triangleVertex"));
        assert!(info.has_entry_point(Stage::Fragment, "trianglePixel"));
        assert!(!info.has_entry_point(Stage::Fragment, "triangleVertex"));
        assert_eq!(info.push_constant_bytes, 8);
    }

    #[test]
    fn no_push_constants_is_zero_bytes() {
        let info = inspect(&Asm::new().entry(5, 1, "main").0).unwrap();
        assert_eq!(info.push_constant_bytes, 0);
        assert!(info.has_entry_point(Stage::Compute, "main"));
    }

    #[test]
    fn rejects_bad_headers() {
        assert!(inspect(&[]).unwrap_err().contains("too short"));
        let mut words = Asm::new().0;
        words[0] = 0x0302_2307; // Byte-swapped magic.
        assert!(inspect(&words).unwrap_err().contains("not SPIR-V"));
        let mut words = Asm::new().0;
        words[1] = 0x0001_0700;
        assert!(inspect(&words).unwrap_err().contains("version"));
        let mut words = Asm::new().0;
        words[3] = 0;
        assert!(inspect(&words).unwrap_err().contains("bound"));
    }

    #[test]
    fn rejects_instructions_running_past_the_end() {
        let mut words = triangle_like().0;
        let last = words.len() - 4;
        words[last] = (50 << 16) | op::VARIABLE; // Claims 50 words.
        assert!(inspect(&words).unwrap_err().contains("claims 50 words"));
        let mut words = Asm::new().0;
        words.push(op::VARIABLE); // Word count 0.
        assert!(inspect(&words).unwrap_err().contains("claims 0 words"));
    }

    #[test]
    fn sizes_vectors_matrices_arrays_and_nested_structs() {
        // struct Inner { vec2 a; }                            size 8
        // struct Block { mat4 m @0 (stride 16); vec4 v @64; float arr[3] @80 (stride 4);
        //                Inner inner @96; uint64_t addr-like pointer @104 }  end = 112
        let words = Asm::new()
            .op(op::MEMBER_DECORATE, &[30, 0, decoration::OFFSET, 0])
            .op(op::MEMBER_DECORATE, &[30, 0, decoration::MATRIX_STRIDE, 16])
            .op(op::MEMBER_DECORATE, &[30, 1, decoration::OFFSET, 64])
            .op(op::MEMBER_DECORATE, &[30, 2, decoration::OFFSET, 80])
            .op(op::MEMBER_DECORATE, &[30, 3, decoration::OFFSET, 96])
            .op(op::MEMBER_DECORATE, &[30, 4, decoration::OFFSET, 104])
            .op(op::MEMBER_DECORATE, &[25, 0, decoration::OFFSET, 0])
            .op(op::DECORATE, &[24, decoration::ARRAY_STRIDE, 4])
            .op(op::TYPE_FLOAT, &[20, 32])
            .op(op::TYPE_INT, &[19, 32, 0])
            .op(op::TYPE_VECTOR, &[21, 20, 4])
            .op(op::TYPE_VECTOR, &[22, 20, 2])
            .op(op::TYPE_MATRIX, &[23, 21, 4])
            .op(op::CONSTANT, &[19, 18, 3])
            .op(op::TYPE_ARRAY, &[24, 20, 18])
            .op(op::TYPE_STRUCT, &[25, 22])
            .op(op::TYPE_POINTER, &[26, STORAGE_PHYSICAL_STORAGE_BUFFER, 20])
            .op(op::TYPE_STRUCT, &[30, 23, 21, 24, 25, 26])
            .op(op::TYPE_POINTER, &[31, STORAGE_PUSH_CONSTANT, 30])
            .op(op::VARIABLE, &[31, 32, STORAGE_PUSH_CONSTANT])
            .0;
        assert_eq!(inspect(&words).unwrap().push_constant_bytes, 112);
    }

    #[test]
    fn row_major_matrices_use_the_row_count() {
        // mat2x4 (2 columns of vec4) stored row-major with stride 8: 4 rows × 8 = 32 bytes.
        let words = Asm::new()
            .op(op::MEMBER_DECORATE, &[30, 0, decoration::OFFSET, 0])
            .op(op::MEMBER_DECORATE, &[30, 0, decoration::MATRIX_STRIDE, 8])
            .op(op::MEMBER_DECORATE, &[30, 0, decoration::ROW_MAJOR])
            .op(op::TYPE_FLOAT, &[20, 32])
            .op(op::TYPE_VECTOR, &[21, 20, 4])
            .op(op::TYPE_MATRIX, &[23, 21, 2])
            .op(op::TYPE_STRUCT, &[30, 23])
            .op(op::TYPE_POINTER, &[31, STORAGE_PUSH_CONSTANT, 30])
            .op(op::VARIABLE, &[31, 32, STORAGE_PUSH_CONSTANT])
            .0;
        assert_eq!(inspect(&words).unwrap().push_constant_bytes, 32);
    }

    #[test]
    fn rejects_unsized_and_undecorated_push_constants() {
        let missing_offset = Asm::new()
            .op(op::TYPE_FLOAT, &[20, 32])
            .op(op::TYPE_STRUCT, &[30, 20])
            .op(op::TYPE_POINTER, &[31, STORAGE_PUSH_CONSTANT, 30])
            .op(op::VARIABLE, &[31, 32, STORAGE_PUSH_CONSTANT])
            .0;
        assert!(inspect(&missing_offset).unwrap_err().contains("no Offset"));

        let runtime_array = Asm::new()
            .op(op::MEMBER_DECORATE, &[30, 0, decoration::OFFSET, 0])
            .op(op::TYPE_FLOAT, &[20, 32])
            .op(op::TYPE_RUNTIME_ARRAY, &[21, 20])
            .op(op::TYPE_STRUCT, &[30, 21])
            .op(op::TYPE_POINTER, &[31, STORAGE_PUSH_CONSTANT, 30])
            .op(op::VARIABLE, &[31, 32, STORAGE_PUSH_CONSTANT])
            .0;
        assert!(
            inspect(&runtime_array)
                .unwrap_err()
                .contains("no fixed size")
        );
    }

    #[test]
    fn rejects_self_referencing_types() {
        let words = Asm::new()
            .op(op::MEMBER_DECORATE, &[30, 0, decoration::OFFSET, 0])
            .op(op::TYPE_STRUCT, &[30, 30])
            .op(op::TYPE_POINTER, &[31, STORAGE_PUSH_CONSTANT, 30])
            .op(op::VARIABLE, &[31, 32, STORAGE_PUSH_CONSTANT])
            .0;
        assert!(inspect(&words).unwrap_err().contains("deeper than"));
    }

    #[test]
    fn array_lengths_must_be_32_bit_integer_constants() {
        // A 64-bit length of 0x1_0000_0001 must not be read as 1.
        let block = |length_words: &[u32], int_width: u32| {
            let mut constant = vec![19, 18];
            constant.extend_from_slice(length_words);
            Asm::new()
                .op(op::MEMBER_DECORATE, &[30, 0, decoration::OFFSET, 0])
                .op(op::DECORATE, &[24, decoration::ARRAY_STRIDE, 4])
                .op(op::TYPE_FLOAT, &[20, 32])
                .op(op::TYPE_INT, &[19, int_width, 0])
                .op(op::CONSTANT, &constant)
                .op(op::TYPE_ARRAY, &[24, 20, 18])
                .op(op::TYPE_STRUCT, &[30, 24])
                .op(op::TYPE_POINTER, &[31, STORAGE_PUSH_CONSTANT, 30])
                .op(op::VARIABLE, &[31, 32, STORAGE_PUSH_CONSTANT])
                .0
        };
        assert_eq!(inspect(&block(&[3], 32)).unwrap().push_constant_bytes, 12);
        assert_eq!(
            inspect(&block(&[3, 0], 64)).unwrap().push_constant_bytes,
            12
        );
        let huge = inspect(&block(&[1, 1], 64)).unwrap_err();
        assert!(huge.contains("non-constant length"), "{huge}");
        let zero = inspect(&block(&[0], 32)).unwrap_err();
        assert!(zero.contains("zero elements"), "{zero}");
    }

    #[test]
    fn a_stride_below_the_element_size_cannot_hide_bytes() {
        // vec4[2] with ArrayStride 4: the last element ends at 4 + 16 = 20, not 2 × 4 = 8.
        let words = Asm::new()
            .op(op::MEMBER_DECORATE, &[30, 0, decoration::OFFSET, 0])
            .op(op::DECORATE, &[24, decoration::ARRAY_STRIDE, 4])
            .op(op::TYPE_FLOAT, &[20, 32])
            .op(op::TYPE_VECTOR, &[21, 20, 4])
            .op(op::TYPE_INT, &[19, 32, 0])
            .op(op::CONSTANT, &[19, 18, 2])
            .op(op::TYPE_ARRAY, &[24, 21, 18])
            .op(op::TYPE_STRUCT, &[30, 24])
            .op(op::TYPE_POINTER, &[31, STORAGE_PUSH_CONSTANT, 30])
            .op(op::VARIABLE, &[31, 32, STORAGE_PUSH_CONSTANT])
            .0;
        assert_eq!(inspect(&words).unwrap().push_constant_bytes, 20);
    }

    #[test]
    fn wide_and_deep_type_graphs_stay_fast() {
        // 16 levels, each a struct of 64 members of the level below: 64^16 paths without
        // memoization. Each level is 0 bytes wide, so no size-based early exit helps either.
        let mut asm = Asm::new().op(op::TYPE_STRUCT, &[100]);
        for level in 1..16_u32 {
            let id = 100 + level;
            let mut operands = vec![id];
            operands.extend(std::iter::repeat_n(id - 1, 64));
            for member in 0..64 {
                asm = asm.op(op::MEMBER_DECORATE, &[id, member, decoration::OFFSET, 0]);
            }
            asm = asm.op(op::TYPE_STRUCT, &operands);
        }
        let words = asm
            .op(op::TYPE_POINTER, &[200, STORAGE_PUSH_CONSTANT, 115])
            .op(op::VARIABLE, &[200, 201, STORAGE_PUSH_CONSTANT])
            .0;
        let started = std::time::Instant::now();
        assert_eq!(inspect(&words).unwrap().push_constant_bytes, 0);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn capabilities_outside_the_allowlist_are_rejected() {
        let allowed = Asm::new()
            .op(op::CAPABILITY, &[1])
            .op(op::CAPABILITY, &[5347])
            .entry(0, 1, "main")
            .0;
        assert!(inspect(&allowed).is_ok());
        let int64 = Asm::new()
            .op(op::CAPABILITY, &[1])
            .op(op::CAPABILITY, &[11])
            .op(op::CAPABILITY, &[6000])
            .0;
        let err = inspect(&int64).unwrap_err();
        assert!(err.contains("Int64 (11)"), "{err}");
        assert!(err.contains("capability 6000"), "{err}");
    }

    #[test]
    fn the_allowlist_matches_the_shader_compiler() {
        let mut compiler: Vec<u32> = oxreal_shader_compiler::ALLOWED_CAPABILITIES
            .iter()
            .map(|c| c.0)
            .collect();
        compiler.sort_unstable();
        let mut ours = ALLOWED_CAPABILITIES.to_vec();
        ours.sort_unstable();
        assert_eq!(ours, compiler, "keep both allowlists in sync (ADR-0013)");
    }

    /// A module declaring one descriptor resource: `element` (operands of its type instruction)
    /// in a runtime array, at `set` and `binding`.
    fn with_resource(element_op: u32, element: &[u32], set: u32, binding: u32) -> Vec<u32> {
        Asm::new()
            .op(op::DECORATE, &[40, decoration::DESCRIPTOR_SET, set])
            .op(op::DECORATE, &[40, decoration::BINDING, binding])
            .op(op::TYPE_FLOAT, &[10, 32])
            .op(element_op, element)
            .op(op::TYPE_RUNTIME_ARRAY, &[30, 20])
            .op(op::TYPE_POINTER, &[31, 0, 30])
            .op(op::VARIABLE, &[31, 40, 0])
            .0
    }

    /// `OpTypeImage %20 %float 2D depth=0 arrayed=0 ms=0 sampled=1 Unknown`.
    const TEXTURE_2D: [u32; 8] = [20, 10, 1, 0, 0, 0, 1, 0];

    #[test]
    fn the_bindless_arrays_are_accepted() {
        assert!(inspect(&with_resource(op::TYPE_IMAGE, &TEXTURE_2D, 0, 0)).is_ok());
        assert!(inspect(&with_resource(op::TYPE_SAMPLER, &[20], 0, 1)).is_ok());
    }

    #[test]
    fn other_descriptor_resources_are_rejected() {
        let other_set = inspect(&with_resource(op::TYPE_IMAGE, &TEXTURE_2D, 1, 0)).unwrap_err();
        assert!(other_set.contains("descriptor set 1"), "{other_set}");
        let other_binding = inspect(&with_resource(op::TYPE_SAMPLER, &[20], 0, 5)).unwrap_err();
        assert!(other_binding.contains("binding 5"), "{other_binding}");
        let swapped = inspect(&with_resource(op::TYPE_SAMPLER, &[20], 0, 0)).unwrap_err();
        assert!(swapped.contains("Texture2D[]"), "{swapped}");
        // A storage image (sampled = 2) is not a sampled texture.
        let storage = [20, 10, 1, 0, 0, 0, 2, 0];
        let storage = inspect(&with_resource(op::TYPE_IMAGE, &storage, 0, 0)).unwrap_err();
        assert!(storage.contains("sampled 2D textures"), "{storage}");
        let cube = [20, 10, 3, 0, 0, 0, 1, 0];
        assert!(
            inspect(&with_resource(op::TYPE_IMAGE, &cube, 0, 0)).is_err(),
            "cube maps aren't 2D"
        );
    }

    #[test]
    fn entry_point_names_must_be_terminated() {
        let words = Asm::new()
            .op(op::ENTRY_POINT, &[0, 1, u32::from_le_bytes(*b"main")])
            .0;
        assert!(inspect(&words).unwrap_err().contains("NUL-terminated"));
    }
}
