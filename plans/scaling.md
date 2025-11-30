# Scaling Analysis for Milestones 6+

This document identifies issues in the current implementation that need to be addressed before or during the upcoming milestones.

---

## 1. Performance Bottlenecks That Won't Scale

### 1.1 SCC Recomputation on Every Query
**Location:** `store.rs:340-349`

```rust
pub fn is_recursive(&self, hash: &blake3::Hash) -> bool {
    let analysis = self.compute_sccs();  // Full O(V+E) computation
    analysis.is_recursive(hash)
}
```

Both `is_recursive()` and `mutual_recursion_group()` recompute the entire SCC analysis from scratch. For a codebase with thousands of functions, this becomes expensive.

**Fix:** Cache `SccAnalysis` and invalidate on store modification. Add a `dirty` flag or generation counter.

### 1.2 Linear Search for Dependencies
**Location:** `bytecode.rs:505`

```rust
if !self.dependencies.contains(&func_hash) {  // O(n) per check
    self.dependencies.push(func_hash);
}
```

For functions with many call sites, this is O(n²) overall.

**Fix:** Use `HashSet<blake3::Hash>` for dependency tracking during build, convert to `Vec` at finalization.

### 1.3 JSON Serialization for Network Protocol
**Location:** `store.rs:127-137`, `remote.rs:71-74`

The store serializes to JSON with base64-encoded bytecode. For Milestone 6 (network execution), this is:
- Verbose (base64 bloats 33%)
- Slow to parse
- No streaming support

**Fix:** Implement binary wire protocol as specified in architecture.md (lines 599-619). Keep JSON for debugging/inspection.

### 1.4 Value Cloning in Hot Paths
**Location:** `vm.rs:281, 287, 294, 322-324, etc.`

Heavy use of `.clone()` on Values:
```rust
let value = self.peek()?.clone();
let a = self.pop()?;  // Every pop moves ownership
```

While Rc mitigates heap allocation, the reference count manipulation and match statements add overhead.

**Fix:** Consider arena allocation for Values or copy-on-write semantics for the value stack.

### 1.5 Jump Offset Limits
**Location:** `bytecode.rs:106-114`

Jump offsets are `i16`, limiting bytecode to ±32KB per function. Large generated functions (e.g., from macros, match expansion, or inlined handlers) could exceed this.

**Fix:** Add `JumpLong` opcodes with `i32` offsets, or implement basic block representation that removes offset limitations.

### 1.6 Constant Pool Size Limit
**Location:** `bytecode.rs:29, 497`

Constant indices are `u16`, limiting to 65,535 constants per function. String-heavy code (logging, error messages) could hit this.

**Fix:** Add overflow detection with clear error message. Consider chunked constant pools for Milestone 15+ standard library.

---

## 2. Architectural Issues to Address

### 2.1 Continuation Serialization Gap
**Location:** `value.rs:47-48`

```rust
#[serde(skip)]
Continuation(Rc<Continuation>),
```

`SuspendedAbility` is serializable but `Continuation` is not. This creates a fundamental asymmetry:
- Can send a suspended ability to remote VM
- Remote VM performs it, captures continuation
- Cannot send continuation back for resumed execution

**Impact on Milestone 6:** Cannot implement ability proxying where client handles abilities for remote code.

**Fix:** Either:
1. Serialize continuations (requires serializing stack segments and frame state)
2. Explicitly document that all ability handlers must exist on the executor side
3. Implement continuation-passing-style transform that avoids runtime continuations

### 2.2 Rc Prevents Multi-threading
**Location:** Throughout `value.rs`, `vm.rs`, `store.rs`

All reference-counted types use `Rc<T>`:
```rust
String(Rc<String>),
Tuple(Rc<Vec<Value>>),
Record(Rc<HashMap<Rc<str>, Value>>),
```

This prevents:
- Sharing values across threads (Milestone 9 concurrency)
- Async execution with tokio/async-std
- Future WASM multi-threading

**Fix:** Change to `Arc<T>`. The atomic overhead is minimal for the flexibility gained. Alternatively, use a thread-local value interner.

### 2.3 VM and Store Duplication
**Location:** `vm.rs:170`, `store.rs:32`

Both maintain their own function maps:
```rust
// VM
functions: HashMap<blake3::Hash, Rc<CompiledFunction>>,

// Store
functions: HashMap<blake3::Hash, Rc<CompiledFunction>>,
```

When executing remotely, functions are copied from Store to VM. This wastes memory and requires synchronization.

