//! Bytecode representation and instruction set for the Ambient VM.
//!
//! This module defines the bytecode format that the VM executes. Instructions are
//! encoded as opcodes followed by their operands.
//!
//! # Module Organization
//!
//! - `opcode` - Bytecode opcode definitions
//! - `builder` - API for constructing bytecode sequences
//! - `debug` - Debug information for source mapping

#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]

mod builder;
mod debug;
mod disasm;
mod opcode;

pub use builder::BytecodeBuilder;
pub use debug::{DebugInfo, SourceMapping};
pub use disasm::{MethodRefSites, disassemble, method_ref_sites};
pub use opcode::Opcode;

use std::collections::HashMap;

use ambient_core::MethodKey;

use crate::value::Value;

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

    /// Debug information for error messages and stack traces.
    ///
    /// This is optional and only generated when debug info is requested.
    /// It does NOT affect the function's content hash, so functions with
    /// and without debug info are considered equivalent.
    pub debug_info: Option<DebugInfo>,

    /// Precomputed [`MethodKey`]s for every `AbilityMethodRef` constant,
    /// keyed by its constant-pool index.
    ///
    /// A [`MethodKey`] is a blake3 hash of an ability method's constant-pool
    /// inputs; those inputs are fixed once the function is compiled, so the
    /// key is derivable at load time rather than on every `Suspend`/perform
    /// and `MakeHandler` arm. This is a derived, in-memory-only cache built
    /// by [`Self::index_method_keys`] at every construction site — it is
    /// NOT part of the function's identity: it is excluded from
    /// [`Self::compute_hash`] and never encoded into the object store (see
    /// `object::function_from_compiled`). It always agrees with `constants`
    /// because it is rebuilt from them, never edited independently.
    pub method_keys: HashMap<u16, MethodKey>,
}

impl CompiledFunction {
    /// Create a new compiled function with the given bytecode and constants.
    #[must_use]
    pub fn new(
        bytecode: Vec<u8>,
        constants: Vec<Value>,
        local_count: u16,
        param_count: u8,
    ) -> Self {
        Self::with_dependencies(bytecode, constants, local_count, param_count, Vec::new())
    }

    /// Create a new compiled function with explicit dependencies.
    #[must_use]
    pub fn with_dependencies(
        bytecode: Vec<u8>,
        constants: Vec<Value>,
        local_count: u16,
        param_count: u8,
        dependencies: Vec<blake3::Hash>,
    ) -> Self {
        // Compute hash from bytecode, constants, and function metadata
        let hash = Self::compute_hash(
            &bytecode,
            &constants,
            local_count,
            param_count,
            &dependencies,
        );
        let method_keys = Self::index_method_keys(&constants);
        Self {
            hash,
            bytecode,
            constants,
            local_count,
            param_count,
            dependencies,
            debug_info: None,
            method_keys,
        }
    }

    /// Create a new compiled function with debug information.
    #[must_use]
    pub fn with_debug_info(
        bytecode: Vec<u8>,
        constants: Vec<Value>,
        local_count: u16,
        param_count: u8,
        dependencies: Vec<blake3::Hash>,
        debug_info: DebugInfo,
    ) -> Self {
        let hash = Self::compute_hash(
            &bytecode,
            &constants,
            local_count,
            param_count,
            &dependencies,
        );
        let method_keys = Self::index_method_keys(&constants);
        Self {
            hash,
            bytecode,
            constants,
            local_count,
            param_count,
            dependencies,
            debug_info: Some(debug_info),
            method_keys,
        }
    }

    /// Attach debug information to this function.
    ///
    /// This creates a new function with the same hash but with debug info attached.
    #[must_use]
    pub fn attach_debug_info(mut self, debug_info: DebugInfo) -> Self {
        self.debug_info = Some(debug_info);
        self
    }

