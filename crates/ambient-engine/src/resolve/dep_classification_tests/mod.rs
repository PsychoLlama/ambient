//! Phase-2 dependency-classification tests.
//!
//! The resolve pass now records two dependency sets on [`ResolveOutcome`]:
//! the full `deps` (every reference, value *and* type, plus `use` imports)
//! and its link-order subset `link_deps` (only value/symbol positions the
//! compiler emits a link-time artifact for). These tests pin the boundary:
//!
//! - value positions (calls, consts, variant/unit-struct construction,
//!   variant *patterns*, scoped/explicit variant paths, module-alias method
//!   calls, ability performs — bare and fully qualified, block-scoped-`use`
//!   references) land in **both** sets;
//! - pure type positions (qualified type annotations, impl targets/headers,
//!   the module-level `use` statement itself, associated `Type::method`
//!   calls, and typed-record construction — the compiler discards the type
//!   name) land in `deps` **only**;
//! - some forms record **no** resolve-pass dep at all, pinned with the why:
//!   trait bounds (a bound needs only the upfront-registered definition),
//!   bare imported type annotations (the `use` records the edge, not the
//!   annotation), unreferenced module aliases, and block-scoped `use`
//!   statements themselves;
//! - `link_deps ⊆ deps` on every fixture.
//!
//! `link_deps`'s sole consumer is the compile-ordering graph
//! (`dispatch_ordering_graph`); these guard the boundary it keys off.

use std::sync::Arc;

use super::{ResolveOutcome, resolve_module};
use crate::ast::{
    AbilityCall, AbilityDef, ConstDef, EnumDef, EnumVariant, Expr, ExprKind, FunctionDef, ImplDef,
    Item, ItemKind, LetBinding, MatchArm, Param, Pattern, PatternKind, QualifiedName, Span, Stmt,
    StmtKind, StructDef, TraitDef, TypeParam, UseDef, UsePrefix,
};
use crate::fqn::ModuleId;
use crate::module_path::ModulePath;
use crate::module_registry::ModuleRegistry;
use crate::types::{NamedType, NominalType, RecordType, Type};

mod forms;
mod positions;

// ─────────────────────────────────────────────────────────────────────────
// Fixture builders
// ─────────────────────────────────────────────────────────────────────────

fn func(name: &str, body: Expr, abilities: Vec<QualifiedName>) -> Item {
    Item::new(
        ItemKind::Function(FunctionDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            abilities,
            body,
        }),
        Span::default(),
    )
}

/// A public function with one annotated parameter (`fn f(x: <ty>) { unit }`) —
/// used to exercise type-position annotations.
fn func_with_param(name: &str, param_ty: Type) -> Item {
    Item::new(
        ItemKind::Function(FunctionDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            params: vec![Param {
                id: 0,
                name: Arc::from("x"),
                ty: Some(param_ty),
                span: Span::default(),
            }],
            ret_ty: None,
            abilities: vec![],
            body: Expr::unit(),
        }),
        Span::default(),
    )
}

fn const_item(name: &str) -> Item {
    Item::new(
        ItemKind::Const(ConstDef {
            id: 0,
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            ty: Some(Type::number()),
            value: Expr::number(1.0),
        }),
        Span::default(),
    )
}

fn enum_item(name: &str, variant: &str, uuid: u128) -> Item {
    Item::new(
        ItemKind::Enum(EnumDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            variants: vec![EnumVariant {
                name: Arc::from(variant),
                payload: None,
                span: Span::default(),
            }],
            uuid: uuid::Uuid::from_u128(uuid),
        }),
        Span::default(),
    )
}

/// A `unique(uuid) struct <name>;` — a unit struct whose bare name is a value.
fn unit_struct(name: &str, uuid: u128) -> Item {
    let id = uuid::Uuid::from_u128(uuid);
    Item::new(
        ItemKind::Struct(StructDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            ty: Type::Nominal(NominalType::new(
                id,
                Type::Record(RecordType::new(vec![])),
                Some(name),
            )),
            unique_id: Some(id),
            is_extern: false,
        }),
        Span::default(),
    )
}

/// A plain fielded struct (`struct <name> { v: Number }`) — a type that is
/// constructed with a typed record, not by its bare name.
fn record_struct(name: &str) -> Item {
    Item::new(
        ItemKind::Struct(StructDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            ty: Type::record([("v", Type::number())]),
            unique_id: None,
            is_extern: false,
        }),
        Span::default(),
    )
}

fn ability_item(name: &str, uuid: u128) -> Item {
    Item::new(
        ItemKind::Ability(AbilityDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            dependencies: vec![],
            methods: vec![],
            uuid: uuid::Uuid::from_u128(uuid),
            resolved_id: None,
        }),
        Span::default(),
    )
}

fn trait_item(name: &str, uuid: u128) -> Item {
    Item::new(
        ItemKind::Trait(TraitDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            uuid: uuid::Uuid::from_u128(uuid),
            type_params: vec![],
            supertraits: vec![],
            methods: vec![],
        }),
        Span::default(),
    )
}

/// `use pkg::defs::<name>;`
fn use_from_defs(name: &str) -> Item {
    Item::new(
        ItemKind::Use(UseDef {
            is_public: false,
            prefix: UsePrefix::Pkg,
            path: vec![
                (Arc::from("defs"), Span::default()),
                (Arc::from(name), Span::default()),
            ],
            alias: None,
        }),
        Span::default(),
    )
}

fn named(name: &str) -> Type {
    Type::Named(NamedType {
        name: Arc::from(name),
        args: vec![],
        uuid: None,
    })
}

fn name_expr(qn: QualifiedName) -> Expr {
    Expr {
        kind: ExprKind::Name(qn),
        span: Span::default(),
        ty: None,
        dicts: None,
    }
}

