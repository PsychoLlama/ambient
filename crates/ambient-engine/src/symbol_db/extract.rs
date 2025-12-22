//! Symbol extraction from typed AST.
//!
//! This module extracts symbol information from a typed module for storage
//! in the symbol database.

#![allow(clippy::ref_option)]

use std::sync::Arc;

use crate::ast::{
    AbilityDef, ConstDef, EnumDef, FunctionDef, Item, ItemKind, Module, Span, TypeAliasDef, UseDef,
    UseKind,
};
use crate::types::{AbilitySet, FunctionType, Type};

/// Information about a symbol extracted from a module.
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    /// Symbol name (unqualified).
    pub name: Arc<str>,
    /// Fully qualified name (e.g., "utils.format.helper").
    pub qualified_name: String,
    /// Symbol kind.
    pub kind: SymbolKind,
    /// Whether this symbol is public.
    pub is_public: bool,
    /// Type signature.
    pub type_signature: Type,
    /// Source span start offset.
    pub span_start: u32,
    /// Source span end offset.
    pub span_end: u32,
    /// Documentation from /// comments.
    pub doc: Option<Arc<str>>,
}

/// The kind of a symbol.
#[derive(Debug, Clone)]
pub enum SymbolKind {
    Function { abilities: AbilitySet },
    Const,
    TypeAlias,
    Enum { variants: Vec<EnumVariantInfo> },
    Ability { methods: Vec<AbilityMethodInfo> },
}

/// Information about an enum variant.
#[derive(Debug, Clone)]
pub struct EnumVariantInfo {
    pub name: Arc<str>,
    pub payload_type: Option<Type>,
    pub span: Span,
}

/// Information about an ability method.
#[derive(Debug, Clone)]
pub struct AbilityMethodInfo {
    pub name: Arc<str>,
    pub params: Vec<Type>,
    pub return_type: Type,
    pub span: Span,
}

/// Information about a module dependency.
#[derive(Debug, Clone)]
pub struct DependencyInfo {
    /// Path of the imported module.
    pub depends_on_path: String,
    /// Import kind.
    pub import_kind: DependencyKind,
    /// Specific items imported (for Items kind).
    pub imported_items: Option<Vec<Arc<str>>>,
}

/// The kind of dependency (import).
#[derive(Debug, Clone, Copy)]
pub enum DependencyKind {
    Module,
    Glob,
    Items,
}

impl DependencyKind {
    /// Convert to string for database storage.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Glob => "glob",
            Self::Items => "items",
        }
    }
}

/// Extract all symbols from a typed module.
///
/// # Arguments
/// * `module` - The typed module to extract from
/// * `module_path` - The canonical module path (e.g., "utils.format")
///
/// # Returns
/// A vector of symbol information for all items in the module.
#[must_use]
pub fn extract_symbols(module: &Module, module_path: &str) -> Vec<SymbolInfo> {
    let mut symbols = Vec::new();

    for item in &module.items {
        if let Some(symbol) = extract_item_symbol(item, module_path) {
            symbols.push(symbol);
        }
    }

    symbols
}

/// Extract symbol information from a single item.
fn extract_item_symbol(item: &Item, module_path: &str) -> Option<SymbolInfo> {
    match &item.kind {
        ItemKind::Function(func) => Some(extract_function_symbol(func, module_path, &item.doc)),
        ItemKind::Const(const_def) => Some(extract_const_symbol(const_def, module_path, &item.doc)),
        ItemKind::TypeAlias(type_alias) => Some(extract_type_alias_symbol(
            type_alias,
            module_path,
            &item.doc,
        )),
        ItemKind::Enum(enum_def) => Some(extract_enum_symbol(enum_def, module_path, &item.doc)),
        ItemKind::Ability(ability_def) => {
            Some(extract_ability_symbol(ability_def, module_path, &item.doc))
        }
        ItemKind::Use(_) => None, // Use statements are handled separately as dependencies
    }
}

