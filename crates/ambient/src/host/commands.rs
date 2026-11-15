//! A command as `system.process` describes one, and as a world runs it.

use super::http::{field, field_text, pairs};
use crate::value::Value;

/// A command to run. No shell reads any of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    /// Where it runs, if not where the program that runs it does.
    pub directory: Option<String>,
    /// Variables it is given beside that program's.
    pub variables: Vec<(String, String)>,
    /// What it reads from its standard input.
    pub input: String,
}

impl Command {
    /// The command a handler was called with.
    pub(super) fn read(args: &[Value]) -> Result<Self, String> {
        let command = args.first().ok_or("no command was given")?;
        let Value::List(given) = field(command, "args")? else {
            return Err("field `args` is not a list".to_owned());
        };
        let text = |arg: &Value| match arg {
            Value::String(text) => Ok(text.clone()),
            _ => Err("an argument is not a string".to_owned()),
        };
        let directory = match field(command, "directory")? {
            Value::Null => None,
            Value::String(directory) => Some(directory.clone()),
            _ => return Err("field `directory` is not a string".to_owned()),
        };
        Ok(Self {
            program: field_text(command, "program")?,
            args: given.iter().map(text).collect::<Result<_, _>>()?,
            directory,
            variables: pairs(field(command, "variables")?)?,
            input: field_text(command, "input")?,
        })
    }
}
