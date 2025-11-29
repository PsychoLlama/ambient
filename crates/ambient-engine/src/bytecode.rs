//! Bytecode representation and instruction set for the Ambient VM.
//!
//! This module defines the bytecode format that the VM executes. Instructions are
//! encoded as opcodes followed by their operands.

#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]

use std::collections::HashMap;
use std::rc::Rc;

use crate::value::Value;

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
    /// Operand: u16 (ability ID)
    /// Operand: u16 (method ID)
    /// Operand: u8 (argument count)
    ///
    /// Pops `arg_count` arguments from the stack and creates a `SuspendedAbility` value.
    Suspend = 0x80,

    /// Perform a suspended ability value.
    ///
    /// Pops a `SuspendedAbility` from the stack, looks up the nearest handler,
    /// captures the continuation, and jumps to the handler code.
    Perform = 0x81,

    /// Install an ability handler and mark a handler boundary.
    /// Operand: u16 (ability ID to handle)
    /// Operand: u16 (handler function index in constant pool)
    /// Operand: i16 (offset to jump to after handled expression completes normally)
    ///
    /// This marks the start of a handled region. When an ability with matching ID
    /// is performed, control transfers to the handler function.
    Handle = 0x82,

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

    // ─────────────────────────────────────────────────────────────────────────
    // Special
    // ─────────────────────────────────────────────────────────────────────────
    /// Halt execution (end of program).
    Halt = 0xFF,
}

impl Opcode {
    /// Decode an opcode from a byte. Returns None for invalid opcodes.
    #[must_use]
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x00 => Some(Self::PushConst),
            0x01 => Some(Self::Pop),
            0x02 => Some(Self::Dup),
            0x10 => Some(Self::StoreLocal),
            0x11 => Some(Self::LoadLocal),
            0x20 => Some(Self::Add),
            0x21 => Some(Self::Sub),
            0x22 => Some(Self::Mul),
            0x23 => Some(Self::Div),
            0x24 => Some(Self::Mod),
            0x25 => Some(Self::Neg),
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
            0x82 => Some(Self::Handle),
            0x83 => Some(Self::Unhandle),
            0x84 => Some(Self::Resume),
            0xFF => Some(Self::Halt),
            _ => None,
        }
    }
}

/// A compiled function ready for execution.
#[derive(Debug, Clone)]
pub struct CompiledFunction {
    /// Unique content-addressed hash for this function.
    pub hash: blake3::Hash,

    /// The bytecode instructions.
    pub bytecode: Vec<u8>,

    /// Constant pool for this function (numbers, strings, function hashes).
    pub constants: Vec<Value>,

    /// Number of local variable slots needed.
    pub local_count: u16,

    /// Number of parameters this function takes.
    pub param_count: u8,

    /// Hashes of functions this one calls (dependencies).
    pub dependencies: Vec<blake3::Hash>,
}

impl CompiledFunction {
    /// Create a new compiled function with the given bytecode and constants.
    #[must_use]
    pub fn new(bytecode: Vec<u8>, constants: Vec<Value>, local_count: u16, param_count: u8) -> Self {
        // Compute hash from bytecode and constants
        let hash = Self::compute_hash(&bytecode, &constants);
        Self {
            hash,
            bytecode,
            constants,
            local_count,
            param_count,
            dependencies: Vec::new(),
        }
    }

    /// Compute the content hash for this function.
    fn compute_hash(bytecode: &[u8], constants: &[Value]) -> blake3::Hash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(bytecode);
        // For now, just hash the debug representation of constants
        // A proper implementation would have a stable serialization format
        for constant in constants {
            hasher.update(format!("{constant:?}").as_bytes());
        }
        hasher.finalize()
    }
}

/// A builder for constructing bytecode sequences.
///
/// Provides a convenient API for emitting instructions without manually
/// managing byte offsets.
#[derive(Debug, Default)]
pub struct BytecodeBuilder {
    code: Vec<u8>,
    constants: Vec<Value>,
    constant_map: HashMap<ConstantKey, u16>,
}

/// Key for deduplicating constants in the constant pool.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ConstantKey {
    Number(u64), // Use bits for exact comparison
    String(Rc<String>),
    Bool(bool),
    Hash(blake3::Hash),
}

