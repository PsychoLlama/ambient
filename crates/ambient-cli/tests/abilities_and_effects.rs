//! User-declared abilities, remote execution with ability dispatch, delimited handler semantics, and the FileSystem ability.

mod common;
use common::*;
use tempfile::TempDir;

// ─────────────────────────────────────────────────────────────────────────────
// User-Declared Abilities (content-addressed)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_user_ability_inline_handler() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-000000000001) ability Greeter {
            fn greet(name: String): String;
        }

        fn hello(): String with Greeter {
            Greeter::greet!("world")
        }

        pub fn run(): String {
            with {
                Greeter::greet(name) => {
                    resume("hi ".concat(name))
                }
            } handle hello()
        }
        "#,
    )
    .expect_output("hi world");
}

#[test]
fn test_user_ability_handler_value_and_generic_method() {
    // A handler value (first-class) for a user ability with a generic
    // method. Also a regression test: `handle ... with value {}` used to
    // silently install no handler because inference typed handler values
    // on cloned AST nodes.
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-000000000002) ability Picker {
            fn pick<T>(a: T, b: T): T;
            fn label(): String;
        }

        fn choose(): Number with Picker {
            Picker::pick!(10, 32)
        }

        pub fn run(): Number {
            let first = {
                Picker::pick(a, b) => resume(a),
                Picker::label() => resume("first")
            };

            with first handle choose()
        }
        "#,
    )
    .expect_output("10");
}

#[test]
fn test_user_ability_unhandled_is_runtime_error() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-000000000003) ability Missing {
            fn gone(): String;
        }

        pub fn run(): String with Missing {
            Missing::gone!()
        }
        "#,
    )
    .expect_error("unhandled ability");
}

#[test]
fn test_user_ability_unknown_method_is_type_error() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-000000000004) ability Greeter {
            fn greet(name: String): String;
        }

        pub fn run(): String with Greeter {
            Greeter::shout!("hi")
        }
        "#,
    )
    .expect_failure();
}

#[test]
fn test_user_ability_wrong_arg_type_is_type_error() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-000000000005) ability Greeter {
            fn greet(name: String): String;
        }

        pub fn run(): String with Greeter {
            Greeter::greet!(42)
        }
        "#,
    )
    .expect_failure();
}

#[test]
fn test_user_ability_unknown_dependency_is_error() {
    CliTest::new(
        r"
        unique(AB000000-0000-0000-0000-000000000006) ability Loud with NoSuchAbility {
            fn shout(msg: String): ();
        }

        pub fn run(): Number {
            7
        }
        ",
    )
    .expect_failure();
}

#[test]
fn test_suspend_form_is_removed() {
    // The `~` suspend-call syntax was removed from the language; using it
    // is now a parse error.
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-000000000007) ability Greeter {
            fn greet(name: String): String;
        }

        pub fn run(): Number {
            let op = Greeter::greet~("later");
            7
        }
        "#,
    )
    .expect_failure();
}

// ─────────────────────────────────────────────────────────────────────────────
// Remote Execution with Ability Dispatch
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_execute_run_with_granted_ability() {
    // Execute.run runs code in an isolated VM. The host grants output
    // abilities (Stdio/Log) to executed code, so an effectful function
    // can be run by hash and its logs land on the executing host.
    CliTest::new(
        r#"
        fn shout(x: Number): Number with core::system::Log, core::system::Stdio {
            core::system::Log::info!("computing remotely");
            x * 2
        }

        pub fn run(): Number with core::system::Execute {
            let thunk = (x) => shout(x);
            let hash = core::protocol::closure_hash(thunk);
            core::system::Execute::run!(hash, 21)
        }
        "#,
    )
    .expect_output("42");
}

#[test]
fn test_execute_run_ungranted_ability_is_unhandled() {
    // Network is NOT granted to executed code: performing it inside an
    // isolated VM is an unhandled-ability error, not a silent escape.
    CliTest::new(
        r#"
        fn phone_home(x: Number): Number with core::system::Network {
            let conn = core::system::Network::connect!(("127.0.0.1", 1));
            x
        }

        pub fn run(): Number with core::system::Execute {
            let thunk = (x) => phone_home(x);
            let hash = core::protocol::closure_hash(thunk);
            core::system::Execute::run!(hash, 1)
        }
        "#,
    )
    .expect_error("unhandled ability");
}

#[test]
fn test_execute_run_with_shipped_handler() {
    // The flagship composition: a user-declared (content-addressed)
    // ability, a first-class handler value whose methods are
    // content-addressed functions, and Execute.run_with installing that
    // handler at the base of the isolated VM. The shipped code performs
    // the ability; the shipped handler answers it.
    CliTest::new(
        r"
        unique(AB000000-0000-0000-0000-000000000008) ability Oracle {
            fn answer(): Number;
        }

        fn consult(x: Number): Number with Oracle {
            x + Oracle::answer!()
        }

        pub fn run(): Number with core::system::Execute {
            let oracle = { Oracle::answer() => resume(40) };
            let thunk = (x) => consult(x);
            let hash = core::protocol::closure_hash(thunk);
            core::system::Execute::run_with!(hash, 2, oracle)
        }
        ",
    )
    .expect_output("42");
}