/// Extract symbol information from a function definition.
fn extract_function_symbol(
    func: &FunctionDef,
    module_path: &str,
    doc: &Option<Arc<str>>,
) -> SymbolInfo {
    // Build the function type from parameters and return type
    let param_types: Vec<Type> = func
        .params
        .iter()
        .map(|p| p.ty.clone().unwrap_or(Type::Hole))
        .collect();
    let ret_type = func.ret_ty.clone().unwrap_or(Type::Unit);

    // TODO: Extract abilities from the function's declared abilities
    // For now, use empty abilities
    let abilities = AbilitySet::Empty;

    let type_signature = Type::Function(FunctionType::with_abilities(
        param_types,
        ret_type,
        abilities.clone(),
    ));

    SymbolInfo {
        name: func.name.clone(),
        qualified_name: format_qualified_name(module_path, &func.name),
        kind: SymbolKind::Function { abilities },
        is_public: func.is_public,
        type_signature,
        span_start: func.name_span.start,
        span_end: func.name_span.end,
        doc: doc.clone(),
    }
}

/// Extract symbol information from a constant definition.
fn extract_const_symbol(
    const_def: &ConstDef,
    module_path: &str,
    doc: &Option<Arc<str>>,
) -> SymbolInfo {
    SymbolInfo {
        name: const_def.name.clone(),
        qualified_name: format_qualified_name(module_path, &const_def.name),
        kind: SymbolKind::Const,
        is_public: false, // Constants don't have visibility modifiers in current AST
        type_signature: const_def.ty.clone(),
        span_start: const_def.name_span.start,
        span_end: const_def.name_span.end,
        doc: doc.clone(),
    }
}

/// Extract symbol information from a type alias definition.
fn extract_type_alias_symbol(
    type_alias: &TypeAliasDef,
    module_path: &str,
    doc: &Option<Arc<str>>,
) -> SymbolInfo {
    SymbolInfo {
        name: type_alias.name.clone(),
        qualified_name: format_qualified_name(module_path, &type_alias.name),
        kind: SymbolKind::TypeAlias,
        is_public: false, // Type aliases don't have visibility in current AST
        type_signature: type_alias.ty.clone(),
        span_start: type_alias.name_span.start,
        span_end: type_alias.name_span.end,
        doc: doc.clone(),
    }
}

/// Extract symbol information from an enum definition.
fn extract_enum_symbol(
    enum_def: &EnumDef,
    module_path: &str,
    doc: &Option<Arc<str>>,
) -> SymbolInfo {
    let variants: Vec<EnumVariantInfo> = enum_def
        .variants
        .iter()
        .map(|v| EnumVariantInfo {
            name: v.name.clone(),
            payload_type: v.payload.clone(),
            span: v.span,
        })
        .collect();

    // The type signature for an enum is the enum type itself
    let type_signature = Type::named_simple(enum_def.name.clone());

    SymbolInfo {
        name: enum_def.name.clone(),
        qualified_name: format_qualified_name(module_path, &enum_def.name),
        kind: SymbolKind::Enum { variants },
        is_public: false, // Enums don't have visibility in current AST
        type_signature,
        span_start: enum_def.name_span.start,
        span_end: enum_def.name_span.end,
        doc: doc.clone(),
    }
}

/// Extract symbol information from an ability definition.
fn extract_ability_symbol(
    ability_def: &AbilityDef,
    module_path: &str,
    doc: &Option<Arc<str>>,
) -> SymbolInfo {
    let methods: Vec<AbilityMethodInfo> = ability_def
        .methods
        .iter()
        .map(|m| AbilityMethodInfo {
            name: m.name.clone(),
            params: m.params.iter().map(|(_, ty)| ty.clone()).collect(),
            return_type: m.ret_ty.clone(),
            span: m.span,
        })
        .collect();

    // The type signature for an ability is the ability type (placeholder)
    let type_signature = Type::named_simple(ability_def.name.clone());

    SymbolInfo {
        name: ability_def.name.clone(),
        qualified_name: format_qualified_name(module_path, &ability_def.name),
        kind: SymbolKind::Ability { methods },
        is_public: false, // Abilities don't have visibility in current AST
        type_signature,
        span_start: ability_def.name_span.start,
        span_end: ability_def.name_span.end,
        doc: doc.clone(),
    }
}

