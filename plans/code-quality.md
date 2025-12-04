# Code Quality Improvement Tickets

Sprint focus: Improve code quality and long-term maintainability.

---

## Critical: Large File Decomposition

### CQ-001: Split vm.rs into modules
**File:** `crates/ambient-engine/src/vm.rs` (3,835 lines)
**Problem:** The `run()` method is 1,160 lines (lines 425-1584) with a 58-arm match statement for opcode dispatch.
**Solution:**
- Extract opcode handlers into `vm/opcodes.rs`
- Create `vm/core.rs` for VM struct and main loop
- Move built-in operations to `vm/builtins.rs`
- Use dispatch pattern or handler trait instead of giant match

---

### CQ-002: Split infer.rs into modules
**File:** `crates/ambient-engine/src/infer.rs` (3,306 lines)
**Problem:** Mixed type inference, unification, and error handling in one file.
**Solution:**
- Extract unification logic to `infer/unify.rs`
- Create `infer/env.rs` for type environment management
- Keep core algorithm in `infer/core.rs`

---

### CQ-003: Split parser.rs into modules
**File:** `crates/ambient-parser/src/parser.rs` (2,507 lines)
**Problem:** Monolithic recursive descent parser.
**Solution:**
- Extract expression parsing to `parser/expr.rs`
- Extract statement parsing to `parser/stmt.rs`
- Extract type parsing to `parser/types.rs`
- Extract declaration parsing to `parser/decl.rs`

---

### CQ-004: Split compiler.rs into modules
**File:** `crates/ambient-engine/src/compiler.rs` (2,235 lines)
**Problem:** Mixed compilation, hashing, and optimization logic.
**Solution:**
- Separate bytecode emission to `compiler/emit.rs`
- Extract hashing utilities to `compiler/hash.rs`
- Keep core compilation in `compiler/core.rs`

---

## High: Error Handling

### CQ-005: Replace panics in VM with proper errors
**File:** `crates/ambient-engine/src/vm.rs`
**Problem:** 13 panic!() calls for type safety (lines 2823, 3047, 3097, 3128, 3204, 3230, 3277, 3324, 3496, 3518, 3541, 3563).
**Example:** `panic!("Expected Number, got {other:?}")`
**Solution:** Return `Result<_, VmError>` variants instead of panicking.

---

### CQ-006: Fix lexer error handling in parser
**File:** `crates/ambient-parser/src/parser.rs:46`
**Problem:** `lexer.tokenize().expect("lexer error")` panics on lexer failure.
**Solution:** Propagate lexer errors through the parse result or implement error recovery.

---

### CQ-007: Remove unsafe unwraps in value serialization
**File:** `crates/ambient-engine/src/value.rs` (lines 673-786)
**Problem:** 16 `.unwrap()` calls in serialization code.
**Solution:** Use proper error propagation with `?` operator or handle errors gracefully.

---

## Medium: Code Duplication

### CQ-008: Consolidate VM type extraction methods
**File:** `crates/ambient-engine/src/vm.rs` (lines 1683-1720)
**Problem:** `pop_number()`, `pop_bool()`, `pop_string()` follow identical patterns, used 30+ times.
**Solution:** Create a generic `pop_typed<T>()` method or macro to reduce repetition.

---

### CQ-009: Consolidate CLI integration test harness
**File:** `crates/ambient-cli/tests/integration_tests.rs`
**Problem:** Tests follow identical pattern: create temp dir, write file, run command, check output.
**Solution:** Create builder-style test helper to reduce boilerplate.

---

### CQ-010: Consolidate LSP request handling
**File:** `crates/ambient-lsp/src/server.rs` (lines 110-132)
**Problem:** Repeated parse-pattern for each LSP request type with similar cloning and parameter parsing.
**Solution:** Create generic request handler with type parameter.

---

## Medium: Test Coverage

### CQ-011: Add fuzz tests for parser and lexer
**Problem:** No fuzz testing despite parser/lexer complexity (2,500+ lines each).
**Solution:**
- Add `cargo-fuzz` targets for parser
- Add fuzz targets for lexer
- Focus on edge cases and malformed input

---

