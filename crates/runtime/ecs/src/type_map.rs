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

    #[test]
    fn hashes_spread_over_the_bits_hash_maps_use() {
        use std::collections::HashSet;
        use std::hash::BuildHasher;

        macro_rules! ids {
            ($($n:literal)*) => { [$(TypeId::of::<[u8; $n]>()),*] };
        }
        let ids = ids!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31);
        let build = BuildHasherDefault::<TypeIdHasher>::default();
        let hashes: Vec<u64> = ids.iter().map(|id| build.hash_one(id)).collect();
        // The standard map's table uses the top 7 bits as a tag and the low bits as the bucket.
        // Both must vary across types, or lookups degrade to scans.
        let tags: HashSet<u64> = hashes.iter().map(|h| h >> 57).collect();
        let buckets: HashSet<u64> = hashes.iter().map(|h| h & 0xFF).collect();
        assert!(
            tags.len() >= 16,
            "only {} distinct tags in 32 types",
            tags.len()
        );
        assert!(
            buckets.len() >= 16,
            "only {} distinct buckets in 32 types",
            buckets.len()
        );
    }
}
