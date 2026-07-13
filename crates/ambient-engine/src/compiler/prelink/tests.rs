//! Round-trip and replay-determinism coverage for the pre-link symbolic form.

use std::sync::Arc;

use crate::ast::{BinaryOp, Expr, FunctionDef, Item, ItemKind, Module, Param, Span};
use crate::compiler::{PrelinkModule, assemble_module, compile_module_capturing};

/// A two-function module: `run` calls `double`, so the symbolic form exercises
/// named functions and a cross-function call reference.
fn sample_module() -> Module {
    Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("double"),
                    name_span: Span::default(),
                    is_public: false,
                    type_params: vec![],
                    params: vec![Param::new(0, "x")],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::binary(BinaryOp::Mul, Expr::local(0), Expr::number(2.0)),
                }),
                Span::default(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::call(Expr::name("double"), vec![Expr::number(21.0)]),
                }),
                Span::default(),
            ),
        ],
    }
}

#[test]
fn prelink_encode_decode_round_trips() {
    let module = sample_module();
    let (_, prelink) =
        compile_module_capturing(&module, crate::compiler::CompileOptions::default())
            .expect("compile");

    let bytes = prelink.encode().expect("encode");
    let decoded = PrelinkModule::decode(&bytes).expect("decode");
    // decode ∘ encode is the identity at the byte level.
    assert_eq!(bytes, decoded.encode().expect("re-encode"));
}

#[test]
fn reassembling_prelink_matches_the_cold_compile() {
    let module = sample_module();
    let (cold, prelink) =
        compile_module_capturing(&module, crate::compiler::CompileOptions::default())
            .expect("compile");

    // Replaying the persisted symbolic form (decoded from bytes) through the
    // shared `assemble_module` reproduces the module byte-for-byte.
    let bytes = prelink.encode().expect("encode");
    let decoded = PrelinkModule::decode(&bytes).expect("decode");
    let replayed = assemble_module(decoded.to_assemble_inputs()).expect("assemble");

    assert_eq!(object_hashes(&cold), object_hashes(&replayed));
    assert_eq!(names(&cold), names(&replayed));
    assert_eq!(cold.entry_point, replayed.entry_point);
    assert_eq!(lambda_parents(&cold), lambda_parents(&replayed));
}

#[test]
fn remap_moves_the_final_hash_and_stays_deterministic() {
    let module = sample_module();
    let (_, prelink) =
        compile_module_capturing(&module, crate::compiler::CompileOptions::default())
            .expect("compile");

    // `double`'s temporary hash is the reference `run` embeds. Remapping it to
    // a fresh hash must change the assembled objects (it is a genuine input to
    // finalization), and doing it twice must land identically.
    let temp = prelink
        .functions
        .iter()
        .find(|f| &*f.name == "double")
        .expect("double present")
        .func
        .hash;
    let new_hash = blake3::hash(b"a different double body");
    let remap = std::iter::once((temp, new_hash)).collect();

    let baseline = assemble_module(prelink.to_assemble_inputs()).expect("baseline");

    let mut a = prelink.clone();
    a.remap(&remap);
    let assembled_a = assemble_module(a.to_assemble_inputs()).expect("assemble a");

    let mut b = prelink.clone();
    b.remap(&remap);
    let assembled_b = assemble_module(b.to_assemble_inputs()).expect("assemble b");

    assert_ne!(
        object_hashes(&baseline),
        object_hashes(&assembled_a),
        "remapping a referenced hash must move the dependent's objects"
    );
    assert_eq!(
        object_hashes(&assembled_a),
        object_hashes(&assembled_b),
        "remap + assemble must be deterministic"
    );
}

fn object_hashes(m: &crate::compiler::CompiledModule) -> Vec<[u8; 32]> {
    let mut hashes: Vec<[u8; 32]> = m.objects.keys().map(|h| *h.as_bytes()).collect();
    hashes.sort_unstable();
    hashes
}

fn names(m: &crate::compiler::CompiledModule) -> Vec<(String, [u8; 32])> {
    let mut out: Vec<(String, [u8; 32])> = m
        .function_names
        .iter()
        .map(|(n, h)| (n.to_string(), *h.as_bytes()))
        .collect();
    out.sort();
    out
}

fn lambda_parents(m: &crate::compiler::CompiledModule) -> Vec<(String, [u8; 32])> {
    let mut out: Vec<(String, [u8; 32])> = m
        .lambda_parents
        .iter()
        .map(|(h, p)| (p.to_string(), *h.as_bytes()))
        .collect();
    out.sort();
    out
}
