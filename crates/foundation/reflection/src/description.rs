//! Type descriptions: the registered, plain-data form of a type's reflection data (ADR-0021 §3).
//!
//! A [`TypeDescription`] refers to other types only by [`TypeKey`], so it holds no Rust
//! generics or trait objects and can be exported unchanged (to a file, a header generator or a
//! C-ABI query). Byte offsets are build-local: they're valid for the running build and never
//! saved.

use std::alloc::Layout;
use std::borrow::Cow;
use std::fmt;

use crate::attributes::Attributes;
use crate::id::{ConstArg, StableTypeId};
use crate::kind::{IntRepr, Primitive};
use crate::ops::{OperationError, Operations};
use crate::registry::GroupId;

/// A type's in-process identity: its index in one [`TypeRegistry`](crate::TypeRegistry)
/// (ADR-0021 §2).
///
/// Keys are meaningful only with the registry that issued them, and only while the process
/// runs. Nothing persists a `TypeKey`: files use [`StableTypeId`]. A removed type's key is never
/// reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeKey(pub(crate) u32);

impl TypeKey {
    /// The key's index in its registry.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

impl fmt::Display for TypeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// How a registered type is identified and saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Identity {
    /// Saved, and identified by this stable ID. For a generic instantiation it's the derived ID.
    Persisted(StableTypeId),
    /// Never saved.
    Transient,
    /// Rebuilt by its owner, so copies and saves skip it. Implies transient.
    Rebuilt,
}

impl Identity {
    /// The stable ID, if the type is saved.
    #[must_use]
    pub const fn stable_id(&self) -> Option<StableTypeId> {
        match self {
            Self::Persisted(id) => Some(*id),
            Self::Transient | Self::Rebuilt => None,
        }
    }

    /// Whether values are never saved.
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        self.stable_id().is_none()
    }
}

/// A generic argument of a registered instantiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericArgument {
    /// A type argument.
    Type(TypeKey),
    /// A const argument.
    Const(ConstArg),
}

/// One registered type's reflection data (ADR-0021 §3).
#[derive(Debug)]
pub struct TypeDescription {
    pub(crate) key: TypeKey,
    pub(crate) identity: Identity,
    pub(crate) name: Cow<'static, str>,
    pub(crate) generic_args: Vec<GenericArgument>,
    pub(crate) layout: Layout,
    pub(crate) kind: TypeKind,
    pub(crate) attributes: Attributes,
    pub(crate) operations: Operations,
    pub(crate) abi: bool,
    pub(crate) group: GroupId,
}

impl TypeDescription {
    /// The type's key in the registry that holds this description.
    #[must_use]
    pub fn key(&self) -> TypeKey {
        self.key
    }

    /// How the type is identified and saved.
    #[must_use]
    pub fn identity(&self) -> Identity {
        self.identity
    }

    /// The stable ID, if the type is saved.
    #[must_use]
    pub fn stable_id(&self) -> Option<StableTypeId> {
        self.identity.stable_id()
    }

    /// The display name, for the editor and diagnostics. Generic instantiations include their
    /// arguments, as in `Vec<f32>` or `[f32; 4]`.
    ///
    /// By default a derived type's name is its module path and identifier, so it changes when
    /// the type moves. Identify types by [`TypeDescription::stable_id`], never by name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The generic arguments, empty for a non-generic type.
    #[must_use]
    pub fn generic_args(&self) -> &[GenericArgument] {
        &self.generic_args
    }

    /// Size and alignment, for this build.
    #[must_use]
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// The kind and its data.
    #[must_use]
    pub fn kind(&self) -> &TypeKind {
        &self.kind
    }

    /// The struct data, if the type is a struct.
    #[must_use]
    pub fn as_struct(&self) -> Option<&StructInfo> {
        match &self.kind {
            TypeKind::Struct(info) => Some(info),
            _ => None,
        }
    }

    /// The type's editor attributes.
    #[must_use]
    pub fn attributes(&self) -> &Attributes {
        &self.attributes
    }

    /// Which of drop, clone and default the type supports. Run them with
    /// [`TypeDescription::drop_value`], [`TypeDescription::clone_value`] and
    /// [`TypeDescription::default_value`].
    #[must_use]
    pub fn operations(&self) -> &Operations {
        &self.operations
    }

    /// Drops the value at `value`, through the type's own `Drop`. Afterwards the memory is
    /// uninitialized.
    ///
    /// If the drop panics, the value still counts as dropped and must not be used or dropped
    /// again.
    ///
    /// # Safety
    ///
    /// - `value` is non-null, aligned for this type, valid for reads and writes of its size
    ///   (with provenance that allows writing: not derived from a shared reference), and points
    ///   to a valid value of **this** type that the caller owns.
    /// - Nothing else accesses the value during the call, and the caller doesn't use it again,
    ///   other than to write a new value into the memory.
    /// - The value may be dropped on this thread (every `Reflect` type is `Send`).
    pub unsafe fn drop_value(&self, value: *mut u8) {
        // SAFETY: the operations belong to this description's type, and the caller's contract
        // is `Operations::drop_in_place`'s.
        unsafe { self.operations.drop_in_place(value) }
    }

