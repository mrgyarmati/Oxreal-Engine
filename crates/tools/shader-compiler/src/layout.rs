//! Reading memory layouts from SPIR-V, for tests that check a shader reads what the Rust side
//! packs.
//!
//! Shaders that read buffers through addresses (ADR-0016 §2) use Slang's natural (C-like)
//! layout, which `scalarBlockLayout` permits. A Slang upgrade or a compiler flag could change
//! it, and neither `spirv-val` nor the validation layers would notice the out-of-bounds reads
//! that would follow. Code that vouches for such a shader (`ShaderCode::from_spirv_unchecked`)
//! tests its layouts with this. It needs debug names, so compile with [`crate::Optimization::Debug`].

use std::collections::HashMap;

const OP_NAME: u32 = 5;
const OP_TYPE_POINTER: u32 = 32;
const OP_DECORATE: u32 = 71;
const OP_MEMBER_DECORATE: u32 = 72;
const DECORATION_ARRAY_STRIDE: u32 = 6;
const DECORATION_OFFSET: u32 = 35;
const STORAGE_PHYSICAL_STORAGE_BUFFER: u32 = 5349;
/// Words before the first instruction.
const HEADER_WORDS: usize = 5;

/// The layout facts of a SPIR-V module: struct member offsets and the strides of pointers to
/// physical storage buffers, by debug name.
#[derive(Debug, Default)]
pub struct SpirvLayouts {
    /// Debug names of result ids.
    names: HashMap<u32, String>,
    /// Member offsets of each struct: (member index, byte offset).
    offsets: HashMap<u32, Vec<(u32, u32)>>,
    /// ArrayStride decorations, by id.
    strides: HashMap<u32, u32>,
    /// Physical-storage-buffer pointer types: (pointer id, pointee id).
    pointers: Vec<(u32, u32)>,
}

impl SpirvLayouts {
    /// Reads the layout facts from `spirv`.
    ///
    /// # Errors
    ///
    /// A description of the problem if the module is truncated or malformed.
    pub fn parse(spirv: &[u32]) -> Result<Self, String> {
        let mut layouts = Self::default();
        let mut at = HEADER_WORDS;
        while at < spirv.len() {
            let count = (spirv[at] >> 16) as usize;
            let opcode = spirv[at] & 0xFFFF;
            if count == 0 || at + count > spirv.len() {
                return Err(format!("malformed SPIR-V instruction at word {at}"));
            }
            let operands = &spirv[at + 1..at + count];
            let operand = |i: usize| {
                operands
                    .get(i)
                    .copied()
                    .ok_or_else(|| format!("opcode {opcode} at word {at} is too short"))
            };
            match opcode {
                OP_NAME => {
                    let bytes: Vec<u8> = operands
                        .get(1..)
                        .unwrap_or_default()
                        .iter()
                        .flat_map(|w| w.to_le_bytes())
                        .collect();
                    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                    let name = String::from_utf8_lossy(&bytes[..end]).into_owned();
                    layouts.names.insert(operand(0)?, name);
                }
                OP_MEMBER_DECORATE if operand(2)? == DECORATION_OFFSET => {
                    layouts
                        .offsets
                        .entry(operand(0)?)
                        .or_default()
                        .push((operand(1)?, operand(3)?));
                }
                OP_DECORATE if operand(1)? == DECORATION_ARRAY_STRIDE => {
                    layouts.strides.insert(operand(0)?, operand(2)?);
                }
                OP_TYPE_POINTER if operand(1)? == STORAGE_PHYSICAL_STORAGE_BUFFER => {
                    layouts.pointers.push((operand(0)?, operand(2)?));
                }
                _ => {}
            }
            at += count;
        }
        Ok(layouts)
    }

    /// The member offsets of every struct whose debug name starts with `prefix`, in member
    /// order. Slang may emit a struct more than once (per storage class), so there can be
    /// several.
    #[must_use]
    pub fn member_offsets(&self, prefix: &str) -> Vec<Vec<u32>> {
        self.structs_named(prefix)
            .into_iter()
            .map(|id| {
                let mut members = self.offsets[&id].clone();
                members.sort_unstable();
                members.into_iter().map(|(_, offset)| offset).collect()
            })
            .collect()
    }

    /// The array strides of physical-storage-buffer pointers to structs whose debug name
    /// starts with `prefix`: the distance between elements read through the pointer.
    #[must_use]
    pub fn pointer_strides(&self, prefix: &str) -> Vec<u32> {
        let targets = self.structs_named(prefix);
        self.pointers
            .iter()
            .filter(|(_, pointee)| targets.contains(pointee))
            .filter_map(|(pointer, _)| self.strides.get(pointer).copied())
            .collect()
    }

    /// The ids of structs whose debug name starts with `prefix`, sorted.
    fn structs_named(&self, prefix: &str) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .names
            .iter()
            .filter(|(id, name)| name.starts_with(prefix) && self.offsets.contains_key(id))
            .map(|(&id, _)| id)
            .collect();
        ids.sort_unstable();
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One instruction: its word count and opcode, then its operands.
    fn instruction(opcode: u32, operands: &[u32]) -> Vec<u32> {
        let count = u32::try_from(operands.len() + 1).unwrap();
        let mut words = vec![(count << 16) | opcode];
        words.extend_from_slice(operands);
        words
    }

    fn name(id: u32, text: &str) -> Vec<u32> {
        // NUL-terminated, padded to whole words.
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        let mut operands = vec![id];
        operands.extend(
            bytes
                .chunks(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])),
        );
        instruction(OP_NAME, &operands)
    }

    #[test]
    fn reads_offsets_and_pointer_strides() {
        let mut spirv = vec![0x0723_0203, 0x0001_0600, 0, 100, 0];
        spirv.extend(name(10, "Vertex"));
        spirv.extend(instruction(
            OP_MEMBER_DECORATE,
            &[10, 1, DECORATION_OFFSET, 12],
        ));
        spirv.extend(instruction(
            OP_MEMBER_DECORATE,
            &[10, 0, DECORATION_OFFSET, 0],
        ));
        spirv.extend(instruction(
            OP_TYPE_POINTER,
            &[11, STORAGE_PHYSICAL_STORAGE_BUFFER, 10],
        ));
        spirv.extend(instruction(OP_DECORATE, &[11, DECORATION_ARRAY_STRIDE, 24]));
        let layouts = SpirvLayouts::parse(&spirv).unwrap();
        assert_eq!(layouts.member_offsets("Vertex"), [vec![0, 12]]);
        assert_eq!(layouts.pointer_strides("Vertex"), [24]);
        assert!(layouts.member_offsets("Other").is_empty());
    }

    #[test]
    fn malformed_modules_are_errors_not_panics() {
        let header = vec![0x0723_0203, 0x0001_0600, 0, 100, 0];
        let mut zero_count = header.clone();
        zero_count.push(OP_NAME);
        assert!(SpirvLayouts::parse(&zero_count).is_err());
        let mut truncated = header.clone();
        truncated.push((9 << 16) | OP_NAME);
        assert!(SpirvLayouts::parse(&truncated).is_err());
        let mut short = header;
        short.extend(instruction(OP_MEMBER_DECORATE, &[10]));
        assert!(SpirvLayouts::parse(&short).is_err());
    }
}
