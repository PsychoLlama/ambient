//! Tests for `use` item parsing and lowering: prefixes (`pkg`/`core`/
//! `self`/`super`), groups, aliases, visibility, and placement. Split from
//! `tests.rs` for the per-file line budget.

use super::Parser;

/// Parse a module expected to contain use items, and flatten them
/// through lowering (the semantic surface tests care about).
fn flatten_uses(source: &str) -> Vec<ambient_engine::ast::UseDef> {
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    let lowered = crate::lower::lower_module(&module).expect("lower error");
    lowered
        .items
        .into_iter()
        .filter_map(|item| match item.kind {
            ambient_engine::ast::ItemKind::Use(u) => Some(u),
            _ => None,
        })
        .collect()
}

fn path_names(u: &ambient_engine::ast::UseDef) -> Vec<&str> {
    u.path.iter().map(|(name, _)| name.as_ref()).collect()
}

#[test]
fn test_parse_use_pkg_module() {
    let uses = flatten_uses("use pkg::utils;");
    assert_eq!(uses.len(), 1);
    assert!(!uses[0].is_public);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Pkg);
    assert_eq!(path_names(&uses[0]), ["utils"]);
    assert!(uses[0].alias.is_none());
}

#[test]
fn test_parse_use_pkg_nested() {
    let uses = flatten_uses("use pkg::utils::format;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Pkg);
    assert_eq!(path_names(&uses[0]), ["utils", "format"]);
}

#[test]
fn test_parse_use_pkg_items() {
    // Braces are pure grouping: the tree flattens to one UseDef per leaf.
    let uses = flatten_uses("use pkg::utils::{format, parse};");
    assert_eq!(uses.len(), 2);
    assert_eq!(path_names(&uses[0]), ["utils", "format"]);
    assert_eq!(path_names(&uses[1]), ["utils", "parse"]);
}

#[test]
fn test_parse_use_nested_groups() {
    let uses = flatten_uses("use pkg::a::{b::c, d::{e, f as g}};");
    assert_eq!(uses.len(), 3);
    assert_eq!(path_names(&uses[0]), ["a", "b", "c"]);
    assert_eq!(path_names(&uses[1]), ["a", "d", "e"]);
    assert_eq!(path_names(&uses[2]), ["a", "d", "f"]);
    assert_eq!(uses[2].alias.as_ref().map(|(n, _)| n.as_ref()), Some("g"));
    assert_eq!(uses[2].local_name().map(AsRef::as_ref), Some("g"));
}

#[test]
fn test_parse_use_root_group() {
    let uses = flatten_uses("use {core::primitives::number, core::system::Stdio};");
    assert_eq!(uses.len(), 2);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Core);
    assert_eq!(path_names(&uses[0]), ["primitives", "number"]);
    assert_eq!(uses[1].prefix, ambient_engine::ast::UsePrefix::Core);
    assert_eq!(path_names(&uses[1]), ["system", "Stdio"]);
}

