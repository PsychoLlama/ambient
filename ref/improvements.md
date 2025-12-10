# Code Quality Improvements

A prioritized list of refactoring and code quality improvements for the Ambient codebase.

## Priority 1: Critical (Large File Splitting)

### 1.1 Split compiler/mod.rs (3228 lines)

**File**: `crates/ambient-engine/src/compiler/mod.rs`

The compiler module is monolithic, mixing expression compilation, intrinsic handling, pattern matching, and lambda compilation. Split into focused submodules:

- [ ] Create `compiler/expr.rs` - Extract `compile_expr()` and related helpers (~1000 lines)
- [ ] Create `compiler/intrinsics.rs` - Extract `try_compile_intrinsic()` (~500 lines)
- [ ] Create `compiler/patterns.rs` - Extract pattern matching logic (~150 lines)
- [ ] Create `compiler/lambdas.rs` - Extract lambda/closure compilation logic
- [ ] Keep `compiler/mod.rs` for module-level orchestration and `CompiledModule`

### 1.2 Split infer/mod.rs (3147 lines)

**File**: `crates/ambient-engine/src/infer/mod.rs`

Type inference is also monolithic. Key functions to extract:

- [ ] Create `infer/expr.rs` - Extract `infer_expr()` with its 500+ line match statement
- [ ] Create `infer/unify.rs` - Extract `unify()` and `unify_abilities()` (~300 lines)
- [ ] Create `infer/pattern.rs` - Extract `infer_pattern()` (~100 lines)
- [ ] Keep `infer/mod.rs` for `Infer` struct and core operations

### 1.3 Extract VM tests from vm/mod.rs (2795 lines)

**File**: `crates/ambient-engine/src/vm/mod.rs`

The VM module contains ~134 test functions mixed with implementation. Tests should be extracted:

- [ ] Move `#[cfg(test)] mod tests` to `vm/tests.rs` or `tests/vm_tests.rs`
- [ ] Reduce main module to ~700 lines of actual implementation

---

## Priority 2: High (Code Duplication)

### 2.1 Option/Result handler pattern duplication

**File**: `crates/ambient-engine/src/vm/dispatch.rs:1539-1785`

`OptionMap`, `OptionAndThen`, `ResultMap`, `ResultMapErr`, `ResultAndThen` follow nearly identical patterns (~250 lines of duplication):

```rust
let closure = match self.pop()? { Value::Closure(c) => c, ... };
let value = self.pop()?;
match value {
    Value::Enum(e) if &*e.type_name == "TYPE" => {
        if e.tag == TAG { /* call closure, wrap result */ }
        else { /* return other variant */ }
    }
}
```

- [ ] Extract helper method `apply_closure_to_enum(type_name, some_tag, none_tag, wrap_action)`
- [ ] Reduce ~250 lines to ~50 lines

### 2.2 Math opcode handlers

**File**: `crates/ambient-engine/src/vm/dispatch.rs:70-110`

`Sqrt`, `Abs`, `Floor`, `Ceil`, `Round`, `Trunc`, `Sin`, `Cos`, `Tan`, etc. all follow identical pattern:

```rust
Opcode::Sqrt => {
    let n = self.pop_number("sqrt")?;
    self.stack.push(Value::Number(n.sqrt()));
}
```

- [ ] Consider table-driven approach or macro generation for math opcodes

### 2.3 Intrinsic compilation cases

**File**: `crates/ambient-engine/src/compiler/mod.rs:1985-2440`

`try_compile_intrinsic()` has 50+ near-identical match arms for different intrinsics.

- [ ] Consider intrinsic descriptor table: `{ name, arg_count, opcode }`
- [ ] Generate match arms from table or use lookup-based dispatch

---

## Priority 3: Medium (Technical Debt from TODOs)

### 3.1 Qualified method call syntax

**File**: `crates/ambient-engine/src/infer/check.rs:449`

```rust
// TODO: Support `utils.helper()` syntax
```

Blocks usability of helper module patterns like `utils.format()`.

- [ ] Add support for qualified method calls in type inference

