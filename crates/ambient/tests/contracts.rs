//! Contracts: what a record holds a value under without reserving a name,
//! run on every backend. See `docs/spec/methods.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use support::{run, value, workspace};

/// `Label`, a contract that holds a string, and `Area`, one that holds a
/// function giving an integer.
fn contracts() -> [Definition; 2] {
    [
        b::contract("Label", ty::string()),
        b::contract("Area", ty::function([], ty::int())),
    ]
}

fn main(body: Expr) -> Definition {
    b::function("main").returns(ty::unknown()).body(body)
}

fn evaluates(body: Expr) -> Value {
    let [label, area] = contracts();
    run([label, area, main(body)], &[]).unwrap()
}

/// The error checking `definition` gives beside the contracts.
fn refused(definition: Definition) -> Error {
    let [label, area] = contracts();
    let module = b::module().define(label).define(area).define(definition);
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    let name = definition_name();
    Engine::new(&workspace, &store)
        .check(&value("main", name))
        .unwrap_err()
}

fn definition_name() -> &'static str {
    "main"
}

/// `{label: 1}` with `value` under `Label` besides.
fn labelled(value: Expr) -> Expr {
    b::keyed(b::record([("label", b::int(1))]), "Label", value)
}

/// The type of a record that holds a `ty` under `Label`.
fn labels(ty: Type) -> Type {
    ty::keyed(ty::record([]), "Label", ty)
}

#[test]
fn a_record_holds_a_value_under_a_contract_beside_a_field_of_that_name() {
    let record = || labelled(b::string("one"));
    let both = b::tuple([b::at(record(), "Label"), b::field(record(), "label")]);
    let expected = Value::Tuple(vec![Value::String("one".into()), Value::Int(1)]);
    assert_eq!(evaluates(both), expected);
}

#[test]
fn a_record_holds_under_a_contract_only_a_value_of_its_type() {
    let error = refused(main(labelled(b::int(2))));
    assert!(matches!(error, Error::ContractType { name, .. } if name == "[Label]"));
}

#[test]
fn a_record_type_holds_under_a_contract_only_its_type() {
    let takes = b::function("main")
        .param("record", labels(ty::int()))
        .returns(ty::int())
        .body(b::int(0));
    assert!(matches!(refused(takes), Error::ContractType { .. }));
}

#[test]
fn a_function_asks_for_a_contract_of_any_record() {
    let label = b::function("label")
        .param("record", labels(ty::string()))
        .returns(ty::string())
        .body(b::at(b::name("record"), "Label"));
    let [key, area] = contracts();
    let body = b::call(b::name("label"), [labelled(b::string("one"))]);
    assert_eq!(
        run([key, area, label, main(body)], &[]).unwrap(),
        Value::String("one".into())
    );
}

#[test]
fn a_method_under_a_contract_receives_its_record() {
    let side = || b::field(b::name("self"), "side");
    let area = b::lambda([("self", None)], b::multiply(side(), side()));
    let square = b::keyed(b::record([("side", b::int(3))]), "Area", b::method(area));
    assert_eq!(evaluates(b::send_key(square, "Area", [])), Value::Int(9));
}

#[test]
fn a_contract_is_not_an_ability() {
    let performs = main(b::perform("Label", []));
    assert!(matches!(refused(performs), Error::UnknownName { .. }));
}