#[test]
fn test_parse_use_alias() {
    let uses = flatten_uses("use core::primitives::number::sqrt as root2;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Core);
    assert_eq!(path_names(&uses[0]), ["primitives", "number", "sqrt"]);
    assert_eq!(uses[0].local_name().map(AsRef::as_ref), Some("root2"));
}

#[test]
fn test_parse_use_local_root() {
    // A path rooted at a module alias from another use.
    let uses = flatten_uses("use pkg::deep::nested;\nuse nested::leaf::f;");
    assert_eq!(uses.len(), 2);
    assert_eq!(uses[1].prefix, ambient_engine::ast::UsePrefix::Local);
    assert_eq!(path_names(&uses[1]), ["nested", "leaf", "f"]);
}

#[test]
fn test_parse_use_super_chain() {
    let uses = flatten_uses("use super::super::m::f;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Super(2));
    assert_eq!(path_names(&uses[0]), ["m", "f"]);
}

#[test]
fn test_parse_use_keyword_mid_path_is_error() {
    let source = "use pkg::a::core::b;";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse ok");
    assert!(crate::lower::lower_module(&module).is_err());
}

#[test]
fn test_parse_use_in_block() {
    let source = "fn f(): Number {\n  use core::primitives::number::sqrt;\n  sqrt(16)\n}";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    let lowered = crate::lower::lower_module(&module).expect("lower error");
    let ambient_engine::ast::ItemKind::Function(f) = &lowered.items[0].kind else {
        panic!("expected function");
    };
    let ambient_engine::ast::ExprKind::Block(stmts, result) = &f.body.kind else {
        panic!("expected block body");
    };
    assert!(matches!(
        stmts[0].kind,
        ambient_engine::ast::StmtKind::Use(_)
    ));
    assert!(result.is_some());
}

#[test]
fn test_parse_use_core_system() {
    // Platform abilities live under `core::system`, an ordinary `core`
    // path — no dedicated root.
    let uses = flatten_uses("use core::system::Tcp;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Core);
    assert_eq!(path_names(&uses[0]), ["system", "Tcp"]);
}

#[test]
fn test_parse_use_pkg_named_platform() {
    // `platform` is now an ordinary identifier: a user path segment
    // `platform` under `pkg` parses as `Pkg`.
    let uses = flatten_uses("use pkg::platform;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Pkg);
    assert_eq!(path_names(&uses[0]), ["platform"]);
}

#[test]
fn test_parse_use_platform_is_local_alias() {
    // With the reserved `platform` root removed, `use platform::X`
    // parses as an alias-rooted (`Local`) path like any other bare
    // head — it no longer names a reserved root.
    let uses = flatten_uses("use platform::Tcp;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Local);
    assert_eq!(path_names(&uses[0]), ["platform", "Tcp"]);
}

#[test]
fn test_parse_use_self() {
    let uses = flatten_uses("use self::sibling;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Self_);
    assert_eq!(path_names(&uses[0]), ["sibling"]);
}

#[test]
fn test_parse_use_super() {
    let uses = flatten_uses("use super::parent;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Super(1));
    assert_eq!(path_names(&uses[0]), ["parent"]);
}

#[test]
fn test_parse_use_super_super() {
    let uses = flatten_uses("use super::super::grandparent;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Super(2));
    assert_eq!(path_names(&uses[0]), ["grandparent"]);
}

#[test]
fn test_parse_pub_use() {
    let uses = flatten_uses("pub use pkg::utils;");
    assert_eq!(uses.len(), 1);
    assert!(uses[0].is_public);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Pkg);
}

#[test]
fn test_parse_use_workspace_package() {
    let uses = flatten_uses("use ::other_pkg::utils::helper;");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Workspace);
    // The package name is the path head; nothing is consumed by the root.
    assert_eq!(path_names(&uses[0]), ["other_pkg", "utils", "helper"]);
}

#[test]
fn test_parse_use_workspace_group_and_alias() {
    let uses = flatten_uses("use ::other_pkg::{a, deep::b as c};");
    assert_eq!(uses.len(), 2);
    assert!(
        uses.iter()
            .all(|u| u.prefix == ambient_engine::ast::UsePrefix::Workspace)
    );
    assert_eq!(path_names(&uses[0]), ["other_pkg", "a"]);
    assert_eq!(path_names(&uses[1]), ["other_pkg", "deep", "b"]);
    assert_eq!(uses[1].alias.as_ref().unwrap().0.as_ref(), "c");
}

#[test]
fn test_parse_pub_use_workspace() {
    let uses = flatten_uses("pub use ::other_pkg::thing;");
    assert_eq!(uses.len(), 1);
    assert!(uses[0].is_public);
    assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Workspace);
}

#[test]
fn test_parse_use_workspace_keyword_head_is_error() {
    // Root keywords never follow the workspace `::` — `::pkg::x` is
    // malformed, not a package named `pkg`.
    let mut parser = Parser::new("use ::pkg::x;").unwrap();
    let module = parser.parse_module().expect("parse error");
    assert!(crate::lower::lower_module(&module).is_err());
}

#[test]
fn test_parse_use_double_sep_mid_path_is_error() {
    // A leading `::` on a group child that continues a path is malformed.
    let mut parser = Parser::new("use pkg::{::other::x};").unwrap();
    let module = parser.parse_module().expect("parse error");
    assert!(crate::lower::lower_module(&module).is_err());
}
