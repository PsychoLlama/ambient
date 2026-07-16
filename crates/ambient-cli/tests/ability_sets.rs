//! Named ability sets and first-order effect rows, end to end through the
//! real `ambient` binary.
//!
//! - First-order rows: performing an ability requires only that ability, never
//!   its declared dependencies (which stay its implementation detail).
//! - `set` declarations: union (comma), `Union<A, B>`, `Difference<A, B>`,
//!   set-to-set references, the prelude `System` set, and cross-module use.

mod common;

use common::CliTest;

#[test]
fn first_order_row_requires_only_the_performed_ability() {
    // `Log`'s default performs `Stdio`, yet performing `Log` requires only
    // `with Log` — the dependency no longer leaks into the caller's row.
    CliTest::new(
        r#"
use core::system::Log;

pub fn run(): () with Log {
  Log::info!("first-order log line");
}
"#,
    )
    .expect_output("first-order log line");
}

#[test]
fn handling_an_ability_discharges_its_dependency_too() {
    // Handling `Log` intercepts the perform before its default runs, so the
    // `Stdio` the default would have performed never lingers in the row and
    // `run` type-checks as pure.
    CliTest::new(
        r#"
use core::system::Log;

fn work(): () with Log { Log::info!("inside"); }

pub fn run(): Number {
  with { Log::info(message) => resume(()) } handle work();
  0
}
"#,
    )
    .expect_success();
}

#[test]
fn a_set_expands_to_its_union_of_members() {
    CliTest::new(
        r#"
use core::system::{Stdio, FileSystem};

set Files = Stdio, FileSystem;

pub fn run(): () with Files {
  Stdio::out!("via the Files set");
  FileSystem::write!("/tmp/ambient_ability_sets_it.txt", "x");
}
"#,
    )
    .expect_output("via the Files set");
}

#[test]
fn union_and_difference_combinators_compose() {
    // `All` unions a set with an ability; `Offline` subtracts one back out.
    CliTest::new(
        r#"
use core::system::{Stdio, FileSystem, Tcp};

set Files   = Stdio, FileSystem;
set All     = Union<Files, Tcp>;
set Offline = Difference<All, Tcp>;

pub fn run(): () with Offline {
  Stdio::out!("offline is All minus Tcp");
}
"#,
    )
    .expect_output("offline is All minus Tcp");
}

#[test]
fn difference_actually_excludes_the_subtracted_ability() {
    // `Offline` excludes `Tcp`, so performing it is a type error.
    CliTest::new(
        r#"
use core::system::Tcp;

set Offline = Difference<System, Tcp>;

pub fn run(): () with Offline {
  match Tcp::connect!(("127.0.0.1", 9)) { Ok(_) => (), Err(_) => () };
}
"#,
    )
    .expect_error("Tcp");
}

#[test]
fn the_system_set_rides_the_prelude() {
    // `System` is the one member of `core::system` on the prelude, so a
    // privileged entry can name its full authority with no import.
    CliTest::new(
        r#"
use core::system::Stdio;

pub fn run(): () with System {
  Stdio::out!("privileged entry");
}
"#,
    )
    .expect_output("privileged entry");
}
