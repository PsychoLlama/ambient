//! VM error types for ability handlers.

use crate::value::Value;

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
    #[must_use]
    pub fn new(error: VmError) -> Self {
        Self {
            error,
            stack_trace: Vec::new(),
            source_context: None,
        }
    }

    /// Create a runtime error with a stack trace.
    #[must_use]
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

    /// A native (extern fn) whose UUID has no implementation registered on
    /// this VM. The loud link-time failure mode for shipped code that
    /// references a host function the executing host does not provide —
    /// never a silent misbehavior. Carries the canonical hyphenated UUID.
    UnboundNative { uuid: String },

    /// Unknown value-object hash (a `const` referenced by `LoadObject` that
    /// was never loaded into the VM).
    UnknownObject(blake3::Hash),

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
    UnhandledAbility {
        ability_id: ambient_core::AbilityId,
        method_id: u16,
    },

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

    /// A language-level exception.
    ///
    /// This is the raise channel for host handlers: returning
    /// `Err(VmError::Exception(value))` from a [`HostHandler`] makes the VM
    /// perform `Exception.throw(value)` at the call site, where the nearest
    /// in-language `handle { Exception.throw(e) => ... }` block catches it.
    /// The variant only surfaces as a hard error when no Exception handler
    /// is in scope — an uncaught exception.
    ///
    /// [`HostHandler`]: crate::HostHandler
    Exception(Value),

    /// I/O error (from Remote ability or other I/O operations).
    IoError(String),

    /// A mutex lock was poisoned.
    LockPoisoned,
}

impl VmError {
    /// Construct a catchable exception carrying a string error message.
    ///
    /// This is the common case for host handlers reporting fallible-operation
    /// failures (file not found, connection refused, ...).
    #[must_use]
    pub fn exception(message: impl Into<String>) -> Self {
        Self::Exception(Value::string(message))
    }
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
            Self::UnboundNative { uuid } => write!(
                f,
                "no native implementation bound for extern fn {uuid} on this host"
            ),
            Self::UnknownObject(hash) => write!(f, "unknown value object: {hash}"),
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
                    "unhandled ability: ability {}, method {method_id}",
                    ability_id.short_hex()
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
            Self::Exception(value) => {
                write!(
                    f,
                    "uncaught exception: {}",
                    crate::format::format_value(value)
                )
            }
            Self::IoError(msg) => write!(f, "I/O error: {msg}"),
            Self::LockPoisoned => write!(f, "mutex lock poisoned"),
        }
    }
}

