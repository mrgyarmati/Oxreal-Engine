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
///     #[reflect(range(min = 0.0, max = 20.0), units = MetresPerSecond)]
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
/// code that reads and writes values through descriptions trusts it:
///
/// - it's built with [`TypeDefinition::builder::<Self>`](TypeDefinition::builder), so its
///   layout and operations are `Self`'s;
/// - every field definition names the field's real type and its real offset (`offset_of!`), and
///   the fields don't overlap;
/// - a primitive kind is used only by that primitive, `String` only by `String`, and so on;
/// - a unit enum's discriminants are its real ones;
/// - [`Reflect::TRANSIENT`] and [`Reflect::ABI`] agree with the definition. Registration checks
///   this and refuses a mismatch.
///
/// A hand-written impl takes on this proof obligation. The derive discharges it by
/// construction.
pub unsafe trait Reflect: 'static {
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
