//! Parser tests for `set` declarations and the row-expression grammar.

use super::Parser;
use crate::cst::{CstItemKind, CstRowExpr};

#[test]
fn test_parse_set_union() {
    let mut parser = Parser::new("pub set IO = Stdio, FileSystem, Tcp;").unwrap();
    let module = parser.parse_module().expect("parse error");
    match &module.items[0].kind {
        CstItemKind::Set(s) => {
            assert!(s.is_public);
            assert_eq!(&*s.name.name, "IO");
            match &s.body {
                CstRowExpr::Union(parts) => {
                    assert_eq!(parts.len(), 3);
                    assert!(
                        matches!(&parts[0], CstRowExpr::Name(n) if &*n.segments[0].name == "Stdio")
                    );
                }
                other => panic!("expected a union body, got {other:?}"),
            }
        }
        other => panic!("expected a set, got {other:?}"),
    }
}

#[test]
fn test_parse_set_combinators_nest() {
    // `Difference<System, Union<Tcp, FileSystem>>` — combinators are generic-
    // style and nest; the `>>` close parses as two `Gt` tokens.
    let mut parser = Parser::new("set Safe = Difference<System, Union<Tcp, FileSystem>>;").unwrap();
    let module = parser.parse_module().expect("parse error");
    match &module.items[0].kind {
        CstItemKind::Set(s) => {
            assert!(!s.is_public);
            match &s.body {
                CstRowExpr::Difference(a, b) => {
                    assert!(
                        matches!(a.as_ref(), CstRowExpr::Name(n) if &*n.segments[0].name == "System")
                    );
                    assert!(matches!(b.as_ref(), CstRowExpr::Union(parts) if parts.len() == 2));
                }
                other => panic!("expected a difference body, got {other:?}"),
            }
        }
        other => panic!("expected a set, got {other:?}"),
    }
}

#[test]
fn test_set_is_only_a_contextual_keyword() {
    // `set` stays an ordinary path segment (the `core::collections::set`
    // module) everywhere but item-declaration position.
    let mut parser = Parser::new("fn f(): Number { core::collections::set::empty() }").unwrap();
    parser
        .parse_module()
        .expect("`set` as a path segment must still parse");
}
