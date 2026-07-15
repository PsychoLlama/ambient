//! Document-symbol extraction: turning a checked AST module into the nested
//! `DocumentSymbol` tree the editor's outline view renders. Split out of
//! `server.rs` to keep it under the per-file line budget; pure rendering with
//! no protocol or `ServerState` coupling.

use lsp_types::{DocumentSymbol, SymbolKind as LspSymbolKind};

use ambient_engine::ast::{ItemKind, Module};

use crate::analysis::format_type;
use crate::convert::offset_range_to_lsp_range;
use crate::hover_format::format_extern_fn_hover;

/// Extract document symbols from an AST module.
pub(crate) fn extract_document_symbols(
    module: &Module,
    doc: &crate::documents::Document,
) -> Vec<DocumentSymbol> {
    module
        .items
        .iter()
        .filter_map(|item| item_to_document_symbol(item, doc))
        .collect()
}

/// Convert a single AST item to a document symbol.
fn item_to_document_symbol(
    item: &ambient_engine::ast::Item,
    doc: &crate::documents::Document,
) -> Option<DocumentSymbol> {
    let range = offset_range_to_lsp_range(doc, item.span.start as usize, item.span.end as usize);

    match &item.kind {
        ItemKind::Function(f) => Some(make_symbol(
            f.name.to_string(),
            Some(format_function_signature(f)),
            LspSymbolKind::FUNCTION,
            range,
            offset_range_to_lsp_range(doc, f.name_span.start as usize, f.name_span.end as usize),
            None,
        )),
        ItemKind::Const(c) => Some(make_symbol(
            c.name.to_string(),
            c.ty.as_ref().map(format_type),
            LspSymbolKind::CONSTANT,
            range,
            offset_range_to_lsp_range(doc, c.name_span.start as usize, c.name_span.end as usize),
            None,
        )),
        ItemKind::Struct(s) => Some(make_symbol(
            s.name.to_string(),
            None,
            LspSymbolKind::STRUCT,
            range,
            offset_range_to_lsp_range(doc, s.name_span.start as usize, s.name_span.end as usize),
            None,
        )),
        ItemKind::TypeAlias(t) => Some(make_symbol(
            t.name.to_string(),
            None,
            LspSymbolKind::TYPE_PARAMETER,
            range,
            offset_range_to_lsp_range(doc, t.name_span.start as usize, t.name_span.end as usize),
            None,
        )),
        ItemKind::Enum(e) => {
            let children = extract_enum_variants(e, doc);
            Some(make_symbol(
                e.name.to_string(),
                None,
                LspSymbolKind::ENUM,
                range,
                offset_range_to_lsp_range(
                    doc,
                    e.name_span.start as usize,
                    e.name_span.end as usize,
                ),
                children,
            ))
        }
        ItemKind::Ability(a) => {
            let children = extract_ability_methods(a, doc);
            Some(make_symbol(
                a.name.to_string(),
                None,
                LspSymbolKind::INTERFACE,
                range,
                offset_range_to_lsp_range(
                    doc,
                    a.name_span.start as usize,
                    a.name_span.end as usize,
                ),
                children,
            ))
        }
        ItemKind::Use(_) => None,
        ItemKind::Trait(t) => {
            let children = extract_trait_methods(t, doc);
            Some(make_symbol(
                t.name.to_string(),
                None,
                LspSymbolKind::INTERFACE,
                range,
                offset_range_to_lsp_range(
                    doc,
                    t.name_span.start as usize,
                    t.name_span.end as usize,
                ),
                children,
            ))
        }
        ItemKind::Impl(i) => Some(make_symbol(
            match &i.trait_name {
                Some(trait_name) => format!("impl {} for ...", trait_name.name.name),
                None => "impl ...".to_string(),
            },
            None,
            LspSymbolKind::CLASS,
            range,
            range,
            None,
        )),
        ItemKind::ExternFn(e) => Some(extern_fn_symbol(e, doc, range)),
    }
}

