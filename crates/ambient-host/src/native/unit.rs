//! What one load into a program brought, and how long it is kept.

use std::ops::Range;

use ambient_runtime::Word;
use memmap2::{Mmap, MmapMut};

use crate::hash::Hash;
use crate::parts::Parts;
use crate::statics::{Make, Statics};

/// What one load brought: code, and the block of words it reads.
///
/// All but a program's first is a unit of the runtime's
/// (`ambient_runtime::UNIT_WORDS`): its block begins with a count of
/// the references into it, and it is dropped once there are none.
pub(super) struct Unit {
    /// Held so that the code stays mapped.
    pub(super) _code: Mmap,
    /// Every definition and its closure, which the runtime writes to:
    /// a box, taken apart so that nothing else claims the words.
    pub(super) statics: *mut [u64],
    /// The strings the block refers to, each a reference this holds.
    pub(super) strings: Vec<Word>,
    /// The units the block refers into, each a reference this holds.
    pub(super) holds: Vec<u64>,
    /// The definitions the block has.
    pub(super) hashes: Vec<Hash>,
    /// Which of the program's externs the block brought.
    pub(super) natives: Range<usize>,
}

impl Unit {
    /// What the runtime makes of text a block refers to. A string of a
    /// unit is a reference the unit holds, so it goes when the unit and
    /// every value that is the string have.
    pub(super) fn text(&mut self, make: Make, text: &[u8], loaded: bool) -> Word {
        let (pointer, length) = (text.as_ptr(), text.len() as u64);
        // SAFETY: the pointer and length are those of a live slice,
        // which the runtime copies.
        unsafe {
            match make {
                Make::String if loaded => {
                    let string = ambient_runtime::amb_string_new(pointer, length);
                    self.strings.push(string);
                    string
                }
                Make::String => ambient_runtime::amb_string_pinned(pointer, length),
                Make::Bytes if loaded => {
                    let bytes = ambient_runtime::amb_bytes_new(pointer, length);
                    self.strings.push(bytes);
                    bytes
                }
                Make::Bytes => ambient_runtime::amb_bytes_pinned(pointer, length),
                Make::Field => ambient_runtime::amb_intern(pointer, length),
                Make::Tag => ambient_runtime::amb_tag(pointer, length),
            }
        }
    }
}

impl Drop for Unit {
    fn drop(&mut self) {
        // SAFETY: the strings and units are references this holds, and
        // the block was a box that nothing refers into any more.
        unsafe {
            for string in self.strings.drain(..) {
                ambient_runtime::amb_release(string);
            }
            for unit in self.holds.drain(..) {
                ambient_runtime::amb_unit_release(unit);
            }
            drop(Box::from_raw(self.statics));
        }
    }
}

/// Maps the code of the parts a plan laid out as executable memory, and
/// says where in it each of [`Parts::codes`] begins.
pub(super) fn map(parts: &Parts, plan: &Statics) -> Result<(Mmap, Vec<usize>), String> {
    let io = |error: std::io::Error| error.to_string();
    let codes = parts.codes();
    let code_of = |index: usize| match plan.placed[index] {
        Some(_) => codes[index].map_or(&[][..], |(_, code)| &code.0),
        None => &[][..],
    };
    let mut offsets = Vec::new();
    let mut size = 0usize;
    for index in 0..codes.len() {
        offsets.push(size);
        size += code_of(index).len().next_multiple_of(16);
    }
    let mut code = MmapMut::map_anon(size.max(1)).map_err(io)?;
    for (index, offset) in offsets.iter().enumerate() {
        let bytes = code_of(index);
        code[*offset..*offset + bytes.len()].copy_from_slice(bytes);
    }
    Ok((code.make_exec().map_err(io)?, offsets))
}
