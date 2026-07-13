//! Unit tests for the encoding, structural hashing, and type rendering.
//!
//! Full-build byte-stability and per-channel sensitivity live in
//! `crates/ambient-cli/tests/module_interface.rs` (they need the parser and
//! `build_package`, which depend on this crate, not the other way around).

use std::sync::Arc;

use super::ast_hash::hash_body;
use super::*;
use crate::ast::{BinaryOp, Expr, ExternFnDef, FunctionDef, Item, ItemKind, Module, Param, Span};
use crate::types::Type;

fn sample_interface() -> ModuleInterface {
    ModuleInterface {
        module: "workspace::pkg::math".to_string(),
        functions: vec![FnSig {
            name: "gcd".to_string(),
            params: vec!["named:Number".to_string(), "named:Number".to_string()],
            ret: "named:Number".to_string(),
            abilities: vec![],
        }],
        consts: vec![ConstEntry {
            name: "PI".to_string(),
            ty: "named:Number".to_string(),
            value_hash: Some([7u8; 32]),
        }],
        structs: vec![StructShape {
            name: "Vec2".to_string(),
            uuid: "a1b2c3d4-0000-0000-0000-000000000001".to_string(),
            is_extern: false,
            type_params: vec![],
            fields: vec![
                ("x".to_string(), "named:Number".to_string()),
                ("y".to_string(), "named:Number".to_string()),
            ],
        }],
        enums: vec![EnumShape {
            name: "Shape".to_string(),
            uuid: "e1b2c3d4-0000-0000-0000-000000000001".to_string(),
            type_params: vec!["T".to_string()],
            variants: vec![
                ("Circle".to_string(), Some("named:Number".to_string())),
                ("Dot".to_string(), None),
            ],
        }],
        aliases: vec![AliasShape {
            name: "Id".to_string(),
            type_params: vec![],
            target: "named:String".to_string(),
        }],
        traits: vec![TraitShape {
            name: "Show".to_string(),
            uuid: "f1b2c3d4-0000-0000-0000-000000000001".to_string(),
            type_params: vec![],
            supertraits: vec![],
            methods: vec![TraitMethodSig {
                name: "show".to_string(),
                has_self: true,
                params: vec![],
                ret: "named:String".to_string(),
                abilities: vec![],
            }],
        }],
        abilities: vec![AbilityShape {
            name: "Log".to_string(),
            ability_id: [3u8; 32],
            dependencies: vec![],
            methods: vec![AbilityMethodEntry {
                name: "info".to_string(),
                params: vec!["named:String".to_string()],
                ret: "()".to_string(),
                never: false,
                body_hash: Some([9u8; 32]),
            }],
        }],
        impls: vec![ImplShape {
            trait_ref: Some("@workspace::pkg::math::Show".to_string()),
            for_type: "nominal:a1b2c3d4-0000-0000-0000-000000000001:{}".to_string(),
            type_params: vec![],
            methods: vec![ImplMethodEntry {
                name: "show".to_string(),
                has_self: true,
                params: vec![],
                ret: "named:String".to_string(),
                abilities: vec![],
                body_hash: [11u8; 32],
            }],
        }],
        reexports: vec![ReExportEntry {
            local: "helper".to_string(),
            kind: REKIND_FUNCTION,
            target: "workspace::pkg::utils::helper".to_string(),
        }],
        externs: vec![ExternEntry {
            name: "sqrt".to_string(),
            uuid: Some("ffffffff-ffff-ffff-fffe-000000000001".to_string()),
            arity: 1,
        }],
    }
}

#[test]
fn encode_decode_roundtrip() {
    let iface = sample_interface();
    let bytes = iface.encode();
    let decoded = ModuleInterface::decode(&bytes).expect("decode");
    assert_eq!(iface, decoded);
    // Re-encoding the decoded value is byte-identical.
    assert_eq!(bytes, decoded.encode());
}

#[test]
fn decode_rejects_trailing_bytes() {
    let mut bytes = sample_interface().encode();
    bytes.push(0);
    assert_eq!(
        ModuleInterface::decode(&bytes),
        Err(InterfaceError::TrailingBytes)
    );
}

#[test]
fn decode_rejects_bad_magic() {
    let bytes = b"XXXX\x01".to_vec();
    assert_eq!(
        ModuleInterface::decode(&bytes),
        Err(InterfaceError::BadMagic)
    );
}

#[test]
fn decode_rejects_truncation() {
    let bytes = sample_interface().encode();
    assert_eq!(
        ModuleInterface::decode(&bytes[..bytes.len() - 4]),
        Err(InterfaceError::Truncated)
    );
}

#[test]
fn interface_hash_is_deterministic() {
    let a = sample_interface();
    let b = sample_interface();
    assert_eq!(a.interface_hash(), b.interface_hash());
}