#[test]
fn test_handler_methods_intrinsic() {
    // handler_methods exposes a handler's content-addressed method
    // hashes so clients can ship the handler's code alongside a function.
    CliTest::new(
        r"
        unique(AB000000-0000-0000-0000-000000000009) ability Oracle {
            fn answer(): Number;
        }

        pub fn run(): Number {
            let oracle = { Oracle::answer() => resume(42) };
            core::protocol::handler_methods(oracle).length()
        }
        ",
    )
    .expect_output("1");
}

// ─────────────────────────────────────────────────────────────────────────────
// Delimited handler semantics (catch-and-continue, resume, else)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_handle_catch_and_continue() {
    // A non-resuming arm's value becomes the handle expression's value,
    // and execution continues after the handle expression. This is the
    // essential try/catch shape.
    CliTest::new(
        r#"
        fn risky(): Number with Exception {
            Exception::throw!("kaboom");
            1
        }

        pub fn run(): Number {
            let caught = with {
                Exception::throw(msg) => 0 - 1
            } handle risky();
            caught + 100
        }
        "#,
    )
    .expect_output("99");
}

#[test]
fn test_resume_restores_locals() {
    // Locals bound before the perform must be intact after resume.
    // Regression test: continuations used to be captured with absolute
    // base pointers, so the resumed frames read the wrong stack slots.
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-00000000000A) ability Oracle {
            fn ask(q: String): Number;
        }

        fn asker(): Number with Oracle {
            let base = 100;
            let answer = Oracle::ask!("q");
            base + answer
        }

        pub fn run(): Number {
            with {
                Oracle::ask(q) => resume(42)
            } handle asker()
        }
        "#,
    )
    .expect_output("142");
}

#[test]
fn test_handle_multi_perform_with_capturing_arm() {
    // Deep handler semantics: the handler stays installed across resumes,
    // so a body performing three times fires the same arm three times.
    // The arm also captures a local from the enclosing scope.
    CliTest::new(
        r"
        unique(AB000000-0000-0000-0000-00000000000B) ability Counter {
            fn next(): Number;
        }

        fn count_three(): Number with Counter {
            let a = Counter::next!();
            let b = Counter::next!();
            let c = Counter::next!();
            a + b + c
        }

        pub fn run(): Number {
            let step = 10;
            with {
                Counter::next() => resume(step)
            } handle count_three()
        }
        ",
    )
    .expect_output("30");
}

#[test]
fn test_handle_else_transforms_normal_completion() {
    // The else clause transforms the body's value on normal completion;
    // handler arms bypass it.
    CliTest::new(
        r"
        pub fn run(): Number {
            with {
                Exception::throw(msg) => 0
            } handle 5 else (r) => r * 2
        }
        ",
    )
    .expect_output("10");
}

#[test]
fn test_exception_unwinds_through_inner_handle() {
    // A throw crosses an inner (non-Exception) handler region to reach
    // the outer Exception handler, and the inner handler is fully
    // uninstalled afterwards.
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-00000000000C) ability Ping {
            fn ping(): Number;
        }

        fn inner(): Number with Ping, Exception {
            let p = Ping::ping!();
            Exception::throw!("escape");
            p
        }

        fn middle(): Number with Exception {
            with {
                Ping::ping() => resume(7)
            } handle inner()
        }

        pub fn run(): Number {
            let x = with {
                Exception::throw(msg) => 50
            } handle middle();
            let y = with {
                Ping::ping() => resume(1),
                Exception::throw(msg) => 2
            } handle inner();
            x + y
        }
        "#,
    )
    .expect_output("52");
}

#[test]
fn test_uncaught_exception_reports_value() {
    // With no handler in scope, the thrown value surfaces in the error.
    let output = CliTest::new(
        r#"
        pub fn run(): Number with Exception {
            Exception::throw!("boom with value 7");
            0
        }
        "#,
    )
    .execute();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("uncaught exception") && stderr.contains("boom with value 7"),
        "expected uncaught exception with thrown value, got: {stderr}"
    );
}

#[test]
fn test_host_raised_exception_is_catchable() {
    // A failing host operation (network connect to a closed port) raises
    // a catchable exception instead of aborting the VM.
    CliTest::new(
        r#"
        fn try_connect(): String with core::system::Network {
            let conn = core::system::Network::connect!(("127.0.0.1", 9));
            "connected"
        }

        pub fn run(): String with core::system::Network {
            with {
                Exception::throw(msg) => "failed"
            } handle try_connect()
        }
        "#,
    )
    .expect_output("failed");
}

#[test]
fn test_host_raised_exception_resume_substitute() {
    // The Exception handler receives the continuation of the failed host
    // call, so it can resume with a substitute value: try_connect
    // continues executing after the failed connect.
    CliTest::new(
        r#"
        fn try_connect(): Number with core::system::Network {
            let conn = core::system::Network::connect!(("127.0.0.1", 9));
            conn + 1000
        }

        pub fn run(): Number with core::system::Network {
            with {
                Exception::throw(msg) => resume(0 - 1)
            } handle try_connect()
        }
        "#,
    )
    .expect_output("999");
}

