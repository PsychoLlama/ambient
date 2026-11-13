//! The methods of iterators, and the functions that make one. See
//! `spec/iteration.md`.
//!
//! An iterator is a built-in shape, and making and stepping one are
//! the language's. Everything that transforms or folds one is written
//! here on top of a step, as any program could write it.

use crate::builder::{self as b, FunctionBuilder, ModuleBuilder, pat, ty};
use crate::surface::{Expr, Type};

fn member() -> Type {
    ty::named("T")
}

fn iterator_of(member: Type) -> Type {
    ty::iterator(member)
}

/// What a step gives: nothing, or an item and the iterator of the rest.
fn step_of(member: Type) -> Type {
    let rest = iterator_of(member.clone());
    ty::union([ty::null(), ty::tuple([member, rest])])
}

/// An iterator whose step is `body`.
fn stepping(body: Expr) -> Expr {
    b::iterator(b::lambda([], body))
}

/// `body` with `item` and `rest` bound to what stepping `iterator`
/// gives, or `otherwise` when nothing is left.
fn stepped(iterator: &str, otherwise: Expr, body: Expr) -> Expr {
    let step = b::step(b::name(iterator));
    let pair = pat::tuple([pat::bind("item"), pat::bind("rest")]);
    b::match_(step, [(pat::is(ty::null()), otherwise), (pair, body)])
}

fn call<const N: usize>(function: &str, args: [Expr; N]) -> Expr {
    b::call(b::name(function), args)
}

fn names<const N: usize>(names: [&str; N]) -> [Expr; N] {
    names.map(b::name)
}

/// A function of iterators of `T`: public if it is a method.
fn over(name: &str, receiver: &str) -> FunctionBuilder {
    b::function(name)
        .type_param("T")
        .param(receiver, iterator_of(member()))
}

fn method(name: &str) -> FunctionBuilder {
    over(name, "self").public()
}

/// `fn from<T>(step: () -> null | (T, Iterator<T>)): Iterator<T>`
fn from() -> crate::surface::Definition {
    b::function("from")
        .public()
        .type_param("T")
        .param("step", ty::function([], step_of(member())))
        .returns(iterator_of(member()))
        .body(b::iterator(b::name("step")))
}

/// `fn step<T>(self: Iterator<T>): null | (T, Iterator<T>)`
fn step() -> crate::surface::Definition {
    method("step")
        .returns(step_of(member()))
        .body(b::step(b::name("self")))
}

/// `fn range(start: int, end: int): Iterator<int>`, each integer from
/// `start` up to `end`, which it leaves out.
fn range() -> crate::surface::Definition {
    b::function("range")
        .public()
        .param("start", ty::int())
        .param("end", ty::int())
        .returns(iterator_of(ty::int()))
        .body(b::range(b::name("start"), b::name("end")))
}

/// `fn map<T, U>(self: Iterator<T>, f: (T) -> U): Iterator<U>`
fn map() -> crate::surface::Definition {
    let mapped = b::call(b::name("f"), [b::name("item")]);
    let more = b::tuple([mapped, call("map", names(["rest", "f"]))]);
    method("map")
        .type_param("U")
        .param("f", ty::function([member()], ty::named("U")))
        .returns(iterator_of(ty::named("U")))
        .body(stepping(stepped("self", b::null(), more)))
}

fn keeps() -> Type {
    ty::function([member()], ty::bool())
}

/// `fn filter<T>(self: Iterator<T>, keep: (T) -> bool): Iterator<T>`
fn filter() -> crate::surface::Definition {
    method("filter")
        .param("keep", keeps())
        .returns(iterator_of(member()))
        .body(stepping(call("seek", names(["self", "keep"]))))
}

/// The first item of `from` that `keep` takes, and a filter of what
/// follows it. It calls itself in tail position past each item `keep`
/// refuses, so a long run of them takes no stack.
fn seek() -> crate::surface::Definition {
    let kept = b::tuple([b::name("item"), call("filter", names(["rest", "keep"]))]);
    let decide = b::match_(
        b::call(b::name("keep"), [b::name("item")]),
        [
            (pat::boolean(true), kept),
            (pat::boolean(false), call("seek", names(["rest", "keep"]))),
        ],
    );
    over("seek", "from")
        .param("keep", keeps())
        .returns(step_of(member()))
        .body(stepped("from", b::null(), decide))
}

/// `fn take<T>(self: Iterator<T>, count: int): Iterator<T>`, the first
/// `count` items.
fn take() -> crate::surface::Definition {
    let fewer = b::subtract(b::name("count"), b::int(1));
    let more = b::tuple([b::name("item"), call("take", [b::name("rest"), fewer])]);
    let step = b::match_(
        b::less(b::int(0), b::name("count")),
        [
            (pat::boolean(true), stepped("self", b::null(), more)),
            (pat::boolean(false), b::null()),
        ],
    );
    method("take")
        .param("count", ty::int())
        .returns(iterator_of(member()))
        .body(stepping(step))
}

/// `fn fold<T, A>(self: Iterator<T>, initial: A, f: (A, T) -> A): A`
fn fold() -> crate::surface::Definition {
    let folded = b::call(b::name("f"), names(["initial", "item"]));
    let more = call("fold", [b::name("rest"), folded, b::name("f")]);
    method("fold")
        .type_param("A")
        .param("initial", ty::named("A"))
        .param(
            "f",
            ty::function([ty::named("A"), member()], ty::named("A")),
        )
        .returns(ty::named("A"))
        .body(stepped("self", b::name("initial"), more))
}

/// `fn to_list<T>(self: Iterator<T>): List<T>`, every item in order.
fn to_list() -> crate::surface::Definition {
    method("to_list")
        .returns(ty::list(member()))
        .body(call("collect", [b::name("self"), b::list([])]))
}

/// `into` followed by every item of `from`.
fn collect() -> crate::surface::Definition {
    let pushed = b::push(b::name("into"), b::name("item"));
    over("collect", "from")
        .param("into", ty::list(member()))
        .returns(ty::list(member()))
        .body(stepped(
            "from",
            b::name("into"),
            call("collect", [b::name("rest"), pushed]),
        ))
}

pub(super) fn module() -> ModuleBuilder {
    let definitions = [
        from(),
        step(),
        range(),
        map(),
        filter(),
        seek(),
        take(),
        fold(),
        to_list(),
        collect(),
    ];
    definitions
        .into_iter()
        .fold(b::module(), ModuleBuilder::define)
}
