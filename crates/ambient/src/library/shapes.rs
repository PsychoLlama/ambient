//! The methods of the built-in shapes: for each shape a module, whose
//! public functions that take the shape first are its methods. See
//! `spec/methods.md`.
//!
//! Each of these wraps an operation (`spec/operations.md`), which is
//! what gives the operation a name to be called by and a function to
//! be passed.

use super::iterator::stepping;
use crate::builder::{self as b, FunctionBuilder, ModuleBuilder, pat, ty};
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

/// The iterator over the members of `list` from `position` on.
///
/// A member that is null and a miss past the end look alike to `get`,
/// so a null is told from the end by the list's length. The checker
/// cannot follow that, so the null is tested against the member type:
/// the one case in which walking a list looks at a type as it runs.
fn walk(list: &Type, member: &Type) -> crate::surface::Definition {
    let rest = || {
        let next = b::add(b::name("position"), b::int(1));
        b::call(b::name("walk"), [this(), next])
    };
    let null_member = b::match_(
        b::null(),
        [
            (
                pat::bind_as("item", pat::is(member.clone())),
                b::tuple([b::name("item"), rest()]),
            ),
            (pat::any(), b::null()),
        ],
    );
    let within = b::less(b::name("position"), b::length(this()));
    let missed = b::match_(
        within,
        [
            (pat::boolean(true), null_member),
            (pat::boolean(false), b::null()),
        ],
    );
    let step = b::match_(
        b::get(this(), b::name("position")),
        [
            (pat::is(ty::null()), missed),
            (pat::bind("item"), b::tuple([b::name("item"), rest()])),
        ],
    );
    b::function("walk")
        .type_param("T")
        .param("self", list.clone())
        .param("position", ty::int())
        .returns(ty::apply("iterator.Iterator", [member.clone()]))
        .body(stepping(step))
}

fn list() -> ModuleBuilder {
    let member = || ty::named("T");
    let list = ty::list(member());
    let method = |name: &str| method(name, list.clone()).type_param("T");
    let iter = method("iter")
        .returns(ty::apply("iterator.Iterator", [member()]))
        .body(b::call(b::name("walk"), [this(), b::int(0)]));
    sequence(&list, &method)
        .import("pkg.iterator")
        .define(walk(&list, &member()))
        .define(iter)
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