// ─────────────────────────────────────────────────────────────────────────────
// FileSystem Ability Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_fs_write_read_roundtrip() {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path().join("note.txt");
    CliTest::new(format!(
        r#"
        pub fn run(): String with core::system::FileSystem {{
            core::system::FileSystem::write!("{path}", "hello from ambient");
            core::system::FileSystem::read!("{path}")
        }}
        "#,
        path = path.display()
    ))
    .expect_output("hello from ambient");
}

#[test]
fn test_fs_read_missing_file_is_catchable_exception() {
    // A failing filesystem operation raises a catchable exception instead
    // of aborting the VM.
    CliTest::new(
        r#"
        fn try_read(): String with core::system::FileSystem {
            core::system::FileSystem::read!("/nonexistent/ambient_fs_test/missing.txt")
        }

        pub fn run(): String with core::system::FileSystem {
            with {
                Exception::throw(msg) => "caught"
            } handle try_read()
        }
        "#,
    )
    .expect_output("caught");
}

#[test]
fn test_fs_exists_false_then_true() {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path().join("probe.txt");
    CliTest::new(format!(
        r#"
        pub fn run(): () with core::system::FileSystem, core::system::Stdio {{
            core::system::Stdio::out!(core::convert::to_string(core::system::FileSystem::exists!("{path}")));
            core::system::FileSystem::write!("{path}", "x");
            core::system::Stdio::out!(core::convert::to_string(core::system::FileSystem::exists!("{path}")));
        }}
        "#,
        path = path.display()
    ))
    .expect_output("false\ntrue");
}

#[test]
fn test_fs_list_returns_written_entries() {
    let dir = TempDir::new().expect("failed to create temp dir");
    let base = dir.path().display().to_string();
    CliTest::new(format!(
        r#"
        pub fn run(): Number with core::system::FileSystem {{
            core::system::FileSystem::write!("{base}/a.txt", "1");
            core::system::FileSystem::write!("{base}/b.txt", "2");
            core::system::FileSystem::list!("{base}").length()
        }}
        "#
    ))
    .expect_output("2");
}

#[test]
fn test_fs_remove_then_exists_is_false() {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path().join("ephemeral.txt");
    CliTest::new(format!(
        r#"
        pub fn run(): Bool with core::system::FileSystem {{
            core::system::FileSystem::write!("{path}", "gone soon");
            core::system::FileSystem::remove!("{path}");
            core::system::FileSystem::exists!("{path}")
        }}
        "#,
        path = path.display()
    ))
    .expect_output("false");
}

#[test]
fn test_core_time_duration() {
    // Exercises core::time::Duration end to end: constructors that
    // normalize sub-second units into the seconds field, the accessors,
    // the borrow path in subtraction, and the operator/ordering traits.
    // `run` returns the number of passing checks (12 when all hold).
    CliTest::new(
        r#"
        use core::time::Duration;

        pub fn run(): Number {
            let a = Duration::from_secs(2);
            let b = Duration::from_millis(1500);   // 1s + 500_000_000ns

            let sum = a + b;                        // 3.5s
            let borrow = b - Duration::from_millis(700); // 0.8s (sub-second borrow)

            let c1 = if b.as_secs() == 1 { 1 } else { 0 };
            let c2 = if b.subsec_millis() == 500 { 1 } else { 0 };
            let c3 = if b.subsec_nanos() == 500000000 { 1 } else { 0 };
            let c4 = if sum.as_millis() == 3500 { 1 } else { 0 };
            let c5 = if sum.as_secs() == 3 { 1 } else { 0 };
            let c6 = if borrow.as_millis() == 800 { 1 } else { 0 };
            let c7 = if Duration::from_micros(1500000).as_secs() == 1 { 1 } else { 0 };
            let c8 = if Duration::from_nanos(2500000000).subsec_nanos() == 500000000 { 1 } else { 0 };
            let c9 = if a.cmp(b) == 1 { 1 } else { 0 };
            let c10 = if a == Duration::from_secs(2) { 1 } else { 0 };
            let c11 = if Duration::from_secs(0).is_zero() { 1 } else { 0 };
            let c12 = if a.as_nanos() == 2000000000 { 1 } else { 0 };

            c1 + c2 + c3 + c4 + c5 + c6 + c7 + c8 + c9 + c10 + c11 + c12
        }
        "#,
    )
    .expect_output("12");
}

#[test]
fn test_execute_run_fs_is_not_granted() {
    // FileSystem is NOT granted to executed code: only Stdio/Log are. A shipped
    // function that touches the filesystem is an unhandled-ability error,
    // not a silent escape.
    CliTest::new(
        r#"
        fn sneaky(x: Number): Number with core::system::FileSystem {
            let content = core::system::FileSystem::read!("/etc/hostname");
            x
        }

        pub fn run(): Number with core::system::Execute {
            let thunk = (x) => sneaky(x);
            let hash = core::protocol::closure_hash(thunk);
            core::system::Execute::run!(hash, 1)
        }
        "#,
    )
    .expect_error("unhandled ability");
}