impl std::error::Error for VmError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vm_error_display_stack_underflow() {
        let err = VmError::StackUnderflow;
        assert_eq!(format!("{err}"), "stack underflow");
    }

    #[test]
    fn test_vm_error_display_invalid_opcode() {
        let err = VmError::InvalidOpcode(0xff);
        assert_eq!(format!("{err}"), "invalid opcode: 0xff");
    }

    #[test]
    fn test_vm_error_display_type_error() {
        let err = VmError::TypeError {
            expected: "Number",
            got: "String",
            operation: "add",
        };
        assert_eq!(
            format!("{err}"),
            "type error in add: expected Number, got String"
        );
    }

    #[test]
    fn test_vm_error_display_type_error_owned() {
        let err = VmError::TypeErrorOwned {
            expected: "list".to_string(),
            got: "Number".to_string(),
        };
        assert_eq!(format!("{err}"), "type error: expected list, got Number");
    }

    #[test]
    fn test_vm_error_display_division_by_zero() {
        let err = VmError::DivisionByZero;
        assert_eq!(format!("{err}"), "division by zero");
    }

    #[test]
    fn test_vm_error_display_unknown_function() {
        let hash = blake3::hash(b"test");
        let err = VmError::UnknownFunction(hash);
        assert!(format!("{err}").contains("unknown function"));
    }

    #[test]
    fn test_vm_error_display_tuple_index_out_of_bounds() {
        let err = VmError::TupleIndexOutOfBounds {
            index: 5,
            length: 3,
        };
        assert_eq!(format!("{err}"), "tuple index 5 out of bounds (length 3)");
    }

    #[test]
    fn test_vm_error_display_record_field_not_found() {
        let err = VmError::RecordFieldNotFound("missing_field".to_string());
        assert_eq!(format!("{err}"), "record field not found: missing_field");
    }

    #[test]
    fn test_vm_error_display_arity_mismatch() {
        let err = VmError::ArityMismatch {
            expected: 2,
            got: 3,
        };
        assert_eq!(
            format!("{err}"),
            "arity mismatch: expected 2 arguments, got 3"
        );
    }

    #[test]
    fn test_vm_error_display_unhandled_ability() {
        let err = VmError::UnhandledAbility {
            ability_id: ambient_core::AbilityId::from_bytes([0xab; 32]),
            method_id: 2,
        };
        assert_eq!(
            format!("{err}"),
            "unhandled ability: ability abababababab, method 2"
        );
    }

    #[test]
    fn test_vm_error_display_continuation_already_resumed() {
        let err = VmError::ContinuationAlreadyResumed;
        assert_eq!(
            format!("{err}"),
            "continuation already resumed (single-shot violation)"
        );
    }

    #[test]
    fn test_vm_error_display_io_error() {
        let err = VmError::IoError("connection refused".to_string());
        assert_eq!(format!("{err}"), "I/O error: connection refused");
    }

    #[test]
    fn test_vm_error_equality() {
        assert_eq!(VmError::StackUnderflow, VmError::StackUnderflow);
        assert_eq!(VmError::DivisionByZero, VmError::DivisionByZero);
        assert_ne!(VmError::StackUnderflow, VmError::DivisionByZero);
    }

    #[test]
    fn test_stack_trace_frame_display_with_full_info() {
        let frame = StackTraceFrame {
            function_name: Some("my_function".to_string()),
            source_file: Some("main.ab".to_string()),
            line: Some(10),
            column: Some(5),
            function_hash: blake3::hash(b"test"),
            bytecode_offset: 42,
        };
        let display = format!("{frame}");
        assert!(display.contains("my_function"));
        assert!(display.contains("main.ab:10:5"));
    }

    #[test]
    fn test_stack_trace_frame_display_without_name() {
        let hash = blake3::hash(b"test");
        let frame = StackTraceFrame {
            function_name: None,
            source_file: Some("main.ab".to_string()),
            line: Some(10),
            column: None,
            function_hash: hash,
            bytecode_offset: 42,
        };
        let display = format!("{frame}");
        // Should show abbreviated hash
        assert!(display.contains(&hash.to_string()[..8]));
        assert!(display.contains("main.ab:10"));
    }

    #[test]
    fn test_runtime_error_new() {
        let err = RuntimeError::new(VmError::StackUnderflow);
        assert_eq!(err.error, VmError::StackUnderflow);
        assert!(err.stack_trace.is_empty());
        assert!(err.source_context.is_none());
    }

    #[test]
    fn test_runtime_error_with_stack_trace() {
        let frames = vec![StackTraceFrame {
            function_name: Some("test".to_string()),
            source_file: None,
            line: None,
            column: None,
            function_hash: blake3::hash(b"test"),
            bytecode_offset: 0,
        }];
        let err = RuntimeError::with_stack_trace(VmError::DivisionByZero, frames);
        assert_eq!(err.stack_trace.len(), 1);
    }

    #[test]
    fn test_runtime_error_with_source_context() {
        let err =
            RuntimeError::new(VmError::DivisionByZero).with_source_context("  x / 0".to_string());
        assert!(err.source_context.is_some());
    }

    #[test]
    fn test_runtime_error_display() {
        let frames = vec![StackTraceFrame {
            function_name: Some("divide".to_string()),
            source_file: Some("math.ab".to_string()),
            line: Some(5),
            column: Some(3),
            function_hash: blake3::hash(b"test"),
            bytecode_offset: 10,
        }];
        let err = RuntimeError::with_stack_trace(VmError::DivisionByZero, frames)
            .with_source_context("  let result = x / 0".to_string());
        let display = format!("{err}");
        assert!(display.contains("Runtime error"));
        assert!(display.contains("division by zero"));
        assert!(display.contains("Stack trace"));
        assert!(display.contains("divide"));
    }

    #[test]
    fn test_runtime_error_from_vm_error() {
        let vm_err = VmError::StackOverflow;
        let runtime_err: RuntimeError = vm_err.into();
        assert_eq!(runtime_err.error, VmError::StackOverflow);
    }
}
