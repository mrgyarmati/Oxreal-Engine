//! `#[derive(Reflect)]` for `oxreal-reflection` ([ADR-0021]).
//!
//! **Layer:** foundation (a proc-macro crate). Use the derive through `oxreal_reflection`'s
//! re-export; that crate documents the attributes. The generated code names only
//! `::oxreal_reflection` paths, never this crate or its dependencies.
//!
//! What the derive checks:
//!
//! - **Here, from the syntax:** a stable ID or persistence flag is given exactly once and the ID
//!   is a well-formed UUID; no unions, lifetimes, data-carrying enums or `#[repr(packed)]`;
//!   `abi` types are non-generic `#[repr(C)]` structs; enum representations are `u8`, `u16` or
//!   `u32`.
//! - **In the generated constants, at compile time:** a saved type has no field of a transient
//!   type unless the field is transient, and an `abi` type has only ABI-safe fields. These
//!   need the field types' own declarations, which the macro can't see.
//! - **By the compiler:** `clone` and `default` need the real `Clone` and `Default` impls.
//!
//! It emits an `unsafe impl Reflect` whose definition is built from the type itself
//! (`offset_of!`, `Layout`, the declared field types), which discharges the trait's safety
//! contract. It never emits lint attributes, so crates that `forbid(unsafe_code)` can use it.
//!
//! [ADR-0021]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0021-reflection-and-type-registry.md

mod attrs;
mod expand;

use proc_macro::TokenStream;

/// Implements `oxreal_reflection::Reflect`. See `oxreal_reflection` for the attributes.
#[proc_macro_derive(Reflect, attributes(reflect))]
pub fn derive_reflect(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    expand::reflect(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