/// Document symbol for an `extern fn` declaration.
fn extern_fn_symbol(
    e: &ambient_engine::ast::ExternFnDef,
    doc: &crate::documents::Document,
    range: lsp_types::Range,
) -> DocumentSymbol {
    let mut signature = String::new();
    format_extern_fn_hover(e, &mut signature);
    make_symbol(
        e.name.to_string(),
        Some(signature),
        LspSymbolKind::FUNCTION,
        range,
        offset_range_to_lsp_range(doc, e.name_span.start as usize, e.name_span.end as usize),
        None,
    )
}

/// Extract trait methods as document symbols.
fn extract_trait_methods(
    trait_def: &ambient_engine::ast::TraitDef,
    doc: &crate::documents::Document,
) -> Option<Vec<DocumentSymbol>> {
    if trait_def.methods.is_empty() {
        return None;
    }

    let symbols: Vec<_> = trait_def
        .methods
        .iter()
        .map(|m| {
            make_symbol(
                m.name.to_string(),
                None,
                LspSymbolKind::METHOD,
                offset_range_to_lsp_range(doc, m.span.start as usize, m.span.end as usize),
                offset_range_to_lsp_range(
                    doc,
                    m.name_span.start as usize,
                    m.name_span.end as usize,
                ),
                None,
            )
        })
        .collect();

    Some(symbols)
}

/// Create a `DocumentSymbol` with the given properties.
#[allow(deprecated)]
fn make_symbol(
    name: String,
    detail: Option<String>,
    kind: LspSymbolKind,
    range: lsp_types::Range,
    selection_range: lsp_types::Range,
    children: Option<Vec<DocumentSymbol>>,
) -> DocumentSymbol {
    DocumentSymbol {
        name,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children,
    }
}

/// Extract enum variants as child document symbols.
fn extract_enum_variants(
    e: &ambient_engine::ast::EnumDef,
    doc: &crate::documents::Document,
) -> Option<Vec<DocumentSymbol>> {
    let children: Vec<_> = e
        .variants
        .iter()
        .map(|v| {
            let r = offset_range_to_lsp_range(doc, v.span.start as usize, v.span.end as usize);
            make_symbol(
                v.name.to_string(),
                v.payload.as_ref().map(format_type),
                LspSymbolKind::ENUM_MEMBER,
                r,
                r,
                None,
            )
        })
        .collect();
    if children.is_empty() {
        None
    } else {
        Some(children)
    }
}

/// Extract ability methods as child document symbols.
fn extract_ability_methods(
    a: &ambient_engine::ast::AbilityDef,
    doc: &crate::documents::Document,
) -> Option<Vec<DocumentSymbol>> {
    let children: Vec<_> = a
        .methods
        .iter()
        .map(|m| {
            let r = offset_range_to_lsp_range(doc, m.span.start as usize, m.span.end as usize);
            make_symbol(
                m.name.to_string(),
                Some(format_ability_method_signature(m)),
                LspSymbolKind::METHOD,
                r,
                r,
                None,
            )
        })
        .collect();
    if children.is_empty() {
        None
    } else {
        Some(children)
    }
}

/// Format a function signature for display.
fn format_function_signature(f: &ambient_engine::ast::FunctionDef) -> String {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| {
            if let Some(ty) = &p.ty {
                format!("{}: {}", p.name, format_type(ty))
            } else {
                p.name.to_string()
            }
        })
        .collect();
    let ret = f
        .ret_ty
        .as_ref()
        .map_or(String::new(), |ty| format!(" -> {}", format_type(ty)));
    format!("fn({}){}", params.join(", "), ret)
}

/// Format an ability method signature for display.
fn format_ability_method_signature(m: &ambient_engine::ast::AbilityMethod) -> String {
    let params: Vec<String> = m
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, format_type(p.declared_ty())))
        .collect();
    format!("fn({}) -> {}", params.join(", "), format_type(&m.ret_ty))
}
