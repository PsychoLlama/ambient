//! The native side of the modules that are a few handlers each:
//! `system.console`, `env`, `random`, `clock`, and `process`.

use super::http::{Handler, Starter};
use super::linked::Offer;
use super::{Command, Work, int, optional, strings, text};
use crate::value::Value;

const WRITING: [(&str, Handler); 3] = [
    ("console_write", |world, args| {
        world.write(text(args, 0)?);
        Ok(Value::Null)
    }),
    ("console_print", |world, args| {
        world.write(&format!("{}\n", text(args, 0)?));
        Ok(Value::Null)
    }),
    ("console_warn", |world, args| {
        world.warn(&format!("{}\n", text(args, 0)?));
        Ok(Value::Null)
    }),
];

const READING: [(&str, Starter); 2] = [
    ("console_read_line", |_| Ok(Work::ReadLine)),
    ("console_read", |args| Ok(Work::Read(int(args, 0)?))),
];

pub(super) const CONSOLE: Offer = Offer {
    handlers: &WRITING,
    starters: &READING,
};

const TOLD: [(&str, Handler); 4] = [
    ("env_args", |world, _| Ok(strings(world.args()))),
    ("env_variable", |world, args| {
        Ok(optional(world.variable(text(args, 0)?)))
    }),
    ("env_variables", |world, _| {
        let pair = |(name, value)| Value::Tuple(vec![Value::String(name), Value::String(value)]);
        Ok(Value::List(
            world.variables().into_iter().map(pair).collect(),
        ))
    }),
    ("env_directory", |world, _| Ok(optional(world.directory()))),
];

pub(super) const ENV: Offer = Offer {
    handlers: &TOLD,
    starters: &[],
};

const BITS: [(&str, Handler); 1] = [("random_bits", |world, _| {
    let bits = world.random().map_err(|failure| failure.message)?;
    Ok(Value::Int(bits.cast_signed()))
})];

pub(super) const RANDOM: Offer = Offer {
    handlers: &BITS,
    starters: &[],
};

const TIMES: [(&str, Handler); 2] = [
    ("clock_now", |world, _| Ok(Value::Int(world.now()))),
    ("clock_elapsed", |world, _| Ok(Value::Int(world.elapsed()))),
];

const SLEEPING: [(&str, Starter); 1] = [("clock_sleep", |args| Ok(Work::Sleep(int(args, 0)?)))];

pub(super) const CLOCK: Offer = Offer {
    handlers: &TIMES,
    starters: &SLEEPING,
};

// What a root handler of an ability that does not return gives is
// what the program gives.
const EXITING: [(&str, Handler); 1] = [("process_exit", |_, args| Ok(Value::Int(int(args, 0)?)))];

const RUNNING: [(&str, Starter); 2] = [
    ("process_signal", |_| Ok(Work::Signal)),
    ("process_run", |args| Ok(Work::Run(Command::read(args)?))),
];

pub(super) const PROCESS: Offer = Offer {
    handlers: &EXITING,
    starters: &RUNNING,
};
