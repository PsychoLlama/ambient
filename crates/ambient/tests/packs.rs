//! Packs: a definition sent from one build and received by another. See
//! `spec/exchange.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::backend::native::Natives;
use ambient::builder::{self as b, ModuleBuilder, ty};
use ambient::core::{Expr, Tree};
use ambient::engine::{Engine, Stage};
use ambient::error::Error;
use ambient::pack::{Pack, Refused, Target};
use ambient::store::MemoryStore;
use ambient::surface::Definition;
use ambient::value::{Function, Value};
use ambient::workspace::Workspace;
use support::{run_entry, value, workspace};

/// `fn double(n: int): int`
fn double() -> Definition {
    b::function("double")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::add(b::name("n"), b::name("n")))
}

/// `fn main(): int`, which is `double(21)`.
fn main() -> Definition {
    b::function("main")
        .returns(ty::int())
        .body(b::call(b::name("double"), [b::int(21)]))
}

fn module(definitions: impl IntoIterator<Item = Definition>) -> Workspace {
    let module = definitions
        .into_iter()
        .fold(b::module(), ModuleBuilder::define);
    workspace([("app", "main", module)])
}

/// Packs `main` of `sender`, leaving out what `has` names there.
fn packed(sender: &Workspace, has: &[&str]) -> Pack {
    let store = MemoryStore::new();
    let engine = Engine::new(sender, &store);
    let has: Vec<_> = has
        .iter()
        .map(|name| engine.hash(&value("main", name)).unwrap())
        .collect();
    engine
        .pack(&value("main", "main"), &|hash| has.contains(hash))
        .unwrap()
}

/// Receives `pack` in a build of `receiver` that knows its `known`, and
/// runs what it brought.
fn received(receiver: &Workspace, known: &[&str], pack: &Pack) -> Result<Value, Error> {
    let store = MemoryStore::new();
    let engine = Engine::new(receiver, &store);
    let known: Vec<_> = known.iter().map(|name| value("main", name)).collect();
    let entry = engine.receive(pack, &known)?;
    let outcome = run_entry(&engine, &entry, &[], Natives::new);
    // What a pack brought is compiled, but never by way of the store.
    let stored = engine.ran(Stage::Compile);
    assert!(stored.iter().all(|subject| subject.starts_with("app:")));
    Ok(outcome.unwrap())
}

#[test]
fn a_pack_runs_where_nothing_of_it_was_written() {
    let pack = packed(&module([double(), main()]), &[]);
    let pack = Pack::decode(&pack.encode()).unwrap();
    assert_eq!(received(&module([]), &[], &pack), Ok(Value::Int(42)));
}

#[test]
fn a_received_definition_has_the_hash_it_was_sent_with() {
    let sender = module([double(), main()]);
    let (sent, kept) = (MemoryStore::new(), MemoryStore::new());
    let sending = Engine::new(&sender, &sent);
    let pack = sending.pack(&value("main", "main"), &|_| false).unwrap();

    let receiver = module([]);
    let receiving = Engine::new(&receiver, &kept);
    let entry = receiving.receive(&pack, &[]).unwrap();
    assert_eq!(receiving.hash(&entry), sending.hash(&value("main", "main")));
}

#[test]
fn a_pack_leaves_out_what_its_receiver_has() {
    let pack = packed(&module([double(), main()]), &["double"]);
    assert_eq!(pack.definitions.len(), 1);
    assert!(matches!(
        pack.definitions[0].slots[..],
        [Target::Outside(_)]
    ));

    let receiver = module([double()]);
    assert_eq!(received(&receiver, &["double"], &pack), Ok(Value::Int(42)));
    assert!(matches!(
        received(&module([]), &[], &pack),
        Err(Error::Pack(Refused::Missing(_)))
    ));
}

#[test]
fn definitions_that_need_each_other_travel_together() {
    // `main` calls `even`, which calls `odd`, which calls `even`.
    let step = |name: &str, other: &str, base: bool| {
        let next = b::call(b::name(other), [b::subtract(b::name("n"), b::int(1))]);
        let arms = [(b::pat::int(0), b::boolean(base)), (b::pat::any(), next)];
        b::function(name)
            .param("n", ty::int())
            .returns(ty::bool())
            .body(b::match_(b::name("n"), arms))
    };
    let main = b::function("main")
        .returns(ty::bool())
        .body(b::call(b::name("even"), [b::int(10)]));
    let sender = module([step("even", "odd", true), step("odd", "even", false), main]);
    let pack = packed(&sender, &[]);
    assert_eq!(received(&module([]), &[], &pack), Ok(Value::Bool(true)));
}

#[test]
fn a_tree_checking_could_not_have_made_is_refused() {
    let mut pack = packed(&module([double(), main()]), &[]);
    if let Tree::Function(function) = &mut pack.definitions[0].tree {
        function.body = Expr::Local(3);
    }
    assert!(matches!(
        received(&module([]), &[], &pack),
        Err(Error::Invalid { .. })
    ));
}

#[test]
fn bytes_that_are_not_a_pack_do_not_decode() {
    let mut pack = packed(&module([double(), main()]), &[]);
    assert!(Pack::decode(&[0xff; 8]).is_err());
    pack.entry = 7;
    assert!(Pack::decode(&pack.encode()).is_err());
}

/// Runs `main` of `program`, which gives a function, and returns it with
/// the hash of `named` there.
fn function_from(program: &Workspace, named: &str) -> (Function, ambient::hash::Hash) {
    let store = MemoryStore::new();
    let engine = Engine::new(program, &store);
    let outcome = run_entry(&engine, &value("main", "main"), &[], Natives::new);
    let Ok(Value::Function(function)) = outcome else {
        panic!("expected a function, got {outcome:?}");
    };
    (function, engine.hash(&value("main", named)).unwrap())
}

#[test]
fn a_function_value_names_its_definition_by_hash() {
    let main = b::function("main")
        .returns(ty::unknown())
        .body(b::name("double"));
    let (function, double) = function_from(&module([double(), main]), "double");
    assert_eq!(function.definition, Some(double));
    assert_eq!(function.captures, Vec::new());
}

#[test]
fn a_function_value_holds_what_it_captured() {
    let adder = b::lambda([("n", Some(ty::int()))], b::add(b::name("n"), b::name("k")));
    let main = b::function("main")
        .returns(ty::unknown())
        .body(b::let_("k", b::int(7), adder));
    let (function, main) = function_from(&module([main]), "main");
    assert!(function.captures.contains(&Value::Int(7)));
    assert!(function.definition.is_some_and(|lambda| lambda != main));
}
