//! Type definitions: what a Rust type declares about itself at compile time (ADR-0021 §3).
//!
//! A [`TypeDefinition`] is the `const` that [`Reflect`] provides, usually emitted by
//! `#[derive(Reflect)]`. It is plain `'static` data built with `const fn` constructors, and it
//! names other types through [`TypeRef`]s. Registering the type in a
//! [`TypeRegistry`](crate::TypeRegistry) resolves those references to [`TypeKey`]s and turns
//! the definition into a [`TypeDescription`](crate::TypeDescription), the form tools read.
//!
//! The definition types are `Copy` and have no destructors, so they can be built and inspected
//! in constants. The derive's compile-time checks read the field types' [`Reflect::TRANSIENT`]
//! and [`Reflect::ABI`] rather than their definitions, because evaluating definitions eagerly
//! would make recursive types a const-evaluation cycle.
//!
//! [`TypeKey`]: crate::TypeKey

use std::alloc::Layout;
use std::any::TypeId;
use std::fmt;
use std::marker::PhantomData;

use crate::Reflect;
use crate::access::{ComponentGet, ComponentSet, EnumAccess, ListAccess, OptionAccess, UnitEnum};
use crate::attributes::AttributesDefinition;
use crate::id::{ConstArg, StableTypeId};
use crate::kind::{IntRepr, Primitive};
use crate::ops::Operations;

/// A reference to a Rust type that implements [`Reflect`], resolved when it's registered.
#[derive(Clone, Copy)]
pub struct TypeRef {
    type_id: TypeId,
    transient: bool,
    abi: bool,
    definition: fn() -> TypeDefinition,
}

impl TypeRef {
    /// The reference to `T`.
    #[must_use]
    pub const fn of<T: Reflect>() -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            transient: T::TRANSIENT,
            abi: T::ABI,
            definition: definition_of::<T>,
        }
    }

    /// The referenced type's `TypeId`.
    #[must_use]
    pub(crate) const fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// Whether the referenced type is never saved ([`Reflect::TRANSIENT`]).
    #[must_use]
    pub(crate) const fn is_transient(&self) -> bool {
        self.transient
    }

    /// Whether the referenced type is ABI-safe ([`Reflect::ABI`]).
    #[must_use]
    pub(crate) const fn is_abi(&self) -> bool {
        self.abi
    }

    /// The referenced type's definition.
    #[must_use]
    pub(crate) fn definition(&self) -> TypeDefinition {
        (self.definition)()
    }
}

fn definition_of<T: Reflect>() -> TypeDefinition {
    T::DEFINITION
}

impl fmt::Debug for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("TypeRef")
            .field(&self.definition().name)
            .finish()
    }
}

/// How a type is identified and persisted (ADR-0021 §2 and §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persistence {
    /// Saved, and identified by this stable ID. For a generic type it's the generic
    /// definition's ID, and each instantiation gets an ID derived from it and its arguments.
    Persisted(StableTypeId),
    /// Never saved, so it needs no stable ID. Copies within the process (Play-in-Editor,
    /// templates) still clone it when it's cloneable.
    Transient,
    /// Rebuilt by its owner (for example `Children` from `ChildOf` links), so copies and saves
    /// skip it. Implies transient.
    Rebuilt,
}

impl Persistence {
    /// Whether values are never saved: [`Persistence::Transient`] or [`Persistence::Rebuilt`].
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        !matches!(self, Self::Persisted(_))
    }
}

/// A generic argument of a generic type's instantiation.
#[derive(Debug, Clone, Copy)]
pub enum GenericArgDefinition {
    /// A type argument.
    Type(TypeRef),
    /// A const argument.
    Const(ConstArg),
}

/// What a Rust type declares about itself: the `const` of [`Reflect`] (ADR-0021 §3).
///
/// Built with [`TypeDefinition::builder`], which takes the type as a parameter so the layout,
/// drop and the optional clone and default operations always belong to that type.
#[derive(Clone, Copy)]
pub struct TypeDefinition {
    type_id: TypeId,
    name: &'static str,
    persistence: Persistence,
    generic_args: &'static [GenericArgDefinition],
    layout: Layout,
    kind: KindDefinition,
    attributes: AttributesDefinition,
    operations: Operations,
    abi: bool,
}

