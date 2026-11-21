//! Persistent type identity: [`StableTypeId`] and the IDs derived for generic instantiations
//! (ADR-0021 §2).

use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};

/// A type's persistent 128-bit identity (ADR-0021 §2).
///
/// Written once in the type's `#[reflect(id = "…")]` attribute as a UUID string, and never
/// changed afterwards: renaming or moving the type keeps it, so saved data keeps loading. Files,
/// overrides, migrations and the C ABI identify types by it, never by Rust's `TypeId` or by name.
///
/// The bytes are in big-endian order, the order of the UUID text (RFC 9562's network byte
/// order). That is also the C-ABI form: `[u8; 16]`, never a `u128`.
///
/// ```
/// use oxreal_reflection::StableTypeId;
///
/// const ID: StableTypeId = StableTypeId::from_uuid("6f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d04");
/// assert_eq!(ID.to_string(), "6f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d04");
/// assert_eq!(ID.to_bytes()[0], 0x6f);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StableTypeId([u8; 16]);

impl StableTypeId {
    /// The ID with these big-endian bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The big-endian bytes, the C-ABI form.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0
    }

    /// Parses a hyphenated UUID string (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, either case) at
    /// compile time.
    ///
    /// Meant for constants in hand-written [`Reflect`](crate::Reflect) impls, always with a string
    /// literal: `cargo xtask type-ids` finds IDs written that way only, to catch duplicates. Code
    /// that parses IDs from data uses [`str::parse`], which returns an error instead.
    ///
    /// # Panics
    ///
    /// If `uuid` is malformed. In a constant this is a compile error.
    #[must_use]
    pub const fn from_uuid(uuid: &str) -> Self {
        match parse_uuid(uuid.as_bytes()) {
            Ok(bytes) => Self(bytes),
            Err(_) => panic!(
                "malformed stable type ID: expected a hyphenated UUID such as \
                 6f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d04"
            ),
        }
    }

    /// The ID of a generic instantiation, derived from the generic definition's ID and its
    /// arguments in order (ADR-0021 §2).
    ///
    /// The function is fixed: SHA-256 over a domain-separated encoding, truncated to the first
    /// 128 bits. Other languages compute the same IDs from the same inputs:
    ///
    /// 1. the ASCII tag `oxreal.reflection.generic-type-id.v1`;
    /// 2. the definition's 16 bytes;
    /// 3. the argument count as a big-endian `u32`;
    /// 4. per argument, a kind byte and its data:
    ///    - `0x01` and the type argument's 16 bytes;
    ///    - `0x02`, the const argument's type ID (16 bytes) and its value as a 16-byte
    ///      big-endian two's-complement integer (`bool` is 0 or 1).
    ///
    /// The encoding never changes: a change would change every derived ID. The test vectors
    /// in this module were computed with an independent implementation.
    ///
    /// # Panics
    ///
    /// With `u32::MAX` or more arguments, which no Rust type has.
    #[must_use]
    pub fn for_instantiation(definition: StableTypeId, arguments: &[GenericIdInput]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(GENERIC_ID_TAG);
        hasher.update(definition.0);
        let count = u32::try_from(arguments.len())
            .expect("a generic type has far fewer than u32::MAX arguments");
        hasher.update(count.to_be_bytes());
        for argument in arguments {
            match argument {
                GenericIdInput::Type(id) => {
                    hasher.update([0x01]);
                    hasher.update(id.0);
                }
                GenericIdInput::Const(constant) => {
                    hasher.update([0x02]);
                    hasher.update(constant.ty.0);
                    hasher.update(constant.value.to_be_bytes());
                }
            }
        }
        let digest = hasher.finalize();
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&digest[..16]);
        Self(bytes)
    }
}

/// The domain-separation tag of [`StableTypeId::for_instantiation`], version 1.
const GENERIC_ID_TAG: &[u8] = b"oxreal.reflection.generic-type-id.v1";

