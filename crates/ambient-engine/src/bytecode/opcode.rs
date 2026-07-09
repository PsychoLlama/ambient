//! Bytecode opcodes for the Ambient VM.
//!
//! Instructions are encoded as a single byte opcode followed by operands
//! specific to each instruction. Operand sizes are documented for each variant.

/// Bytecode opcodes for the Ambient VM.
///
/// Instructions are encoded as a single byte opcode followed by operands specific
/// to each instruction. Operand sizes are documented for each variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    // ─────────────────────────────────────────────────────────────────────────
    // Stack operations
    // ─────────────────────────────────────────────────────────────────────────
    /// Push a constant from the constant pool onto the stack.
    /// Operand: u16 (constant pool index)
    PushConst = 0x00,

    /// Pop and discard the top value from the stack.
    Pop = 0x01,

    /// Duplicate the top value on the stack.
    Dup = 0x02,

    /// Push a content-addressed `const` value onto the stack.
    /// Operand: u16 (constant pool index holding a `Value::ObjectRef(hash)`)
    ///
    /// Resolves the object hash against the VM's value objects and pushes a
    /// clone of the stored value. Mirrors how `Call` reads a
    /// `Value::FunctionRef` from the pool.
    LoadObject = 0x03,

    // ─────────────────────────────────────────────────────────────────────────
    // Local variables
    // ─────────────────────────────────────────────────────────────────────────
    /// Store top of stack into a local variable slot. Does not pop.
    /// Operand: u16 (local slot index)
    StoreLocal = 0x10,

    /// Load a local variable onto the stack.
    /// Operand: u16 (local slot index)
    LoadLocal = 0x11,

    // ─────────────────────────────────────────────────────────────────────────
    // Arithmetic (number operands only)
    // ─────────────────────────────────────────────────────────────────────────
    /// Add two numbers.
    Add = 0x20,

    /// Subtract top from second.
    Sub = 0x21,

    /// Multiply two numbers.
    Mul = 0x22,

    /// Divide second by top.
    Div = 0x23,

    /// Modulo second by top.
    Mod = 0x24,

    /// Negate top of stack.
    Neg = 0x25,

    // ─────────────────────────────────────────────────────────────────────────
    // Comparison (number operands)
    // ─────────────────────────────────────────────────────────────────────────
    /// Test equality.
    Eq = 0x30,

    /// Test inequality.
    Ne = 0x31,

    /// Test less than.
    Lt = 0x32,

    /// Test less or equal.
    Le = 0x33,

    /// Test greater than.
    Gt = 0x34,

    /// Test greater or equal.
    Ge = 0x35,

    // ─────────────────────────────────────────────────────────────────────────
    // Logic (bool operands only)
    // ─────────────────────────────────────────────────────────────────────────
    /// Logical AND.
    And = 0x40,

    /// Logical OR.
    Or = 0x41,

    /// Logical NOT.
    Not = 0x42,

    // ─────────────────────────────────────────────────────────────────────────
    // Control flow
    // ─────────────────────────────────────────────────────────────────────────
    /// Unconditional jump.
    /// Operand: i16 (signed offset from current instruction)
    Jump = 0x50,

    /// Jump if top of stack is true (pops the condition).
    /// Operand: i16 (signed offset)
    JumpIf = 0x51,

    /// Jump if top of stack is false (pops the condition).
    /// Operand: i16 (signed offset)
    JumpIfNot = 0x52,

    // ─────────────────────────────────────────────────────────────────────────
    // Functions
    // ─────────────────────────────────────────────────────────────────────────
    /// Call a function by hash. Arguments should be on the stack.
    /// Operand: u16 (constant pool index containing the function hash)
    /// Operand: u8 (argument count)
    Call = 0x60,

    /// Return from the current function. Top of stack is the return value.
    Return = 0x61,

    // ─────────────────────────────────────────────────────────────────────────
    // Data structures
    // ─────────────────────────────────────────────────────────────────────────
    /// Create a tuple from N values on the stack.
    /// Operand: u8 (arity - number of elements)
    MakeTuple = 0x70,

    /// Get an element from a tuple.
    /// Operand: u8 (element index)
    TupleGet = 0x71,

    /// Create a record from N field-value pairs on the stack.
    /// Fields are pushed as string constants, then values.
    /// Operand: u8 (field count)
    MakeRecord = 0x72,

    /// Get a field from a record.
    /// Operand: u16 (constant pool index for field name string)
    RecordGet = 0x73,

    // ─────────────────────────────────────────────────────────────────────────
    // Abilities (Milestone 2)
    // ─────────────────────────────────────────────────────────────────────────
    /// Create a suspended ability value from arguments on the stack.
    /// Operand: u16 (constant pool index of the ability-method reference)
    /// Operand: u8 (argument count)
    ///
    /// Pops `arg_count` arguments from the stack and creates a `SuspendedAbility` value.
    Suspend = 0x80,

    /// Perform a suspended ability value.
    ///
    /// Pops a `SuspendedAbility` from the stack, looks up the nearest handler,
    /// captures the continuation, and jumps to the handler code.
    Perform = 0x81,

    /// Remove the most recent ability handler.
    ///
    /// Called when exiting a handled region normally (not via ability performance).
    Unhandle = 0x83,

    /// Resume a suspended continuation with a value.
    ///
    /// Pops a continuation and a value from the stack. Restores the continuation's
    /// stack and frames, then pushes the value as the result of the Perform.
    /// Single-shot: errors if continuation was already resumed.
    Resume = 0x84,

    /// Get an argument from a suspended ability value.
    /// Operand: u8 (argument index)
    ///
    /// Pops a `SuspendedAbility` from the stack and pushes the argument at the given index.
    /// Used in handler functions to extract ability method arguments.
    GetAbilityArg = 0x85,

    // ─────────────────────────────────────────────────────────────────────────
    // Closures
    // ─────────────────────────────────────────────────────────────────────────
    /// Create a closure from a function and captured variables.
    /// Operand: u16 (constant pool index for function hash)
    /// Operand: u8 (capture count - number of values to capture from stack)
    ///
    /// Pops `capture_count` values from the stack and creates a closure value
    /// combining the function with the captured environment.
    MakeClosure = 0xA0,

    /// Call a closure on the stack.
    /// Operand: u8 (argument count)
    ///
    /// Stack: [closure, arg1, arg2, ..., argN] -> [result]
    /// The closure is popped first, then arguments. The closure's captured
    /// environment is prepended to the arguments when calling the function.
    CallClosure = 0xA1,

    /// Load a captured variable from the closure environment.
    /// Operand: u16 (capture slot index)
    ///
    /// Loads a value from the current closure's captured environment.
    /// Only valid inside a closure body.
    LoadCapture = 0xA2,

    // ─────────────────────────────────────────────────────────────────────────
    // Handler literals (Milestone 13)
    // ─────────────────────────────────────────────────────────────────────────
    /// Create a handler value from method implementations.
    /// Operand: u16 (constant pool index of the ability reference)
    /// Operand: u8 (method count)
    /// Operand: u8 (capture count - values to capture from stack)
    ///
    /// Following the operands, `method_count` pairs of:
    ///   - u16 (constant pool index of the arm's ability-method reference)
    ///   - u16 (constant pool index for the arm's function hash)
    ///
    /// Pops `capture_count` values from the stack (captures), then pushes
    /// a `HandlerValue` keyed by derived method keys.
    MakeHandler = 0xB0,

    /// Install a handler from a `HandlerValue` on the stack.
    /// Operand: i16 (offset to jump to after handled expression completes normally)
    ///
    /// Pops a `HandlerValue` from the stack and installs it as the current handler
    /// for the ability. When an ability operation is performed, the handler's
    /// method functions will be called based on the method ID.
    HandleWithValue = 0xB1,

    // ─────────────────────────────────────────────────────────────────────────
    // Lists (Milestone 15 - Standard Library)
    // ─────────────────────────────────────────────────────────────────────────
    /// Create a list from N values on the stack.
    /// Operand: u16 (number of elements)
    ///
    /// Pops N values from the stack and creates a List value.
    MakeList = 0xC0,

    /// Create a set from N values on the stack.
    /// Operand: u16 (number of elements)
    ///
    /// Stack: `[v1, v2, ..., vN] -> [set]`
    MakeSet = 0xF1,

    // ─────────────────────────────────────────────────────────────────────────
    // Enum operations (Milestone 15 - Option/Result)
    // ─────────────────────────────────────────────────────────────────────────
    /// Create an enum variant value.
    /// Operand: u16 (constant pool index for type name string)
    /// Operand: u16 (variant tag)
    /// Operand: u16 (constant pool index for variant name string)
    /// Operand: u8 (1 if has payload, 0 if unit variant)
    ///
    /// Stack (with payload): `[payload] -> [enum_value]`
    /// Stack (unit variant): `[] -> [enum_value]`
    MakeEnum = 0xFA,

    /// Check if an enum value matches a specific variant tag.
    /// Operand: u16 (expected variant tag)
    ///
    /// Stack: `[enum_value] -> [bool]`
    /// Does NOT consume the enum value from the stack.
    EnumIs = 0xFB,

    /// Extract the payload from an enum value.
    /// The enum must have a payload (not a unit variant).
    ///
    /// Stack: `[enum_value] -> [payload]`
    /// Errors if the enum is a unit variant.
    EnumPayload = 0xFC,

    // ─────────────────────────────────────────────────────────────────────────
    // Special
    // ─────────────────────────────────────────────────────────────────────────
    /// Halt execution (end of program).
    Halt = 0xFF,
}

