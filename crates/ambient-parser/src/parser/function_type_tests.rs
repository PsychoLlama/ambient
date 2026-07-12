//! Parser tests for `with` ability rows on function *types* — the comma
//! disambiguation between an ability row and the surrounding list
//! (parameters, record fields, generic arguments).

use super::Parser;
use crate::cst::{CstItemKind, CstTypeExprKind};

/// A function-typed parameter carrying a `with` clause must not swallow the
/// parameters that follow it. Two effectful function-typed params in one
/// signature (the `State::init_versioned` shape) must each keep their own
/// single-ability `with` list and stay distinct parameters.
#[test]
fn test_effectful_fn_params_do_not_consume_next_param() {
    let source = "fn f(make: () -> S with E, migrate: (O) -> S with F, tag: String): () { () }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    let CstItemKind::Function(f) = &module.items[0].kind else {
        panic!("Expected function");
    };
    assert_eq!(f.params.len(), 3);
    assert_eq!(&*f.params[0].name.name, "make");
    assert_eq!(&*f.params[1].name.name, "migrate");
    assert_eq!(&*f.params[2].name.name, "tag");

    // Each function-typed param keeps exactly its own single-ability row.
    for (idx, expect) in [(0usize, "E"), (1, "F")] {
        match &f.params[idx].ty.as_ref().unwrap().kind {
            CstTypeExprKind::Function { abilities, .. } => {
                assert_eq!(abilities.len(), 1, "param {idx} row length");
                assert_eq!(&*abilities[0].segments[0].name, expect);
            }
            other => panic!("param {idx} should be a function type, got {other:?}"),
        }
    }
}

/// An effectful function-typed param followed by an ordinary param: the
/// `with` list stops before the next parameter name.
#[test]
fn test_effectful_fn_param_before_plain_param() {
    let source = "fn f(body: () -> () with E, name: String): () { () }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    let CstItemKind::Function(f) = &module.items[0].kind else {
        panic!("Expected function");
    };
    assert_eq!(f.params.len(), 2);
    assert_eq!(&*f.params[1].name.name, "name");
    match &f.params[0].ty.as_ref().unwrap().kind {
        CstTypeExprKind::Function { abilities, .. } => {
            assert_eq!(abilities.len(), 1);
            assert_eq!(&*abilities[0].segments[0].name, "E");
        }
        other => panic!("body should be a function type, got {other:?}"),
    }
}

/// A nested function type carrying a `with` clause inside a parameter list:
/// the inner `with` row stops at the outer `->`, and the outer param list
/// continues to the trailing param.
#[test]
fn test_nested_effectful_fn_type_in_param_list() {
    let source = "fn f(g: (() -> () with E) -> (), tag: String): () { () }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    let CstItemKind::Function(f) = &module.items[0].kind else {
        panic!("Expected function");
    };
    assert_eq!(f.params.len(), 2);
    assert_eq!(&*f.params[1].name.name, "tag");
    // Outer type is `(inner) -> ()`, itself effect-free.
    let CstTypeExprKind::Function {
        params, abilities, ..
    } = &f.params[0].ty.as_ref().unwrap().kind
    else {
        panic!("g should be a function type");
    };
    assert!(abilities.is_empty(), "outer arrow carries no abilities");
    // Its single parameter is the effectful inner function type.
    match &params[0].kind {
        CstTypeExprKind::Function { abilities, .. } => {
            assert_eq!(abilities.len(), 1);
            assert_eq!(&*abilities[0].segments[0].name, "E");
        }
        other => panic!("inner param should be a function type, got {other:?}"),
    }
}

/// A qualified ability in a function-type `with` list still parses as a
/// multi-ability row when a real second ability (not a parameter) follows.
#[test]
fn test_effectful_fn_param_multi_ability_row() {
    let source = "fn f(body: () -> () with A, B, tag: String): () { () }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    let CstItemKind::Function(f) = &module.items[0].kind else {
        panic!("Expected function");
    };
    assert_eq!(f.params.len(), 2);
    assert_eq!(&*f.params[1].name.name, "tag");
    match &f.params[0].ty.as_ref().unwrap().kind {
        CstTypeExprKind::Function { abilities, .. } => {
            let names: Vec<&str> = abilities
                .iter()
                .map(|a| a.segments[0].name.as_ref())
                .collect();
            assert_eq!(names, vec!["A", "B"]);
        }
        other => panic!("body should be a function type, got {other:?}"),
    }
}

/// A `with` clause on a function-typed *generic argument* must not swallow
/// the type argument that follows it. `Map<() -> () with E, Number>` has two
/// generic arguments — an effectful function type and `Number` — not a
/// function type with a two-ability row `E, Number`. Unlike a parameter or
/// record-field list, a generic argument list has no `name :` terminator, so
/// the comma itself must end the row.
#[test]
fn test_effectful_fn_generic_arg_before_plain_arg() {
    let source = "fn f(m: Map<() -> () with E, Number>): () { () }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    let CstItemKind::Function(f) = &module.items[0].kind else {
        panic!("Expected function");
    };
    let CstTypeExprKind::Generic { args, .. } = &f.params[0].ty.as_ref().unwrap().kind else {
        panic!("m should be a generic type");
    };
    assert_eq!(args.len(), 2, "Map has two generic arguments");
    match &args[0].kind {
        CstTypeExprKind::Function { abilities, .. } => {
            assert_eq!(abilities.len(), 1, "the function type's row is just `E`");
            assert_eq!(&*abilities[0].segments[0].name, "E");
        }
        other => panic!("first arg should be a function type, got {other:?}"),
    }
    match &args[1].kind {
        CstTypeExprKind::Name(name) => assert_eq!(&*name.segments[0].name, "Number"),
        other => panic!("second arg should be the `Number` type, got {other:?}"),
    }
}

