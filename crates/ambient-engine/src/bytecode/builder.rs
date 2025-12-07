//! Bytecode builder for constructing bytecode sequences.
//!
//! Provides a convenient API for emitting instructions without manually
//! managing byte offsets. Automatically tracks function call dependencies.

use std::collections::HashMap;
use std::sync::Arc;

use super::debug::DebugInfo;
use super::opcode::Opcode;
use super::CompiledFunction;
use crate::value::Value;

/// A builder for constructing bytecode sequences.
///
/// Provides a convenient API for emitting instructions without manually
/// managing byte offsets. Automatically tracks function call dependencies.
#[derive(Debug, Default)]
pub struct BytecodeBuilder {
    code: Vec<u8>,
    constants: Vec<Value>,
    constant_map: HashMap<ConstantKey, u16>,
    /// Collected function dependencies (hashes of functions called).
    dependencies: Vec<blake3::Hash>,
}

/// Key for deduplicating constants in the constant pool.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ConstantKey {
    Number(u64), // Use bits for exact comparison
    String(Arc<String>),
    Bool(bool),
    Hash(blake3::Hash),
}

impl BytecodeBuilder {
    /// Create a new bytecode builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            code: Vec::new(),
            constants: Vec::new(),
            constant_map: HashMap::new(),
            dependencies: Vec::new(),
        }
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
            Value::String(s) => ConstantKey::String(Arc::clone(s)),
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
    ///
    /// The function hash is automatically tracked as a dependency.
    pub fn emit_call(&mut self, func_hash: blake3::Hash, arg_count: u8) {
        // Track this as a dependency
        if !self.dependencies.contains(&func_hash) {
            self.dependencies.push(func_hash);
        }

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

    /// Emit a `MakeClosure` instruction.
    ///
    /// Creates a closure from a function hash and captured values on the stack.
    pub fn emit_make_closure(&mut self, func_hash: blake3::Hash, capture_count: u8) {
        // Track the closure's function as a dependency
        if !self.dependencies.contains(&func_hash) {
            self.dependencies.push(func_hash);
        }

        let idx = self.add_constant(Value::FunctionRef(func_hash));
        self.code.push(Opcode::MakeClosure as u8);
        self.code.extend_from_slice(&idx.to_le_bytes());
        self.code.push(capture_count);
    }

    /// Emit a `CallClosure` instruction.
    ///
    /// Calls a closure on the stack with the given number of arguments.
    pub fn emit_call_closure(&mut self, arg_count: u8) {
        self.code.push(Opcode::CallClosure as u8);
        self.code.push(arg_count);
    }

    /// Emit a `MakeHandler` instruction.
    ///
    /// Creates a handler value from method implementations.
    /// Methods is a list of (`method_id`, `function_hash`) pairs.
    pub fn emit_make_handler(
        &mut self,
        ability_id: u16,
        methods: &[(u16, blake3::Hash)],
        capture_count: u8,
    ) {
        // Track method functions as dependencies
        for (_, func_hash) in methods {
            if !self.dependencies.contains(func_hash) {
                self.dependencies.push(*func_hash);
            }
        }

        self.code.push(Opcode::MakeHandler as u8);
        self.code.extend_from_slice(&ability_id.to_le_bytes());
        self.code.push(methods.len() as u8);
        self.code.push(capture_count);

        // Emit method mappings
        for (method_id, func_hash) in methods {
            let idx = self.add_constant(Value::FunctionRef(*func_hash));
            self.code.extend_from_slice(&method_id.to_le_bytes());
            self.code.extend_from_slice(&idx.to_le_bytes());
        }
    }

    /// Emit a `HandleWithValue` instruction.
    ///
    /// Expects a `HandlerValue` on the stack. Pops it and installs as the handler
    /// for the ability. Returns the offset for patching the normal completion jump.
    pub fn emit_handle_with_value(&mut self) -> usize {
        self.code.push(Opcode::HandleWithValue as u8);
        let jump_offset = self.code.len();
        self.code.extend_from_slice(&[0, 0]); // Placeholder for normal completion jump
        jump_offset
    }

    /// Patch the normal completion jump offset for a `HandleWithValue` instruction.
    pub fn patch_handle_with_value(&mut self, handle_jump_offset: usize) {
        let target = self.code.len();
        // The offset is from right after the HandleWithValue opcode
        let instruction_end = handle_jump_offset + 2; // After the i16 offset field
        let relative = (target as isize - instruction_end as isize) as i16;
        let bytes = relative.to_le_bytes();
        self.code[handle_jump_offset] = bytes[0];
        self.code[handle_jump_offset + 1] = bytes[1];
    }

    /// Emit a `LoadCapture` instruction.
    ///
    /// Loads a captured variable from the current closure's environment.
    pub fn emit_load_capture(&mut self, capture_slot: u16) {
        self.code.push(Opcode::LoadCapture as u8);
        self.code.extend_from_slice(&capture_slot.to_le_bytes());
    }

    /// Emit a `GetAbilityArg` instruction.
    ///
    /// Extracts an argument at the given index from a `SuspendedAbility` on the stack.
    pub fn emit_get_ability_arg(&mut self, arg_index: u8) {
        self.code.push(Opcode::GetAbilityArg as u8);
        self.code.push(arg_index);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // List operations (Milestone 15)
    // ─────────────────────────────────────────────────────────────────────────

    /// Emit a `MakeList` instruction.
    ///
    /// Creates a list from `count` values on the stack.
    pub fn emit_make_list(&mut self, count: u16) {
        self.code.push(Opcode::MakeList as u8);
        self.code.extend_from_slice(&count.to_le_bytes());
    }

    /// Emit a `ListGet` instruction.
    ///
    /// Pops a list and index, pushes the element at that index.
    pub fn emit_list_get(&mut self) {
        self.code.push(Opcode::ListGet as u8);
    }

    /// Emit a `ListLength` instruction.
    ///
    /// Pops a list and pushes its length.
    pub fn emit_list_length(&mut self) {
        self.code.push(Opcode::ListLength as u8);
    }

    /// Emit a `ListConcat` instruction.
    ///
    /// Pops two lists and pushes their concatenation.
    pub fn emit_list_concat(&mut self) {
        self.code.push(Opcode::ListConcat as u8);
    }

    /// Emit a `ListAppend` instruction.
    ///
    /// Pops a list and value, pushes a new list with the value appended.
    pub fn emit_list_append(&mut self) {
        self.code.push(Opcode::ListAppend as u8);
    }

    /// Emit a `ListHead` instruction.
    ///
    /// Pops a list and pushes the first element.
    pub fn emit_list_head(&mut self) {
        self.code.push(Opcode::ListHead as u8);
    }

    /// Emit a `ListTail` instruction.
    ///
    /// Pops a list and pushes a list without the first element.
    pub fn emit_list_tail(&mut self) {
        self.code.push(Opcode::ListTail as u8);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // String operations (Milestone 15)
    // ─────────────────────────────────────────────────────────────────────────

    /// Emit a `StringLength` instruction.
    pub fn emit_string_length(&mut self) {
        self.code.push(Opcode::StringLength as u8);
    }

    /// Emit a `StringSplit` instruction.
    pub fn emit_string_split(&mut self) {
        self.code.push(Opcode::StringSplit as u8);
    }

    /// Emit a `StringJoin` instruction.
    pub fn emit_string_join(&mut self) {
        self.code.push(Opcode::StringJoin as u8);
    }

    /// Emit a `StringTrim` instruction.
    pub fn emit_string_trim(&mut self) {
        self.code.push(Opcode::StringTrim as u8);
    }

    /// Emit a `StringContains` instruction.
    pub fn emit_string_contains(&mut self) {
        self.code.push(Opcode::StringContains as u8);
    }

    /// Emit a `StringConcat` instruction.
    pub fn emit_string_concat(&mut self) {
        self.code.push(Opcode::StringConcat as u8);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Type conversion (Milestone 15)
    // ─────────────────────────────────────────────────────────────────────────

    /// Emit a `ToString` instruction.
    pub fn emit_to_string(&mut self) {
        self.code.push(Opcode::ToString as u8);
    }

    /// Emit a `ParseNumber` instruction.
    pub fn emit_parse_number(&mut self) {
        self.code.push(Opcode::ParseNumber as u8);
    }

    /// Emit a `ParseBool` instruction.
    pub fn emit_parse_bool(&mut self) {
        self.code.push(Opcode::ParseBool as u8);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Set operations (Milestone 15)
    // ─────────────────────────────────────────────────────────────────────────

    /// Emit a `MakeEmptySet` instruction.
    ///
    /// Creates an empty set.
    pub fn emit_make_empty_set(&mut self) {
        self.code.push(Opcode::MakeEmptySet as u8);
    }

    /// Emit a `MakeSet` instruction.
    ///
    /// Creates a set from `count` values on the stack.
    pub fn emit_make_set(&mut self, count: u16) {
        self.code.push(Opcode::MakeSet as u8);
        self.code.extend_from_slice(&count.to_le_bytes());
    }

    /// Emit a `SetInsert` instruction.
    ///
    /// Pops a set and value, pushes a new set with the value inserted.
    pub fn emit_set_insert(&mut self) {
        self.code.push(Opcode::SetInsert as u8);
    }

    /// Emit a `SetRemove` instruction.
    ///
    /// Pops a set and value, pushes a new set with the value removed.
    pub fn emit_set_remove(&mut self) {
        self.code.push(Opcode::SetRemove as u8);
    }

    /// Emit a `SetContains` instruction.
    ///
    /// Pops a set and value, pushes a boolean.
    pub fn emit_set_contains(&mut self) {
        self.code.push(Opcode::SetContains as u8);
    }

    /// Emit a `SetLength` instruction.
    ///
    /// Pops a set and pushes its length.
    pub fn emit_set_length(&mut self) {
        self.code.push(Opcode::SetLength as u8);
    }

    /// Emit a `SetUnion` instruction.
    ///
    /// Pops two sets and pushes their union.
    pub fn emit_set_union(&mut self) {
        self.code.push(Opcode::SetUnion as u8);
    }

    /// Emit a `SetIntersection` instruction.
    ///
    /// Pops two sets and pushes their intersection.
    pub fn emit_set_intersection(&mut self) {
        self.code.push(Opcode::SetIntersection as u8);
    }

    /// Emit a `SetDifference` instruction.
    ///
    /// Pops two sets and pushes the difference (set1 - set2).
    pub fn emit_set_difference(&mut self) {
        self.code.push(Opcode::SetDifference as u8);
    }

    /// Emit a `SetToList` instruction.
    ///
    /// Pops a set and pushes it as a list.
    pub fn emit_set_to_list(&mut self) {
        self.code.push(Opcode::SetToList as u8);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Enum operations (Milestone 15 - Option/Result)
    // ─────────────────────────────────────────────────────────────────────────

    /// Emit a `MakeEnum` instruction.
    ///
    /// Creates an enum variant value. If `has_payload` is true, expects a payload
    /// value on the stack which will be consumed. Otherwise creates a unit variant.
    pub fn emit_make_enum(
        &mut self,
        type_name: &str,
        tag: u16,
        variant_name: &str,
        has_payload: bool,
    ) {
        let type_name_idx = self.add_constant(Value::string(type_name));
        let variant_name_idx = self.add_constant(Value::string(variant_name));
        self.code.push(Opcode::MakeEnum as u8);
        self.code.extend_from_slice(&type_name_idx.to_le_bytes());
        self.code.extend_from_slice(&tag.to_le_bytes());
        self.code.extend_from_slice(&variant_name_idx.to_le_bytes());
        self.code.push(u8::from(has_payload));
    }

    /// Emit a `EnumIs` instruction.
    ///
    /// Checks if the enum on top of stack matches the given tag.
    /// Does NOT consume the enum from the stack.
    pub fn emit_enum_is(&mut self, expected_tag: u16) {
        self.code.push(Opcode::EnumIs as u8);
        self.code.extend_from_slice(&expected_tag.to_le_bytes());
    }

    /// Emit a `EnumPayload` instruction.
    ///
    /// Extracts the payload from an enum value on the stack.
    pub fn emit_enum_payload(&mut self) {
        self.code.push(Opcode::EnumPayload as u8);
    }

    /// Emit a `EnumTag` instruction.
    ///
    /// Gets the tag (variant index) from an enum value as a number.
    pub fn emit_enum_tag(&mut self) {
        self.code.push(Opcode::EnumTag as u8);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Convenience methods for Option and Result
    // ─────────────────────────────────────────────────────────────────────────

    /// Emit code to create `Option::None`.
    pub fn emit_none(&mut self) {
        self.emit_make_enum("Option", 0, "None", false);
    }

    /// Emit code to create `Option::Some(value)`.
    /// Expects the payload value to already be on the stack.
    pub fn emit_some(&mut self) {
        self.emit_make_enum("Option", 1, "Some", true);
    }

    /// Emit code to create `Result::Ok(value)`.
    /// Expects the payload value to already be on the stack.
    pub fn emit_ok(&mut self) {
        self.emit_make_enum("Result", 0, "Ok", true);
    }

    /// Emit code to create `Result::Err(error)`.
    /// Expects the error value to already be on the stack.
    pub fn emit_err(&mut self) {
        self.emit_make_enum("Result", 1, "Err", true);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Option/Result utility operations
    // ─────────────────────────────────────────────────────────────────────────

    /// Emit `OptionUnwrapOr` instruction.
    /// Stack: `[option, default] -> [value]`
    pub fn emit_option_unwrap_or(&mut self) {
        self.code.push(Opcode::OptionUnwrapOr as u8);
    }

    // The following emit methods are defined but the VM operations are not yet
    // fully implemented (they require continuation frames for closure calls).

    /// Emit `OptionMap` instruction.
    /// Stack: `[option, closure] -> [option]`
    /// NOTE: Not yet implemented in VM.
    pub fn emit_option_map(&mut self) {
        self.code.push(Opcode::OptionMap as u8);
    }

    /// Emit `OptionAndThen` instruction.
    /// Stack: `[option, closure] -> [option]`
    /// NOTE: Not yet implemented in VM.
    pub fn emit_option_and_then(&mut self) {
        self.code.push(Opcode::OptionAndThen as u8);
    }

    /// Emit `ResultMap` instruction.
    /// Stack: `[result, closure] -> [result]`
    /// NOTE: Not yet implemented in VM.
    pub fn emit_result_map(&mut self) {
        self.code.push(Opcode::ResultMap as u8);
    }

    /// Emit `ResultMapErr` instruction.
    /// Stack: `[result, closure] -> [result]`
    /// NOTE: Not yet implemented in VM.
    pub fn emit_result_map_err(&mut self) {
        self.code.push(Opcode::ResultMapErr as u8);
    }

    /// Emit `ResultAndThen` instruction.
    /// Stack: `[result, closure] -> [result]`
    /// NOTE: Not yet implemented in VM.
    pub fn emit_result_and_then(&mut self) {
        self.code.push(Opcode::ResultAndThen as u8);
    }

    /// Build the final compiled function.
    ///
    /// Dependencies are automatically collected from `emit_call` invocations.
    #[must_use]
    pub fn build(self, local_count: u16, param_count: u8) -> CompiledFunction {
        CompiledFunction::with_dependencies(
            self.code,
            self.constants,
            local_count,
            param_count,
            self.dependencies,
        )
    }

    /// Build the final compiled function with explicit dependencies.
    ///
    /// This overrides the automatically collected dependencies.
    #[must_use]
    pub fn build_with_dependencies(
        self,
        local_count: u16,
        param_count: u8,
        dependencies: Vec<blake3::Hash>,
    ) -> CompiledFunction {
        CompiledFunction::with_dependencies(
            self.code,
            self.constants,
            local_count,
            param_count,
            dependencies,
        )
    }

    /// Build the final compiled function with debug information.
    #[must_use]
    pub fn build_with_debug_info(
        self,
        local_count: u16,
        param_count: u8,
        debug_info: DebugInfo,
    ) -> CompiledFunction {
        CompiledFunction::with_debug_info(
            self.code,
            self.constants,
            local_count,
            param_count,
            self.dependencies,
            debug_info,
        )
    }

    /// Get the collected dependencies.
    #[must_use]
    pub fn dependencies(&self) -> &[blake3::Hash] {
        &self.dependencies
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
