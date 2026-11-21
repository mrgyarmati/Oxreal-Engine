//! Generating the `Reflect` impl.

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{
    Data, DataEnum, DeriveInput, Fields, GenericParam, Ident, LitStr, Member, Type, parse_quote,
};

use crate::attrs::{self, MemberAttributes, Persistence, Repr, TypeAttributes};

pub(crate) fn reflect(input: &DeriveInput) -> syn::Result<TokenStream> {
    let attrs = attrs::type_attributes(&input.attrs)?;
    let repr = attrs::repr(&input.attrs)?;
    let ident = &input.ident;

    let Some((persistence, _)) = &attrs.persistence else {
        return Err(syn::Error::new(
            ident.span(),
            "#[derive(Reflect)] needs a stable type ID or a persistence flag: add \
             #[reflect(id = \"<uuid>\")] (generate one with `cargo xtask new-type-id`), \
             #[reflect(transient)] if it's never saved, or #[reflect(rebuilt)] if its owner \
             rebuilds it",
        ));
    };
    if let Some(span) = repr.packed {
        return Err(syn::Error::new(
            span,
            "#[repr(packed)] types can't derive Reflect: their fields can't be referenced safely",
        ));
    }

    let generics = Generics::of(input)?;
    if let Some(span) = attrs.abi {
        if !repr.c_layout {
            return Err(syn::Error::new(
                span,
                "`abi` types need #[repr(C)] (or #[repr(transparent)]) so other languages \
                 can rely on their layout",
            ));
        }
        if !generics.is_empty() {
            return Err(syn::Error::new(span, "generic types can't be `abi`"));
        }
        if !matches!(input.data, Data::Struct(_)) {
            return Err(syn::Error::new(
                span,
                "only structs can be `abi`; use integer fields instead of enums",
            ));
        }
    }

    let name = match &attrs.name {
        Some(name) => quote!(#name),
        None => {
            let ident = LitStr::new(&ident.to_string(), ident.span());
            quote!(::core::concat!(::core::module_path!(), "::", #ident))
        }
    };
    let display = ident.to_string();
    let kind = if attrs.opaque {
        quote!(::oxreal_reflection::KindDefinition::opaque())
    } else {
        match &input.data {
            Data::Struct(data) => struct_kind(&data.fields, &display, &attrs)?,
            Data::Enum(data) => enum_kind(data, &repr)?,
            Data::Union(_) => {
                return Err(syn::Error::new(
                    ident.span(),
                    "unions can't derive Reflect: which field is valid isn't known",
                ));
            }
        }
    };
    // Checks that need the field types' declarations, chained onto the builder and evaluated
    // with the definition.
    let checks = if attrs.opaque {
        TokenStream::new()
    } else {
        match &input.data {
            Data::Struct(data) => field_checks(&data.fields, &display, &attrs)?,
            Data::Enum(_) | Data::Union(_) => TokenStream::new(),
        }
    };

    let persistence_tokens = match persistence {
        Persistence::Persisted(bytes) => {
            quote!(::oxreal_reflection::Persistence::Persisted(
                ::oxreal_reflection::StableTypeId::from_bytes([#(#bytes),*])
            ))
        }
        Persistence::Transient => quote!(::oxreal_reflection::Persistence::Transient),
        Persistence::Rebuilt => quote!(::oxreal_reflection::Persistence::Rebuilt),
    };
    let type_params = &generics.types;
    let transient = match persistence {
        Persistence::Persisted(_) => {
            quote!(false #(|| <#type_params as ::oxreal_reflection::Reflect>::TRANSIENT)*)
        }
        Persistence::Transient | Persistence::Rebuilt => quote!(true),
    };
    let abi = attrs.abi.is_some();

    let mut builder = quote!(
        ::oxreal_reflection::TypeDefinition::builder::<Self>(#name, #persistence_tokens, #kind)
    );
    if !generics.is_empty() {
        let args = &generics.args;
        builder.extend(quote!(.generic_args(&[#(#args),*])));
    }
    let type_attributes = attrs.editor.to_tokens();
    builder.extend(quote!(.attributes(#type_attributes)));
    if attrs.clone {
        builder.extend(quote!(.cloneable()));
    }
    if attrs.default {
        builder.extend(quote!(.defaultable()));
    }
    if abi {
        builder.extend(quote!(.abi()));
    }

    // Bounds: every type parameter is reflected, and declared operations exist for the
    // instantiation.
    let mut where_clause = input
        .generics
        .where_clause
        .clone()
        .unwrap_or_else(|| parse_quote!(where));
    for ty in type_params {
        where_clause
            .predicates
            .push(parse_quote!(#ty: ::oxreal_reflection::Reflect));
    }
    if !generics.is_empty() {
        if attrs.clone {
            where_clause
                .predicates
                .push(parse_quote!(Self: ::core::clone::Clone));
        }
        if attrs.default {
            where_clause
                .predicates
                .push(parse_quote!(Self: ::core::default::Default));
        }
    }
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();

    // A non-generic type's definition is evaluated right here, so its checks fail the build
    // even if nothing registers the type. A generic one's is checked per instantiation.
    let force = if generics.is_empty() {
        quote!(
            const _: ::oxreal_reflection::TypeDefinition =
                <#ident as ::oxreal_reflection::Reflect>::DEFINITION;
        )
    } else {
        TokenStream::new()
    };

    Ok(quote! {
        unsafe impl #impl_generics ::oxreal_reflection::Reflect for #ident #ty_generics
        #where_clause
        {
            const TRANSIENT: bool = #transient;
            const ABI: bool = #abi;
            const DEFINITION: ::oxreal_reflection::TypeDefinition = #builder #checks .build();
        }
        #force
    })
}

/// The type's generic parameters, as bounds and as definition arguments.
struct Generics {
    types: Vec<Ident>,
    args: Vec<TokenStream>,
}

impl Generics {
    fn of(input: &DeriveInput) -> syn::Result<Self> {
        let mut out = Self {
            types: Vec::new(),
            args: Vec::new(),
        };
        for param in &input.generics.params {
            match param {
                GenericParam::Lifetime(lifetime) => {
                    return Err(syn::Error::new(
                        lifetime.span(),
                        "reflected types can't have lifetime parameters: descriptions are \
                         'static data",
                    ));
                }
                GenericParam::Type(ty) => {
                    let ident = &ty.ident;
                    out.args.push(quote!(::oxreal_reflection::GenericArg::Type(
                        ::oxreal_reflection::TypeRef::of::<#ident>()
                    )));
                    out.types.push(ident.clone());
                }
                GenericParam::Const(constant) => {
                    let ident = &constant.ident;
                    let constructor = const_constructor(&constant.ty)?;
                    out.args.push(quote!(::oxreal_reflection::GenericArg::Const(
                        ::oxreal_reflection::ConstArg::#constructor(#ident)
                    )));
                }
            }
        }
        Ok(out)
    }

    fn is_empty(&self) -> bool {
        self.args.is_empty()
    }
}

/// The `ConstArg` constructor for a const parameter's type.
fn const_constructor(ty: &Type) -> syn::Result<Ident> {
    const SUPPORTED: [&str; 10] = [
        "bool", "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64",
    ];
    if let Type::Path(path) = ty
        && let Some(ident) = path.path.get_ident()
        && SUPPORTED.contains(&ident.to_string().as_str())
    {
        return Ok(ident.clone());
    }
    Err(syn::Error::new(
        ty.span(),
        "const parameters of reflected types must be bool, an integer of at most 64 bits, or \
         usize",
    ))
}

fn struct_kind(fields: &Fields, ty: &str, attrs: &TypeAttributes) -> syn::Result<TokenStream> {
    let mut definitions = Vec::new();
    for (index, field) in fields.iter().enumerate() {
        let member = member(field.ident.as_ref(), index);
        let name = member_name(&member);
        let member_attrs = attrs::member_attributes(&field.attrs)?;
        if let (Some(span), Some(_)) = (member_attrs.opaque, attrs.abi) {
            return Err(syn::Error::new(
                span,
                format!("`{ty}` is `abi`, so its fields can't be opaque"),
            ));
        }
        let field_ty = &field.ty;
        let offset = quote!(::core::mem::offset_of!(Self, #member));
        let mut definition = if member_attrs.opaque.is_some() {
            quote!(::oxreal_reflection::FieldDefinition::opaque(#name, #offset))
        } else {
            quote!(::oxreal_reflection::FieldDefinition::new::<#field_ty>(#name, #offset))
        };
        definition.extend(member_tokens(&member_attrs));
        if member_attrs.transient {
            definition.extend(quote!(.transient()));
        }
        definitions.push(definition);
    }
    Ok(match fields {
        Fields::Unnamed(_) => {
            quote!(::oxreal_reflection::KindDefinition::tuple_struct(&[#(#definitions),*]))
        }
        Fields::Named(_) | Fields::Unit => {
            quote!(::oxreal_reflection::KindDefinition::named_struct(&[#(#definitions),*]))
        }
    })
}

/// `const` checks on the field types, as `.ensure(..)` calls on the builder: a saved type has no
/// transient field type unless the field is transient, and an `abi` type has only ABI-safe fields.
fn field_checks(fields: &Fields, ty: &str, attrs: &TypeAttributes) -> syn::Result<TokenStream> {
    let mut checks = TokenStream::new();
    for (index, field) in fields.iter().enumerate() {
        let member_attrs = attrs::member_attributes(&field.attrs)?;
        if member_attrs.opaque.is_some() {
            continue;
        }
        let name = member_name(&member(field.ident.as_ref(), index)).value();
        let field_ty = &field.ty;
        let span = field.ty.span();
        if !member_attrs.transient {
            let message = format!(
                "field `{name}` of `{ty}` has a transient type, but `{ty}` is saved: mark the \
                 field #[reflect(transient)], or make `{ty}` transient too"
            );
            checks.extend(quote_spanned! {span=>
                .ensure(
                    Self::TRANSIENT || !<#field_ty as ::oxreal_reflection::Reflect>::TRANSIENT,
                    #message,
                )
            });
        }
        if attrs.abi.is_some() {
            let message = format!(
                "field `{name}` of the `abi` type `{ty}` isn't ABI-safe: use integers, floats, \
                 fixed arrays and other `abi` types (u8 instead of bool, an integer instead of \
                 an enum)"
            );
            checks.extend(quote_spanned! {span=>
                .ensure(<#field_ty as ::oxreal_reflection::Reflect>::ABI, #message)
            });
        }
    }
    Ok(checks)
}

fn enum_kind(data: &DataEnum, repr: &Repr) -> syn::Result<TokenStream> {
    let int_repr = match &repr.int {
        None => None,
        Some(int) => Some(match int.to_string().as_str() {
            "u8" => quote!(U8),
            "u16" => quote!(U16),
            "u32" => quote!(U32),
            _ => {
                return Err(syn::Error::new(
                    int.span(),
                    "reflected enums use #[repr(u8)], #[repr(u16)], #[repr(u32)] or no repr",
                ));
            }
        }),
    };
    if repr.c_layout {
        return Err(syn::Error::new(
            Span::call_site(),
            "reflected enums use #[repr(u8)], #[repr(u16)], #[repr(u32)] or no repr, not \
             #[repr(C)]",
        ));
    }
    let mut variants = Vec::new();
    for variant in &data.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new(
                variant.ident.span(),
                "Reflect supports only enums whose variants carry no data, for now \
                 (data-carrying enums come with the Details panel, ADR-0021 §8)",
            ));
        }
        let member_attrs = attrs::member_attributes(&variant.attrs)?;
        if member_attrs.transient || member_attrs.opaque.is_some() {
            return Err(syn::Error::new(
                variant.ident.span(),
                "variants can't be `transient` or `opaque`",
            ));
        }
        let ident = &variant.ident;
        let name = LitStr::new(&ident.to_string(), ident.span());
        let mut definition = if int_repr.is_some() {
            quote!(::oxreal_reflection::VariantDefinition::with_discriminant(
                #name,
                Self::#ident as u32
            ))
        } else {
            quote!(::oxreal_reflection::VariantDefinition::new(#name))
        };
        definition.extend(member_tokens(&member_attrs));
        variants.push(definition);
    }
    let repr_tokens = match int_repr {
        Some(int) => quote!(::core::option::Option::Some(::oxreal_reflection::IntRepr::#int)),
        None => quote!(::core::option::Option::None),
    };
    Ok(quote!(::oxreal_reflection::KindDefinition::unit_enum(#repr_tokens, &[#(#variants),*])))
}

/// `.attributes(...)` and `.aliases(...)` for a field or variant.
fn member_tokens(attrs: &MemberAttributes) -> TokenStream {
    let editor = attrs.editor.to_tokens();
    let mut out = quote!(.attributes(#editor));
    if !attrs.aliases.is_empty() {
        let aliases = &attrs.aliases;
        out.extend(quote!(.aliases(&[#(#aliases),*])));
    }
    out
}

fn member(ident: Option<&Ident>, index: usize) -> Member {
    match ident {
        Some(ident) => Member::Named(ident.clone()),
        None => Member::Unnamed(index.into()),
    }
}

/// A field's reflected name: its identifier (without `r#`), or its index.
fn member_name(member: &Member) -> LitStr {
    match member {
        Member::Named(ident) => {
            let text = ident.to_string();
            let text = text.strip_prefix("r#").unwrap_or(&text);
            LitStr::new(text, ident.span())
        }
        Member::Unnamed(index) => LitStr::new(&index.index.to_string(), index.span),
    }
}
