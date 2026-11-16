//! Making and reading the values of a native program: the runtime is
//! in this process, so each is a call of one of its entry points.

use ambient_runtime::Word;

use super::load::Program;
use crate::hash::Hash;
use crate::value::{self, Construct, Inspect, Value};

/// Makes words for one program, which is what it takes to make a
/// function: a function is a definition the program has.
pub struct Inside<'a>(pub &'a Program);

impl Construct for Inside<'_> {
    fn int(&mut self, value: i64) -> Word {
        Construct::int(&mut Host, value)
    }

    fn float(&mut self, bits: u64) -> Word {
        Construct::float(&mut Host, bits)
    }

    fn string(&mut self, bytes: &[u8]) -> Word {
        Construct::string(&mut Host, bytes)
    }

    fn bytes(&mut self, bytes: &[u8]) -> Word {
        Construct::bytes(&mut Host, bytes)
    }

    fn key(&mut self, name: &[u8]) -> Word {
        Construct::key(&mut Host, name)
    }

    fn tag(&mut self, text: &[u8]) -> Word {
        Construct::tag(&mut Host, text)
    }

    fn handle(&mut self, brand: &[u8], number: u64) -> Word {
        Construct::handle(&mut Host, brand, number)
    }

    fn sequence(&mut self, list: bool, length: u64) -> Word {
        Construct::sequence(&mut Host, list, length)
    }

    fn iterator(&mut self, source: u64) -> Word {
        Construct::iterator(&mut Host, source)
    }

    fn iterator_set(&mut self, iterator: Word, index: u64, value: Word) {
        Construct::iterator_set(&mut Host, iterator, index, value);
    }

    fn sequence_set(&mut self, sequence: Word, index: u64, value: Word) {
        Construct::sequence_set(&mut Host, sequence, index, value);
    }

    fn record(&mut self, length: u64) -> Word {
        Construct::record(&mut Host, length)
    }

    fn record_set(&mut self, record: Word, key: Word, value: Word) {
        Construct::record_set(&mut Host, record, key, value);
    }

    fn release(&mut self, word: Word) {
        Construct::release(&mut Host, word);
    }

    fn function(&mut self, definition: &Hash, captures: u64) -> Option<Word> {
        let definition = self.0.definition(definition)?;
        // SAFETY: the definition is one the program has, and every
        // capture is set before the closure is seen.
        Some(unsafe { ambient_runtime::amb_closure_new(definition, captures) })
    }

    fn capture(&mut self, function: Word, index: u64, value: Word) {
        // SAFETY: `encode` stays within the captures it asked for.
        unsafe { ambient_runtime::amb_closure_set(function, index, value) }
    }

    fn method(&mut self, function: Word) {
        // SAFETY: the function was just made, and nothing else holds it.
        unsafe { ambient_runtime::amb_method_mark(function) }
    }
}

/// A value as a word the caller owns.
#[must_use]
pub fn encode(value: &Value) -> Word {
    value::encode(value, &mut Host)
}

/// Reads and makes words through the runtime linked into this process.
pub struct Host;

impl Inspect for Host {
    fn kind(&mut self, word: Word) -> u64 {
        // SAFETY: the word is a value a program returned.
        unsafe { ambient_runtime::amb_kind(word) }
    }

    fn int(&mut self, word: Word) -> i64 {
        // SAFETY: `decode` asks only for what `kind` reported.
        unsafe { ambient_runtime::amb_int_value(word) }
    }

    fn float_bits(&mut self, word: Word) -> u64 {
        // SAFETY: `decode` asks only for what `kind` reported.
        unsafe { ambient_runtime::amb_float_bits(word) }
    }

    fn length(&mut self, word: Word) -> u64 {
        // SAFETY: `decode` asks only about objects.
        unsafe { ambient_runtime::amb_length(word) }
    }

    fn word_at(&mut self, word: Word, index: u64) -> Word {
        // SAFETY: `decode` stays within the length it was told.
        unsafe { ambient_runtime::amb_word_at(word, index) }
    }

    fn definition(&mut self, word: Word) -> Word {
        // SAFETY: `decode` asks only about functions.
        unsafe { ambient_runtime::amb_closure_hash(word) }
    }

    fn string(&mut self, word: Word) -> Vec<u8> {
        // SAFETY: the word is a string, and its bytes are as long as its
        // length says.
        unsafe {
            let length = usize::try_from(ambient_runtime::amb_length(word)).unwrap_or(0);
            std::slice::from_raw_parts(ambient_runtime::amb_string_bytes(word), length).to_vec()
        }
    }
}

// SAFETY, throughout: each constructor is given what it documents: lengths
// that match the pointers, and indices within the lengths `encode` just
// asked for.
impl Construct for Host {
    fn int(&mut self, value: i64) -> Word {
        unsafe { ambient_runtime::amb_int_new(value) }
    }

    fn float(&mut self, bits: u64) -> Word {
        unsafe { ambient_runtime::amb_float_new(bits) }
    }

    fn string(&mut self, bytes: &[u8]) -> Word {
        unsafe { ambient_runtime::amb_string_new(bytes.as_ptr(), bytes.len() as u64) }
    }

    fn bytes(&mut self, bytes: &[u8]) -> Word {
        unsafe { ambient_runtime::amb_bytes_new(bytes.as_ptr(), bytes.len() as u64) }
    }

    fn key(&mut self, name: &[u8]) -> Word {
        unsafe { ambient_runtime::amb_intern(name.as_ptr(), name.len() as u64) }
    }

    fn tag(&mut self, text: &[u8]) -> Word {
        unsafe { ambient_runtime::amb_tag(text.as_ptr(), text.len() as u64) }
    }

    fn handle(&mut self, brand: &[u8], number: u64) -> Word {
        let brand = self.tag(brand);
        unsafe { ambient_runtime::amb_handle_new(brand, number) }
    }

    fn sequence(&mut self, list: bool, length: u64) -> Word {
        unsafe { ambient_runtime::amb_sequence_new(u64::from(list), length) }
    }

    fn iterator(&mut self, source: u64) -> Word {
        unsafe { ambient_runtime::amb_iterator_of(source) }
    }

    fn iterator_set(&mut self, iterator: Word, index: u64, value: Word) {
        unsafe { ambient_runtime::amb_iterator_hold(iterator, index, value) }
    }

    fn sequence_set(&mut self, sequence: Word, index: u64, value: Word) {
        unsafe { ambient_runtime::amb_sequence_set(sequence, index, value) }
    }

    fn record(&mut self, length: u64) -> Word {
        unsafe { ambient_runtime::amb_record_new(length) }
    }

    fn record_set(&mut self, record: Word, key: Word, value: Word) {
        unsafe { ambient_runtime::amb_record_set(record, key, value) }
    }

    fn release(&mut self, word: Word) {
        unsafe { ambient_runtime::amb_release(word) }
    }
}
