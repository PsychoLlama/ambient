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
use std::sync::Arc;

use crate::value::Value;

// ─────────────────────────────────────────────────────────────────────────────
// Debug Information
// ─────────────────────────────────────────────────────────────────────────────

/// Debug information for a compiled function.
///
/// This allows mapping bytecode locations back to source code for
/// error messages and stack traces.
#[derive(Debug, Clone, Default)]
pub struct DebugInfo {
    /// Path to the source file (if available).
    pub source_file: Option<String>,

    /// Name of the function (for display in stack traces).
    pub function_name: Option<String>,

    /// Maps bytecode offsets to source spans.
    ///
    /// Each entry maps a bytecode byte offset to a source span.
    /// The spans refer to byte offsets in the source file.
    pub source_map: Vec<SourceMapping>,

    /// Maps local variable slots to their names.
    ///
    /// This helps with debugging output by showing meaningful variable names.
    pub local_names: HashMap<u16, String>,
}

/// A single mapping from bytecode offset to source location.
#[derive(Debug, Clone)]
pub struct SourceMapping {
    /// Byte offset in the bytecode.
    pub bytecode_offset: usize,

    /// Start byte offset in the source file.
    pub source_start: usize,

    /// End byte offset in the source file.
    pub source_end: usize,

    /// Line number (1-indexed) in the source file.
    pub line: u32,

    /// Column number (1-indexed) in the source file.
    pub column: u32,
}

impl DebugInfo {
    /// Create empty debug info.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create debug info with a source file and function name.
    #[must_use]
    pub fn with_source(source_file: impl Into<String>, function_name: impl Into<String>) -> Self {
        Self {
            source_file: Some(source_file.into()),
            function_name: Some(function_name.into()),
            ..Self::default()
        }
    }

    /// Add a source mapping.
    pub fn add_mapping(
        &mut self,
        bytecode_offset: usize,
        source_start: usize,
        source_end: usize,
        line: u32,
        column: u32,
    ) {
        self.source_map.push(SourceMapping {
            bytecode_offset,
            source_start,
            source_end,
            line,
            column,
        });
    }

    /// Add a local variable name mapping.
    pub fn add_local_name(&mut self, slot: u16, name: impl Into<String>) {
        self.local_names.insert(slot, name.into());
    }

    /// Find the source location for a bytecode offset.
    ///
    /// Returns the mapping with the highest bytecode offset that is <= the given offset.
    #[must_use]
    pub fn find_source_location(&self, bytecode_offset: usize) -> Option<&SourceMapping> {
        // Binary search for the largest offset <= bytecode_offset
        // Since mappings are added in order, we can search efficiently
        let mut result: Option<&SourceMapping> = None;
        for mapping in &self.source_map {
            if mapping.bytecode_offset <= bytecode_offset {
                result = Some(mapping);
            } else {
                break;
            }
        }
        result
    }