impl TypeDefinition {
    /// Starts the definition of `T`, with its layout and drop operation.
    ///
    /// - `name` is the type's display name for the editor and diagnostics. The derive uses the
    ///   module path and identifier, or `#[reflect(name = "...")]`. For a generic type it's the
    ///   name without arguments: registration appends them.
    /// - `persistence` says whether the type is saved, and its stable ID if so.
    pub const fn builder<T: 'static>(
        name: &'static str,
        persistence: Persistence,
        kind: KindDefinition,
    ) -> DefinitionBuilder<T> {
        DefinitionBuilder {
            definition: Self {
                type_id: TypeId::of::<T>(),
                name,
                persistence,
                generic_args: &[],
                layout: Layout::new::<T>(),
                kind,
                attributes: AttributesDefinition::new(),
                operations: Operations::of::<T>(),
                abi: false,
            },
            ty: PhantomData,
        }
    }

    /// The defined type's `TypeId`.
    #[must_use]
    pub(crate) const fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// The display name, without generic arguments.
    #[must_use]
    pub(crate) const fn name(&self) -> &'static str {
        self.name
    }

    /// Whether and how the type is saved. For a generic type this is the generic definition's
    /// persistence: an instantiation with a transient type argument is transient too.
    #[must_use]
    pub(crate) const fn persistence(&self) -> Persistence {
        self.persistence
    }

    /// The generic arguments, empty for a non-generic type.
    #[must_use]
    pub(crate) const fn generic_args(&self) -> &'static [GenericArgDefinition] {
        self.generic_args
    }

    /// Size and alignment.
    #[must_use]
    pub(crate) const fn layout(&self) -> Layout {
        self.layout
    }

    /// The kind and its data.
    #[must_use]
    pub(crate) const fn kind(&self) -> &KindDefinition {
        &self.kind
    }

    /// The editor attributes.
    #[must_use]
    pub(crate) const fn attributes(&self) -> &AttributesDefinition {
        &self.attributes
    }

    /// Drop, clone and default.
    #[must_use]
    pub(crate) const fn operations(&self) -> Operations {
        self.operations
    }

    /// Whether the type is ABI-safe: plain data with a C layout that other languages may read
    /// directly (ADR-0021 §3, the C-ABI contract).
    #[must_use]
    pub(crate) const fn is_abi(&self) -> bool {
        self.abi
    }
}

impl fmt::Debug for TypeDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypeDefinition")
            .field("name", &self.name)
            .field("persistence", &self.persistence)
            .field("layout", &self.layout)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// Builds the [`TypeDefinition`] of `T`. Returned by [`TypeDefinition::builder`].
#[must_use = "a builder does nothing until `build` is called"]
pub struct DefinitionBuilder<T> {
    definition: TypeDefinition,
    ty: PhantomData<fn() -> T>,
}

impl<T> fmt::Debug for DefinitionBuilder<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DefinitionBuilder")
            .field(&self.definition)
            .finish()
    }
}

impl<T: 'static> DefinitionBuilder<T> {
    /// Declares clone through `T`'s [`Clone`] impl (`#[reflect(clone)]`).
    pub const fn cloneable(mut self) -> Self
    where
        T: Clone,
    {
        self.definition.operations = self.definition.operations.with_clone::<T>();
        self
    }

    /// Declares default construction through `T`'s [`Default`] impl (`#[reflect(default)]`).
    pub const fn defaultable(mut self) -> Self
    where
        T: Default,
    {
        self.definition.operations = self.definition.operations.with_default::<T>();
        self
    }

    /// The generic arguments of this instantiation, in declaration order.
    pub const fn generic_args(mut self, args: &'static [GenericArgDefinition]) -> Self {
        self.definition.generic_args = args;
        self
    }

    /// The editor attributes of the type.
    pub const fn attributes(mut self, attributes: AttributesDefinition) -> Self {
        self.definition.attributes = attributes;
        self
    }

    /// Marks the type ABI-safe (`#[reflect(abi)]`). Registration checks that its fields are
    /// ABI-safe and that it isn't generic. Only a `#[repr(C)]` or `#[repr(transparent)]` type may
    /// be marked: generated C headers declare its layout from the field definitions, which
    /// registration can't compare with the Rust layout. The derive checks the `repr`; a
    /// hand-written impl takes it on as part of [`Reflect`]'s safety contract.
    pub const fn abi(mut self) -> Self {
        self.definition.abi = true;
        self
    }

    /// Fails with `message` unless `condition` holds. In a constant, that fails the build.
    ///
    /// The derive checks what only the field types know this way (their [`Reflect::TRANSIENT`]
    /// and [`Reflect::ABI`]), such as a saved type having a field of a transient type. It's an expression, not a statement, so
    /// the definition stays a constant expression whose `&[...]` lists are `'static`.
    ///
    /// # Panics
    ///
    /// If `condition` is false.
    pub const fn ensure(self, condition: bool, message: &'static str) -> Self {
        if !condition {
            panic!("{}", message);
        }
        self
    }

    /// Replaces the operations. For the built-in containers, whose clone and default depend on
    /// their element type.
    pub(crate) const fn operations(mut self, operations: Operations) -> Self {
        self.definition.operations = operations;
        self
    }

    /// The finished definition.
    #[must_use]
    pub const fn build(self) -> TypeDefinition {
        self.definition
    }
}