    /// Every content hash this function references: the recorded
    /// `dependencies` plus every hash the constant pool mentions.
    ///
    /// The builder mirrors call / closure / handler-arm / suspend-impl
    /// targets into `dependencies`, but a bare function-as-value and a
    /// trait-dictionary tuple entry are plain `PushConst`s that record
    /// nothing — so any closure walker (pack extraction, disk-store gc,
    /// `load_closure`, the retirement trace) must consult this, never
    /// `dependencies` alone, or it under-counts and strands (or purges)
    /// live code.
    pub fn referenced_hashes(&self) -> impl Iterator<Item = blake3::Hash> + '_ {
        let constants = self.constants.iter().filter_map(|value| match value {
            Value::FunctionRef(hash) | Value::ObjectRef(hash) => Some(*hash),
            Value::AbilityMethodRef(m) => m.impl_fn,
            _ => None,
        });
        self.dependencies.iter().copied().chain(constants)
    }

    /// Look up the precomputed [`MethodKey`] for the constant at `idx`.
    ///
    /// Returns `Some` iff that constant is an `AbilityMethodRef` (every
    /// `Suspend` / handler-arm site indexes one). The VM uses this to skip
    /// re-hashing the key on the hot perform/install paths.
    #[must_use]
    pub fn method_key(&self, idx: u16) -> Option<MethodKey> {
        self.method_keys.get(&idx).copied()
    }

    /// Derive the [`MethodKey`] of every `AbilityMethodRef` in a constant
    /// pool, keyed by constant index. This is the sole builder of the
    /// `method_keys` cache, so it can never drift from `constants`.
    pub(crate) fn index_method_keys(constants: &[Value]) -> HashMap<u16, MethodKey> {
        constants
            .iter()
            .enumerate()
            .filter_map(|(idx, value)| match value {
                Value::AbilityMethodRef(m) => Some((idx as u16, m.method_key())),
                _ => None,
            })
            .collect()
    }

    /// Compute the content hash for this function.
    ///
    /// The hash includes:
    /// - Bytecode
    /// - Constants (using stable binary representation)
    /// - Local count and param count
    /// - Dependencies (function hashes this function calls)
    ///
    /// This provides a stable, content-addressed identifier that:
    /// - Is deterministic across runs
    /// - Changes when any aspect of the function changes
    /// - Enables deduplication of identical functions
    fn compute_hash(
        bytecode: &[u8],
        constants: &[Value],
        local_count: u16,
        param_count: u8,
        dependencies: &[blake3::Hash],
    ) -> blake3::Hash {
        let mut hasher = blake3::Hasher::new();

        // Hash bytecode
        hasher.update(&(bytecode.len() as u32).to_le_bytes());
        hasher.update(bytecode);

        // Hash constants using stable binary format
        hasher.update(&(constants.len() as u32).to_le_bytes());
        for constant in constants {
            hash_value(&mut hasher, constant);
        }

        // Hash metadata
        hasher.update(&local_count.to_le_bytes());
        hasher.update(&[param_count]);

        // Hash dependencies
        hasher.update(&(dependencies.len() as u32).to_le_bytes());
        for dep in dependencies {
            hasher.update(dep.as_bytes());
        }

        hasher.finalize()
    }
}

