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
`main.rs` (1,093 lines) should be split:
```
src/
├── main.rs         (routing only)
├── commands/
│   ├── compile.rs
│   ├── run.rs
│   ├── check.rs
│   └── ...
├── repl/
│   ├── mod.rs
│   ├── highlighter.rs
│   └── evaluator.rs
└── diagnostics/
    └── formatter.rs
```

### 3.2 Standardize Module Patterns
Adopt consistent `mod.rs` + submodules pattern used by `vm/` across all large modules.

### 3.3 Extract Span/Location Abstraction
Create a shared utility for byte offset to line/column calculations, used by:
- Error display in CLI
- LSP analysis
- Completions context

## Completed

- Extract Error Formatting Abstraction - See `crates/ambient-cli/src/diagnostic.rs`
- Refactor Completions Duplication - See `collect_params_if_in_scope()` in LSP
- Split infer module - Extracted error.rs, env.rs, check.rs (735 lines total)
- Split compiler module - Extracted error.rs, repl.rs (300 lines total)
- Split parser module - Extracted expr.rs, patterns.rs, types.rs (1,563 lines total)
- Split bytecode module - Extracted opcode.rs, builder.rs, debug.rs (1,599 lines total)
