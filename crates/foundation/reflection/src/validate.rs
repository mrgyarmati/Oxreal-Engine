//! Validating values built from bytes, before any Rust code sees them (ADR-0021 §4).
//!
//! Only plain data can be built from bytes: primitives, unit enums with an explicit `repr`,
//! handles, fixed arrays of plain data, and structs whose fields are all plain data. Each kind
//! has its validity rule: a `bool` is 0 or 1, an enum's discriminant names a variant, and
//! integers, floats and handles accept any bit pattern. Padding bytes aren't checked or read.

use crate::description::{TypeDescription, TypeKind};
use crate::kind::{IntRepr, Primitive};
use crate::registry::TypeRegistry;
use crate::value::ValueError;

/// Checks that `bytes[offset..offset + size]` is a valid value of `description`'s type.
/// `bytes` holds the whole outer value, so errors report offsets into it.
pub(crate) fn validate(
    registry: &TypeRegistry,
    description: &TypeDescription,
    bytes: &[u8],
    offset: usize,
) -> Result<(), ValueError> {
    let invalid = |problem: String| ValueError::InvalidBytes {
        ty: description.name().to_owned(),
        offset,
        problem,
    };
    match description.kind() {
        TypeKind::Primitive(Primitive::Bool) => match bytes[offset] {
            0 | 1 => Ok(()),
            other => Err(invalid(format!("a bool must be 0 or 1, found {other}"))),
        },
        TypeKind::Primitive(_) | TypeKind::Handle(_) => Ok(()),
        TypeKind::UnitEnum(info) => {
            let Some(repr) = info.repr() else {
                return Err(not_plain(description));
            };
            let discriminant = read_discriminant(repr, &bytes[offset..]);
            if info
                .variants()
                .iter()
                .any(|variant| variant.discriminant() == Some(discriminant))
            {
                Ok(())
            } else {
                Err(invalid(format!(
                    "{discriminant} isn't the discriminant of any variant"
                )))
            }
        }
        TypeKind::Struct(info) => {
            for field in info.fields() {
                let key = field.ty().ok_or_else(|| not_plain(description))?;
                let field_type = registry
                    .description(key)
                    .ok_or(ValueError::UnknownType { key })?;
                validate(registry, &field_type, bytes, offset + field.offset())?;
            }
            Ok(())
        }
        TypeKind::Array(info) => {
            let element = registry
                .description(info.element())
                .ok_or(ValueError::UnknownType {
                    key: info.element(),
                })?;
            let stride = element.layout().size();
            for index in 0..info.len() {
                validate(registry, &element, bytes, offset + index * stride)?;
            }
            Ok(())
        }
        _ => Err(not_plain(description)),
    }
}

fn not_plain(description: &TypeDescription) -> ValueError {
    ValueError::NotPlainData {
        ty: description.name().to_owned(),
    }
}

/// The discriminant stored in native byte order at the start of `bytes`.
fn read_discriminant(repr: IntRepr, bytes: &[u8]) -> u32 {
    match repr {
        IntRepr::U8 => u32::from(bytes[0]),
        IntRepr::U16 => u32::from(u16::from_ne_bytes([bytes[0], bytes[1]])),
        IntRepr::U32 => u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
    }
}