fn perform_expr(ability: QualifiedName, method: &str) -> Expr {
    Expr {
        kind: ExprKind::Perform(AbilityCall {
            ability: Some(ability),
            method: Arc::from(method),
            method_span: Span::default(),
            args: vec![],
            fingerprints: None,
            span: Span::default(),
        }),
        span: Span::default(),
        ty: None,
        dicts: None,
    }
}

fn typed_record_expr(type_name: QualifiedName) -> Expr {
    Expr {
        kind: ExprKind::TypedRecord {
            type_name,
            fields: vec![],
        },
        span: Span::default(),
        ty: None,
        dicts: None,
    }
}

/// Register a `defs` module and a `main` module referencing it, resolve
/// `main`, and return its outcome alongside `defs`'s [`ModuleId`].
fn resolve_main(defs_items: Vec<Item>, main_items: Vec<Item>) -> (ResolveOutcome, ModuleId) {
    let (outcome, defs, _) = resolve_main_module(defs_items, main_items);
    (outcome, defs)
}

/// [`resolve_main`], additionally returning the resolved `main` module so a
/// "records no dep" test can assert the reference *did* resolve (guarding
/// against a vacuous pass on a malformed fixture).
fn resolve_main_module(
    defs_items: Vec<Item>,
    main_items: Vec<Item>,
) -> (ResolveOutcome, ModuleId, crate::ast::Module) {
    let mut registry = ModuleRegistry::new();
    let defs_path = ModulePath::from_str_segments(&["defs"]).unwrap();
    let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
    let mut main = crate::ast::Module {
        name: Arc::from("main"),
        doc: None,
        items: main_items,
    };
    registry.register(
        &defs_path,
        Arc::new(crate::ast::Module {
            name: Arc::from("defs"),
            doc: None,
            items: defs_items,
        }),
    );
    registry.register(&main_path, Arc::new(main.clone()));
    let outcome = resolve_module(&mut main, &main_path, &registry);
    let defs_id = registry.module_id(&defs_path);
    (outcome, defs_id, main)
}

/// Assert the superset invariant on every outcome.
fn assert_subset(outcome: &ResolveOutcome) {
    assert!(
        outcome.link_deps.is_subset(&outcome.deps),
        "link_deps ({:?}) must be a subset of deps ({:?})",
        outcome.link_deps,
        outcome.deps,
    );
}

/// `fn describe(self): Number { 1 }` as an impl method.
fn describe_method() -> crate::ast::ImplMethod {
    crate::ast::ImplMethod {
        name: Arc::from("describe"),
        name_span: Span::default(),
        type_params: vec![],
        has_self: true,
        self_id: 0,
        params: vec![],
        ret_ty: Some(Type::number()),
        abilities: vec![],
        body: Expr::number(1.0),
        span: Span::default(),
        resolved_symbol: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Additional fixture builders for the remaining reference forms
// ─────────────────────────────────────────────────────────────────────────

/// `use pkg::defs;` — a *module* import, binding the alias `defs`.
fn use_defs_module() -> Item {
    Item::new(
        ItemKind::Use(UseDef {
            is_public: false,
            prefix: UsePrefix::Pkg,
            path: vec![(Arc::from("defs"), Span::default())],
            alias: None,
        }),
        Span::default(),
    )
}

/// `match 1 { <pattern> => () }`.
fn match_expr(pattern: Pattern) -> Expr {
    Expr {
        kind: ExprKind::Match(
            Box::new(Expr::number(1.0)),
            vec![MatchArm::new(pattern, Expr::unit())],
        ),
        span: Span::default(),
        ty: None,
        dicts: None,
    }
}

/// A variant pattern spelled by an arbitrary `QualifiedName`.
fn variant_pattern(qn: QualifiedName) -> Pattern {
    Pattern::new(PatternKind::Variant(qn, None), Span::default())
}

/// `<receiver>.<method>()` — a method call on a bare name, the shape the
/// module-alias rewrite in `walk` disambiguates.
fn method_call_expr(receiver: &str, method: &str) -> Expr {
    Expr {
        kind: ExprKind::MethodCall {
            receiver: Box::new(Expr::name(receiver)),
            method: Arc::from(method),
            method_span: Span::default(),
            args: vec![],
            resolved_method: None,
        },
        span: Span::default(),
        ty: None,
        dicts: None,
    }
}

/// `{ <stmts>; <result> }` as an expression.
fn block_expr(stmts: Vec<Stmt>, result: Option<Expr>) -> Expr {
    Expr {
        kind: ExprKind::Block(stmts, result.map(Box::new)),
        span: Span::default(),
        ty: None,
        dicts: None,
    }
}

/// `use pkg::defs::<name>;` as a *statement* (block-scoped import).
fn use_from_defs_stmt(name: &str) -> Stmt {
    let ItemKind::Use(use_def) = use_from_defs(name).kind else {
        unreachable!()
    };
    Stmt::new(StmtKind::Use(use_def), Span::default())
}

/// `pub fn <name><T: bound>() { unit }` — a generic function whose single
/// type parameter carries `bound`.
fn bounded_func(name: &str, bound: QualifiedName) -> Item {
    let bound = crate::ast::TraitRef {
        name: bound,
        args: vec![],
    };
    Item::new(
        ItemKind::Function(FunctionDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![TypeParam {
                name: Arc::from("T"),
                is_ability: false,
                bounds: vec![bound],
                span: Span::default(),
            }],
            params: vec![],
            ret_ty: None,
            abilities: vec![],
            body: Expr::unit(),
        }),
        Span::default(),
    )
}

// ─────────────────────────────────────────────────────────────────────────
// Enum-variant patterns and scoped variant paths
