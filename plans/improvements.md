# Code Quality Improvements

This document tracks code quality improvements identified during a feature freeze.

## Priority 1: High Impact

### 1.1 Split `infer/mod.rs` (4,004 lines)
**Status:** Future work - substantial refactoring required

The type inference module is monolithic but well-organized internally with
clear section separators. Consider splitting into:
- `infer/error.rs` - TypeError and display implementations
- `infer/env.rs` - Type environment and schemes
- `infer/unify.rs` - Unification algorithm
- `infer/checker.rs` - Main inference logic
- `infer/module.rs` - Module-level checking

Note: The module has tight coupling between sections, sharing many types
and imports. Tests exercise all parts together. Splitting would require
careful import management.

### 1.2 Split `compiler/mod.rs` (3,397 lines)
**Status:** Future work - substantial refactoring required

The compiler module should be split into:
- `compiler/error.rs` - CompileError types
- `compiler/expr.rs` - Expression compilation (extract from compile_expr)
- `compiler/patterns.rs` - Pattern compilation
- `compiler/abilities.rs` - Ability-related compilation


## Priority 2: Medium Impact

### 2.1 Split `parser/mod.rs` (2,612 lines)
The parser should be split into:
- `parser/module.rs` - Module/item parsing
- `parser/types.rs` - Type expression parsing
- `parser/expr.rs` - Expression parsing with precedence climbing
- `parser/patterns.rs` - Pattern parsing


### 2.3 Split `bytecode.rs` (2,080 lines)
Split into:
- `bytecode/debug.rs` - Debug information structures
- `bytecode/opcode.rs` - Opcode definitions
- `bytecode/builder.rs` - BytecodeBuilder implementation

### 2.4 Split `vm/dispatch.rs` (1,787 lines)
Split by opcode category:
- `dispatch/stack_ops.rs`
- `dispatch/arithmetic.rs`
- `dispatch/control_flow.rs`
- `dispatch/data_structures.rs`

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