impl BytecodeBuilder {
    /// Create a new bytecode builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the current bytecode offset (for jump targets).
    #[must_use]
    pub fn current_offset(&self) -> usize {
        self.code.len()
    }

    /// Add a constant to the pool and return its index.
    /// Deduplicates identical constants.
    pub fn add_constant(&mut self, value: Value) -> u16 {
        let key = match &value {
            Value::Number(n) => ConstantKey::Number(n.to_bits()),
            Value::String(s) => ConstantKey::String(Rc::clone(s)),
            Value::Bool(b) => ConstantKey::Bool(*b),
            Value::FunctionRef(h) => ConstantKey::Hash(*h),
            // For complex types, don't deduplicate (they're rare as constants)
            _ => {
                let idx = self.constants.len() as u16;
                self.constants.push(value);
                return idx;
            }
        };

        if let Some(&idx) = self.constant_map.get(&key) {
            idx
        } else {
            let idx = self.constants.len() as u16;
            self.constants.push(value);
            self.constant_map.insert(key, idx);
            idx
        }
    }

    /// Emit a single-byte opcode.
    pub fn emit(&mut self, op: Opcode) {
        self.code.push(op as u8);
    }

    /// Emit an opcode with a u8 operand.
    pub fn emit_u8(&mut self, op: Opcode, operand: u8) {
        self.code.push(op as u8);
        self.code.push(operand);
    }

    /// Emit an opcode with a u16 operand (little-endian).
    pub fn emit_u16(&mut self, op: Opcode, operand: u16) {
        self.code.push(op as u8);
        self.code.extend_from_slice(&operand.to_le_bytes());
    }

    /// Emit an opcode with an i16 operand (little-endian).
    pub fn emit_i16(&mut self, op: Opcode, operand: i16) {
        self.code.push(op as u8);
        self.code.extend_from_slice(&operand.to_le_bytes());
    }

    /// Emit a push constant instruction, automatically adding to constant pool.
    pub fn emit_const(&mut self, value: Value) {
        let idx = self.add_constant(value);
        self.emit_u16(Opcode::PushConst, idx);
    }

    /// Emit a Call instruction.
    pub fn emit_call(&mut self, func_hash: blake3::Hash, arg_count: u8) {
        let idx = self.add_constant(Value::FunctionRef(func_hash));
        self.code.push(Opcode::Call as u8);
        self.code.extend_from_slice(&idx.to_le_bytes());
        self.code.push(arg_count);
    }

    /// Emit a placeholder jump and return its offset for later patching.
    pub fn emit_jump_placeholder(&mut self, op: Opcode) -> usize {
        let offset = self.code.len();
        self.code.push(op as u8);
        self.code.extend_from_slice(&[0, 0]); // Placeholder
        offset
    }

    /// Patch a previously emitted jump instruction with the correct offset.
    pub fn patch_jump(&mut self, jump_offset: usize) {
        let target = self.code.len();
        let relative = (target as isize - jump_offset as isize - 3) as i16;
        let bytes = relative.to_le_bytes();
        self.code[jump_offset + 1] = bytes[0];
        self.code[jump_offset + 2] = bytes[1];
    }

    /// Emit a Suspend instruction to create a suspended ability value.
    pub fn emit_suspend(&mut self, ability_id: u16, method_id: u16, arg_count: u8) {
        self.code.push(Opcode::Suspend as u8);
        self.code.extend_from_slice(&ability_id.to_le_bytes());
        self.code.extend_from_slice(&method_id.to_le_bytes());
        self.code.push(arg_count);
    }

    /// Emit a Handle instruction to install an ability handler.
    /// Returns the offset for patching the normal completion jump.
    pub fn emit_handle(&mut self, ability_id: u16, handler_func: blake3::Hash) -> usize {
        let handler_idx = self.add_constant(Value::FunctionRef(handler_func));
        self.code.push(Opcode::Handle as u8);
        self.code.extend_from_slice(&ability_id.to_le_bytes());
        self.code.extend_from_slice(&handler_idx.to_le_bytes());
        let jump_offset = self.code.len();
        self.code.extend_from_slice(&[0, 0]); // Placeholder for normal completion jump
        jump_offset
    }

