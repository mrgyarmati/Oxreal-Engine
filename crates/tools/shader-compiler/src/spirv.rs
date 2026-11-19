//! Reading the capabilities a SPIR-V module declares (ADR-0013 §2).
//!
//! Capability numbers come from the official SPIR-V grammar (`spirv.core.grammar.json`,
//! Vulkan SDK 1.4.363.1).

use std::fmt;

/// A SPIR-V capability, by its number in the SPIR-V specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Capability(pub u32);

/// Names of the capabilities Oxreal uses or expects, for readable errors.
const NAMES: &[(u32, &str)] = &[
    (0, "Matrix"),
    (1, "Shader"),
    (11, "Int64"),
    (29, "SampledImageArrayDynamicIndexing"),
    (50, "ImageQuery"),
    (56, "StorageImageWriteWithoutFormat"),
    (4427, "DrawParameters"),
    (5301, "ShaderNonUniform"),
    (5302, "RuntimeDescriptorArray"),
    (5307, "SampledImageArrayNonUniformIndexing"),
    (5308, "StorageBufferArrayNonUniformIndexing"),
    (5309, "StorageImageArrayNonUniformIndexing"),
    (5347, "PhysicalStorageBufferAddresses"),
];

impl Capability {
    /// The capability's specification name, if it's one Oxreal knows.
    #[must_use]
    pub fn name(self) -> Option<&'static str> {
        NAMES
            .iter()
            .find(|(n, _)| *n == self.0)
            .map(|(_, name)| *name)
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "Capability({})", self.0),
        }
    }
}

/// Capabilities shaders may declare: exactly what Oxreal's required device features (ADR-0016
/// §3) make available. Extending this list means extending the required features first.
pub const ALLOWED_CAPABILITIES: &[Capability] = &[
    Capability(0),    // Matrix (implied by Shader)
    Capability(1),    // Shader
    Capability(5301), // ShaderNonUniform (descriptor indexing)
    Capability(5302), // RuntimeDescriptorArray (runtimeDescriptorArray)
    Capability(5307), // SampledImageArrayNonUniformIndexing
    Capability(5309), // StorageImageArrayNonUniformIndexing
    Capability(5347), // PhysicalStorageBufferAddresses (bufferDeviceAddress)
];

const MAGIC: u32 = 0x0723_0203;
const OP_CAPABILITY: u32 = 17;
const HEADER_WORDS: usize = 5;

/// Every capability `spirv` declares, in declaration order.
///
/// # Errors
///
/// A description of the problem if `spirv` is not a well-formed SPIR-V module in host byte order.
pub fn capabilities(spirv: &[u32]) -> Result<Vec<Capability>, String> {
    match spirv.first() {
        Some(&MAGIC) => {}
        Some(&word) if word == MAGIC.swap_bytes() => {
            return Err("SPIR-V is in the opposite byte order".to_owned());
        }
        _ => return Err("not a SPIR-V module (bad magic number)".to_owned()),
    }
    if spirv.len() < HEADER_WORDS {
        return Err("SPIR-V header is truncated".to_owned());
    }
    let mut found = Vec::new();
    let mut at = HEADER_WORDS;
    while at < spirv.len() {
        let word = spirv[at];
        let count = (word >> 16) as usize;
        let opcode = word & 0xFFFF;
        if count == 0 || at + count > spirv.len() {
            return Err(format!("malformed instruction at word {at}"));
        }
        if opcode == OP_CAPABILITY {
            if count < 2 {
                return Err(format!("OpCapability without operand at word {at}"));
            }
            found.push(Capability(spirv[at + 1]));
        }
        at += count;
    }
    Ok(found)
}

/// The capabilities in `found` that are not in [`ALLOWED_CAPABILITIES`], deduplicated.
pub(crate) fn disallowed(found: &[Capability]) -> Vec<Capability> {
    let mut extra: Vec<Capability> = found
        .iter()
        .copied()
        .filter(|c| !ALLOWED_CAPABILITIES.contains(c))
        .collect();
    extra.sort_unstable();
    extra.dedup();
    extra
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal module: header, then the given capabilities, then an unrelated instruction.
    fn module(caps: &[u32]) -> Vec<u32> {
        let mut words = vec![MAGIC, 0x0001_0600, 0, 100, 0];
        for &c in caps {
            words.extend([(2 << 16) | OP_CAPABILITY, c]);
        }
        words.extend([(3 << 16) | 14, 0, 1]); // OpMemoryModel Logical GLSL450
        words
    }

    #[test]
    fn finds_declared_capabilities_in_order() {
        let caps = capabilities(&module(&[1, 5302, 5301])).unwrap();
        assert_eq!(
            caps,
            vec![Capability(1), Capability(5302), Capability(5301)]
        );
    }

    #[test]
    fn rejects_malformed_modules() {
        assert!(capabilities(&[]).is_err());
        assert!(capabilities(&[0xdead_beef, 0, 0, 0, 0]).is_err());
        assert!(
            capabilities(&[MAGIC.swap_bytes(), 0, 0, 0, 0])
                .unwrap_err()
                .contains("byte order")
        );
        let mut truncated = module(&[1]);
        truncated.truncate(6); // cut inside OpCapability
        assert!(capabilities(&truncated).is_err());
        let zero_count = vec![MAGIC, 0, 0, 0, 0, 0];
        assert!(capabilities(&zero_count).is_err());
    }

    #[test]
    fn allowlist_flags_only_unexpected_capabilities() {
        let found = [
            Capability(1),
            Capability(11),
            Capability(5308),
            Capability(11),
        ];
        assert_eq!(disallowed(&found), vec![Capability(11), Capability(5308)]);
        assert!(disallowed(&[Capability(1), Capability(5347)]).is_empty());
    }

    #[test]
    fn capabilities_display_by_name() {
        assert_eq!(
            Capability(5309).to_string(),
            "StorageImageArrayNonUniformIndexing"
        );
        assert_eq!(
            Capability(5308).to_string(),
            "StorageBufferArrayNonUniformIndexing"
        );
        assert_eq!(Capability(123_456).to_string(), "Capability(123456)");
    }
}