/// A `with` clause on a function-typed *tuple element* must not swallow the
/// element that follows it. `(() -> () with E, Number)` is a 2-tuple — an
/// effectful function type and `Number` — not a 1-tuple whose sole element has
/// a two-ability row `E, Number`. Like a generic argument list, a tuple has no
/// `name :` terminator, so the comma itself must end the row.
#[test]
fn test_effectful_fn_tuple_element_before_plain_element() {
    let source = "fn f(x: (() -> () with E, Number)): () { () }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    let CstItemKind::Function(f) = &module.items[0].kind else {
        panic!("Expected function");
    };
    let CstTypeExprKind::Tuple(elements) = &f.params[0].ty.as_ref().unwrap().kind else {
        panic!("x should be a tuple type");
    };
    assert_eq!(elements.len(), 2, "tuple has two elements");
    match &elements[0].kind {
        CstTypeExprKind::Function { abilities, .. } => {
            assert_eq!(abilities.len(), 1, "the function type's row is just `E`");
            assert_eq!(&*abilities[0].segments[0].name, "E");
        }
        other => panic!("first element should be a function type, got {other:?}"),
    }
    match &elements[1].kind {
        CstTypeExprKind::Name(name) => assert_eq!(&*name.segments[0].name, "Number"),
        other => panic!("second element should be the `Number` type, got {other:?}"),
    }
}

/// A `with` clause on a function-typed element of a *function-type parameter
/// list* must not swallow the parameter that follows it. In
/// `((A) -> B with E, Number) -> X` the parenthesized parameter list has two
/// params — an effectful function type and `Number` — not one param with a
/// two-ability row `E, Number`.
#[test]
fn test_effectful_fn_type_param_before_plain_param() {
    let source = "fn f(g: ((A) -> B with E, Number) -> X): () { () }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    let CstItemKind::Function(f) = &module.items[0].kind else {
        panic!("Expected function");
    };
    let CstTypeExprKind::Function { params, .. } = &f.params[0].ty.as_ref().unwrap().kind else {
        panic!("g should be a function type");
    };
    assert_eq!(params.len(), 2, "the parameter list has two params");
    match &params[0].kind {
        CstTypeExprKind::Function { abilities, .. } => {
            assert_eq!(abilities.len(), 1, "the function type's row is just `E`");
            assert_eq!(&*abilities[0].segments[0].name, "E");
        }
        other => panic!("first param should be a function type, got {other:?}"),
    }
    match &params[1].kind {
        CstTypeExprKind::Name(name) => assert_eq!(&*name.segments[0].name, "Number"),
        other => panic!("second param should be the `Number` type, got {other:?}"),
    }
}

/// The same effectful tuple nested inside a generic argument: both the tuple's
/// comma and the generic list's `>` bound the row, so `Number` stays a distinct
/// tuple element.
#[test]
fn test_effectful_fn_tuple_element_in_generic_arg() {
    let source = "fn f(x: List<(() -> () with E, Number)>): () { () }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    let CstItemKind::Function(f) = &module.items[0].kind else {
        panic!("Expected function");
    };
    let CstTypeExprKind::Generic { args, .. } = &f.params[0].ty.as_ref().unwrap().kind else {
        panic!("x should be a generic type");
    };
    assert_eq!(args.len(), 1, "List has one generic argument");
    let CstTypeExprKind::Tuple(elements) = &args[0].kind else {
        panic!("List's argument should be a tuple");
    };
    assert_eq!(elements.len(), 2, "tuple has two elements");
    match &elements[1].kind {
        CstTypeExprKind::Name(name) => assert_eq!(&*name.segments[0].name, "Number"),
        other => panic!("second element should be `Number`, got {other:?}"),
    }
}

/// Nested generics: the inner function type's `with` row stops at the inner
/// comma, and both `>`s close cleanly (the lexer emits two `Gt`s, never a
/// shift-right token).
#[test]
fn test_effectful_fn_generic_arg_nested() {
    let source = "fn f(m: List<Map<() -> () with E, Number>>): () { () }";
    let mut parser = Parser::new(source).unwrap();
    let module = parser.parse_module().expect("parse error");
    let CstItemKind::Function(f) = &module.items[0].kind else {
        panic!("Expected function");
    };
    let CstTypeExprKind::Generic { args, .. } = &f.params[0].ty.as_ref().unwrap().kind else {
        panic!("m should be a generic type");
    };
    assert_eq!(args.len(), 1, "List has one generic argument");
    let CstTypeExprKind::Generic { args: inner, .. } = &args[0].kind else {
        panic!("List's argument should be a Map");
    };
    assert_eq!(inner.len(), 2, "inner Map has two generic arguments");
    match &inner[1].kind {
        CstTypeExprKind::Name(name) => assert_eq!(&*name.segments[0].name, "Number"),
        other => panic!("inner second arg should be `Number`, got {other:?}"),
    }
}