impl fmt::Display for StableTypeId {
    /// The lowercase hyphenated UUID form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for StableTypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StableTypeId({self})")
    }
}

impl FromStr for StableTypeId {
    type Err = ParseStableTypeIdError;

    /// Parses a hyphenated UUID, in either case.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_uuid(s.as_bytes())
            .map(Self)
            .map_err(|()| ParseStableTypeIdError {
                input: s.to_owned(),
            })
    }
}

/// A string that isn't a hyphenated UUID was parsed as a [`StableTypeId`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "'{input}' is not a stable type ID: expected a hyphenated UUID such as \
     6f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d04"
)]
pub struct ParseStableTypeIdError {
    input: String,
}

/// Parses `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`. `const` so constants fail to compile when
/// malformed.
const fn parse_uuid(text: &[u8]) -> Result<[u8; 16], ()> {
    if text.len() != 36 {
        return Err(());
    }
    let mut bytes = [0; 16];
    let mut out = 0;
    let mut i = 0;
    while i < 36 {
        if matches!(i, 8 | 13 | 18 | 23) {
            if text[i] != b'-' {
                return Err(());
            }
            i += 1;
            continue;
        }
        let (Some(high), Some(low)) = (hex_value(text[i]), hex_value(text[i + 1])) else {
            return Err(());
        };
        bytes[out] = (high << 4) | low;
        out += 1;
        i += 2;
    }
    Ok(bytes)
}

const fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// One input of [`StableTypeId::for_instantiation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericIdInput {
    /// A type argument, by its stable ID.
    Type(StableTypeId),
    /// A const generic argument.
    Const(ConstArg),
}

/// A const generic argument: its type and value (ADR-0021 §2).
///
/// Built through the constructor named after the type. `usize` arguments, such as array
/// lengths, are encoded as `u64`, because `usize` isn't a reflected type (its size depends on the
/// platform).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstArg {
    ty: StableTypeId,
    value: ConstValue,
}

/// The value of a [`ConstArg`]: every const generic type Oxreal supports fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConstValue {
    Unsigned(u64),
    Signed(i64),
    Bool(bool),
}

impl ConstValue {
    /// The 16-byte big-endian two's-complement form hashed by the generic-ID function.
    fn to_be_bytes(self) -> [u8; 16] {
        match self {
            Self::Unsigned(v) => u128::from(v).to_be_bytes(),
            Self::Signed(v) => i128::from(v).to_be_bytes(),
            Self::Bool(v) => u128::from(v).to_be_bytes(),
        }
    }
}

impl ConstArg {
    /// A `bool` argument.
    #[must_use]
    pub const fn bool(value: bool) -> Self {
        Self::new(builtin::BOOL, ConstValue::Bool(value))
    }

    /// A `u8` argument.
    #[must_use]
    pub const fn u8(value: u8) -> Self {
        Self::new(builtin::U8, ConstValue::Unsigned(value as u64))
    }

    /// A `u16` argument.
    #[must_use]
    pub const fn u16(value: u16) -> Self {
        Self::new(builtin::U16, ConstValue::Unsigned(value as u64))
    }

    /// A `u32` argument.
    #[must_use]
    pub const fn u32(value: u32) -> Self {
        Self::new(builtin::U32, ConstValue::Unsigned(value as u64))
    }

    /// A `u64` argument.
    #[must_use]
    pub const fn u64(value: u64) -> Self {
        Self::new(builtin::U64, ConstValue::Unsigned(value))
    }

    /// A `usize` argument, such as an array length, encoded as a `u64`.
    #[must_use]
    pub const fn usize(value: usize) -> Self {
        // `usize` is at most 64 bits on every supported target (ADR-0002).
        Self::new(builtin::U64, ConstValue::Unsigned(value as u64))
    }

    /// An `i8` argument.
    #[must_use]
    pub const fn i8(value: i8) -> Self {
        Self::new(builtin::I8, ConstValue::Signed(value as i64))
    }

