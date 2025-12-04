//! VM error types.

/// Runtime error during VM execution.
#[derive(Debug, Clone, PartialEq)]
pub enum VmError {
    /// Stack underflow - tried to pop from empty stack.
    StackUnderflow,

    /// Invalid opcode encountered.
    InvalidOpcode(u8),

    /// Type mismatch for an operation.
    TypeError {
        expected: &'static str,
        got: &'static str,
        operation: &'static str,
    },

    /// Division by zero.
    DivisionByZero,

    /// Invalid constant pool index.
    InvalidConstant(u16),

    /// Invalid local variable slot.
    InvalidLocal(u16),

    /// Unknown function hash.
    UnknownFunction(blake3::Hash),

    /// Tuple index out of bounds.
    TupleIndexOutOfBounds { index: u8, length: usize },

    /// Record field not found.
    RecordFieldNotFound(String),

    /// Instruction pointer out of bounds.
    InstructionOutOfBounds,

    /// Call stack overflow.
    StackOverflow,

    /// Wrong number of arguments to function.
    ArityMismatch { expected: u8, got: u8 },

    /// No handler found for an ability.
    UnhandledAbility { ability_id: u16, method_id: u16 },

    /// Tried to resume an already-resumed continuation (single-shot violation).
    ContinuationAlreadyResumed,

    /// Expected a suspended ability value but got something else.
    ExpectedSuspendedAbility { got: &'static str },

    /// Expected a continuation but got something else.
    ExpectedContinuation { got: &'static str },

    /// Ability argument index out of bounds.
    AbilityArgOutOfBounds { index: usize, length: usize },
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StackUnderflow => write!(f, "stack underflow"),
            Self::InvalidOpcode(op) => write!(f, "invalid opcode: 0x{op:02x}"),
            Self::TypeError {
                expected,
                got,
                operation,
            } => {
                write!(
                    f,
                    "type error in {operation}: expected {expected}, got {got}"
                )
            }
            Self::DivisionByZero => write!(f, "division by zero"),
            Self::InvalidConstant(idx) => write!(f, "invalid constant index: {idx}"),
            Self::InvalidLocal(slot) => write!(f, "invalid local variable slot: {slot}"),
            Self::UnknownFunction(hash) => write!(f, "unknown function: {hash}"),
            Self::TupleIndexOutOfBounds { index, length } => {
                write!(f, "tuple index {index} out of bounds (length {length})")
            }
            Self::RecordFieldNotFound(field) => write!(f, "record field not found: {field}"),
            Self::InstructionOutOfBounds => write!(f, "instruction pointer out of bounds"),
            Self::StackOverflow => write!(f, "call stack overflow"),
            Self::ArityMismatch { expected, got } => {
                write!(
                    f,
                    "arity mismatch: expected {expected} arguments, got {got}"
                )
            }
            Self::UnhandledAbility {
                ability_id,
                method_id,
            } => {
                write!(
                    f,
                    "unhandled ability: ability {ability_id}, method {method_id}"
                )
            }
            Self::ContinuationAlreadyResumed => {
                write!(f, "continuation already resumed (single-shot violation)")
            }
            Self::ExpectedSuspendedAbility { got } => {
                write!(f, "expected suspended ability, got {got}")
            }
            Self::ExpectedContinuation { got } => {
                write!(f, "expected continuation, got {got}")
            }
            Self::AbilityArgOutOfBounds { index, length } => {
                write!(
                    f,
                    "ability argument index {index} out of bounds (length {length})"
                )
            }
        }
    }
}

impl std::error::Error for VmError {}
