# Code Quality Improvements

This document tracks code quality improvements identified during a feature freeze.

## Priority 1: High Impact

### 1.1 Split `infer/mod.rs` (4,004 lines)
The type inference module is monolithic and should be split into:
- `infer/error.rs` - TypeError and display implementations
- `infer/env.rs` - Type environment and schemes
- `infer/unify.rs` - Unification algorithm
- `infer/checker.rs` - Main inference logic
- `infer/module.rs` - Module-level checking

### 1.2 Split `compiler/mod.rs` (3,397 lines)
The compiler module should be split into:
- `compiler/error.rs` - CompileError types
- `compiler/expr.rs` - Expression compilation (extract from compile_expr)
- `compiler/patterns.rs` - Pattern compilation
- `compiler/abilities.rs` - Ability-related compilation

### 1.3 Extract Error Formatting Abstraction
`ambient-cli/src/main.rs` has duplicated error formatting logic in:
- `print_parse_error()` (lines 632-675)
- `print_type_error()` (lines 678-720+)

Extract a common `ErrorFormatter` or `DiagnosticFormatter` trait.

## Priority 2: Medium Impact

### 2.1 Split `parser/mod.rs` (2,612 lines)
The parser should be split into:
- `parser/module.rs` - Module/item parsing
- `parser/types.rs` - Type expression parsing
- `parser/expr.rs` - Expression parsing with precedence climbing
- `parser/patterns.rs` - Pattern parsing

### 2.2 Refactor Completions Duplication
`ambient-lsp/src/completions.rs` has similar functions:
- `collect_block_locals` (lines 524-564)
- `collect_match_locals` (lines 567-582)
- `collect_lambda_locals` (lines 585-600)
- `collect_handle_locals` (lines 603-625)

Extract a generic `collect_params_in_scope()` helper or use a trait-based approach.

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

(Items will be moved here as they are completed)