/// What kind of type a definition describes, with the kind's data (ADR-0021 §3).
#[derive(Clone, Copy)]
pub struct KindDefinition(pub(crate) KindData);

#[derive(Clone, Copy)]
pub(crate) enum KindData {
    Primitive(Primitive),
    String,
    Struct {
        fields: &'static [FieldDefinition],
        tuple: bool,
    },
    UnitEnum {
        repr: Option<IntRepr>,
        variants: &'static [VariantDefinition],
        access: EnumAccess,
    },
    Array {
        element: TypeRef,
        len: usize,
    },
    List {
        element: TypeRef,
        access: ListAccess,
    },
    Option {
        some: TypeRef,
        access: OptionAccess,
    },
    Handle {
        domain: StableTypeId,
    },
    Math {
        components: &'static [MathComponentDefinition],
    },
    Opaque,
}

impl KindDefinition {
    /// A primitive: `bool`, an integer or a float.
    #[must_use]
    pub const fn primitive(primitive: Primitive) -> Self {
        Self(KindData::Primitive(primitive))
    }

    /// `String`.
    pub(crate) const fn string() -> Self {
        Self(KindData::String)
    }

    /// A struct with named fields.
    #[must_use]
    pub const fn named_struct(fields: &'static [FieldDefinition]) -> Self {
        Self(KindData::Struct {
            fields,
            tuple: false,
        })
    }

    /// A tuple struct, whose fields are named `0`, `1` and so on.
    #[must_use]
    pub const fn tuple_struct(fields: &'static [FieldDefinition]) -> Self {
        Self(KindData::Struct {
            fields,
            tuple: true,
        })
    }

    /// An enum whose variants carry no data.
    ///
    /// With an explicit `#[repr(u8/u16/u32)]` (`repr`), every variant carries its discriminant
    /// and values are stored as that integer. Without one, the layout is Rust's, and values are
    /// read, written and saved by variant name. Either way, reflection reads and writes them
    /// through `T`'s [`UnitEnum`] impl, so `variants` must be in declaration order.
    #[must_use]
    pub const fn unit_enum<T: UnitEnum>(
        repr: Option<IntRepr>,
        variants: &'static [VariantDefinition],
    ) -> Self {
        Self(KindData::UnitEnum {
            repr,
            variants,
            access: EnumAccess::of::<T>(),
        })
    }

    /// `[T; N]`.
    pub(crate) const fn array<T: Reflect, const N: usize>() -> Self {
        Self(KindData::Array {
            element: TypeRef::of::<T>(),
            len: N,
        })
    }

    /// `Vec<T>`.
    pub(crate) const fn list<T: Reflect>() -> Self {
        Self(KindData::List {
            element: TypeRef::of::<T>(),
            access: ListAccess::of::<T>(),
        })
    }

    /// `Option<T>`.
    pub(crate) const fn option<T: Reflect>() -> Self {
        Self(KindData::Option {
            some: TypeRef::of::<T>(),
            access: OptionAccess::of::<T>(),
        })
    }

    /// A handle: a plain value that refers to something, such as an entity or an asset, and is
    /// remapped when copied or saved. `domain` identifies what it refers to.
    #[must_use]
    pub const fn handle(domain: StableTypeId) -> Self {
        Self(KindData::Handle { domain })
    }

    /// An `oxreal-math` type, reached through named components rather than field offsets,
    /// because glam's SIMD types hide their fields.
    pub(crate) const fn math(components: &'static [MathComponentDefinition]) -> Self {
        Self(KindData::Math { components })
    }

    /// Identity, layout and operations only (`#[reflect(opaque)]`): not navigable or editable,
    /// and saved only through a codec of the serialization layer (D-07).
    #[must_use]
    pub const fn opaque() -> Self {
        Self(KindData::Opaque)
    }
}

