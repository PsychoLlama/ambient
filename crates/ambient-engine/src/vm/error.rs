//! VM error types.

/// A single frame in a runtime stack trace.
#[derive(Debug, Clone)]
pub struct StackTraceFrame {
    /// Name of the function, if known from debug info.
    pub function_name: Option<String>,

    /// Path to the source file, if known.
    pub source_file: Option<String>,

    /// Line number in the source file (1-indexed).
    pub line: Option<u32>,

    /// Column number in the source file (1-indexed).
    pub column: Option<u32>,

    /// The function hash for identification.
    pub function_hash: blake3::Hash,

    /// Bytecode offset where the error occurred.
    pub bytecode_offset: usize,
}

impl std::fmt::Display for StackTraceFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Format: "  at function_name (file.ab:10:5)"
        write!(f, "  at ")?;

        if let Some(name) = &self.function_name {
            write!(f, "{name}")?;
        } else {
            // Show abbreviated hash if no name
            write!(f, "<{}>", &self.function_hash.to_string()[..8])?;
        }

        match (&self.source_file, self.line, self.column) {
            (Some(file), Some(line), Some(col)) => {
                write!(f, " ({file}:{line}:{col})")
            }
            (Some(file), Some(line), None) => {
                write!(f, " ({file}:{line})")
            }
            (Some(file), None, None) => {
                write!(f, " ({file})")
            }
            _ => Ok(()),
        }
    }
}

/// A runtime error with optional stack trace.
#[derive(Debug, Clone)]
pub struct RuntimeError {
    /// The underlying error.
    pub error: VmError,

    /// Stack trace at the point of the error.
    pub stack_trace: Vec<StackTraceFrame>,

    /// Source context for the error location (the line of code).
    pub source_context: Option<String>,
}

impl RuntimeError {
    /// Create a new runtime error without a stack trace.
    pub fn new(error: VmError) -> Self {
        Self {
            error,
            stack_trace: Vec::new(),
            source_context: None,
        }
    }

    /// Create a runtime error with a stack trace.
    pub fn with_stack_trace(error: VmError, stack_trace: Vec<StackTraceFrame>) -> Self {
        Self {
            error,
            stack_trace,
            source_context: None,
        }
    }

    /// Add source context to the error.
    #[must_use]
    pub fn with_source_context(mut self, context: String) -> Self {
        self.source_context = Some(context);
        self
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Runtime error: {}", self.error)?;

        if let Some(context) = &self.source_context {
            writeln!(f)?;
            writeln!(f, "{context}")?;
        }

        if !self.stack_trace.is_empty() {
            writeln!(f)?;
            writeln!(f, "Stack trace:")?;
            for frame in &self.stack_trace {
                writeln!(f, "{frame}")?;
            }
        }

        Ok(())
    }
}

impl std::error::Error for RuntimeError {}

impl From<VmError> for RuntimeError {
    fn from(error: VmError) -> Self {
        Self::new(error)
    }
}

/// Runtime error during VM execution.
#[derive(Debug, Clone, PartialEq)]
pub enum VmError {
    /// Stack underflow - tried to pop from empty stack.
    StackUnderflow,

    /// Invalid opcode encountered.
    InvalidOpcode(u8),

    /// Type mismatch for an operation (static strings, used in hot paths).
    TypeError {
        expected: &'static str,
        got: &'static str,
        operation: &'static str,
    },

    /// Type mismatch for an operation (owned strings, used in ability handlers).
    TypeErrorOwned { expected: String, got: String },

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

    /// Tried to extract payload from a unit enum variant.
    EnumPayloadMissing {
        type_name: String,
        variant_name: String,
    },

    /// Feature not yet implemented.
    Unsupported { operation: &'static str },

    /// I/O error (from Remote ability or other I/O operations).
    IoError(String),

    /// A mutex lock was poisoned.
    LockPoisoned,
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
            Self::TypeErrorOwned { expected, got } => {
                write!(f, "type error: expected {expected}, got {got}")
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
            Self::EnumPayloadMissing {
                type_name,
                variant_name,
            } => {
                write!(
                    f,
                    "attempted to extract payload from unit variant {type_name}::{variant_name}"
                )
            }
            Self::Unsupported { operation } => {
                write!(f, "unsupported operation: {operation}")
            }
            Self::IoError(msg) => write!(f, "I/O error: {msg}"),
            Self::LockPoisoned => write!(f, "mutex lock poisoned"),
        }
    }
}

impl std::error::Error for VmError {}
