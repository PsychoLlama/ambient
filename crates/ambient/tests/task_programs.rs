//! Programs of tasks made at random, run on every backend. See
//! `docs/spec/tasks.md`, `docs/spec/cells.md`, and `docs/spec/cleanup.md`.
//!
//! `support::run` fails if the backends disagree or either leaks, so
//! running a program is the whole of the test: whatever tasks do to one
//! another, a program ends one way, and leaves nothing behind.

#![allow(clippy::unwrap_used)]

mod support;

use std::io::Write;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use ambient::builder::{self as b, pat, ty};
use ambient::surface::Expr;
use proptest::prelude::*;
use support::run;
use support::tasks::{note, set, then, until};

/// Something a task does, which gives an integer or does not come back.
#[derive(Clone, Debug)]
enum Step {
    Give(i64),
    /// Adds a digit to the log.
    Note(i64),
    /// Puts a number in the cell.
    Set(i64),
    /// Waits until the cell holds a number, which may be never.
    Until(i64),
    Overflow,
    All(Vec<Step>),
    First(Vec<Step>),
    /// Spawns each of the first in a group, whose body is the second.
    Group(Vec<Step>, Box<Step>),
    /// The second, with the first as its cleanup.
    Cleanup(Box<Step>, Box<Step>),
    /// Inside a boundary: one, or minus one for a fault.
    Contained(Box<Step>),
}

impl Step {
    /// Runs the step as a program on every backend, which fails if they
    /// disagree or either leaks.
    fn run(&self) {
        // What it gave, what its tasks noted, and what they left in the cell.
        let seen = b::tuple([self.expr(), b::read(b::name("log")), b::read(b::name("c"))]);
        let body = b::let_("c", b::cell(ty::int(), b::int(0)), seen);
        let body = b::let_("log", b::cell(ty::int(), b::int(0)), body);
        let main = b::function("main").returns(ty::unknown()).body(body);

        // A fault or a wait on nothing is an end like any other.
        let _ = run([note(), set(), until(), main], &[]);
    }

    fn expr(&self) -> Expr {
        let (c, log) = (|| b::name("c"), || b::name("log"));
        let exprs = |steps: &[Self]| steps.iter().map(Self::expr).collect::<Vec<_>>();
        match self {
            Self::Give(value) => b::int(*value),
            Self::Note(digit) => {
                let noted = b::call(b::name("note"), [log(), b::int(*digit)]);
                then(noted, b::int(*digit))
            }
            Self::Set(value) => {
                let set = b::call(b::name("set"), [c(), b::int(*value)]);
                then(set, b::int(*value))
            }
            Self::Until(value) => b::call(b::name("until"), [c(), b::int(*value)]),
            Self::Overflow => b::add(b::int(i64::MAX), b::int(1)),
            Self::All(steps) => then(b::all(exprs(steps)), b::int(0)),
            Self::First(steps) => b::first(exprs(steps)),
            Self::Group(spawned, body) => {
                let spawns = exprs(spawned).into_iter().map(b::spawn);
                let body = spawns.rfold(body.expr(), |body, spawn| then(spawn, body));
                b::group(body)
            }
            Self::Cleanup(cleanup, body) => support::cleanup(cleanup.expr(), body.expr()),
            Self::Contained(body) => b::match_(
                b::boundary(body.expr()),
                [(pat::is(ty::int()), b::int(1)), (pat::any(), b::int(-1))],
            ),
        }
    }
}

/// How long a program is given before it is taken never to end. One
/// runs in milliseconds.
const PATIENCE: Duration = Duration::from_mins(1);

/// Runs `step` as a program, and ends the test if it does not end.
///
/// A program that hangs would otherwise be a test that never ends and
/// never says which program it was on. Nothing can stop the thread it
/// runs on, so the process ends there, and the program is not shrunk.
fn ends(step: &Step) {
    let (done, over) = mpsc::channel();
    let program = step.clone();
    let ran = std::thread::spawn(move || {
        program.run();
        let _ = done.send(());
    });
    match over.recv_timeout(PATIENCE) {
        Ok(()) => {}
        Err(RecvTimeoutError::Timeout) => {
            let _ = writeln!(std::io::stderr(), "a program did not end: {step:?}");
            std::process::exit(1);
        }
        // It failed, which is the test's to report and shrink.
        Err(RecvTimeoutError::Disconnected) => {
            std::panic::resume_unwind(ran.join().unwrap_err());
        }
    }
}

fn steps() -> impl Strategy<Value = Step> {
    let leaf = prop_oneof![
        (0..9i64).prop_map(Step::Give),
        (1..9i64).prop_map(Step::Note),
        (1..4i64).prop_map(Step::Set),
        (0..4i64).prop_map(Step::Until),
        Just(Step::Overflow),
    ];
    leaf.prop_recursive(4, 24, 3, |inner| {
        let several = || prop::collection::vec(inner.clone(), 2..4);
        let one = || inner.clone().prop_map(Box::new);
        prop_oneof![
            several().prop_map(Step::All),
            several().prop_map(Step::First),
            (several(), one()).prop_map(|(spawned, body)| Step::Group(spawned, body)),
            (one(), one()).prop_map(|(cleanup, body)| Step::Cleanup(cleanup, body)),
            one().prop_map(Step::Contained),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn a_program_of_tasks_ends_the_same_way_on_every_backend(step in steps()) {
        ends(&step);
    }
}