### 3.2 Generic enum parameters

**File**: `crates/ambient-engine/src/infer/check.rs:512`

```rust
vec![], // TODO: handle generic enum parameters
```

- [ ] Implement generic type parameter handling for enum definitions

### 3.3 Module registry for qualified names

**Files**:
- `crates/ambient-parser/src/resolve.rs:211`
- `crates/ambient-parser/src/resolve.rs:524`

```rust
// TODO: implement once we have a module registry
// TODO: handle qualified names from other modules
```

- [ ] Integrate module registry with name resolution

### 3.4 Ability lowering from CST to AST

**Files**:
- `crates/ambient-parser/src/lower.rs:849`
- `crates/ambient-parser/src/lower.rs:863`

```rust
abilities: AbilitySet::empty(), // TODO: lower abilities
```

- [ ] Implement ability set lowering from parsed syntax

### 3.5 Advanced pattern matching

**File**: `crates/ambient-engine/src/compiler/mod.rs:2500`

```rust
// TODO: Full pattern matching with nested patterns and guards.
```

- [ ] Implement nested pattern compilation
- [ ] Implement pattern guards

---

## Priority 4: Low (Dead Code & Suppressions)

### 4.1 Dead code in compiler

**File**: `crates/ambient-engine/src/compiler/mod.rs`

- Line 214: `#[allow(dead_code)] fn new_with_source()` - Unused constructor
- Line 295: `#[allow(dead_code)] fn finalize_debug_info()` - Unused method

- [ ] Remove if not needed for future milestones, or document intent

### 4.2 Dead code in LSP

**File**: `crates/ambient-lsp/src/package.rs`

- Line 23, 37: `#[allow(dead_code)]` on struct fields

- [ ] Evaluate if these fields are needed or remove them

### 4.3 Clippy too_many_lines suppressions

After splitting large files, these can be removed:

- `crates/ambient-engine/src/vm/mod.rs:55` - `#![allow(clippy::too_many_lines)]`
- Various function-level `#[allow(clippy::too_many_lines)]` in infer and parser

- [ ] Remove suppressions after refactoring is complete

---

## Priority 5: Test Coverage

### 5.1 Compiler tests

**File**: `crates/ambient-engine/src/compiler/mod.rs`

Only 14 tests for 3228 lines (~1 test per 230 lines).

- [ ] Add expression compilation tests for each `ExprKind` variant
- [ ] Add intrinsic compilation tests
- [ ] Add pattern matching edge case tests
- [ ] Target: 50+ tests

### 5.2 Parser property tests

**File**: `crates/ambient-parser/src/parser/mod.rs`

33 tests for 1351 lines.

- [ ] Extend property-based tests (following `tests/property_tests.rs` pattern)
- [ ] Add error recovery tests

### 5.3 Integration test pipeline

- [ ] Add end-to-end tests: parse -> infer -> compile -> execute
- [ ] Test error messages through the full pipeline

---

## Priority 6: Backlog Items

From `ref/backlog.md`:

- [ ] Investigate alternatives for serialization, moving away from `base64`
- [ ] Use binary format for compiled files, replacing JSON

---

## Tracking

| Ticket | Status | Notes |
|--------|--------|-------|
| 1.1 Split compiler | partial | Extracted intrinsics.rs and patterns.rs |
| 1.2 Split infer | pending | |
| 1.3 Extract VM tests | pending | |
| 2.1 Option/Result helpers | pending | |
| 2.2 Math opcodes | pending | |
| 2.3 Intrinsic table | pending | |
| 3.1 Qualified methods | pending | |
| 3.2 Generic enums | pending | |
| 3.3 Module registry | pending | |
| 3.4 Ability lowering | pending | |
| 3.5 Pattern matching | pending | |
| 4.1 Compiler dead code | pending | |
| 4.2 LSP dead code | pending | |
| 4.3 Clippy suppressions | pending | |
| 5.1 Compiler tests | pending | |
| 5.2 Parser tests | pending | |
| 5.3 Integration tests | pending | |
