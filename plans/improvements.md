# Code Quality Improvements

This document tracks code quality improvements identified during a feature freeze.

## Priority 1: High Impact

### 1.1 Split `infer/mod.rs` (4,004 lines)
**Status:** Partially complete

Extracted:
- `infer/error.rs` - TypeError and TypeErrorKind (299 lines)
- `infer/env.rs` - TypeEnv and Scheme (157 lines)
- `infer/check.rs` - Module-level type checking (279 lines)

Remaining in mod.rs (3,115 lines):
- Infer struct and all inference methods
- Unification code could be extracted to `infer/unify.rs` in future

### 1.2 Split `compiler/mod.rs` (3,397 lines)
**Status:** Partially complete

Extracted:
- `compiler/error.rs` - CompileError and CompileErrorKind (79 lines)
- `compiler/repl.rs` - REPL compilation support (221 lines)

Remaining in mod.rs (3,073 lines):
- Module/function compilation
- Expression compilation
- Statement compilation
- Pattern matching compilation


## Priority 2: Medium Impact

### 2.1 Split `parser/mod.rs` (2,612 lines)
**Status:** Complete

Extracted:
- `parser/expr.rs` - Expression parsing (1,178 lines)
- `parser/patterns.rs` - Pattern parsing (204 lines)
- `parser/types.rs` - Type expression parsing (181 lines)

Remaining in mod.rs (1,079 lines):
- Parser struct and core methods
- Module/item parsing
- Function/definition parsing
- Tests

### 2.3 Split `bytecode.rs` (2,080 lines)
**Status:** Complete

Split into:
- `bytecode/opcode.rs` - Opcode enum (850 lines)
- `bytecode/builder.rs` - BytecodeBuilder (635 lines)
- `bytecode/debug.rs` - Debug information (114 lines)
- `bytecode/mod.rs` - CompiledFunction (510 lines)

### 2.4 Split `vm/dispatch.rs` (1,787 lines)
**Status:** Deferred - requires architectural consideration

The file is a single match statement in `run()`. Splitting would require either:
- Extracting helper methods per category (still one big match)
- Converting to a dispatch table pattern (significant refactor)

Consider for a future architectural review rather than simple extraction.

## Priority 3: Organization & Cleanup

### 3.1 CLI Module Organization
**Status:** Complete

Split `main.rs` (977 lines) into:
```
src/
├── main.rs           (57 lines - routing only)
├── commands/
│   ├── mod.rs        (67 lines - shared helpers)
│   ├── compile.rs    (31 lines)
│   ├── run.rs        (64 lines)
│   ├── check.rs      (40 lines)
│   └── dev.rs        (154 lines)
├── repl/
│   ├── mod.rs        (258 lines - REPL logic)
│   └── highlighter.rs (216 lines - syntax highlighting)
└── serialize.rs      (153 lines - module serialization)
```

### 3.2 Standardize Module Patterns
**Status:** Analyzed - Current state is good

Large modules now follow the `mod.rs + submodules` pattern:
- `infer/` - mod.rs + error.rs, env.rs, check.rs
- `compiler/` - mod.rs + error.rs, repl.rs
- `parser/` - mod.rs + expr.rs, patterns.rs, types.rs
- `bytecode/` - mod.rs + opcode.rs, builder.rs, debug.rs
- `vm/` - mod.rs + dispatch.rs, error.rs, core.rs
- `commands/` - mod.rs + compile.rs, run.rs, check.rs, dev.rs
- `repl/` - mod.rs + highlighter.rs

Remaining large standalone files are logically cohesive:
- `types.rs` (1,618 lines) - Type system, highly interrelated
- `value.rs` (1,086 lines) - Value enum and implementations
- `lexer.rs` (1,017 lines) - Token definitions and lexing
- `ast.rs` (1,002 lines) - AST node definitions
- `abilities.rs` (865 lines) - Standard ability implementations

These don't need splitting - they're single-purpose modules.

### 3.3 Extract Span/Location Abstraction
**Status:** Analyzed - Not needed

After analysis, the two implementations serve different purposes:
- CLI (`diagnostic.rs:find_line_info`) - simple O(n) scan, no UTF-16 support (31 lines)
- LSP (`documents.rs`) - precomputed line offsets, binary search, UTF-16 for LSP protocol

The implementations are appropriately different:
- CLI needs one-shot conversion for error messages
- LSP needs repeated fast lookups with UTF-16 code unit support

Sharing would require either:
- Adding UTF-16 complexity to CLI (unnecessary)
- Creating a shared crate between CLI and LSP (more coupling)
- Duplicating without UTF-16 (no benefit)

Current separation is reasonable.

## Completed

- Extract Error Formatting Abstraction - See `crates/ambient-cli/src/diagnostic.rs`
- Refactor Completions Duplication - See `collect_params_if_in_scope()` in LSP
- Split infer module - Extracted error.rs, env.rs, check.rs (735 lines total)
- Split compiler module - Extracted error.rs, repl.rs (300 lines total)
- Split parser module - Extracted expr.rs, patterns.rs, types.rs (1,563 lines total)
- Split bytecode module - Extracted opcode.rs, builder.rs, debug.rs (1,599 lines total)
- Split CLI main.rs - Extracted commands/, repl/, serialize.rs (920 lines extracted)
