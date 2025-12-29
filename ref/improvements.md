# Code Quality Improvements

Improvement tickets for the Ambient codebase. Each ticket targets maintainability, reducing technical debt, or improving code organization.

---

## File Size / Modularization

### IMP-001: Split `compiler/mod.rs` (2,774 lines) ✅

**Status**: COMPLETE

**Changes made**:
- Created `compiler/expr.rs` (420 lines): Expression and statement compilation
- Created `compiler/hash.rs` (518 lines): Content-addressed hash computation
- Reduced `mod.rs` from 2,774 to 1,893 lines

The module is now better organized with focused, single-responsibility files.

---

### IMP-002: Split `vm/dispatch.rs` (1,681 lines) ✅

**Status**: PARTIAL COMPLETION

**Changes made**:
- Created `vm/abilities.rs` (235 lines): Extracted ability handling operations
  - `op_suspend`, `op_perform`, `op_handle`, `op_handle_with_value`, `op_resume`, `op_get_ability_arg`
- Reduced `dispatch.rs` from 1,681 to 1,511 lines

The ability handling code (the most complex opcode handlers involving continuation
capture) has been extracted. Further splitting by opcode category is possible but
diminishing returns given the match statement structure.

---

### IMP-003: Split `infer/expr.rs` (1,150 lines) ✅

**Status**: PARTIAL COMPLETION

**Changes made**:
- Created `infer/effects.rs` (171 lines): Effect-related type inference
  - `infer_perform`, `infer_suspend`, `infer_handle`
- Reduced `expr.rs` from 1,150 to 1,031 lines

The effect inference code (the most complex expression handlers) has been
extracted. Further splitting is possible but diminishing returns given
most expression handlers are concise.

---

### IMP-004: Split `parser/mod.rs` (1,340 lines) and `parser/expr.rs` (1,237 lines)

**Files**:
- `crates/ambient-parser/src/parser/mod.rs`
- `crates/ambient-parser/src/parser/expr.rs`

Two functions marked `#[allow(clippy::too_many_lines)]`:
- `parse_postfix_expr()` (expr.rs:206)
- `parse_binary_expr()` (expr.rs:361)

**Suggested split**:
- `parser/statements.rs` - Statement parsing
- `parser/expr/postfix.rs` - Postfix expression parsing
- `parser/expr/binary.rs` - Binary expression parsing
- `parser/expr/primary.rs` - Primary/literal parsing

---

### IMP-005: Split `lsp/server.rs` (1,468 lines)

**File**: `crates/ambient-lsp/src/server.rs`

The LSP server handles many different request types in one file.

**Suggested split by LSP capability**:
- `server/handlers.rs` - Request handler dispatch
- `server/text_document.rs` - Text document sync handlers
- `server/completion.rs` - Completion request handling
- `server/navigation.rs` - Go-to-definition, references, etc.
- `server/symbols.rs` - Document/workspace symbols

---

## Dead Code Cleanup

### IMP-006: Remove dead code in `ambient-lsp` ✅

**Status**: COMPLETE

**Changes made**:
- `package.rs`: Removed incorrect `#[allow(dead_code)]` from `root` field (it IS used).
  Added documentation explaining why `host_abilities`, `source`, `runtime_config()`,
  and `ability_resolver()` are retained for future ability-aware type checking.
- `semantic_tokens.rs`: Consolidated individual `#[allow(dead_code)]` into module-level
  allows with documentation explaining these are for LSP protocol completeness.
- `analysis.rs`: Removed incorrect `#[allow(dead_code)]` from `analyze()` function
  (it IS used by `completion_service.rs` and re-exported from `lib.rs`).
- `lib.rs`: No changes needed - the test_harness allow is appropriate.

---

### IMP-007: Remove dead code in `ambient-engine` ✅

**Status**: COMPLETE

**Changes made**:
- `symbol_db/db.rs`: Removed incorrect `#[allow(dead_code)]` from `TypeKind::from_str`
  (it IS used at lines 504 and 545 for parsing type kinds from database).
