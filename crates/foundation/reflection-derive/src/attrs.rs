//! Parsing `#[reflect(...)]`, `#[repr(...)]` and doc comments.

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, quote};
use syn::meta::ParseNestedMeta;
use syn::{Attribute, Expr, ExprLit, Ident, Lit, LitStr, Meta, Token};

/// How the type is identified and saved.
pub(crate) enum Persistence {
    /// `id = "<uuid>"`, as its 16 big-endian bytes.
    Persisted([u8; 16]),
    /// `transient`
    Transient,
    /// `rebuilt`
    Rebuilt,
}

/// Editor attributes shared by types, fields and variants.
#[derive(Default)]
pub(crate) struct EditorAttributes {
    display_name: Option<LitStr>,
    category: Option<LitStr>,
    tooltip: Option<LitStr>,
    range: Option<(f64, f64, Option<f64>)>,
    units: Option<Ident>,
    hidden: bool,
    read_only: bool,
    /// The first line of the doc comment, the tooltip's default.
    doc: Option<String>,
}

impl EditorAttributes {
    /// Parses one editor attribute. Returns `Ok(false)` if `meta` isn't one.
    fn parse(&mut self, meta: &ParseNestedMeta<'_>) -> syn::Result<bool> {
        let path = &meta.path;
        if path.is_ident("display_name") {
            set(&mut self.display_name, meta.value()?.parse()?, meta)?;
        } else if path.is_ident("category") {
            set(&mut self.category, meta.value()?.parse()?, meta)?;
        } else if path.is_ident("tooltip") {
            set(&mut self.tooltip, meta.value()?.parse()?, meta)?;
        } else if path.is_ident("units") {
            set(&mut self.units, meta.value()?.parse()?, meta)?;
        } else if path.is_ident("hidden") {
            self.hidden = true;
        } else if path.is_ident("read_only") {
            self.read_only = true;
        } else if path.is_ident("range") {
            let (mut min, mut max, mut step) = (None, None, None);
            meta.parse_nested_meta(|inner| {
                let slot = if inner.path.is_ident("min") {
                    &mut min
                } else if inner.path.is_ident("max") {
                    &mut max
                } else if inner.path.is_ident("step") {
                    &mut step
                } else {
                    return Err(inner.error("expected `min`, `max` or `step`"));
                };
                set(slot, number(&inner)?, &inner)
            })?;
            let (Some(min), Some(max)) = (min, max) else {
                return Err(
                    meta.error("`range` needs `min` and `max`, as in range(min = 0.0, max = 1.0)")
                );
            };
            if self.range.replace((min, max, step)).is_some() {
                return Err(meta.error("`range` is given twice"));
            }
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// The `AttributesDefinition` expression.
    pub(crate) fn to_tokens(&self) -> TokenStream {
        let mut out = quote!(::oxreal_reflection::AttributesDefinition::new());
        if let Some(v) = &self.display_name {
            out.extend(quote!(.display_name(#v)));
        }
        if let Some(v) = &self.category {
            out.extend(quote!(.category(#v)));
        }
        let tooltip = self.tooltip.clone().or_else(|| {
            self.doc
                .as_ref()
                .map(|doc| LitStr::new(doc, Span::call_site()))
        });
        if let Some(v) = tooltip {
            out.extend(quote!(.tooltip(#v)));
        }
        if let Some((min, max, step)) = self.range {
            let (min, max) = (f64_literal(min), f64_literal(max));
            let mut range = quote!(::oxreal_reflection::Range::new(#min, #max));
            if let Some(step) = step {
                let step = f64_literal(step);
                range.extend(quote!(.with_step(#step)));
            }
            out.extend(quote!(.range(#range)));
        }
        if let Some(unit) = &self.units {
            out.extend(quote!(.units(::oxreal_reflection::Unit::#unit)));
        }
        if self.hidden {
            out.extend(quote!(.hidden()));
        }
        if self.read_only {
            out.extend(quote!(.read_only()));
        }
        out
    }
}

/// Attributes of the type itself.
#[derive(Default)]
pub(crate) struct TypeAttributes {
    pub(crate) persistence: Option<(Persistence, Span)>,
    pub(crate) name: Option<LitStr>,
    pub(crate) clone: bool,
    pub(crate) default: bool,
    pub(crate) abi: Option<Span>,
    pub(crate) opaque: bool,
    pub(crate) editor: EditorAttributes,
}

/// Attributes of a field or variant.
#[derive(Default)]
pub(crate) struct MemberAttributes {
    pub(crate) transient: bool,
    pub(crate) opaque: Option<Span>,
    pub(crate) aliases: Vec<LitStr>,
    pub(crate) editor: EditorAttributes,
}

/// The `#[repr(...)]` facts the derive cares about.
#[derive(Default)]
pub(crate) struct Repr {
    /// `C` or `transparent`.
    pub(crate) c_layout: bool,
    pub(crate) packed: Option<Span>,
    /// `u8`, `u16` or `u32`, or another integer (rejected for enums).
    pub(crate) int: Option<Ident>,
}

const TYPE_KEYS: &str = "id, transient, rebuilt, name, clone, default, abi, opaque, display_name, \
                         category, tooltip, range, units, hidden, read_only";
const MEMBER_KEYS: &str = "transient, opaque, alias, display_name, category, tooltip, range, \
                           units, hidden, read_only";

pub(crate) fn type_attributes(attrs: &[Attribute]) -> syn::Result<TypeAttributes> {
    let mut out = TypeAttributes::default();
    out.editor.doc = first_doc_line(attrs);
    for attr in attrs.iter().filter(|a| a.path().is_ident("reflect")) {
        attr.parse_nested_meta(|meta| {
            let span = meta
                .path
                .get_ident()
                .map_or_else(Span::call_site, Ident::span);
            if meta.path.is_ident("id") {
                let text: LitStr = meta.value()?.parse()?;
                let bytes = parse_uuid(&text.value()).ok_or_else(|| {
                    syn::Error::new(
                        text.span(),
                        "malformed stable type ID: expected a hyphenated UUID such as \
                         6f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d04 (generate one with \
                         `cargo xtask new-type-id`)",
                    )
                })?;
                set_persistence(&mut out, Persistence::Persisted(bytes), span, &meta)
            } else if meta.path.is_ident("transient") {
                set_persistence(&mut out, Persistence::Transient, span, &meta)
            } else if meta.path.is_ident("rebuilt") {
                set_persistence(&mut out, Persistence::Rebuilt, span, &meta)
            } else if meta.path.is_ident("name") {
                set(&mut out.name, meta.value()?.parse()?, &meta)
            } else if meta.path.is_ident("clone") {
                out.clone = true;
                Ok(())
            } else if meta.path.is_ident("default") {
                out.default = true;
                Ok(())
            } else if meta.path.is_ident("abi") {
                out.abi = Some(span);
                Ok(())
            } else if meta.path.is_ident("opaque") {
                out.opaque = true;
                Ok(())
            } else if out.editor.parse(&meta)? {
                Ok(())
            } else {
                Err(meta.error(format!(
                    "unknown reflect attribute; expected one of: {TYPE_KEYS}"
                )))
            }
        })?;
    }
    Ok(out)
}

pub(crate) fn member_attributes(attrs: &[Attribute]) -> syn::Result<MemberAttributes> {
    let mut out = MemberAttributes {
        editor: EditorAttributes {
            doc: first_doc_line(attrs),
            ..EditorAttributes::default()
        },
        ..MemberAttributes::default()
    };
    for attr in attrs.iter().filter(|a| a.path().is_ident("reflect")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("transient") {
                out.transient = true;
            } else if meta.path.is_ident("opaque") {
                out.opaque = Some(
                    meta.path
                        .get_ident()
                        .map_or_else(Span::call_site, Ident::span),
                );
            } else if meta.path.is_ident("alias") {
                out.aliases.push(meta.value()?.parse()?);
            } else if !out.editor.parse(&meta)? {
                return Err(meta.error(format!(
                    "unknown reflect attribute; expected one of: {MEMBER_KEYS}"
                )));
            }
            Ok(())
        })?;
    }
    Ok(out)
}

pub(crate) fn repr(attrs: &[Attribute]) -> syn::Result<Repr> {
    let mut out = Repr::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("repr")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("C") || meta.path.is_ident("transparent") {
                out.c_layout = true;
            } else if meta.path.is_ident("packed") {
                out.packed = Some(
                    meta.path
                        .get_ident()
                        .map_or_else(Span::call_site, Ident::span),
                );
                if meta.input.peek(syn::token::Paren) {
                    let _: TokenStream = meta.input.parse()?;
                }
            } else if meta.path.is_ident("align") {
                let _: TokenStream = meta.input.parse()?;
            } else if let Some(ident) = meta.path.get_ident() {
                out.int = Some(ident.clone());
            }
            Ok(())
        })?;
    }
    Ok(out)
}

fn set_persistence(
    out: &mut TypeAttributes,
    persistence: Persistence,
    span: Span,
    meta: &ParseNestedMeta<'_>,
) -> syn::Result<()> {
    if out.persistence.replace((persistence, span)).is_some() {
        return Err(meta.error(
            "give exactly one of `id = \"...\"`, `transient` and `rebuilt`: a transient type \
             is never saved, so it has no stable ID",
        ));
    }
    Ok(())
}

fn set<T>(slot: &mut Option<T>, value: T, meta: &ParseNestedMeta<'_>) -> syn::Result<()> {
    if slot.replace(value).is_some() {
        return Err(meta.error(format!("`{}` is given twice", meta.path.to_token_stream())));
    }
    Ok(())
}

/// A number literal, optionally negative, as written in `range(...)`.
fn number(meta: &ParseNestedMeta<'_>) -> syn::Result<f64> {
    let input = meta.value()?;
    let negative = input.peek(Token![-]);
    if negative {
        input.parse::<Token![-]>()?;
    }
    let value = match input.parse::<Lit>()? {
        Lit::Float(lit) => lit.base10_parse::<f64>()?,
        Lit::Int(lit) => lit.base10_parse::<f64>()?,
        other => return Err(syn::Error::new(other.span(), "expected a number")),
    };
    Ok(if negative { -value } else { value })
}

fn f64_literal(value: f64) -> proc_macro2::Literal {
    proc_macro2::Literal::f64_suffixed(value)
}

/// The first non-empty line of the doc comment, trimmed.
fn first_doc_line(attrs: &[Attribute]) -> Option<String> {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            Meta::NameValue(nv) => match &nv.value {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(s), ..
                }) => Some(s.value()),
                _ => None,
            },
            _ => None,
        })
        .flat_map(|doc| doc.lines().map(str::to_owned).collect::<Vec<_>>())
        .map(|line| line.trim().to_owned())
        .find(|line| !line.is_empty())
}

/// Parses `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` into big-endian bytes.
fn parse_uuid(text: &str) -> Option<[u8; 16]> {
    let bytes = text.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    let mut out = [0u8; 16];
    let mut hex = bytes
        .iter()
        .enumerate()
        .filter(|(i, _)| !matches!(i, 8 | 13 | 18 | 23));
    if [8, 13, 18, 23].iter().any(|&i| bytes[i] != b'-') {
        return None;
    }
    for byte in &mut out {
        let (_, high) = hex.next()?;
        let (_, low) = hex.next()?;
        // `to_digit` accepts hex digits only (unlike `from_str_radix`, which takes a sign).
        let digit = |c: u8| {
            char::from(c)
                .to_digit(16)
                .and_then(|d| u8::try_from(d).ok())
        };
        *byte = (digit(*high)? << 4) | digit(*low)?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::parse_uuid;

    #[test]
    fn uuids_parse_to_big_endian_bytes() {
        let bytes = parse_uuid("6f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d04").expect("well-formed");
        assert_eq!(bytes[0], 0x6f);
        assert_eq!(bytes[15], 0x04);
        assert_eq!(
            parse_uuid("6F1C2E4A-93B0-4C1E-8D3F-2B7A9E5C1D04"),
            Some(bytes),
            "either case"
        );
    }

    #[test]
    fn malformed_uuids_are_rejected() {
        for bad in [
            "",
            "6f1c2e4a93b04c1e8d3f2b7a9e5c1d04",
            "6f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d0g",
            "6f1c2e4a-93b0-4c1e-8d3f_2b7a9e5c1d04",
            "+f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d04",
        ] {
            assert_eq!(parse_uuid(bad), None, "{bad}");
        }
    }
}
