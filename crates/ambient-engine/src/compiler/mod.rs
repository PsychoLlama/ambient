//! Compiler that transforms typed AST into bytecode.
//!
#![allow(clippy::cast_possible_truncation)]
//! This module implements the final stage of the Ambient compilation pipeline:
//! - Takes a type-checked AST (from `infer`)
//! - Emits bytecode instructions (using `bytecode::BytecodeBuilder`)
//! - Produces `CompiledFunction` values ready for the VM
//!
//! # Architecture
//!
//! ```text
//! Typed AST (Module)
//!       │
//!       ▼
//! ┌──────────────┐
//! │   Compiler   │ ─── Compiles each function/constant
//! └──────┬───────┘
//!        │
//!        ▼
//! CompiledModule { functions, entry_point }
//! ```
//!
//! # Module Organization
//!
//! - [`entry`] - [`CompileOptions`] and the `compile_module*` entry points
//!   that orchestrate a module compile
//! - [`context`] - Per-function ([`FunctionCompiler`]) and per-module
//!   ([`ModuleContext`]) compilation state
//! - [`module_output`] - The [`CompiledModule`] output container and its
//!   pack (de)serialization
//! - [`expr`] - Expression and statement compilation
//! - [`lambdas`] - Lambda/closure compilation
//! - [`patterns`] - `match` compilation
//! - [`hash`] - Content-addressed hash computation
//! - [`error`] - Compilation error types

mod context;
mod entry;
mod error;
mod expr;
mod hash;
mod lambdas;
mod module_output;
mod patterns;

pub use context::VariantInfo;
pub use entry::{
    CompileOptions, compile_module, compile_module_with_imports,
    compile_module_with_imports_and_source, compile_module_with_options,
    compile_module_with_source,
};
pub use error::{CompileError, CompileErrorKind};
pub use module_output::CompiledModule;

// Re-exports for sibling submodules (`use super::…`).
use context::{FunctionCompiler, ModuleContext};
use entry::str_to_value;
use expr::compile_expr;

// ─────────────────────────────────────────────────────────────────────────────
// Handler implicit parameter names
// ─────────────────────────────────────────────────────────────────────────────

/// Name for the implicit continuation parameter in handler functions (slot 0).
const HANDLER_PARAM_CONTINUATION: &str = "__continuation";

/// Name for the implicit suspended ability parameter in handler functions (slot 1).
const HANDLER_PARAM_SUSPENDED_ABILITY: &str = "__suspended_ability";

#[cfg(test)]
mod tests;