#[test]
fn interface_hash_flips_on_any_field_change() {
    let base = sample_interface();
    let base_hash = base.interface_hash();

    let mut edited = sample_interface();
    edited.functions[0].ret = "named:String".to_string();
    assert_ne!(base_hash, edited.interface_hash());

    let mut edited = sample_interface();
    edited.impls[0].methods[0].body_hash = [12u8; 32];
    assert_ne!(base_hash, edited.interface_hash());

    let mut edited = sample_interface();
    edited.consts[0].value_hash = Some([8u8; 32]);
    assert_ne!(base_hash, edited.interface_hash());
}

// ─────────────────────────────────────────────────────────────────────────────
// Type rendering
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn render_type_is_total_and_deterministic() {
    // Every "pre-check-only" variant renders without panicking.
    for ty in [
        Type::Unit,
        Type::Never,
        Type::Hole,
        Type::Param(Arc::from("T")),
        Type::number(),
        Type::list(Type::string()),
        Type::tuple(vec![Type::bool(), Type::Unit]),
    ] {
        assert_eq!(render_type(&ty), render_type(&ty.clone()));
    }
    // A bare, unresolved nominal head renders by name.
    assert_eq!(
        render_type(&Type::named_simple("Number")),
        "named:Number".to_string()
    );
    // Distinct primitives render distinctly.
    assert_ne!(render_type(&Type::number()), render_type(&Type::string()));
}

// ─────────────────────────────────────────────────────────────────────────────
// Structural body / module hashing
// ─────────────────────────────────────────────────────────────────────────────

fn param(id: u32, name: &str) -> Param {
    Param::new(id, name)
}

fn add_body(a: u32, b: u32) -> Expr {
    Expr::binary(BinaryOp::Add, Expr::local(a), Expr::local(b))
}

#[test]
fn body_hash_ignores_absolute_binding_ids() {
    // Same shape, different (module-global) binding ids → same hash. This is
    // what stops an unrelated edit that renumbers later bindings from
    // spuriously invalidating an impl-method body hash.
    let h1 = hash_body(&[param(0, "x"), param(1, "y")], &add_body(0, 1));
    let h2 = hash_body(&[param(40, "x"), param(41, "y")], &add_body(40, 41));
    assert_eq!(h1, h2);
}

#[test]
fn body_hash_ignores_local_names() {
    // Renaming a parameter is not a content change.
    let h1 = hash_body(&[param(0, "x"), param(1, "y")], &add_body(0, 1));
    let h2 = hash_body(&[param(0, "a"), param(1, "b")], &add_body(0, 1));
    assert_eq!(h1, h2);
}

#[test]
fn body_hash_flips_on_shape_change() {
    let add = hash_body(&[param(0, "x"), param(1, "y")], &add_body(0, 1));
    let sub = hash_body(
        &[param(0, "x"), param(1, "y")],
        &Expr::binary(BinaryOp::Sub, Expr::local(0), Expr::local(1)),
    );
    assert_ne!(add, sub);
}

fn fn_item(name: &str, is_public: bool, body: Expr) -> Item {
    Item::new(
        ItemKind::Function(FunctionDef {
            name: Arc::from(name),
            name_span: Span::default(),
            is_public,
            type_params: vec![],
            params: vec![param(0, "x")],
            ret_ty: Some(Type::number()),
            abilities: vec![],
            body,
        }),
        Span::default(),
    )
}

fn module(items: Vec<Item>) -> Module {
    Module {
        name: Arc::from("m"),
        doc: None,
        items,
    }
}

#[test]
fn module_ast_hash_is_order_independent() {
    let a = fn_item("a", true, Expr::local(0));
    let b = fn_item("b", false, Expr::number(1.0));
    let forward = module_ast_hash(&module(vec![a.clone(), b.clone()]));
    let reversed = module_ast_hash(&module(vec![b, a]));
    assert_eq!(forward, reversed);
}

#[test]
fn module_ast_hash_flips_on_private_body_change() {
    let base = module_ast_hash(&module(vec![fn_item("a", false, Expr::number(1.0))]));
    let edited = module_ast_hash(&module(vec![fn_item("a", false, Expr::number(2.0))]));
    assert_ne!(base, edited);
}

#[test]
fn module_ast_hash_flips_on_pubness_toggle() {
    let private = module_ast_hash(&module(vec![fn_item("a", false, Expr::number(1.0))]));
    let public = module_ast_hash(&module(vec![fn_item("a", true, Expr::number(1.0))]));
    assert_ne!(private, public);
}

#[test]
fn extern_fn_item_hashes() {
    // An extern fn item participates in the module hash (no body to walk).
    let item = Item::new(
        ItemKind::ExternFn(ExternFnDef {
            name: Arc::from("sqrt"),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            params: vec![Param::with_type(0, "x", Type::number())],
            ret_ty: Type::number(),
        }),
        Span::default(),
    );
    let a = module_ast_hash(&module(vec![item.clone()]));
    let b = module_ast_hash(&module(vec![item]));
    assert_eq!(a, b);
}