/// Extract all dependencies (imports) from a module.
///
/// # Arguments
/// * `module` - The module to extract dependencies from
///
/// # Returns
/// A vector of dependency information for all use statements.
#[must_use]
pub fn extract_dependencies(module: &Module) -> Vec<DependencyInfo> {
    let mut dependencies = Vec::new();

    for item in &module.items {
        if let ItemKind::Use(use_def) = &item.kind {
            if let Some(dep) = extract_use_dependency(use_def) {
                dependencies.push(dep);
            }
        }
    }

    dependencies
}

/// Extract dependency information from a use statement.
fn extract_use_dependency(use_def: &UseDef) -> Option<DependencyInfo> {
    // Build the module path from prefix and path segments
    let prefix = match use_def.prefix {
        crate::ast::UsePrefix::Pkg => "pkg",
        crate::ast::UsePrefix::Core => "core",
        crate::ast::UsePrefix::Self_ => "self",
        crate::ast::UsePrefix::Super(n) => {
            // For super imports, we'd need to resolve them relative to current module
            // For now, just record it as "super"
            if n == 1 {
                "super"
            } else {
                // Can't easily represent super.super in a single string
                return None;
            }
        }
    };

    let path_segments: Vec<&str> = use_def.path.iter().map(|(s, _)| s.as_ref()).collect();
    let depends_on_path = if path_segments.is_empty() {
        prefix.to_string()
    } else {
        format!("{}.{}", prefix, path_segments.join("."))
    };

    let (import_kind, imported_items) = match &use_def.kind {
        UseKind::Module => (DependencyKind::Module, None),
        UseKind::Glob => (DependencyKind::Glob, None),
        UseKind::Items(items) => (DependencyKind::Items, Some(items.clone())),
    };

    Some(DependencyInfo {
        depends_on_path,
        import_kind,
        imported_items,
    })
}

/// Format a fully qualified name.
fn format_qualified_name(module_path: &str, name: &str) -> String {
    if module_path.is_empty() {
        name.to_string()
    } else {
        format!("{module_path}.{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, ExprKind, FunctionDef, Item, ItemKind, Param, Span};

    fn make_test_module() -> Module {
        Module {
            name: Arc::from("test"),
            doc: None,
            items: vec![
                Item::new(
                    ItemKind::Function(FunctionDef {
                        name: Arc::from("add"),
                        name_span: Span::new(0, 3),
                        is_public: true,
                        type_params: vec![],
                        params: vec![
                            Param::with_type(0, "a", Type::Number),
                            Param::with_type(1, "b", Type::Number),
                        ],
                        ret_ty: Some(Type::Number),
                        abilities: vec![],
                        body: Expr::new(ExprKind::Unit, Span::default()),
                    }),
                    Span::new(0, 50),
                ),
                Item::new(
                    ItemKind::Function(FunctionDef {
                        name: Arc::from("helper"),
                        name_span: Span::new(60, 66),
                        is_public: false,
                        type_params: vec![],
                        params: vec![],
                        ret_ty: Some(Type::Unit),
                        abilities: vec![],
                        body: Expr::new(ExprKind::Unit, Span::default()),
                    }),
                    Span::new(60, 80),
                ),
            ],
        }
    }

    #[test]
    fn test_extract_symbols() {
        let module = make_test_module();
        let symbols = extract_symbols(&module, "utils.math");

        assert_eq!(symbols.len(), 2);

        let add_sym = &symbols[0];
        assert_eq!(add_sym.name.as_ref(), "add");
        assert_eq!(add_sym.qualified_name, "utils.math.add");
        assert!(add_sym.is_public);
        assert!(matches!(add_sym.kind, SymbolKind::Function { .. }));

        let helper_sym = &symbols[1];
        assert_eq!(helper_sym.name.as_ref(), "helper");
        assert_eq!(helper_sym.qualified_name, "utils.math.helper");
        assert!(!helper_sym.is_public);
    }

    #[test]
    fn test_format_qualified_name() {
        assert_eq!(format_qualified_name("utils.math", "add"), "utils.math.add");
        assert_eq!(format_qualified_name("", "main"), "main");
        assert_eq!(format_qualified_name("core.list", "map"), "core.list.map");
    }
}