    /// Get the name of a local variable, if known.
    #[must_use]
    pub fn get_local_name(&self, slot: u16) -> Option<&str> {
        self.local_names.get(&slot).map(String::as_str)
    }
}

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

    /// Square root.
    ///
    /// Stack: `[number] -> [sqrt]`
    Sqrt = 0x26,

    /// Absolute value.
    ///
    /// Stack: `[number] -> [abs]`
    Abs = 0x27,

    /// Floor (round towards negative infinity).
    ///
    /// Stack: `[number] -> [floor]`
    Floor = 0x28,

    /// Ceiling (round towards positive infinity).
    ///
    /// Stack: `[number] -> [ceil]`
    Ceil = 0x29,

    /// Round to nearest integer.
    ///
    /// Stack: `[number] -> [rounded]`
    Round = 0x2A,

    /// Truncate (round towards zero).
    ///
    /// Stack: `[number] -> [truncated]`
    Trunc = 0x2B,

    /// Sine (radians).
    ///
    /// Stack: `[number] -> [sin]`
    Sin = 0x2C,

    /// Cosine (radians).
    ///
    /// Stack: `[number] -> [cos]`
    Cos = 0x2D,

    /// Tangent (radians).
    ///
    /// Stack: `[number] -> [tan]`
    Tan = 0x2E,

    /// Natural logarithm.
    ///
    /// Stack: `[number] -> [ln]`
    Ln = 0x2F,

    /// Exponential (e^x).
    ///
    /// Stack: `[number] -> [exp]`
    Exp = 0x36,

    /// Power (base^exponent).
    ///
    /// Stack: `[base, exponent] -> [result]`
    Pow = 0x37,

    /// Minimum of two numbers.
    ///
    /// Stack: `[a, b] -> [min]`
    Min = 0x38,

    /// Maximum of two numbers.
    ///
    /// Stack: `[a, b] -> [max]`
    Max = 0x39,

    /// Arc sine (radians).
    ///
    /// Stack: `[number] -> [asin]`
    Asin = 0x3A,

    /// Arc cosine (radians).
    ///
    /// Stack: `[number] -> [acos]`
    Acos = 0x3B,

    /// Arc tangent (radians).
    ///
    /// Stack: `[number] -> [atan]`
    Atan = 0x3C,

    /// Two-argument arc tangent (radians).
    ///
    /// Stack: `[y, x] -> [atan2]`
    Atan2 = 0x3D,

    /// Log base 10.
    ///
    /// Stack: `[number] -> [log10]`
    Log10 = 0x3E,

    /// Log base 2.
    ///
    /// Stack: `[number] -> [log2]`
    Log2 = 0x3F,

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

    /// Get an argument from a suspended ability value.
    /// Operand: u8 (argument index)
    ///
    /// Pops a `SuspendedAbility` from the stack and pushes the argument at the given index.
    /// Used in handler functions to extract ability method arguments.
    GetAbilityArg = 0x85,

    // ─────────────────────────────────────────────────────────────────────────
    // Concurrency (Milestone 9)
    // ─────────────────────────────────────────────────────────────────────────
    /// Perform multiple suspended abilities concurrently and collect all results.
    /// Operand: u8 (count - number of ability values on stack)
    ///
    /// Pops `count` suspended ability values from the stack, performs them all
    /// (potentially concurrently), and pushes a tuple of results in the same order.
    AsyncAll = 0x90,

    /// Race multiple suspended abilities, returning the first to complete.
    /// Operand: u8 (count - number of ability values on stack)
    ///
    /// Pops `count` suspended ability values from the stack, performs them
    /// (potentially concurrently), and pushes the result of the first to complete.
    /// Other operations are cancelled.
    AsyncRace = 0x91,

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
    /// Operand: u16 (ability ID)
    /// Operand: u8 (method count)
    /// Operand: u8 (capture count - values to capture from stack)
    ///
    /// Following the operands, `method_count` pairs of:
    ///   - u16 (method ID)
    ///   - u16 (constant pool index for function hash)
    ///
    /// Pops `capture_count` values from the stack (captures), then pushes
    /// a `HandlerValue` containing the ability ID, methods map, and captures.
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

    /// Get an element from a list by index.
    ///
    /// Stack: `[list, index] -> [element]`
    /// Returns Unit if index is out of bounds.
    ListGet = 0xC1,

    /// Get the length of a list.
    ///
    /// Stack: `[list] -> [length]`
    ListLength = 0xC2,

    /// Concatenate two lists.
    ///
    /// Stack: `[list1, list2] -> [result]`
    ListConcat = 0xC3,

    /// Append a value to the end of a list.
    ///
    /// Stack: `[list, value] -> [new_list]`
    ListAppend = 0xC4,

    /// Get the first element of a list (head).
    ///
    /// Stack: `[list] -> [element]`
    /// Returns Unit if list is empty.
    ListHead = 0xC5,

    /// Get all elements except the first (tail).
    ///
    /// Stack: `[list] -> [rest]`
    /// Returns empty list if list has 0 or 1 elements.
    ListTail = 0xC6,

    /// Reverse a list.
    ///
    /// Stack: `[list] -> [reversed_list]`
    ListReverse = 0xC7,

    /// Sort a list (elements must be comparable).
    ///
    /// Stack: `[list] -> [sorted_list]`
    ListSort = 0xC8,

    /// Get a slice of a list.
    ///
    /// Stack: `[list, start, end] -> [slice]`
    /// Indices are inclusive start, exclusive end.
    ListSlice = 0xC9,

    /// Check if list is empty.
    ///
    /// Stack: `[list] -> [bool]`
    ListIsEmpty = 0xCA,

    // ─────────────────────────────────────────────────────────────────────────
    // String operations (Milestone 15 - Standard Library)
    // ─────────────────────────────────────────────────────────────────────────
    /// Get the length of a string.
    ///
    /// Stack: `[string] -> [length]`
    StringLength = 0xD0,

    /// Split a string by delimiter.
    ///
    /// Stack: `[string, delimiter] -> [list]`
    StringSplit = 0xD1,

    /// Join a list of strings with delimiter.
    ///
    /// Stack: `[list, delimiter] -> [string]`
    StringJoin = 0xD2,

    /// Trim whitespace from both ends of a string.
    ///
    /// Stack: `[string] -> [trimmed]`
    StringTrim = 0xD3,

    /// Check if a string contains a substring.
    ///
    /// Stack: `[string, substring] -> [bool]`
    StringContains = 0xD4,

    /// Concatenate two strings.
    ///
    /// Stack: `[string1, string2] -> [result]`
    StringConcat = 0xD5,

    /// Get a substring (slice).
    ///
    /// Stack: `[string, start, end] -> [substring]`
    /// Indices are character positions (inclusive start, exclusive end).
    StringSlice = 0xD6,

    /// Convert string to list of characters.
    ///
    /// Stack: `[string] -> [list<string>]`
    /// Each character is a single-character string.
    StringChars = 0xD7,

    /// Replace occurrences of a pattern with replacement.
    ///
    /// Stack: `[string, pattern, replacement] -> [result]`
    StringReplace = 0xD8,

    /// Check if string starts with prefix.
    ///
    /// Stack: `[string, prefix] -> [bool]`
    StringStartsWith = 0xD9,

    /// Check if string ends with suffix.
    ///
    /// Stack: `[string, suffix] -> [bool]`
    StringEndsWith = 0xDA,

    /// Convert string to uppercase.
    ///
    /// Stack: `[string] -> [uppercase_string]`
    StringToUpper = 0xDB,

    /// Convert string to lowercase.
    ///
    /// Stack: `[string] -> [lowercase_string]`
    StringToLower = 0xDC,

    /// Find index of substring, returns -1 if not found.
    ///
    /// Stack: `[string, substring] -> [index]`
    StringIndexOf = 0xDD,

    /// Repeat a string N times.
    ///
    /// Stack: `[string, count] -> [repeated_string]`
    StringRepeat = 0xDE,

    /// Reverse a string.
    ///
    /// Stack: `[string] -> [reversed_string]`
    StringReverse = 0xDF,

    // ─────────────────────────────────────────────────────────────────────────
    // Type conversion (Milestone 15 - Standard Library)
    // ─────────────────────────────────────────────────────────────────────────
    /// Convert any value to its string representation.
    ///
    /// Stack: `[value] -> [string]`
    ToString = 0xE0,

    /// Parse a string to a number. Returns a tuple (success: bool, value: number).
    ///
    /// Stack: `[string] -> [(bool, number)]`
    ParseNumber = 0xE1,

    /// Parse a string to a boolean. Returns a tuple (success: bool, value: bool).
    ///
    /// Stack: `[string] -> [(bool, bool)]`
    ParseBool = 0xE2,

    // ─────────────────────────────────────────────────────────────────────────
    // Map operations (Milestone 15 - Standard Library)
    // ─────────────────────────────────────────────────────────────────────────
    /// Create an empty map.
    ///
    /// Stack: `[] -> [map]`
    MakeEmptyMap = 0xE8,

    /// Get a value from a map by key.
    ///
    /// Stack: `[map, key] -> [value]`
    /// Returns Unit if key is not found.
    MapGet = 0xE9,

    /// Insert a key-value pair into a map (returns new map).
    ///
    /// Stack: `[map, key, value] -> [new_map]`
    MapInsert = 0xEA,

    /// Remove a key from a map (returns new map).
    ///
    /// Stack: `[map, key] -> [new_map]`
    MapRemove = 0xEB,

    /// Check if a map contains a key.
    ///
    /// Stack: `[map, key] -> [bool]`
    MapContains = 0xEC,

    /// Get the number of entries in a map.
    ///
    /// Stack: `[map] -> [number]`
    MapLength = 0xED,

    /// Get all keys from a map as a list.
    ///
    /// Stack: `[map] -> [list]`
    MapKeys = 0xEE,

    /// Get all values from a map as a list.
    ///
    /// Stack: `[map] -> [list]`
    MapValues = 0xEF,

    // ─────────────────────────────────────────────────────────────────────────
    // Set operations (Milestone 15 - Standard Library)
    // ─────────────────────────────────────────────────────────────────────────
    /// Create an empty set.
    ///
    /// Stack: `[] -> [set]`
    MakeEmptySet = 0xF0,

    /// Create a set from N values on the stack.
    /// Operand: u16 (number of elements)
    ///
    /// Stack: `[v1, v2, ..., vN] -> [set]`
    MakeSet = 0xF1,

    /// Insert a value into a set (returns new set).
    ///
    /// Stack: `[set, value] -> [new_set]`
    SetInsert = 0xF2,

    /// Remove a value from a set (returns new set).
    ///
    /// Stack: `[set, value] -> [new_set]`
    SetRemove = 0xF3,

    /// Check if a set contains a value.
    ///
    /// Stack: `[set, value] -> [bool]`
    SetContains = 0xF4,

    /// Get the number of elements in a set.
    ///
    /// Stack: `[set] -> [number]`
    SetLength = 0xF5,

    /// Compute the union of two sets.
    ///
    /// Stack: `[set1, set2] -> [union_set]`
    SetUnion = 0xF6,

    /// Compute the intersection of two sets.
    ///
    /// Stack: `[set1, set2] -> [intersection_set]`
    SetIntersection = 0xF7,

    /// Compute the difference of two sets (set1 - set2).
    ///
    /// Stack: `[set1, set2] -> [difference_set]`
    SetDifference = 0xF8,

    /// Convert a set to a list.
    ///
    /// Stack: `[set] -> [list]`
    SetToList = 0xF9,

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

    /// Get the variant tag from an enum value.
    ///
    /// Stack: `[enum_value] -> [tag_number]`
    EnumTag = 0xFD,

    // ─────────────────────────────────────────────────────────────────────────
    // Option/Result utilities
    // ─────────────────────────────────────────────────────────────────────────
    /// `Option.unwrap_or`: Get inner value or default.
    ///
    /// Stack: `[option, default] -> [value]`
    /// - If `Some(x)`: returns x
    /// - If None: returns default
    OptionUnwrapOr = 0xE3,

    /// `Option.map`: Apply a function to the inner value if Some.
    ///
    /// Stack: `[option, closure] -> [option]`
    /// - If `Some(x)`: calls closure with x, returns `Some(result)`
    /// - If None: returns None
    ///
    /// Not yet implemented (requires continuation frames).
    OptionMap = 0xE4,

    /// `Option.and_then`: Chain Option-returning functions.
    ///
    /// Stack: `[option, closure] -> [option]`
    /// - If `Some(x)`: calls closure with x (closure returns Option), returns that
    /// - If None: returns None
    ///
    /// Not yet implemented (requires continuation frames).
    OptionAndThen = 0xE5,

    /// `Result.map`: Apply a function to the Ok value.
    ///
    /// Stack: `[result, closure] -> [result]`
    /// - If `Ok(x)`: calls closure with x, returns `Ok(result)`
    /// - If `Err(e)`: returns `Err(e)`
    ///
    /// Not yet implemented (requires continuation frames).
    ResultMap = 0xE6,

    /// `Result.map_err`: Apply a function to the Err value.
    ///
    /// Stack: `[result, closure] -> [result]`
    /// - If `Ok(x)`: returns `Ok(x)`
    /// - If `Err(e)`: calls closure with e, returns `Err(result)`
    ///
    /// Not yet implemented (requires continuation frames).
    ResultMapErr = 0xE7,

    /// `Result.and_then`: Chain Result-returning functions.
    ///
    /// Stack: `[result, closure] -> [result]`
    /// - If `Ok(x)`: calls closure with x (closure returns Result), returns that
    /// - If `Err(e)`: returns `Err(e)`
    ///
    /// Not yet implemented (requires continuation frames).
    ResultAndThen = 0x5A,

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
            // Math functions
            0x26 => Some(Self::Sqrt),
            0x27 => Some(Self::Abs),
            0x28 => Some(Self::Floor),
            0x29 => Some(Self::Ceil),
            0x2A => Some(Self::Round),
            0x2B => Some(Self::Trunc),
            0x2C => Some(Self::Sin),
            0x2D => Some(Self::Cos),
            0x2E => Some(Self::Tan),
            0x2F => Some(Self::Ln),
            0x36 => Some(Self::Exp),
            0x37 => Some(Self::Pow),
            0x38 => Some(Self::Min),
            0x39 => Some(Self::Max),
            0x3A => Some(Self::Asin),
            0x3B => Some(Self::Acos),
            0x3C => Some(Self::Atan),
            0x3D => Some(Self::Atan2),
            0x3E => Some(Self::Log10),
            0x3F => Some(Self::Log2),
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
            0x85 => Some(Self::GetAbilityArg),
            // Concurrency
            0x90 => Some(Self::AsyncAll),
            0x91 => Some(Self::AsyncRace),
            // Closures
            0xA0 => Some(Self::MakeClosure),
            0xA1 => Some(Self::CallClosure),
            0xA2 => Some(Self::LoadCapture),
            // Handler literals
            0xB0 => Some(Self::MakeHandler),
            0xB1 => Some(Self::HandleWithValue),
            // Lists
            0xC0 => Some(Self::MakeList),
            0xC1 => Some(Self::ListGet),
            0xC2 => Some(Self::ListLength),
            0xC3 => Some(Self::ListConcat),
            0xC4 => Some(Self::ListAppend),
            0xC5 => Some(Self::ListHead),
            0xC6 => Some(Self::ListTail),
            0xC7 => Some(Self::ListReverse),
            0xC8 => Some(Self::ListSort),
            0xC9 => Some(Self::ListSlice),
            0xCA => Some(Self::ListIsEmpty),
            // Strings
            0xD0 => Some(Self::StringLength),
            0xD1 => Some(Self::StringSplit),
            0xD2 => Some(Self::StringJoin),
            0xD3 => Some(Self::StringTrim),
            0xD4 => Some(Self::StringContains),
            0xD5 => Some(Self::StringConcat),
            0xD6 => Some(Self::StringSlice),
            0xD7 => Some(Self::StringChars),
            0xD8 => Some(Self::StringReplace),
            0xD9 => Some(Self::StringStartsWith),
            0xDA => Some(Self::StringEndsWith),
            0xDB => Some(Self::StringToUpper),
            0xDC => Some(Self::StringToLower),
            0xDD => Some(Self::StringIndexOf),
            0xDE => Some(Self::StringRepeat),
            0xDF => Some(Self::StringReverse),
            // Type conversion
            0xE0 => Some(Self::ToString),
            0xE1 => Some(Self::ParseNumber),
            0xE2 => Some(Self::ParseBool),
            // Maps
            0xE8 => Some(Self::MakeEmptyMap),
            0xE9 => Some(Self::MapGet),
            0xEA => Some(Self::MapInsert),
            0xEB => Some(Self::MapRemove),
            0xEC => Some(Self::MapContains),
            0xED => Some(Self::MapLength),
            0xEE => Some(Self::MapKeys),
            0xEF => Some(Self::MapValues),
            // Sets
            0xF0 => Some(Self::MakeEmptySet),
            0xF1 => Some(Self::MakeSet),
            0xF2 => Some(Self::SetInsert),
            0xF3 => Some(Self::SetRemove),
            0xF4 => Some(Self::SetContains),
            0xF5 => Some(Self::SetLength),
            0xF6 => Some(Self::SetUnion),
            0xF7 => Some(Self::SetIntersection),
            0xF8 => Some(Self::SetDifference),
            0xF9 => Some(Self::SetToList),
            // Enums
            0xFA => Some(Self::MakeEnum),
            0xFB => Some(Self::EnumIs),
            0xFC => Some(Self::EnumPayload),
            0xFD => Some(Self::EnumTag),
            // Option/Result utilities
            0xE3 => Some(Self::OptionUnwrapOr),
            0xE4 => Some(Self::OptionMap),
            0xE5 => Some(Self::OptionAndThen),
            0xE6 => Some(Self::ResultMap),
            0xE7 => Some(Self::ResultMapErr),
            0x5A => Some(Self::ResultAndThen),
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

    /// Debug information for error messages and stack traces.
    ///
    /// This is optional and only generated when debug info is requested.
    /// It does NOT affect the function's content hash, so functions with
    /// and without debug info are considered equivalent.
    pub debug_info: Option<DebugInfo>,
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
        Self {
            hash,
            bytecode,
            constants,
            local_count,
            param_count,
            dependencies,
            debug_info: None,
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
        Self {
            hash,
            bytecode,
            constants,
            local_count,
            param_count,
            dependencies,
            debug_info: Some(debug_info),
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
        Value::FunctionRef(h) => {
            hasher.update(&[TYPE_FUNCTION_REF]);
            hasher.update(h.as_bytes());
        }
        Value::SuspendedAbility(ability) => {
            hasher.update(&[TYPE_SUSPENDED_ABILITY]);
            hasher.update(&ability.ability_id.to_le_bytes());
            hasher.update(&ability.method_id.to_le_bytes());
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
            hasher.update(&handler.ability_id.to_le_bytes());
            // Hash methods in sorted order for deterministic hashing
            let mut methods: Vec<_> = handler.methods.iter().collect();
            methods.sort_by_key(|(k, _)| *k);
            hasher.update(&(methods.len() as u32).to_le_bytes());
            for (method_id, func_hash) in methods {
                hasher.update(&method_id.to_le_bytes());
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
            // BTreeMap is already sorted, so iteration order is deterministic
            hasher.update(&(map.entries.len() as u32).to_le_bytes());
            for (key, val) in &map.entries {
                hasher.update(&(key.len() as u32).to_le_bytes());
                hasher.update(key.as_bytes());
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
    }
}

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
            // Concurrency
            Opcode::AsyncAll,
            Opcode::AsyncRace,
            // Closures
            Opcode::MakeClosure,
            Opcode::CallClosure,
            Opcode::LoadCapture,
            // Handler literals
            Opcode::MakeHandler,
            Opcode::HandleWithValue,
            // Lists
            Opcode::MakeList,
            Opcode::ListGet,
            Opcode::ListLength,
            Opcode::ListConcat,
            Opcode::ListAppend,
            Opcode::ListHead,
            Opcode::ListTail,
            // Strings
            Opcode::StringLength,
            Opcode::StringSplit,
            Opcode::StringJoin,
            Opcode::StringTrim,
            Opcode::StringContains,
            Opcode::StringConcat,
            // Type conversion
            Opcode::ToString,
            Opcode::ParseNumber,
            Opcode::ParseBool,
            // Maps
            Opcode::MakeEmptyMap,
            Opcode::MapGet,
            Opcode::MapInsert,
            Opcode::MapRemove,
            Opcode::MapContains,
            Opcode::MapLength,
            Opcode::MapKeys,
            Opcode::MapValues,
            // Sets
            Opcode::MakeEmptySet,
            Opcode::MakeSet,
            Opcode::SetInsert,
            Opcode::SetRemove,
            Opcode::SetContains,
            Opcode::SetLength,
            Opcode::SetUnion,
            Opcode::SetIntersection,
            Opcode::SetDifference,
            Opcode::SetToList,
            // Enums
            Opcode::MakeEnum,
            Opcode::EnumIs,
            Opcode::EnumPayload,
            Opcode::EnumTag,
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