    /// An `i16` argument.
    #[must_use]
    pub const fn i16(value: i16) -> Self {
        Self::new(builtin::I16, ConstValue::Signed(value as i64))
    }

    /// An `i32` argument.
    #[must_use]
    pub const fn i32(value: i32) -> Self {
        Self::new(builtin::I32, ConstValue::Signed(value as i64))
    }

    /// An `i64` argument.
    #[must_use]
    pub const fn i64(value: i64) -> Self {
        Self::new(builtin::I64, ConstValue::Signed(value))
    }

    const fn new(ty: StableTypeId, value: ConstValue) -> Self {
        Self { ty, value }
    }

    /// The stable ID of the argument's type (`u64` for `usize`).
    #[must_use]
    pub const fn stable_type_id(&self) -> StableTypeId {
        self.ty
    }
}

impl fmt::Display for ConstArg {
    /// The value as Rust source spells it, for display names such as `[f32; 4]`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.value {
            ConstValue::Unsigned(v) => write!(f, "{v}"),
            ConstValue::Signed(v) => write!(f, "{v}"),
            ConstValue::Bool(v) => write!(f, "{v}"),
        }
    }
}

/// The fixed stable IDs of the built-in types (ADR-0021 §2). Generated once, never changed.
pub(crate) mod builtin {
    use super::StableTypeId;

    /// Declares each built-in ID and `ALL` from one list, so they can't drift apart.
    macro_rules! builtin_ids {
        ($($name:ident = $id:expr;)*) => {
            $(pub(crate) const $name: StableTypeId = $id;)*

            /// Every built-in ID, generic definitions included.
            #[cfg(test)]
            pub(crate) const ALL: &[StableTypeId] = &[$($name),*];
        };
    }