**Fix:** Have VM reference the Store directly, or use a shared function registry with interior mutability.

### 2.4 Error Context Loss
**Location:** `remote.rs:139-141`

```rust
impl From<VmError> for RemoteError {
    fn from(err: VmError) -> Self {
        Self::Execution(err.to_string())  // Loses structured error
    }
}
```

Debugging remote execution requires full error context (stack trace, local values, handler state). String conversion destroys this.

**Fix:** Add `VmError` variant to `RemoteError`, or create `ExecutionError` struct with full diagnostics.

### 2.5 No Bytecode Versioning
**Location:** `bytecode.rs` (entire file)

No version marker in bytecode format. If opcodes change between versions:
- Old bytecode silently misbehaves
- No migration path
- No clear error on version mismatch

**Fix:** Add magic number and version to `CompiledFunction`. Add `PortableStore.version` handling beyond just storing "1".

### 2.6 Missing Type Information in Bytecode
**Location:** `bytecode.rs:239-259`

`CompiledFunction` has no type signature field:
```rust
pub struct CompiledFunction {
    pub hash: blake3::Hash,
    pub bytecode: Vec<u8>,
    pub constants: Vec<Value>,
    pub local_count: u16,
    pub param_count: u8,
    pub dependencies: Vec<blake3::Hash>,
    // No type_signature field!
}
```

**Impact on Milestone 7-8:** Type checker will need to store type information somewhere. Either:
- Add `type_signature: TypeSig` to `CompiledFunction`
- Maintain separate type metadata store
- Embed type info in a debug info companion structure

---

## 3. Code Quality Issues Limiting Development

### 3.1 Monolithic VM Execution Loop
**Location:** `vm.rs:265-658`

The `run()` method is a 400-line match statement. This makes:
- Adding new opcodes error-prone
- Testing individual operations difficult
- Profiling opcode performance imprecise

**Fix:** Extract opcode handlers into methods or a dispatch table:
```rust
impl Vm {
    fn exec_push_const(&mut self) -> Result<(), VmError> { ... }
    fn exec_add(&mut self) -> Result<(), VmError> { ... }
}
```

### 3.2 Magic Numbers for Abilities
**Location:** `abilities.rs:7-24`, `vm.rs:1363-1368`

```rust
// In abilities.rs
pub const ABILITY_ID: u16 = 0x0001;

// In tests (different file)
const ABILITY_CONSOLE: u16 = 1;  // Duplicate definition!
```

Ability IDs are scattered as magic numbers. No centralized registry. Easy to create conflicts.

**Fix:** Create typed ability registry:
```rust
pub struct AbilityId(u16);
impl AbilityId {
    pub const CONSOLE: Self = Self(0x0001);
    pub const EXCEPTION: Self = Self(0x0002);
    // ...
}
```

### 3.3 Test Utility Duplication
**Location:** `test_utils.rs:54-535`, `test_utils.rs:542-768`

`VmTest` and `FunctionBuilder` have nearly identical APIs (push, add, sub, load_local, etc.) with duplicated implementations.

**Fix:** Extract common builder trait:
```rust
trait BytecodeEmitter {
    fn builder_mut(&mut self) -> &mut BytecodeBuilder;

    fn push(mut self, n: f64) -> Self where Self: Sized {
        self.builder_mut().emit_const(Value::Number(n));
        self
    }
    // ... shared methods
}
```

### 3.4 Inconsistent Hash Creation
**Location:** Various test files

Tests create hashes inconsistently:
```rust
// Method 1: Direct hash
let hash_a = blake3::hash(b"test::a");

// Method 2: Via builder
let func = FunctionBuilder::new("test::factorial");  // Also hashes

// Method 3: Via CompiledFunction
let func = builder.build(0, 0);  // Content-addressed hash
```

Methods 1 and 2 create arbitrary hashes, method 3 creates content hashes. Mixing them can cause subtle bugs where a "hash" doesn't match the actual function content.

**Fix:**
- Use `FunctionBuilder` consistently for test functions
- Add `#[cfg(test)]` helper that clearly distinguishes "test hash" from "content hash"

### 3.5 Implicit Return Emission
**Location:** `test_utils.rs:427`, `test_utils.rs:763`

Both `VmTest::run()` and `FunctionBuilder::build()` auto-emit Return:
```rust
pub fn run(mut self) -> Result<Value, VmError> {
    self.builder.emit(Opcode::Return);  // Always added
    // ...
}
```

