/// Represents a value in the language.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Value {
    Boolean(bool),
    Int32(i32),
    Reference(blake3::Hash),
}