    builtin_ids! {
        BOOL = StableTypeId::from_uuid("d760993b-9c97-4114-8a54-b191698217ad");
        I8 = StableTypeId::from_uuid("6a60f8e7-842d-47fb-8df4-6a8cbb895bd8");
        I16 = StableTypeId::from_uuid("0b515f49-6350-4ad2-be15-2ebb2abfb52c");
        I32 = StableTypeId::from_uuid("0e91a223-5ac3-40c1-88fb-9013a9bbb90d");
        I64 = StableTypeId::from_uuid("db51a05c-e387-4bac-836f-470df1138ffa");
        U8 = StableTypeId::from_uuid("9b05e98c-1245-4c92-b8d2-0efdb87f6dee");
        U16 = StableTypeId::from_uuid("f6b1c7b0-cb25-49e2-9dca-2d02040ab164");
        U32 = StableTypeId::from_uuid("e6af0bef-092e-4dc7-99ba-aef2b777e38d");
        U64 = StableTypeId::from_uuid("7d14da3a-7301-4717-bbf6-e6627ecea1b4");
        F32 = StableTypeId::from_uuid("442e25d3-c7cf-451e-b3f1-f980f8d12f09");
        F64 = StableTypeId::from_uuid("7b91d37a-5cfc-4d1a-aa9e-170bd15506f5");
        STRING = StableTypeId::from_uuid("2f6b16a8-b337-48de-83a6-185d0c3368bc");
        // Generic definitions: instantiations get derived IDs.
        VEC = StableTypeId::from_uuid("60822c5a-8f53-4e72-9b01-f05e81b7ca1a");
        OPTION = StableTypeId::from_uuid("fe605f63-af26-4bad-ab39-0fc92def9890");
        ARRAY = StableTypeId::from_uuid("13bf13b4-5fb7-491c-9aec-1dfe3c2a6c87");
        VEC2 = StableTypeId::from_uuid("8df6ae33-6b3b-41e3-aec1-271a48707638");
        VEC3 = StableTypeId::from_uuid("36620101-db80-4adf-8dc2-8add08cab957");
        VEC3A = StableTypeId::from_uuid("a8b17938-9ce3-4e8f-8d1d-c7f28a69b515");
        VEC4 = StableTypeId::from_uuid("c618f81e-7e2d-4b7e-b696-3bf260a3250d");
        QUAT = StableTypeId::from_uuid("f0289c39-46c1-4f2d-a5ef-921afd2f2554");
        MAT2 = StableTypeId::from_uuid("f026ab1c-c361-4692-9770-952cabc6c466");
        MAT3 = StableTypeId::from_uuid("1ed6f4fe-39ed-4a37-bd60-cd8bbbc0579a");
        MAT3A = StableTypeId::from_uuid("b6d991c8-d0f9-4efd-99f8-90e8385b93aa");
        MAT4 = StableTypeId::from_uuid("68ee3eda-142f-42d6-bd63-626cdb5e3c59");
        AFFINE3A = StableTypeId::from_uuid("3200baa2-4d70-477d-95d1-58766b6d1796");
        DVEC2 = StableTypeId::from_uuid("740727ae-5fcb-4006-b7ea-7459c4c48856");
        DVEC3 = StableTypeId::from_uuid("257d488d-fbbf-40c4-b139-5c28bde541df");
        DVEC4 = StableTypeId::from_uuid("f47e47b9-0805-4392-8a4f-ca0478e729be");
        DQUAT = StableTypeId::from_uuid("d91f088b-223b-49bc-a310-1254c2a75778");
        DMAT4 = StableTypeId::from_uuid("02c9dbb8-fe53-439f-90ea-d65aefdc2826");
        IVEC2 = StableTypeId::from_uuid("8b0d3111-3429-4a06-8725-5e16356ee4e1");
        IVEC3 = StableTypeId::from_uuid("42d9cc40-77ce-4c16-85d8-4b226565f51b");
        IVEC4 = StableTypeId::from_uuid("a9a4ab68-164f-41f6-a678-e9c7e041383b");
        UVEC2 = StableTypeId::from_uuid("f8a52c36-cefe-4124-84a4-23cd930d0688");
        UVEC3 = StableTypeId::from_uuid("9a6a6f35-6b9d-4ab3-9c79-67a44dbebe90");
        UVEC4 = StableTypeId::from_uuid("cddfa6ed-7bb7-4fef-b18e-5f9a4b69023c");
        TRANSFORM = StableTypeId::from_uuid("b89fb55d-4995-4d37-ac1a-5d1204717831");
        LINEAR_RGBA = StableTypeId::from_uuid("b92f71ea-afbd-4dfa-8f2b-7f05cf26610c");
    }
}

#[cfg(test)]
mod tests {
    use super::builtin::{ARRAY, BOOL, F32, I32, I64, OPTION, STRING, U8, U64, VEC};
    use super::*;

    fn id(text: &str) -> StableTypeId {
        text.parse().expect("test IDs are well-formed")
    }

    fn ty(id: StableTypeId) -> GenericIdInput {
        GenericIdInput::Type(id)
    }

    #[test]
    fn display_and_parse_round_trip_in_lowercase() {
        let parsed = id("6F1C2E4A-93B0-4C1E-8D3F-2B7A9E5C1D04");
        assert_eq!(parsed.to_string(), "6f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d04");
        assert_eq!(
            parsed,
            StableTypeId::from_uuid("6f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d04")
        );
        assert_eq!(
            parsed.to_bytes(),
            [
                0x6f, 0x1c, 0x2e, 0x4a, 0x93, 0xb0, 0x4c, 0x1e, 0x8d, 0x3f, 0x2b, 0x7a, 0x9e, 0x5c,
                0x1d, 0x04
            ]
        );
    }