    /// Writes a clone of the value at `source` into `target`, through the type's own `Clone`
    /// (for containers, their elements').
    ///
    /// If the clone panics, `target` stays uninitialized and nothing created by the clone leaks.
    ///
    /// # Errors
    ///
    /// [`OperationError::NotCloneable`], naming the type, if the type can't be cloned. Nothing
    /// is written then.
    ///
    /// # Safety
    ///
    /// - `source` is non-null, aligned for this type, and points to a valid value of **this**
    ///   type whose own invariants hold. Nothing writes to it during the call (it's read
    ///   through a shared reference).
    /// - `target` is non-null, aligned for this type, valid for writes of its size, and doesn't
    ///   overlap `source`. It's overwritten without being dropped, so anything it held leaks
    ///   (which is safe, but usually a bug).
    pub unsafe fn clone_value(
        &self,
        source: *const u8,
        target: *mut u8,
    ) -> Result<(), OperationError> {
        // SAFETY: the operations belong to this description's type, and the caller's contract
        // is `Operations::clone_into`'s.
        if unsafe { self.operations.clone_into(source, target) } {
            Ok(())
        } else {
            Err(OperationError::NotCloneable {
                ty: self.name.clone().into_owned(),
            })
        }
    }

    /// Writes the type's default value into `target`, through its own `Default` (for
    /// containers: empty, or their elements' default).
    ///
    /// # Errors
    ///
    /// [`OperationError::NoDefault`], naming the type, if the type has no default. Nothing is
    /// written then.
    ///
    /// # Safety
    ///
    /// `target` is non-null, aligned for this type and valid for writes of its size. It's
    /// overwritten without being dropped, so anything it held leaks (which is safe, but usually
    /// a bug).
    pub unsafe fn default_value(&self, target: *mut u8) -> Result<(), OperationError> {
        // SAFETY: the operations belong to this description's type, and the caller's contract
        // is `Operations::default_into`'s.
        if unsafe { self.operations.default_into(target) } {
            Ok(())
        } else {
            Err(OperationError::NoDefault {
                ty: self.name.clone().into_owned(),
            })
        }
    }

    /// Whether the type is ABI-safe: plain data with a C layout (ADR-0021 §3).
    #[must_use]
    pub fn is_abi(&self) -> bool {
        self.abi
    }

    /// The registration group the type belongs to.
    #[must_use]
    pub fn group(&self) -> GroupId {
        self.group
    }

    /// Every type this description refers to: field, element, component and generic argument
    /// types.
    pub(crate) fn referenced_types(&self) -> impl Iterator<Item = TypeKey> + '_ {
        let generic = self.generic_args.iter().filter_map(|arg| match arg {
            GenericArgument::Type(key) => Some(*key),
            GenericArgument::Const(_) => None,
        });
        let kind: Vec<TypeKey> = match &self.kind {
            TypeKind::Struct(info) => info.fields.iter().filter_map(Field::ty).collect(),
            TypeKind::Array(info) => vec![info.element],
            TypeKind::List(info) => vec![info.element],
            TypeKind::Option(info) => vec![info.some],
            TypeKind::Math(info) => info.components.iter().map(|c| c.ty).collect(),
            TypeKind::Primitive(_)
            | TypeKind::String
            | TypeKind::UnitEnum(_)
            | TypeKind::Handle(_)
            | TypeKind::Opaque => Vec::new(),
        };
        generic.chain(kind)
    }
}

/// What kind of type a description describes, with the kind's data (ADR-0021 §3).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum TypeKind {
    /// `bool`, an integer or a float.
    Primitive(Primitive),
    /// `String`.
    String,
    /// A struct with named or positional fields.
    Struct(StructInfo),
    /// An enum whose variants carry no data.
    UnitEnum(UnitEnumInfo),
    /// `[T; N]`.
    Array(ArrayInfo),
    /// `Vec<T>`.
    List(ListInfo),
    /// `Option<T>`.
    Option(OptionInfo),
    /// A value that refers to something (an entity, an asset) and is remapped when copied or
    /// saved.
    Handle(HandleInfo),
    /// An `oxreal-math` type, reached through named components.
    Math(MathInfo),
    /// Identity, layout and operations only: not navigable or editable.
    Opaque,
}

/// The fields of a struct.
#[derive(Debug, Clone, PartialEq)]
pub struct StructInfo {
    pub(crate) fields: Vec<Field>,
    pub(crate) tuple: bool,
}

impl StructInfo {
    /// The fields, in declaration order.
    #[must_use]
    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    /// Whether it's a tuple struct, whose fields are named `0`, `1` and so on.
    #[must_use]
    pub fn is_tuple(&self) -> bool {
        self.tuple
    }

    /// The field with this name, or with this name among its aliases (a former name).
    #[must_use]
    pub fn field(&self, name: &str) -> Option<(usize, &Field)> {
        self.fields
            .iter()
            .enumerate()
            .find(|(_, field)| field.name == name)
            .or_else(|| {
                self.fields
                    .iter()
                    .enumerate()
                    .find(|(_, field)| field.aliases.iter().any(|alias| alias == name))
            })
    }
}

