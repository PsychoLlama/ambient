//! What a real process is asked from outside. See "Standard targets"
//! in `docs/spec/targets.md`.

#![allow(clippy::unwrap_used)]
#![cfg(unix)]

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use ambient::host::System;
use ambient::value::Value;
use support::scripted::over;

#[test]
fn a_process_that_waits_is_told_it_was_asked_to_stop_and_is_not_ended() {
    // The command's parent is this process, which a task of it waits
    // in before the command is run.
    let source = r#"
        use system.process;
        use ambient.tasks;

        fn asked(): string with process.signal =>
            match process.signal!() {
                process.terminate => "asked to stop",
                _ => "asked something else",
            };

        fn main(): string with process {
            let (told, _) = tasks.all(
                fn() => asked(),
                fn() => process.run!(process.command("sh", ["-c", "kill -TERM $PPID"])),
            );
            return told;
        }
    "#;

    let outcome = over(source, || Rc::new(RefCell::new(System::new(Vec::new()))));

    assert_eq!(outcome, Ok(Value::String("asked to stop".to_owned())));
}