/// Hash a Value using a stable binary representation.
///
/// This is used for content addressing and must be deterministic.
#[allow(clippy::too_many_lines)]
fn hash_value(hasher: &mut blake3::Hasher, value: &Value) {
    // Type discriminant for stable hashing
    const TYPE_UNIT: u8 = 0;
    const TYPE_BOOL: u8 = 1;
    const TYPE_NUMBER: u8 = 2;
    const TYPE_STRING: u8 = 3;
    const TYPE_TUPLE: u8 = 4;
    const TYPE_RECORD: u8 = 5;
    const TYPE_FUNCTION_REF: u8 = 6;
    const TYPE_SUSPENDED_ABILITY: u8 = 7;
    const TYPE_CONTINUATION: u8 = 8;

    match value {
        Value::Unit => {
            hasher.update(&[TYPE_UNIT]);
        }
        Value::Bool(b) => {
            hasher.update(&[TYPE_BOOL, u8::from(*b)]);
        }
        Value::Number(n) => {
            hasher.update(&[TYPE_NUMBER]);
            hasher.update(&n.to_bits().to_le_bytes());
        }
        Value::String(s) => {
            hasher.update(&[TYPE_STRING]);
            hasher.update(&(s.len() as u32).to_le_bytes());
            hasher.update(s.as_bytes());
        }
        Value::Binary(b) => {
            const TYPE_BYTES: u8 = 17;
            hasher.update(&[TYPE_BYTES]);
            hasher.update(&(b.len() as u32).to_le_bytes());
            hasher.update(b);
        }
        Value::Tuple(elements) => {
            hasher.update(&[TYPE_TUPLE]);
            hasher.update(&(elements.len() as u32).to_le_bytes());
            for elem in elements.iter() {
                hash_value(hasher, elem);
            }
        }
        Value::Record(fields) => {
            hasher.update(&[TYPE_RECORD]);
            // Sort fields for deterministic hashing
            let mut sorted_fields: Vec<_> = fields.iter().collect();
            sorted_fields.sort_by(|a, b| a.0.cmp(b.0));
            hasher.update(&(sorted_fields.len() as u32).to_le_bytes());
            for (key, val) in sorted_fields {
                hasher.update(&(key.len() as u32).to_le_bytes());
                hasher.update(key.as_bytes());
                hash_value(hasher, val);
            }
        }
        Value::AbilityRef(id) => {
            const TYPE_ABILITY_REF: u8 = 18;
            hasher.update(&[TYPE_ABILITY_REF]);
            hasher.update(id.as_bytes());
        }
        Value::FunctionRef(h) => {
            hasher.update(&[TYPE_FUNCTION_REF]);
            hasher.update(h.as_bytes());
        }
        Value::ObjectRef(h) => {
            const TYPE_OBJECT_REF: u8 = 19;
            hasher.update(&[TYPE_OBJECT_REF]);
            hasher.update(h.as_bytes());
        }
        Value::AbilityMethodRef(m) => {
            const TYPE_ABILITY_METHOD_REF: u8 = 20;
            hasher.update(&[TYPE_ABILITY_METHOD_REF]);
            hasher.update(m.ability_id.as_bytes());
            hasher.update(m.ability_uuid.as_bytes());
            hasher.update(m.signature.as_bytes());
            hasher.update(&[u8::from(m.never)]);
            match &m.impl_fn {
                Some(h) => {
                    hasher.update(&[1]);
                    hasher.update(h.as_bytes());
                }
                None => {
                    hasher.update(&[0]);
                }
            }
        }
        Value::SuspendedAbility(ability) => {
            hasher.update(&[TYPE_SUSPENDED_ABILITY]);
            hasher.update(ability.ability_id.as_bytes());
            hasher.update(ability.method.as_bytes());
            hasher.update(&(ability.args.len() as u32).to_le_bytes());
            for arg in &ability.args {
                hash_value(hasher, arg);
            }
        }
        Value::Continuation(_) => {
            // Continuations cannot be content-hashed as they contain runtime state
            // Use a fixed marker to indicate presence
            hasher.update(&[TYPE_CONTINUATION]);
        }
        Value::Closure(closure) => {
            const TYPE_CLOSURE: u8 = 9;
            hasher.update(&[TYPE_CLOSURE]);
            hasher.update(closure.function_hash.as_bytes());
            hasher.update(&(closure.environment.len() as u32).to_le_bytes());
            for val in &closure.environment {
                hash_value(hasher, val);
            }
        }
        Value::Handler(handler) => {
            const TYPE_HANDLER: u8 = 10;
            hasher.update(&[TYPE_HANDLER]);
            hasher.update(handler.ability_id.as_bytes());
            // Hash methods in sorted order for deterministic hashing
            let mut methods: Vec<_> = handler.methods.iter().collect();
            methods.sort_by_key(|(k, _)| *k);
            hasher.update(&(methods.len() as u32).to_le_bytes());
            for (method, func_hash) in methods {
                hasher.update(method.as_bytes());
                hasher.update(func_hash.as_bytes());
            }
            // Hash captures
            hasher.update(&(handler.captures.len() as u32).to_le_bytes());
            for val in &handler.captures {
                hash_value(hasher, val);
            }
        }
        Value::List(elements) => {
            const TYPE_LIST: u8 = 11;
            hasher.update(&[TYPE_LIST]);
            hasher.update(&(elements.len() as u32).to_le_bytes());
            for elem in elements.iter() {
                hash_value(hasher, elem);
            }
        }
        Value::Map(map) => {
            const TYPE_MAP: u8 = 12;
            hasher.update(&[TYPE_MAP]);
            // Entries are in insertion order, so iteration is deterministic.
            hasher.update(&(map.entries.len() as u32).to_le_bytes());
            for (key, val) in &map.entries {
                hash_value(hasher, key);
                hash_value(hasher, val);
            }
        }
        Value::Set(set) => {
            const TYPE_SET: u8 = 13;
            hasher.update(&[TYPE_SET]);
            hasher.update(&(set.elements.len() as u32).to_le_bytes());
            for elem in &set.elements {
                hash_value(hasher, elem);
            }
        }
        Value::Enum(e) => {
            const TYPE_ENUM: u8 = 14;
            hasher.update(&[TYPE_ENUM]);
            // Hash type name
            hasher.update(&(e.type_name.len() as u32).to_le_bytes());
            hasher.update(e.type_name.as_bytes());
            // Hash tag
            hasher.update(&e.tag.to_le_bytes());
            // Hash variant name
            hasher.update(&(e.variant_name.len() as u32).to_le_bytes());
            hasher.update(e.variant_name.as_bytes());
            // Hash payload (if any)
            if let Some(payload) = e.payload.as_deref() {
                hasher.update(&[1u8]); // has payload marker
                hash_value(hasher, payload);
            } else {
                hasher.update(&[0u8]); // no payload marker
            }
        }
        Value::Module(m) => {
            const TYPE_MODULE: u8 = 15;
            hasher.update(&[TYPE_MODULE]);
            hasher.update(&(m.path.len() as u32).to_le_bytes());
            hasher.update(m.path.as_bytes());
        }
        Value::ModuleMember(m) => {
            const TYPE_MODULE_MEMBER: u8 = 16;
            hasher.update(&[TYPE_MODULE_MEMBER]);
            hasher.update(&(m.path.len() as u32).to_le_bytes());
            hasher.update(m.path.as_bytes());
        }
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
            Opcode::Unhandle,
            Opcode::Resume,
            // Closures
            Opcode::MakeClosure,
            Opcode::CallClosure,
            Opcode::LoadCapture,
            // Handler literals
            Opcode::MakeHandler,
            Opcode::HandleWithValue,
            // Lists
            Opcode::MakeList,
            // Strings
            // Type conversion
            // Maps
            // Sets
            Opcode::MakeSet,
            // Enums
            Opcode::MakeEnum,
            Opcode::EnumIs,
            Opcode::EnumPayload,
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

        assert_eq!(
            builder.bytecode(),
            &[Opcode::Add as u8, Opcode::Return as u8]
        );
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

    #[test]
    fn test_automatic_dependency_extraction() {
        let hash1 = blake3::hash(b"func1");
        let hash2 = blake3::hash(b"func2");

        let mut builder = BytecodeBuilder::new();
        builder.emit_call(hash1, 0);
        builder.emit_call(hash2, 1);
        builder.emit_call(hash1, 2); // Duplicate call shouldn't add duplicate dependency

        let func = builder.build(0, 0);

        assert_eq!(func.dependencies.len(), 2);
        assert!(func.dependencies.contains(&hash1));
        assert!(func.dependencies.contains(&hash2));
    }

    #[test]
    fn test_no_dependencies_when_no_calls() {
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(42.0));
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);

        assert!(func.dependencies.is_empty());
    }
}