/// One field of a struct.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub(crate) name: Cow<'static, str>,
    pub(crate) ty: Option<TypeKey>,
    pub(crate) offset: usize,
    pub(crate) attributes: Attributes,
    pub(crate) aliases: Vec<Cow<'static, str>>,
    pub(crate) transient: bool,
}

impl Field {
    /// The field's name. Tuple-struct fields are named `0`, `1` and so on.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The field's type, or `None` for an opaque field (`#[reflect(opaque)]`), which can't be
    /// navigated or edited.
    #[must_use]
    pub fn ty(&self) -> Option<TypeKey> {
        self.ty
    }

    /// Whether the field is opaque.
    #[must_use]
    pub fn is_opaque(&self) -> bool {
        self.ty.is_none()
    }

    /// The byte offset within the struct, for this build only. Never save it.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The field's editor attributes.
    #[must_use]
    pub fn attributes(&self) -> &Attributes {
        &self.attributes
    }

    /// Former names, which field paths and files may still use.
    pub fn aliases(&self) -> impl ExactSizeIterator<Item = &str> {
        self.aliases.iter().map(|alias| &**alias)
    }

    /// Whether the field is never saved. It gets its type's default on load.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        self.transient
    }
}

/// The variants of a unit enum.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitEnumInfo {
    pub(crate) repr: Option<IntRepr>,
    pub(crate) variants: Vec<Variant>,
}

impl UnitEnumInfo {
    /// The explicit integer representation. With one, values are stored as that integer and
    /// every variant has a discriminant. Without one, values are matched by variant name.
    #[must_use]
    pub fn repr(&self) -> Option<IntRepr> {
        self.repr
    }

    /// The variants, in declaration order.
    #[must_use]
    pub fn variants(&self) -> &[Variant] {
        &self.variants
    }

    /// The variant with this name, or with this name among its aliases.
    #[must_use]
    pub fn variant(&self, name: &str) -> Option<(usize, &Variant)> {
        self.variants
            .iter()
            .enumerate()
            .find(|(_, variant)| variant.name == name)
            .or_else(|| {
                self.variants
                    .iter()
                    .enumerate()
                    .find(|(_, variant)| variant.aliases.iter().any(|alias| alias == name))
            })
    }
}

/// One variant of a unit enum.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub(crate) name: Cow<'static, str>,
    pub(crate) discriminant: Option<u32>,
    pub(crate) attributes: Attributes,
    pub(crate) aliases: Vec<Cow<'static, str>>,
}

impl Variant {
    /// The variant's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The discriminant, when the enum has an explicit representation.
    #[must_use]
    pub fn discriminant(&self) -> Option<u32> {
        self.discriminant
    }

    /// The variant's editor attributes.
    #[must_use]
    pub fn attributes(&self) -> &Attributes {
        &self.attributes
    }

    /// Former names, which files may still use.
    pub fn aliases(&self) -> impl ExactSizeIterator<Item = &str> {
        self.aliases.iter().map(|alias| &**alias)
    }
}

/// The element type and length of `[T; N]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrayInfo {
    pub(crate) element: TypeKey,
    pub(crate) len: usize,
}

impl ArrayInfo {
    /// The element type.
    #[must_use]
    pub fn element(&self) -> TypeKey {
        self.element
    }

    /// The number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the array has no elements.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// The element type of `Vec<T>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListInfo {
    pub(crate) element: TypeKey,
}

impl ListInfo {
    /// The element type.
    #[must_use]
    pub fn element(&self) -> TypeKey {
        self.element
    }
}

/// The payload type of `Option<T>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionInfo {
    pub(crate) some: TypeKey,
}

impl OptionInfo {
    /// The type inside `Some`.
    #[must_use]
    pub fn some(&self) -> TypeKey {
        self.some
    }
}

/// What a handle refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandleInfo {
    pub(crate) domain: StableTypeId,
}

impl HandleInfo {
    /// The stable ID naming what the handle refers to, such as entities or assets, so
    /// reflection needs no dependency on the crates that own them.
    #[must_use]
    pub fn domain(&self) -> StableTypeId {
        self.domain
    }
}

/// The named components of a math type.
#[derive(Debug, Clone, PartialEq)]
pub struct MathInfo {
    pub(crate) components: Vec<MathComponent>,
}

impl MathInfo {
    /// The components, such as `x`, `y` and `z` of `Vec3`.
    #[must_use]
    pub fn components(&self) -> &[MathComponent] {
        &self.components
    }
}

/// One named component of a math type.
#[derive(Debug, Clone, PartialEq)]
pub struct MathComponent {
    pub(crate) name: Cow<'static, str>,
    pub(crate) ty: TypeKey,
}

impl MathComponent {
    /// The component's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The component's type: a scalar or another math type.
    #[must_use]
    pub fn ty(&self) -> TypeKey {
        self.ty
    }
}