- `test_utils.rs`: Added documentation explaining why `ability_id` field is
  retained for debugging purposes in test utilities.

---

## Test Coverage Gaps

### IMP-008: Add unit tests for type inference ✅

**Status**: NOT NEEDED

Upon re-evaluation, the type inference system already has comprehensive unit tests:
- `infer/expr.rs`: 31 tests covering expression inference and error cases
- `infer/unify.rs`: 21 tests covering unification algorithm
- `infer/abilities.rs`: 12 tests covering ability tracking
- `infer/mod.rs`: 14 tests covering high-level inference

Total: 76 unit tests for the type inference system.

---

### IMP-009: Add unit tests for compiler ✅

**Status**: NOT NEEDED

Upon re-evaluation, the compiler already has 37 unit tests in `compiler/mod.rs`:
- Expression compilation tests (literals, operators, control flow)
- Function compilation tests (simple functions, recursion, mutual recursion)
- Pattern matching tests
- Closure and lambda tests
- Content-addressed hash tests
- Debug info generation tests

---

### IMP-010: Add tests for runtime abilities

**Directory**: `crates/ambient-runtime/src/`

The runtime abilities (1,187 lines) are **completely untested**:
- `console.rs` - Console I/O
- `network.rs` - TCP networking
- `execute.rs` - Remote execution
- `time.rs` - Time operations
- `random.rs` - Random number generation
- `async_ability.rs` - Async operations
- `log.rs` - Logging

---

### IMP-011: Add tests for `ambient-ability` crate

**Directory**: `crates/ambient-ability/src/`

The ability authoring framework (1,595 lines) has only minimal inline tests:
- `value.rs` (909 lines) - Value representation - **no tests**
- `format.rs` (458 lines) - Value formatting - has some inline tests
- `error.rs` (276 lines) - Error types - **no tests**
- `handler.rs` (72 lines) - Handler traits - **no tests**

---

### IMP-012: Add tests for `ambient-core` crate

**Directory**: `crates/ambient-core/src/`

Core library (356 lines) with **no tests**:
- `exception.rs` (185 lines) - Exception handling
- `descriptor.rs` (138 lines) - Ability descriptors

---

## Code Quality

### IMP-013: Reduce `#[allow(clippy::too_many_lines)]` usage

21 occurrences of `#[allow(clippy::too_many_lines)]` across the codebase indicate functions that are too long. Each should be refactored:

| File | Line | Function |
|------|------|----------|
| `vm/dispatch.rs` | 13 | `Vm::run()` |
| `compiler/mod.rs` | 593 | `finalize_module_hashes()` |
| `compiler/mod.rs` | 911 | `compile_record_fields()` |
| `compiler/mod.rs` | 1186 | `compile_expr()` |
| `infer/expr.rs` | 25 | `infer_expr()` |
| `infer/unify.rs` | 21 | `unify()` |
| `infer/unify.rs` | 188 | `unify_inner()` |
| `infer/error.rs` | 180 | error formatting |
| `infer/intrinsics.rs` | 37 | intrinsic registration |
| `parser/expr.rs` | 206 | `parse_postfix_expr()` |
| `parser/expr.rs` | 361 | `parse_binary_expr()` |
| `parser/patterns.rs` | 11 | pattern parsing |
| `parser/mod.rs` | (various) | parser functions |
| `lower.rs` | 284, 771 | AST lowering |
| `lexer.rs` | 430 | tokenization |
| `abilities.rs` | 452, 643 | ability handling |
| `bytecode/opcode.rs` | 755 | opcode display |
| `bytecode/mod.rs` | 182 | bytecode operations |
| `completions.rs` | 459 | LSP completions |
| `resolve.rs` | 278 | name resolution |

---

### IMP-014: Address `#[allow(clippy::unnecessary_wraps)]` suppressions ✅

**Status**: COMPLETE

