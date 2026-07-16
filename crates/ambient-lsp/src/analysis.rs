//! Analysis facade for the LSP.
//!
//! All real analysis lives in `ambient-analysis` — the same pipeline
//! `ambient check` runs — so the server can never disagree with the
//! compiler about what parses, what type-checks, or what a diagnostic
//! says. This module only re-exports it.

pub use ambient_analysis::queries::{
    AssocTypeAt, assoc_type_signature, definition_item, expr_signature, extern_fn_signature,
    find_assoc_type_at_offset, find_definition, find_expr_at_offset, find_item_at_offset,
    format_type, item_doc, item_signature, method_signature_at, module_doc, module_signature,
};
pub use ambient_analysis::{
    AnalysisResult, analyze, analyze_with_registry, platform_prelude_resolver,
};