    /// Patch the normal completion jump offset for a Handle instruction.
    pub fn patch_handle(&mut self, handle_jump_offset: usize) {
        let target = self.code.len();
        // The offset is from the end of the Handle instruction
        let handle_start = handle_jump_offset - 4; // Back to ability_id start
        let relative = (target as isize - handle_start as isize - 7) as i16;
        let bytes = relative.to_le_bytes();
        self.code[handle_jump_offset] = bytes[0];
        self.code[handle_jump_offset + 1] = bytes[1];
    }

    /// Build the final compiled function.
    #[must_use]
    pub fn build(self, local_count: u16, param_count: u8) -> CompiledFunction {
        CompiledFunction::new(self.code, self.constants, local_count, param_count)
    }

    /// Get the raw bytecode (for testing).
    #[must_use]
    pub fn bytecode(&self) -> &[u8] {
        &self.code
    }

    /// Get the constants (for testing).
    #[must_use]
    pub fn constants(&self) -> &[Value] {
        &self.constants
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opcode_roundtrip() {
        let opcodes = [
            Opcode::PushConst,
            Opcode::Pop,
            Opcode::Dup,
            Opcode::StoreLocal,
            Opcode::LoadLocal,
            Opcode::Add,
            Opcode::Sub,
            Opcode::Mul,
            Opcode::Div,
            Opcode::Mod,
            Opcode::Neg,
            Opcode::Eq,
            Opcode::Ne,
            Opcode::Lt,
            Opcode::Le,
            Opcode::Gt,
            Opcode::Ge,
            Opcode::And,
            Opcode::Or,
            Opcode::Not,
            Opcode::Jump,
            Opcode::JumpIf,
            Opcode::JumpIfNot,
            Opcode::Call,
            Opcode::Return,
            Opcode::MakeTuple,
            Opcode::TupleGet,
            Opcode::MakeRecord,
            Opcode::RecordGet,
            // Abilities
            Opcode::Suspend,
            Opcode::Perform,
            Opcode::Handle,
            Opcode::Unhandle,
            Opcode::Resume,
            Opcode::Halt,
        ];

        for op in opcodes {
            let byte = op as u8;
            let decoded = Opcode::from_byte(byte);
            assert_eq!(decoded, Some(op), "Failed roundtrip for {op:?}");
        }
    }

    #[test]
    fn test_invalid_opcode() {
        assert_eq!(Opcode::from_byte(0xFE), None);
        assert_eq!(Opcode::from_byte(0x99), None);
    }

    #[test]
    fn test_bytecode_builder_emit() {
        let mut builder = BytecodeBuilder::new();
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);

        assert_eq!(builder.bytecode(), &[Opcode::Add as u8, Opcode::Return as u8]);
    }

    #[test]
    fn test_bytecode_builder_constants() {
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(42.0));
        builder.emit_const(Value::Number(42.0)); // Deduplicated

        // Should only have one constant
        assert_eq!(builder.constants().len(), 1);
        assert_eq!(builder.constants()[0], Value::Number(42.0));
    }

    #[test]
    fn test_bytecode_builder_emit_u16() {
        let mut builder = BytecodeBuilder::new();
        builder.emit_u16(Opcode::LoadLocal, 0x1234);

        assert_eq!(builder.bytecode(), &[Opcode::LoadLocal as u8, 0x34, 0x12]);
    }

    #[test]
    fn test_jump_patching() {
        let mut builder = BytecodeBuilder::new();
        let jump_offset = builder.emit_jump_placeholder(Opcode::JumpIfNot);
        builder.emit(Opcode::Pop);
        builder.emit(Opcode::Pop);
        builder.patch_jump(jump_offset);

        // Jump should skip the two Pop instructions
        // Offset is calculated from after the jump instruction (3 bytes)
        let expected_offset: i16 = 2; // 2 bytes of Pop instructions
        let bytes = expected_offset.to_le_bytes();
        assert_eq!(builder.bytecode()[1], bytes[0]);
        assert_eq!(builder.bytecode()[2], bytes[1]);
    }
}