**Changes made**:
Added documentation to all functions explaining that the `Result` return type is
for API consistency with other lex/lower functions that can fail:
- `lexer.rs`: `lex_whitespace`, `lex_line_comment`, `lex_identifier`
- `lower.rs`: `lower_use`

These functions are called in contexts where other branches can fail, so they
need matching signatures for ergonomic use in match expressions.

---

### IMP-015: Review `#[allow(clippy::expect_used)]` suppressions ✅

**Status**: COMPLETE

**Changes made**:
- `lower.rs`: Added `# Panics` documentation to `lower_type` and `lower_qualified_name`
  explaining that qualified names always have at least one segment by parser construction.
- `compiler/mod.rs`: Improved the invariant comment to explain why SCC functions
  must exist in `all_functions`.

All uses of `expect()` are for internal invariants that cannot fail at runtime.

---

### IMP-016: Consolidate numeric cast suppressions

Multiple files suppress `clippy::cast_possible_truncation`, `clippy::cast_sign_loss`, and `clippy::cast_precision_loss`. Consider:
- Creating safe wrapper functions for common casts
- Documenting invariants that make truncation safe
- Using `TryFrom`/`TryInto` where appropriate

**Affected files**:
- `vm/dispatch.rs` (5 occurrences)
- `abilities.rs` (12 occurrences)
- `protocol.rs`, `network_state.rs`, `random.rs`, `time.rs`
- `lsp/server.rs`, `lsp/completions.rs`, `lsp/documents.rs`

---

## Architecture Improvements

### IMP-017: Reduce code duplication in abilities

**Files**:
- `crates/ambient-engine/src/abilities.rs` (1,187 lines)
- `crates/ambient-runtime/src/` (various ability implementations)

The built-in ability implementations in `abilities.rs` and the runtime implementations share patterns. Consider:
- Extracting common ability handler patterns
- Creating macros for repetitive ability registration
- Unifying the two locations of ability definitions

---

### IMP-018: Improve error handling consistency

Some modules use `expect()` heavily while others use proper `Result` types. Standardize error handling:
- `parser/mod.rs` - 69 `expect()` calls
- `parser/expr.rs` - 50 `expect()` calls
- `compiler/mod.rs` - 46 `expect()` calls

Many of these are internal invariant checks. Consider:
- Using `debug_assert!` for development-only checks
- Creating a consistent pattern for "this should never happen" errors
- Adding context to error messages

---

## Documentation

### IMP-019: Add module-level documentation ✅

**Status**: NOT NEEDED

Upon review, all three modules already have comprehensive documentation:
- `infer/mod.rs` - 54 lines documenting Algorithm W, ability tracking, module organization
- `compiler/mod.rs` - Architecture diagram, module organization
- `vm/mod.rs` - Architecture with 4 data structures, opcode categories, error handling

No changes needed.

---

## Infrastructure

### IMP-020: Add code coverage reporting

No visible coverage reporting setup. Add:
- `cargo-tarpaulin` or `llvm-cov` integration
- CI reporting (codecov or similar)
- Minimum coverage thresholds for new code

---

### IMP-021: Expand property-based testing

Only `ambient-parser` has property-based tests (726 lines). Extend to:
- Type inference (constraint generation, unification)
- Compiler (bytecode correctness properties)
- VM (execution invariants)

---

## Priority Order

**Critical** (blocks confidence in core functionality):
1. IMP-008 - Type inference tests
2. IMP-009 - Compiler tests
3. IMP-001 - Split compiler/mod.rs

**High** (significant maintainability impact):
4. IMP-002 - Split vm/dispatch.rs
5. IMP-003 - Split infer/expr.rs
6. IMP-010 - Runtime ability tests
7. IMP-006 - LSP dead code cleanup

**Medium** (quality of life improvements):
8. IMP-013 - Reduce too_many_lines suppressions
9. IMP-017 - Reduce ability code duplication
10. IMP-018 - Error handling consistency

**Low** (nice to have):
11. IMP-019 - Module documentation
12. IMP-020 - Coverage reporting
13. IMP-021 - Property testing expansion
