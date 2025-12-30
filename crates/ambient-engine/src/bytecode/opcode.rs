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

    /// Get the last element of a list.
    ///
    /// Stack: `[list] -> [element]`
    /// Returns Unit if list is empty.
    ListLast = 0xCB,

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
    // Serialization (for remote execution protocol)
    // ─────────────────────────────────────────────────────────────────────────
    /// Serialize a value to binary format (bincode).
    ///
    /// Stack: `[value] -> [list<number>]`
    /// Returns a list of bytes (0-255) representing the serialized value.
    SerializeValue = 0x5B,

    /// Deserialize a value from binary format (bincode).
    ///
    /// Stack: `[list<number>] -> [option<value>]`
    /// Returns Some(value) on success, None on failure.
    DeserializeValue = 0x5C,

    /// Get the function hash from a closure.
    ///
    /// Stack: `[closure] -> [string]`
    /// Returns the hex-encoded blake3 hash of the closure's function.
    ClosureHash = 0x5D,

    /// Get the captured environment from a closure as serialized bytes.
    ///
    /// Stack: `[closure] -> [list<number>]`
    /// Returns the serialized captured values.
    ClosureCaptures = 0x5E,

    /// Convert hex string to bytes.
    ///
    /// Stack: `[string] -> [option<list<number>>]`
    /// Returns Some(bytes) on valid hex, None on invalid.
    HexToBytes = 0x5F,

    /// Convert bytes to hex string.
    ///
    /// Stack: `[list<number>] -> [string]`
    BytesToHex = 0x62,

    // ─────────────────────────────────────────────────────────────────────────
    // Bytes operations
    // ─────────────────────────────────────────────────────────────────────────
    /// Create Bytes from a list of numbers.
    ///
    /// Stack: `[list<number>] -> [bytes]`
    /// Each number is truncated to a byte (0-255).
    BytesFrom = 0x63,

    /// Convert Bytes to a list of numbers.
    ///
    /// Stack: `[bytes] -> [list<number>]`
    BytesToList = 0x64,

    /// Get the length of Bytes.
    ///
    /// Stack: `[bytes] -> [number]`
    BytesLength = 0x65,

    /// Get a single byte at index.
    ///
    /// Stack: `[bytes, index] -> [number]`
    /// Returns 0 if index is out of bounds.
    BytesGet = 0x66,

    /// Get a slice of Bytes.
    ///
    /// Stack: `[bytes, start, end] -> [bytes]`
    /// Indices are clamped to valid bounds.
    BytesSlice = 0x67,

    /// Concatenate two Bytes values.
    ///
    /// Stack: `[bytes, bytes] -> [bytes]`
    BytesConcat = 0x68,

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
            0xCB => Some(Self::ListLast),
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
            // Serialization
            0x5B => Some(Self::SerializeValue),
            0x5C => Some(Self::DeserializeValue),
            0x5D => Some(Self::ClosureHash),
            0x5E => Some(Self::ClosureCaptures),
            0x5F => Some(Self::HexToBytes),
            0x62 => Some(Self::BytesToHex),
            // Bytes operations
            0x63 => Some(Self::BytesFrom),
            0x64 => Some(Self::BytesToList),
            0x65 => Some(Self::BytesLength),
            0x66 => Some(Self::BytesGet),
            0x67 => Some(Self::BytesSlice),
            0x68 => Some(Self::BytesConcat),
            0xFF => Some(Self::Halt),
            _ => None,
        }
    }
}
