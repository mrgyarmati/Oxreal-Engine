//! Reflection for Oxreal: what the engine knows about a type at run time ([ADR-0021]).
//!
//! **Layer:** foundation. The editor's Details panel, saving and loading, undo, actor
//! templates, visual scripting and other languages all read types through this crate, so one
//! description per type serves all of them. It knows nothing about worlds or components: the
//! ECS builds on it, never the reverse.
//!
//! ```
//! use oxreal_reflection::{Reflect, TypeKind, TypeRegistry};
//!
//! /// A door that can be locked.
//! #[derive(Reflect, Clone, Default)]
//! #[reflect(id = "3c4b2a19-8e7d-4f60-a5b1-9d2c8e7f6a54", clone, default)]
//! struct Door {
//!     #[reflect(category = "State")]
//!     locked: bool,
//!     #[reflect(range(min = 0.0, max = 180.0), units = Radians)]
//!     open_angle: f32,
//! }
//!
//! let registry = TypeRegistry::new();
//! let key = registry.register::<Door>()?;
//! let door = registry.description(key).expect("just registered");
//!
//! let TypeKind::Struct(info) = door.kind() else { unreachable!() };
//! let (_, locked) = info.field("locked").expect("declared above");
//! assert_eq!(locked.attributes().category(), Some("State"));
//! assert_eq!(registry.description(locked.ty().expect("not opaque")).unwrap().name(), "bool");
//! # Ok::<(), oxreal_reflection::RegistrationError>(())
//! ```
//!
//! # Model
//!
//! - **Two identities** (ADR-0021 §2). A [`TypeKey`] identifies a type within one
//!   [`TypeRegistry`] while the process runs. A [`StableTypeId`] identifies it on disk and
//!   across languages: an explicit UUID in `#[reflect(id = "...")]` that survives renames and
//!   moves. Generic instantiations such as `Vec<f32>` get IDs derived from their arguments.
//! - **Persistence.** Every type either has a stable ID or is `transient` (never saved) or
//!   `rebuilt` (recreated by its owner). A saved type can't hide a transient field type unless
//!   the field is transient too.
//! - **Definitions and descriptions.** A [`TypeDefinition`] is what a Rust type declares at
//!   compile time, as the `const` of [`Reflect`]. Registering it produces a
//!   [`TypeDescription`]: plain data that refers to other types by key, with field offsets for
//!   this build, editor [`Attributes`] and erased [`Operations`].
//! - **Explicit registration** (ADR-0021 §5). Registries are values the application creates
//!   and shares; there's no global registry and no registration before `main`. Each crate with
//!   reflected types exposes a `register_types` function.
//!
//! # `#[derive(Reflect)]`
//!
//! Supported: structs with named or positional fields, unit-only enums (with an explicit
//! `#[repr(u8/u16/u32)]` or without one), and generic types whose parameters are reflected.
//! Not yet: data-carrying enums, maps, lifetimes, unions.
//!
//! Type attributes, in `#[reflect(...)]`:
//!
//! | Attribute | Meaning |
//! |---|---|
//! | `id = "<uuid>"` | The stable type ID. Generate one with `cargo xtask new-type-id`. |
//! | `transient` | Never saved; no ID needed |
//! | `rebuilt` | Rebuilt by its owner; copies and saves skip it. Implies `transient`. |
//! | `name = "..."` | A stable display name instead of the module path |
//! | `clone`, `default` | Declares the [`Clone`] or [`Default`] impl to reflection |
//! | `abi` | Plain C data other languages may read directly; needs `#[repr(C)]` |
//! | `opaque` | Identity, layout and operations only, no fields |
//!
//! Field and variant attributes: `display_name`, `category`, `tooltip` (defaults to the first
//! doc-comment line), `range(min = .., max = .., step = ..)`, `units = <Unit variant>`,
//! `hidden`, `read_only` and `alias = "old_name"`. Fields also take `transient` and `opaque`;
//! types take the editor attributes too.
//!
//! The derive rejects what it can see is wrong, at compile time:
//!
//! ```compile_fail
//! # use oxreal_reflection::Reflect;
//! #[derive(Reflect)] // error: needs `id = "..."`, `transient` or `rebuilt`
//! struct Unidentified { value: f32 }
//! ```
//!
//! ```compile_fail,E0080
//! # use oxreal_reflection::Reflect;
//! #[derive(Reflect)]
//! #[reflect(transient)]
//! struct Cache { hits: u32 }
//!
//! #[derive(Reflect)]
//! #[reflect(id = "5b8e1f2a-6c3d-4e9f-8a7b-1c2d3e4f5a6b")]
//! struct Saved { cache: Cache } // error: a saved type with a transient field type
//! ```
//!
//! ```compile_fail
//! # use oxreal_reflection::Reflect;
//! #[derive(Reflect)]
//! #[reflect(id = "7d9e2b4c-1a3f-4b5d-9e8c-2f4a6b8c0d1e", abi)] // error: abi needs #[repr(C)]
//! struct Vertex { x: f32 }
//! ```
//!
//! ```compile_fail,E0080
//! # use oxreal_reflection::Reflect;
//! #[derive(Reflect)]
//! #[reflect(id = "8e0f3c5d-2b4a-4c6e-8f9d-3a5b7c9d1e2f", abi)]
//! #[repr(C)]
//! struct Flags { enabled: bool } // error: bool isn't ABI-safe; use u8
//! ```
//!
//! ```compile_fail
//! # use oxreal_reflection::Reflect;
//! #[derive(Reflect)]
//! #[reflect(id = "9f1a4d6e-3c5b-4d7f-9a0e-4b6c8d0e2f3a")]
//! #[repr(packed)] // error: packed fields can't be referenced safely
//! struct Packed { a: u8, b: u32 }
//! ```
//!
//! ```compile_fail,E0277
//! # use oxreal_reflection::Reflect;
//! #[derive(Reflect)]
//! #[reflect(id = "0a2b5e7f-4d6c-4e8a-8b1f-5c7d9e1f3a4b", clone)] // error: no Clone impl
//! struct NotClone { value: f32 }
//! ```
//!
//! [ADR-0021]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0021-reflection-and-type-registry.md

// The derive emits `::oxreal_reflection::...` paths; this lets the unit tests use it.
#[cfg(test)]
extern crate self as oxreal_reflection;

mod attributes;
mod builtin;
mod definition;
mod description;
mod id;
mod kind;
mod ops;
mod reflect;
mod registry;

pub use attributes::{Attributes, AttributesDefinition, Range, Unit};
pub use definition::{
    DefinitionBuilder, FieldDefinition, GenericArg, KindDefinition, Persistence, TypeDefinition,
    TypeRef, VariantDefinition,
};
pub use description::{
    ArrayInfo, Field, GenericArgument, HandleInfo, Identity, ListInfo, MathComponent, MathInfo,
    OptionInfo, StructInfo, TypeDescription, TypeKey, TypeKind, UnitEnumInfo, Variant,
};
pub use id::{ConstArg, GenericIdInput, ParseStableTypeIdError, StableTypeId};
pub use kind::{IntRepr, Primitive};
pub use ops::{Operations, UnsupportedOperation};
pub use oxreal_reflection_derive::Reflect;
pub use reflect::Reflect;
pub use registry::{
    DefinitionProblem, GroupBuilder, GroupId, RegistrationError, RegistryId, TypeRegistry,
};

#[cfg(test)]
mod tests;