impl Opcode {
    /// Decode an opcode from a byte. Returns None for invalid opcodes.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x00 => Some(Self::PushConst),
            0x01 => Some(Self::Pop),
            0x02 => Some(Self::Dup),
            0x03 => Some(Self::LoadObject),
            0x10 => Some(Self::StoreLocal),
            0x11 => Some(Self::LoadLocal),
            0x20 => Some(Self::Add),
            0x21 => Some(Self::Sub),
            0x22 => Some(Self::Mul),
            0x23 => Some(Self::Div),
            0x24 => Some(Self::Mod),
            0x25 => Some(Self::Neg),
            // Math functions
            0x30 => Some(Self::Eq),
            0x31 => Some(Self::Ne),
            0x32 => Some(Self::Lt),
            0x33 => Some(Self::Le),
            0x34 => Some(Self::Gt),
            0x35 => Some(Self::Ge),
            0x40 => Some(Self::And),
            0x41 => Some(Self::Or),
            0x42 => Some(Self::Not),
            0x50 => Some(Self::Jump),
            0x51 => Some(Self::JumpIf),
            0x52 => Some(Self::JumpIfNot),
            0x60 => Some(Self::Call),
            0x61 => Some(Self::Return),
            0x70 => Some(Self::MakeTuple),
            0x71 => Some(Self::TupleGet),
            0x72 => Some(Self::MakeRecord),
            0x73 => Some(Self::RecordGet),
            // Abilities
            0x80 => Some(Self::Suspend),
            0x81 => Some(Self::Perform),
            0x83 => Some(Self::Unhandle),
            0x84 => Some(Self::Resume),
            0x85 => Some(Self::GetAbilityArg),
            // Closures
            0xA0 => Some(Self::MakeClosure),
            0xA1 => Some(Self::CallClosure),
            0xA2 => Some(Self::LoadCapture),
            // Handler literals
            0xB0 => Some(Self::MakeHandler),
            0xB1 => Some(Self::HandleWithValue),
            // Lists
            0xC0 => Some(Self::MakeList),
            // Strings
            // Type conversion
            // Maps
            // Sets
            0xF1 => Some(Self::MakeSet),
            // Enums
            0xFA => Some(Self::MakeEnum),
            0xFB => Some(Self::EnumIs),
            0xFC => Some(Self::EnumPayload),
            // Serialization
            // Binary operations
            0xFF => Some(Self::Halt),
            _ => None,
        }
    }
}
