//! Handles: opaque references native code mints. See `spec/values.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use ambient::backend::native::{Answer, Natives, Waits};
use ambient::exchange::{Unsendable, encode};
use ambient::value::Value;
use support::execute_in;
use support::text::workspace;

/// A host that mints files and notes each one its program drops.
#[derive(Default)]
struct Files {
    dropped: Vec<u64>,
}

impl Waits for Files {
    fn next(&mut self) -> Option<Answer> {
        None
    }

    fn dropped(&mut self, package: &str, brand: &str, number: u64) {
        assert_eq!((package, brand), ("app", "main.File"));
        self.dropped.push(number);
    }
}

fn file(number: u64) -> Value {
    Value::Handle {
        package: "app".to_owned(),
        brand: "main.File".to_owned(),
        number,
    }
}

/// Runs `main` of a program that declares `File` and what mints and
/// reads one, on every backend. Gives what it gave, and the files each
/// backend's host heard were dropped, in order.
fn run(body: &str) -> (Value, Vec<Vec<u64>>) {
    let source = format!(
        "brand File;
        brand Other;
        extern fn open(number: int): File;
        extern fn number(file: File): int;
        {body}"
    );
    let hosts = RefCell::new(Vec::new());
    let natives = || {
        let host = Rc::new(RefCell::new(Files::default()));
        hosts.borrow_mut().push(Rc::clone(&host));
        Natives::new()
            .answered_by(host)
            .verifiable("app", "main.open", |args| match args {
                [Value::Int(number)] => Ok(file(number.cast_unsigned())),
                _ => Err("not a number".to_owned()),
            })
            .verifiable("app", "main.number", |args| match args {
                [Value::Handle { number, .. }] => Ok(Value::Int(number.cast_signed())),
                _ => Err("not a file".to_owned()),
            })
    };
    let gave = execute_in(&workspace(&source), &[], None, natives).unwrap();
    let hosts = hosts.into_inner();
    let dropped = |host: &Rc<RefCell<Files>>| host.borrow().dropped.clone();
    (gave, hosts.iter().map(dropped).collect())
}

#[test]
fn a_handle_is_handed_back_to_the_code_that_minted_it() {
    let (gave, _) = run("fn main(): int {
        let held = [open(7), open(8)];
        return held.map(fn(file: File): int => number(file)).sum();
    }");
    assert_eq!(gave, Value::Int(15));
}

#[test]
fn a_host_hears_when_the_last_of_a_handle_goes() {
    let (gave, dropped) = run("fn main(): int {
        let kept = open(1);
        let copies = [kept, kept];
        let _ = number(open(2));
        return number(open(3)) + copies.length();
    }");
    assert_eq!(gave, Value::Int(5));
    // Each is heard of once, when nothing holds it: the two that were
    // only passed along as soon as they were, and the one that was
    // kept when the function was over.
    for heard in dropped {
        assert_eq!(heard, [2, 3, 1]);
    }
}

#[test]
fn handles_are_the_same_by_brand_and_number() {
    let (gave, _) = run("fn main(): List<bool> {
        let file = open(1);
        return [file == file, file == open(1), file == open(2)];
    }");
    let same = [true, true, false].map(Value::Bool).to_vec();
    assert_eq!(gave, Value::List(same));
}

#[test]
fn a_handle_is_tested_for_by_its_brand() {
    let (gave, _) = run("fn kind(value: unknown): string {
        match value {
            let _: Other => { return \"other\"; }
            let _: File => { return \"file\"; }
            _ => { return \"neither\"; }
        }
    }
    fn main(): string => kind(open(1)) + \" \" + kind(1);");
    assert_eq!(gave, Value::String("file neither".to_owned()));
}

#[test]
fn a_handle_is_not_sent() {
    assert_eq!(encode(&Value::List(vec![file(1)])), Err(Unsendable::Handle));
}
