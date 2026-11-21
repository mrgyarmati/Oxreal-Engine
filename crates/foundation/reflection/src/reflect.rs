//! The [`Reflect`] trait.

use crate::TypeDefinition;

/// A type the engine can describe at run time (ADR-0021).
///
/// Implement it with `#[derive(Reflect)]`, which checks the declaration and is safe to use:
///
/// ```
/// use oxreal_reflection::{Reflect, TypeRegistry};
///
/// /// How fast something moves.
/// #[derive(Reflect, Clone, Default)]
/// #[reflect(id = "0b9e8d4c-7a61-4f3e-9c2d-5e8f1a7b6c30", clone, default)]
/// struct Speed {
///     #[reflect(range(min = 0.0, max = 20.0), units = MetersPerSecond)]
///     max: f32,
///     /// Recomputed every frame, never saved.
///     #[reflect(transient)]
///     current: f32,
/// }
///
/// let registry = TypeRegistry::new();
/// let key = registry.register::<Speed>()?;
/// let description = registry.description(key).expect("just registered");
/// assert!(description.name().ends_with("Speed"));
/// assert_eq!(description.as_struct().expect("a struct").fields().len(), 2);
/// # Ok::<(), oxreal_reflection::RegistrationError>(())
/// ```
///
/// # Safety
///
/// Implementing `Reflect` promises that [`Reflect::DEFINITION`] describes `Self` exactly, because
/// code that reads and writes values through descriptions trusts it. Registration checks what it
/// can (marked *checked*); the rest is the implementer's proof:
///
/// - It's built with [`TypeDefinition::builder::<Self>`](TypeDefinition::builder), so its layout
///   and operations are `Self`'s (*checked*: the definition's `TypeId`).
/// - **Every** field is listed, in a struct, with its real type and real offset (`offset_of!`). A
///   field whose type isn't reflected is listed as opaque. Fields don't overlap (*checked*).
/// - `Self` isn't `#[repr(packed)]`: every field is aligned within any aligned `Self` (*checked*:
///   no field type is more aligned than `Self`, and each offset is a multiple of its alignment).
/// - **Every** variant of a unit enum is listed, with its real discriminant when the enum has an
///   explicit `repr` (*checked*: unique and within the `repr`).
/// - A primitive kind is used only by that primitive (*checked*: its layout), `String` only by
///   `String`, and so on.
/// - A handle kind is used only by plain data without `Drop` for which every bit pattern is a
///   valid value (*checked*: no `Drop`).
/// - [`Reflect::ABI`] (and `.abi()`) only for a `#[repr(C)]` or `#[repr(transparent)]` type
///   without `Drop` (*checked*) whose layout is exactly its field definitions, every field
///   ABI-safe (*checked*). Other languages read it as that C struct. Padding bytes may exist and
///   have no defined value: readers copy them but never interpret them.
/// - [`Reflect::TRANSIENT`] and [`Reflect::ABI`] agree with the definition (*checked*).
///
/// A hand-written impl takes on this proof obligation. The derive discharges it by construction.
///
/// **Deriving exposes the fields.** Reflection can read and, through later APIs, write every
/// field that isn't opaque, private ones included. A type whose own `unsafe` code relies on an
/// invariant between its fields (such as `len <= capacity`) must mark those fields
/// `#[reflect(opaque)]` (and `transient` if they're not saved) or implement `Reflect` by hand.
///
/// `Reflect` types are `Send + Sync`, like components: descriptions and their erased operations
/// are shared between threads, so values may be cloned and dropped on any thread.
pub unsafe trait Reflect: Send + Sync + 'static {
    /// Whether values of this type are never saved: the type is transient or rebuilt, or it's a
    /// generic instantiation with a transient type argument.
    ///
    /// It's separate from [`Reflect::DEFINITION`] so that checks can read it without evaluating
    /// definitions, which would make recursive types such as `struct Node { children:
    /// Vec<Node> }` a compile-time cycle.
    const TRANSIENT: bool;

    /// Whether the type is ABI-safe: plain data with a C layout that other languages may read
    /// directly (ADR-0021 §3). Integers, floats, fixed arrays of ABI-safe elements and types
    /// declared `#[reflect(abi)]` are. `bool`, Rust enums and generic types (other than arrays)
    /// never are.
    const ABI: bool;

    /// What the type declares about itself.
    const DEFINITION: TypeDefinition;
}