### CQ-012: Add error case coverage for type inference
**File:** `crates/ambient-engine/src/infer.rs`
**Problem:** 47 error variants exist but tests primarily cover happy path.
**Solution:** Add tests for each TypeError variant to ensure proper error messages.

---

### CQ-013: Expand LSP test coverage
**Files:** `crates/ambient-lsp/src/`
**Problem:** Only 4 tests for analysis, 4 for documents, 6 for completions.
**Solution:** Add comprehensive tests for:
- Completion edge cases
- Hover information
- Go-to-definition scenarios
- Error diagnostics

---

## Medium: Dead Code & Suppressions

### CQ-014: Remove or document dead code in parser
**File:** `crates/ambient-parser/src/parser.rs:27-28`
**Problem:** `#[allow(dead_code)]` on `source` field that's never used.
**Solution:** Remove the field or document why it's needed for future use.

---

### CQ-015: Clean up VM dead code annotations
**File:** `crates/ambient-engine/src/vm.rs:176`
**Problem:** `#[allow(dead_code)]` on `normal_completion_ip` with vague comment about "future optimizations".
**Solution:** Either implement the optimization or remove the field.

---

### CQ-016: Remove compiler debug code suppressions
**File:** `crates/ambient-engine/src/compiler.rs:325, 349`
**Problem:** Dead code allowances for debugging utilities.
**Solution:** Move to a debug feature flag or remove if not actively used.

---

## Low: Performance

### CQ-017: Reduce clones in LSP server
**File:** `crates/ambient-lsp/src/server.rs`
**Problem:** 15 clone() calls including entire JSON params cloning (lines 110, 114, 123, 132, 297-323).
**Solution:** Use references and borrowing where possible, consider `Cow<T>` for clone-on-write.

---

### CQ-018: Optimize store graph algorithm clones
**File:** `crates/ambient-engine/src/store.rs` (lines 291-319)
**Problem:** Repeated node clones in SCC algorithm and graph traversal.
**Solution:** Use references or indices instead of cloning nodes.

---

## Low: TODO Items

### CQ-019: Implement proper List type
**File:** `crates/ambient-engine/src/compiler.rs:1044`
**TODO:** `// TODO: Implement proper List type.`

---

### CQ-020: Implement full pattern matching
**File:** `crates/ambient-engine/src/compiler.rs:1830`
**TODO:** `// TODO: Full pattern matching with nested patterns and guards.`

---

### CQ-021: Implement module registry for resolver
**File:** `crates/ambient-parser/src/resolve.rs:211`
**TODO:** `// TODO: implement once we have a module registry`

---

### CQ-022: Handle qualified names from other modules
**File:** `crates/ambient-parser/src/resolve.rs:524`
**TODO:** `// TODO: handle qualified names from other modules`

---

### CQ-023: Lower abilities in HIR
**File:** `crates/ambient-parser/src/lower.rs:778, 792`
**TODO:** `// TODO: lower abilities`

---

## Low: Documentation

### CQ-024: Add VM architecture documentation
**File:** `crates/ambient-engine/src/vm.rs`
**Problem:** No module-level docs for VM architecture despite 3,800+ lines of complexity.
**Solution:** Add comprehensive module docs explaining:
- Bytecode format
- Stack layout
- Opcode semantics
- Error handling strategy

---

### CQ-025: Add type inference algorithm documentation
**File:** `crates/ambient-engine/src/infer.rs`
**Problem:** References "Algorithm W" but lacks detailed explanation.
**Solution:** Add docs explaining HM algorithm implementation, constraint generation, and unification.

---

### CQ-026: Document compiler bytecode emission
**File:** `crates/ambient-engine/src/compiler.rs`
**Problem:** Missing documentation for bytecode emission patterns.
**Solution:** Document how each AST construct maps to bytecode.

---

## Summary

| Priority | Tickets | Focus Area |
|----------|---------|------------|
| Critical | CQ-001 to CQ-004 | Large file decomposition |
| High | CQ-005 to CQ-007 | Error handling |
| Medium | CQ-008 to CQ-016 | Duplication, tests, dead code |
| Low | CQ-017 to CQ-026 | Performance, TODOs, docs |

**Recommended starting point:** CQ-001 (Split vm.rs) - This is the biggest maintainability bottleneck with a 1,160-line function.
