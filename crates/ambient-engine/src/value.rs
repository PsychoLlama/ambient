/// Represents a value in the language.
#[derive(Debug, PartialEq, Eq)]
pub enum Value {
    Boolean(bool),
    Int32(i32),
}
