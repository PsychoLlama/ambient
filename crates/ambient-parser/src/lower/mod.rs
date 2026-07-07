//! CST to AST lowering.
//!
//! This module converts the parser's CST representation to the AST
//! defined in `ambient-engine`, which is used for type checking and
//! compilation.

mod exprs;
mod items;
#[cfg(test)]
mod tests;
mod types;

use std::sync::Arc;

use ambient_engine::ast::{BindingId, Expr, Item, Module};

use crate::cst::{CstExpr, CstItem, CstModule};
use crate::error::ParseError;
use exprs::lower_expression;
use items::lower_item_impl;
/// Re-exported at its pre-split path for the parser unit tests, its only
/// crate-internal consumer.
#[cfg(test)]
pub(crate) use items::lower_struct_def;

/// Context for lowering, tracking binding IDs.
struct LoweringContext {
    next_binding_id: BindingId,
}

impl LoweringContext {
    fn new() -> Self {
        Self { next_binding_id: 0 }
    }

    fn fresh_binding(&mut self) -> BindingId {
        let id = self.next_binding_id;
        self.next_binding_id += 1;
        id
    }
}

/// Lower a CST module to an AST module.
///
/// # Errors
///
/// Returns a `ParseError` if the CST cannot be lowered to an AST.
pub fn lower_module(cst: &CstModule) -> Result<Module, ParseError> {
    let mut ctx = LoweringContext::new();

    // Extract module-level documentation from inner doc comments (`//!`)
    let doc = cst
        .leading_trivia
        .extract_inner_doc_comments()
        .map(Arc::from);

    let mut items = Vec::new();
    for cst_item in &cst.items {
        items.extend(lower_item_impl(&mut ctx, cst_item)?);
    }

    Ok(Module {
        name: cst.name.clone(),
        doc,
        items,
    })
}

/// Lower a CST module to an AST module, skipping items that fail to lower.
///
/// Companion to `Parser::parse_module_recovering`: each item that cannot be
/// lowered (invalid UUID, missing `unique` on an enum, ...) is dropped and its
/// error collected, so tooling can work with the rest of the module.
pub fn lower_module_recovering(cst: &CstModule) -> (Module, Vec<ParseError>) {
    let mut ctx = LoweringContext::new();

    // Extract module-level documentation from inner doc comments (`//!`)
    let doc = cst
        .leading_trivia
        .extract_inner_doc_comments()
        .map(Arc::from);

    let mut items = Vec::new();
    let mut errors = Vec::new();
    for cst_item in &cst.items {
        match lower_item_impl(&mut ctx, cst_item) {
            Ok(lowered) => items.extend(lowered),
            Err(e) => errors.push(e),
        }
    }

    let module = Module {
        name: cst.name.clone(),
        doc,
        items,
    };
    (module, errors)
}

/// Lower a single expression.
pub fn lower_expr(cst: &CstExpr) -> Result<Expr, ParseError> {
    let mut ctx = LoweringContext::new();
    lower_expression(&mut ctx, cst)
}

/// Lower a single item. A `use` tree flattens to one item per imported
/// leaf; everything else lowers 1:1.
pub fn lower_item(item: &CstItem) -> Result<Vec<Item>, ParseError> {
    let mut ctx = LoweringContext::new();
    lower_item_impl(&mut ctx, item)
}
