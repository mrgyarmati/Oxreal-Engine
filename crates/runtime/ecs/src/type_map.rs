//! Hash maps keyed by `TypeId`, without the cost of SipHash.

use std::any::TypeId;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// A `HashMap` keyed by `TypeId`.
///
/// The standard hasher (SipHash) resists collision attacks, which matters for keys an attacker
/// chooses. A `TypeId` is chosen by the compiler and is already a well-distributed hash of the
/// type, so hashing it again only costs time. Component, resource and bundle lookups go
/// through these maps on every access.
pub(crate) type TypeIdMap<V> = HashMap<TypeId, V, BuildHasherDefault<TypeIdHasher>>;

/// Passes a `TypeId`'s own hash through.
#[derive(Debug, Default)]
pub(crate) struct TypeIdHasher(u64);

impl Hasher for TypeIdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write_u64(&mut self, value: u64) {
        // TypeId hashes itself as one u64 today. Mixing keeps any later change correct.
        self.0 = self.0.rotate_left(5) ^ value;
    }

    fn write(&mut self, bytes: &[u8]) {
        // Not used by TypeId today. FNV-1a, so any other way it hashes stays correct.
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_types_stay_distinct() {
        let mut map: TypeIdMap<&str> = TypeIdMap::default();
        map.insert(TypeId::of::<u8>(), "u8");
        map.insert(TypeId::of::<u16>(), "u16");
        map.insert(TypeId::of::<String>(), "String");
        map.insert(TypeId::of::<(u8, u16)>(), "tuple");
        assert_eq!(map.len(), 4);
        assert_eq!(map.get(&TypeId::of::<u16>()), Some(&"u16"));
        assert_eq!(map.get(&TypeId::of::<(u8, u16)>()), Some(&"tuple"));
        assert_eq!(map.get(&TypeId::of::<u32>()), None);
    }
}
