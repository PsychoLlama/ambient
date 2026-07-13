//! Shared fixtures for the compiler unit-test suite. Split out of the former
//! monolithic `tests.rs` (per-file line budgets) so every feature-focused
//! test module can reach the same tiny helpers.

use std::collections::HashMap;
use std::sync::Arc;

use super::entry::compile_function_with_hash;
use super::hash::compute_temporary_hash;
use super::*;
use crate::ast::{FunctionDef, Span};
use crate::bytecode::CompiledFunction;
use crate::fqn::NameKey;

/// A throwaway span for hand-built AST nodes.
pub(super) fn test_span() -> Span {
    Span::default()
}

/// Compile a single function for testing, keyed by its bare short name.
pub(super) fn compile_test_function(func: &FunctionDef) -> Result<CompiledFunction, CompileError> {
    let mut hashes = HashMap::new();
    let hash = compute_temporary_hash(&func.name);
    hashes.insert(NameKey::Bare(Arc::clone(&func.name)), hash);
    let mut ctx = ModuleContext::new(None);
    compile_function_with_hash(func, &hashes, &mut ctx, None, None)
}
