//! Export hash computation for smart cache invalidation.
//!
//! The export hash is computed from a module's public interface:
//! - Public function signatures
//! - Public constant types
//! - Public type aliases
//! - Public enum definitions (including variants)
//! - Public ability definitions (including methods)
//!
//! When a module's source changes but its export hash remains the same,
//! dependents don't need to be recompiled.

use crate::ast::Module;
use crate::symbol_db::extract::{extract_symbols, SymbolKind};
use crate::symbol_db::serialize::serialize_type;

/// Compute the export hash for a module.
///
/// This hash represents the module's public interface. If two versions
/// of a module have the same export hash, dependents don't need to
/// recompile.
///
/// # Arguments
/// * `module` - The typed module
/// * `module_path` - The canonical module path
///
/// # Returns
/// A blake3 hash of the public exports.
#[must_use]
pub fn compute_export_hash(module: &Module, module_path: &str) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();

    // Extract and sort symbols by name for deterministic ordering
    let mut symbols = extract_symbols(module, module_path);
    symbols.sort_by(|a, b| a.name.cmp(&b.name));

    // Only hash public symbols
    for symbol in symbols.iter().filter(|s| s.is_public) {
        // Hash the symbol name
        hasher.update(symbol.name.as_bytes());
        hasher.update(b"\x00");

        // Hash the kind tag
        let kind_tag: &[u8] = match &symbol.kind {
            SymbolKind::Function { .. } => b"function",
            SymbolKind::Const => b"const",
            SymbolKind::TypeAlias => b"type_alias",
            SymbolKind::Enum { .. } => b"enum",
            SymbolKind::Ability { .. } => b"ability",
        };
        hasher.update(kind_tag);
        hasher.update(b"\x00");

        // Hash the type signature
        let type_json = serialize_type(&symbol.type_signature);
        hasher.update(type_json.as_bytes());
        hasher.update(b"\x00");

        // For enums, hash variants
        if let SymbolKind::Enum { variants } = &symbol.kind {
            for variant in variants {
                hasher.update(variant.name.as_bytes());
                hasher.update(b"\x00");
                if let Some(payload) = &variant.payload_type {
                    hasher.update(serialize_type(payload).as_bytes());
                }
                hasher.update(b"\x00");
            }
        }

        // For abilities, hash methods
        if let SymbolKind::Ability { methods } = &symbol.kind {
            for method in methods {
                hasher.update(method.name.as_bytes());
                hasher.update(b"\x00");
                for param in &method.params {
                    hasher.update(serialize_type(param).as_bytes());
                    hasher.update(b"\x00");
                }
                hasher.update(serialize_type(&method.return_type).as_bytes());
                hasher.update(b"\x00");
            }
        }
    }

    hasher.finalize()
}

/// Compute the source hash for a module's source code.
///
/// This is used to detect when source has changed.
#[must_use]
pub fn compute_source_hash(source: &str) -> blake3::Hash {
    blake3::hash(source.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, ExprKind, FunctionDef, Item, ItemKind, Param, Span};
    use crate::types::Type;
    use std::sync::Arc;

    fn make_public_function(name: &str) -> Item {
        Item::new(
            ItemKind::Function(FunctionDef {
                name: Arc::from(name),
                name_span: Span::default(),
                is_public: true,
                type_params: vec![],
                params: vec![Param::with_type(0, "x", Type::Number)],
                ret_ty: Some(Type::Number),
                abilities: vec![],
                body: Expr::new(ExprKind::Unit, Span::default()),
            }),
            Span::default(),
        )
    }

    fn make_private_function(name: &str) -> Item {
        Item::new(
            ItemKind::Function(FunctionDef {
                name: Arc::from(name),
                name_span: Span::default(),
                is_public: false,
                type_params: vec![],
                params: vec![Param::with_type(0, "x", Type::Number)],
                ret_ty: Some(Type::Number),
                abilities: vec![],
                body: Expr::new(ExprKind::Unit, Span::default()),
            }),
            Span::default(),
        )
    }

    #[test]
    fn test_export_hash_ignores_private_changes() {
        let module1 = Module {
            name: Arc::from("test"),
            doc: None,
            items: vec![make_public_function("foo"), make_private_function("bar")],
        };

        let module2 = Module {
            name: Arc::from("test"),
            doc: None,
            items: vec![
                make_public_function("foo"),
                make_private_function("different"),
            ],
        };

        let hash1 = compute_export_hash(&module1, "test");
        let hash2 = compute_export_hash(&module2, "test");

        // Same public interface, different private - hashes should be equal
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_export_hash_changes_on_public_changes() {
        let module1 = Module {
            name: Arc::from("test"),
            doc: None,
            items: vec![make_public_function("foo")],
        };

        let module2 = Module {
            name: Arc::from("test"),
            doc: None,
            items: vec![make_public_function("bar")],
        };

        let hash1 = compute_export_hash(&module1, "test");
        let hash2 = compute_export_hash(&module2, "test");

        // Different public interface - hashes should differ
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_source_hash() {
        let source1 = "fn foo() { 1 }";
        let source2 = "fn foo() { 2 }";

        let hash1 = compute_source_hash(source1);
        let hash2 = compute_source_hash(source2);

        assert_ne!(hash1, hash2);
        assert_eq!(hash1, compute_source_hash(source1)); // Same source = same hash
    }
}
