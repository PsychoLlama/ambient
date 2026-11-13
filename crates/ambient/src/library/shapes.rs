//! The methods of the built-in shapes: for each shape a module, whose
//! public functions that take the shape first are its methods. See
//! `spec/methods.md`.
//!
//! Each of these wraps an operation (`spec/operations.md`), which is
//! what gives the operation a name to be called by and a function to
//! be passed.

use crate::builder::{self as b, FunctionBuilder, ModuleBuilder, ty};
use crate::surface::{Expr, Shape, Type};

fn this() -> Expr {
    b::name("self")
}

/// A public function `name` whose first parameter is `self`.
fn method(name: &str, receiver: Type) -> FunctionBuilder {
    b::function(name).public().param("self", receiver)
}

/// A type that a miss may stand in for.
fn or_null(ty: Type) -> Type {
    ty::union([ty, ty::null()])
}

fn int() -> ModuleBuilder {
    b::module()
        .define(
            method("to_float", ty::int())
                .returns(ty::float())
                .body(b::to_float(this())),
        )
        .define(
            method("to_text", ty::int())
                .returns(ty::string())
                .body(b::to_text(this())),
        )
}

fn float() -> ModuleBuilder {
    b::module()
        .define(
            method("to_int", ty::float())
                .returns(or_null(ty::int()))
                .body(b::to_int(this())),
        )
        .define(
            method("to_text", ty::float())
                .returns(ty::string())
                .body(b::to_text(this())),
        )
}

/// What a string and a list both have, given the type of either and a
/// function to start each method with.
fn sequence(of: &Type, method: &dyn Fn(&str) -> FunctionBuilder) -> ModuleBuilder {
    let slice = b::slice(this(), b::name("start"), b::name("end"));
    b::module()
        .define(method("length").returns(ty::int()).body(b::length(this())))
        .define(
            method("join")
                .param("other", of.clone())
                .returns(of.clone())
                .body(b::join(this(), b::name("other"))),
        )
        .define(
            method("slice")
                .param("start", ty::int())
                .param("end", ty::int())
                .returns(or_null(of.clone()))
                .body(slice),
        )
}

fn string() -> ModuleBuilder {
    let method = |name: &str| method(name, ty::string());
    sequence(&ty::string(), &method)
        .define(
            method("to_int")
                .returns(or_null(ty::int()))
                .body(b::to_int(this())),
        )
        .define(
            method("to_float")
                .returns(or_null(ty::float()))
                .body(b::to_float(this())),
        )
}

fn list() -> ModuleBuilder {
    let member = || ty::named("T");
    let list = ty::list(member());
    let method = |name: &str| method(name, list.clone()).type_param("T");
    sequence(&list, &method)
        .define(
            method("get")
                .param("position", ty::int())
                .returns(or_null(member()))
                .body(b::get(this(), b::name("position"))),
        )
        .define(
            method("push")
                .param("member", member())
                .returns(list.clone())
                .body(b::push(this(), b::name("member"))),
        )
}

/// The module of each shape that has methods yet.
pub(super) fn modules() -> Vec<(Shape, ModuleBuilder)> {
    vec![
        (Shape::Int, int()),
        (Shape::Float, float()),
        (Shape::String, string()),
        (Shape::List, list()),
    ]
}