This is convenient but hides control flow. If you manually emit Return, you get two Returns.

**Fix:** Either:
- Track whether Return was emitted
- Require explicit `.ret()` call
- Document the behavior prominently

---

## 4. Specific Fixes for Upcoming Milestones

### 4.1 Milestone 6: Network Execution

**Must fix:**
1. Binary wire protocol (replace JSON)
2. Proper error propagation (not string conversion)
3. Connection/timeout handling infrastructure
4. Decide on continuation serialization strategy

**Should fix:**
1. Streaming support for large function transfers
2. Compression for bytecode
3. Authentication/capability framework

### 4.2 Milestone 7-8: Type System

**Must fix:**
1. Add type signature storage
2. Create AST representation (currently only bytecode)
3. Source span tracking for error messages

**Should fix:**
1. Ability polymorphism representation (`E!` syntax from spec)
2. Unification state structure
3. Type environment with scoping

### 4.3 Milestone 9: Concurrency

**Must fix:**
1. Change Rc to Arc throughout
2. Make VM operations async-compatible
3. Implement operation cancellation

**Should fix:**
1. Structured concurrency primitives
2. Ability value batching for `Async.all!`

---

## 5. Suggested Abstractions

### 5.1 Instruction Iterator

Replace manual IP management with bounds-checked iterator:
```rust
struct InstructionCursor<'a> {
    bytecode: &'a [u8],
    ip: usize,
}

impl InstructionCursor<'_> {
    fn read_opcode(&mut self) -> Option<Opcode> { ... }
    fn read_u16(&mut self) -> Option<u16> { ... }
}
```

### 5.2 Value Visitor Pattern

For operations that traverse/transform values:
```rust
trait ValueVisitor {
    fn visit_number(&mut self, n: f64) -> Result<Value, E>;
    fn visit_tuple(&mut self, elements: &[Value]) -> Result<Value, E>;
    // ...
}
```

Reduces duplication between:
- `hash_value()` in bytecode.rs
- Serde serialization in value.rs
- Future deep-clone, deep-freeze, etc.

### 5.3 Execution Context Trait

Abstract over different execution modes:
```rust
trait ExecutionContext {
    fn lookup_function(&self, hash: &blake3::Hash) -> Option<&CompiledFunction>;
    fn handle_ability(&mut self, ability: &SuspendedAbility) -> Result<Value, VmError>;
    fn on_call(&mut self, hash: &blake3::Hash, args: &[Value]);
}
```

Enables:
- Mock execution for testing
- Profiled execution
- Stepped debugging

### 5.4 Protocol Abstraction

For Milestone 6:
```rust
trait RemoteProtocol {
    async fn send_request(&mut self, request: ExecutionRequest) -> Result<(), ProtocolError>;
    async fn receive_response(&mut self) -> Result<ExecutionResponse, ProtocolError>;
}

// Implementations
struct InProcessProtocol { ... }
struct TcpProtocol { ... }
struct WebSocketProtocol { ... }
```

### 5.5 Frame Unification

Unified representation for all frame types:
```rust
enum Frame {
    Call { function: Rc<CompiledFunction>, ip: usize, bp: usize },
    Handler { ability_id: u16, handler_func: blake3::Hash, ... },
    Captured { original: Box<Frame>, ... },
}
```

---

## 6. Quick Wins (Low Effort, High Value)

1. **Add `#[inline]` hints** to hot-path VM methods (pop, push, get_constant)
2. **Pre-allocate stack vectors** with expected capacity in `Vm::new()`
3. **Use `SmallVec`** for constant pools under 8 elements
4. **Add debug assertions** for stack bounds in debug builds
5. **Implement `Display` for `Value`** to improve error messages
6. **Add `store.validate()`** method to verify hash integrity
7. **Extract ability ID constants** into a single `ability_ids.rs` module

---

## 7. Priority Order

For Milestone 6 (Network Execution):
1. **P0:** Binary protocol design
2. **P0:** Error context preservation
3. **P1:** Continuation strategy decision
4. **P1:** Rc → Arc migration
5. **P2:** Store/VM function deduplication

For Milestone 7-8 (Type System):
1. **P0:** Type signature in CompiledFunction
2. **P0:** AST representation
3. **P0:** Source spans
4. **P1:** Diagnostic accumulator

For Milestone 9 (Concurrency):
1. **P0:** Arc everywhere
2. **P0:** Async execution model
3. **P1:** Cancellation tokens
