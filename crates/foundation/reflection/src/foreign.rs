//! Types defined outside Rust: C#, C++ and script components, described at run time
//! (ADR-0021 §3 and §7).

use crate::description::TypeKey;
use crate::id::StableTypeId;

/// A type defined outside Rust, described at run time: plain data with a C layout (ADR-0021 §3).
///
/// Foreign types are restricted to ABI-safe plain data until engine-owned string and list types
/// exist for them: every field's type must be ABI-safe (integers, floats, handles, fixed arrays
/// and other ABI-safe types; never `bool` or Rust enums). Registration validates the whole
/// description, so no foreign description can make safe code create an invalid Rust value
/// (ADR-0021 §7):
///
/// - the alignment is a power of two and the size a multiple of it;
/// - every field type is registered and ABI-safe;
/// - every field lies inside the type, aligned, without overlapping another, with a unique name.
///
/// Foreign values have no `Drop`, are cloned by copying their bytes, and default to zero bytes,
/// which is a valid value of every ABI-safe type.
///
/// ```
/// use oxreal_reflection::{ForeignTypeDefinition, StableTypeId, TypeRegistry};
///
/// let registry = TypeRegistry::new();
/// let f32_type = registry.key_of::<f32>().expect("built in");
/// let u32_type = registry.key_of::<u32>().expect("built in");
///
/// // struct Health { float current; float max; uint32_t flags; } from C#.
/// let health = ForeignTypeDefinition::new(
///     StableTypeId::from_uuid("6e4a0c3b-8d5f-4b9a-9c7e-3f5a7b9c1d4e"),
///     "Game.Health",
///     12,
///     4,
/// )
/// .field("current", f32_type, 0)
/// .field("max", f32_type, 4)
/// .field("flags", u32_type, 8);
/// let key = registry.register_foreign(health)?;
/// assert_eq!(registry.description(key).expect("registered").name(), "Game.Health");
/// # Ok::<(), oxreal_reflection::RegistrationError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignTypeDefinition {
    pub(crate) id: StableTypeId,
    pub(crate) name: String,
    pub(crate) size: usize,
    pub(crate) align: usize,
    pub(crate) fields: Vec<ForeignField>,
}

/// One field of a [`ForeignTypeDefinition`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeignField {
    pub(crate) name: String,
    pub(crate) ty: TypeKey,
    pub(crate) offset: usize,
}

impl ForeignTypeDefinition {
    /// A type with this stable ID, display name, size and alignment, and no fields yet.
    #[must_use]
    pub fn new(id: StableTypeId, name: impl Into<String>, size: usize, align: usize) -> Self {
        Self {
            id,
            name: name.into(),
            size,
            align,
            fields: Vec::new(),
        }
    }

    /// Adds a field of the registered type `ty` at byte `offset`.
    #[must_use]
    pub fn field(mut self, name: impl Into<String>, ty: TypeKey, offset: usize) -> Self {
        self.fields.push(ForeignField {
            name: name.into(),
            ty,
            offset,
        });
        self
    }
}