    #[test]
    fn malformed_ids_are_errors_naming_the_input() {
        for bad in [
            "",
            "6f1c2e4a93b04c1e8d3f2b7a9e5c1d04",
            "6f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d0",
            "6f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d04a",
            "6f1c2e4a_93b0-4c1e-8d3f-2b7a9e5c1d04",
            "6f1c2e4g-93b0-4c1e-8d3f-2b7a9e5c1d04",
            "{6f1c2e4a-93b0-4c1e-8d3f-2b7a9e5c1d04}",
        ] {
            let error = bad.parse::<StableTypeId>().expect_err(bad);
            assert!(error.to_string().contains(&format!("'{bad}'")), "{error}");
        }
    }

    /// Vectors from an independent implementation of the encoding (.NET's SHA-256, not
    /// RustCrypto). If one fails, the encoding changed, which would change every derived ID
    /// already saved: restore the encoding, never the vector.
    #[test]
    fn generic_ids_match_the_checked_in_vectors() {
        let custom = id("00112233-4455-6677-8899-aabbccddeeff");
        let vec_u8 = StableTypeId::for_instantiation(VEC, &[ty(U8)]);
        let cases = [
            (
                "Vec<f32>",
                StableTypeId::for_instantiation(VEC, &[ty(F32)]),
                "4fdf80ff-0bbd-6cde-61d4-18b31a945d77",
            ),
            (
                "Option<String>",
                StableTypeId::for_instantiation(OPTION, &[ty(STRING)]),
                "e86c420d-80de-4bcd-a4ff-4e38e53a4063",
            ),
            (
                "[f32; 4]",
                StableTypeId::for_instantiation(
                    ARRAY,
                    &[ty(F32), GenericIdInput::Const(ConstArg::usize(4))],
                ),
                "f8c37ab2-39b9-653f-585b-3198a76711c8",
            ),
            ("Vec<u8>", vec_u8, "d74428bc-dbb8-7ad8-7bd2-6f0ea70fecdb"),
            (
                "Vec<Vec<u8>>",
                StableTypeId::for_instantiation(VEC, &[ty(vec_u8)]),
                "0aa52579-6b0f-9fb6-903a-5645e9f79c78",
            ),
            (
                "no arguments",
                StableTypeId::for_instantiation(custom, &[]),
                "c3bd131c-d12d-6df9-9ef4-ceabd9d71ea3",
            ),
            (
                "-1i32, true",
                StableTypeId::for_instantiation(
                    custom,
                    &[
                        GenericIdInput::Const(ConstArg::i32(-1)),
                        GenericIdInput::Const(ConstArg::bool(true)),
                    ],
                ),
                "2f4d61b8-8489-0641-2005-cbb053e6cbab",
            ),
            (
                "u64::MAX",
                StableTypeId::for_instantiation(
                    custom,
                    &[GenericIdInput::Const(ConstArg::u64(u64::MAX))],
                ),
                "edf4f19f-4fc6-826c-6cf8-833e04e0f3be",
            ),
            (
                "i64::MIN",
                StableTypeId::for_instantiation(
                    custom,
                    &[GenericIdInput::Const(ConstArg::i64(i64::MIN))],
                ),
                "185b6c90-0472-c208-fe26-e2088d0ea827",
            ),
        ];
        for (what, derived, expected) in cases {
            assert_eq!(derived.to_string(), expected, "{what}");
        }
    }

    #[test]
    fn const_arguments_name_their_types() {
        assert_eq!(ConstArg::usize(4).stable_type_id(), U64);
        assert_eq!(ConstArg::i32(-1).stable_type_id(), I32);
        assert_eq!(ConstArg::i64(-1).stable_type_id(), I64);
        assert_eq!(ConstArg::bool(true).stable_type_id(), BOOL);
        assert_eq!(ConstArg::i32(-7).to_string(), "-7");
    }

    #[test]
    fn builtin_ids_are_distinct() {
        let all = super::builtin::ALL;
        let mut sorted = all.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            all.len(),
            "two built-in types share a stable ID"
        );
    }
}