impl fmt::Debug for KindDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.0 {
            KindData::Primitive(p) => return write!(f, "Primitive({p:?})"),
            KindData::String => "String",
            KindData::Struct { .. } => "Struct",
            KindData::UnitEnum { .. } => "UnitEnum",
            KindData::Array { .. } => "Array",
            KindData::List { .. } => "List",
            KindData::Option { .. } => "Option",
            KindData::Handle { .. } => "Handle",
            KindData::Math { .. } => "Math",
            KindData::Opaque => "Opaque",
        };
        f.write_str(name)
    }
}

/// One field of a struct definition.
#[derive(Debug, Clone, Copy)]
pub struct FieldDefinition {
    pub(crate) name: &'static str,
    pub(crate) ty: Option<TypeRef>,
    pub(crate) offset: usize,
    pub(crate) attributes: AttributesDefinition,
    pub(crate) aliases: &'static [&'static str],
    pub(crate) transient: bool,
}

impl FieldDefinition {
    /// A field of type `T` at byte `offset` (from `offset_of!`).
    #[must_use]
    pub const fn new<T: Reflect>(name: &'static str, offset: usize) -> Self {
        Self {
            name,
            ty: Some(TypeRef::of::<T>()),
            offset,
            attributes: AttributesDefinition::new(),
            aliases: &[],
            transient: false,
        }
    }

    /// An opaque field (`#[reflect(opaque)]`): its type needs no [`Reflect`], and it can't be
    /// navigated or edited. Saving the containing type is an error unless the serialization
    /// layer has a codec for it (ADR-0021 §3), or the field is also transient.
    #[must_use]
    pub const fn opaque(name: &'static str, offset: usize) -> Self {
        Self {
            name,
            ty: None,
            offset,
            attributes: AttributesDefinition::new(),
            aliases: &[],
            transient: false,
        }
    }

    /// The field's editor attributes.
    #[must_use]
    pub const fn attributes(mut self, attributes: AttributesDefinition) -> Self {
        self.attributes = attributes;
        self
    }

    /// Former names of the field, so field paths and files that use them still resolve.
    #[must_use]
    pub const fn aliases(mut self, aliases: &'static [&'static str]) -> Self {
        self.aliases = aliases;
        self
    }

    /// Never saved (`#[reflect(transient)]`). It gets its type's default on load, so the type
    /// must be defaultable.
    #[must_use]
    pub const fn transient(mut self) -> Self {
        self.transient = true;
        self
    }
}

/// One variant of a unit-enum definition.
#[derive(Debug, Clone, Copy)]
pub struct VariantDefinition {
    pub(crate) name: &'static str,
    pub(crate) discriminant: Option<u32>,
    pub(crate) attributes: AttributesDefinition,
    pub(crate) aliases: &'static [&'static str],
}

impl VariantDefinition {
    /// A variant of an enum without an explicit `repr`, matched by name.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            discriminant: None,
            attributes: AttributesDefinition::new(),
            aliases: &[],
        }
    }

    /// A variant of an enum with an explicit `repr`, with its discriminant.
    #[must_use]
    pub const fn with_discriminant(name: &'static str, discriminant: u32) -> Self {
        Self {
            discriminant: Some(discriminant),
            ..Self::new(name)
        }
    }

    /// The variant's editor attributes.
    #[must_use]
    pub const fn attributes(mut self, attributes: AttributesDefinition) -> Self {
        self.attributes = attributes;
        self
    }

    /// Former names of the variant, so files that use them still load.
    #[must_use]
    pub const fn aliases(mut self, aliases: &'static [&'static str]) -> Self {
        self.aliases = aliases;
        self
    }
}

/// One named component of a math type, such as `x` of `Vec3` or `rotation` of `Transform`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MathComponentDefinition {
    pub(crate) name: &'static str,
    pub(crate) ty: TypeRef,
    pub(crate) get: ComponentGet,
    pub(crate) set: ComponentSet,
}

impl MathComponentDefinition {
    /// A component of type `T`, copied out and in by `get` and `set`, which must work on the
    /// math type and `T`. `T` is `Copy`: the setter copies the value and its source stays valid.
    pub(crate) const fn new<T: Reflect + Copy>(
        name: &'static str,
        get: ComponentGet,
        set: ComponentSet,
    ) -> Self {
        Self {
            name,
            ty: TypeRef::of::<T>(),
            get,
            set,
        }
    }
}
