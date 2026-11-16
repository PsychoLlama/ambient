//! Files as a world holds them, and the handlers of `system.fs`.

use std::collections::BTreeMap;

use super::http::Handler;
use super::{Failure, Kind, PACKAGE, World, bytes, error, strings, text};
use crate::value::Value;

/// What kind of thing a path leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    File,
    Directory,
    Other,
}

/// What is at a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub kind: Entry,
    /// Its size in bytes.
    pub size: i64,
    /// When it last changed, in milliseconds since the start of 1970.
    pub modified: i64,
}

impl Info {
    /// As `system.fs.Info` describes it.
    fn value(self) -> Value {
        let kind = match self.kind {
            Entry::File => "fs.file",
            Entry::Directory => "fs.directory",
            Entry::Other => "fs.other",
        };
        let kind = Value::Tag {
            package: PACKAGE.to_owned(),
            tag: kind.to_owned(),
        };
        let fields = [
            ("kind", kind),
            ("size", Value::Int(self.size)),
            ("modified", Value::Int(self.modified)),
        ];
        let named = |(name, value): (&str, Value)| (name.to_owned(), value);
        Value::Record(fields.into_iter().map(named).collect::<BTreeMap<_, _>>())
    }
}

/// What a file operation gave, as the program sees it: the value, or
/// the error with a kind of `system.fs`.
fn gave<T>(done: Result<T, Failure>, value: impl FnOnce(T) -> Value) -> Value {
    done.map_or_else(|failure| error("fs", &failure), value)
}

fn nothing(done: Result<(), Failure>) -> Value {
    gave(done, |()| Value::Null)
}

/// What a file holds, as text.
fn read_text(world: &mut dyn World, path: &str) -> Result<String, Failure> {
    String::from_utf8(world.read(path)?).map_err(|_| Failure {
        kind: Kind::Invalid,
        message: format!("the file at {path} is not text"),
    })
}

pub(super) const HANDLERS: [(&str, Handler); 13] = [
    ("fs_read", |world, args| {
        Ok(gave(read_text(world, text(args, 0)?), Value::String))
    }),
    ("fs_read_bytes", |world, args| {
        Ok(gave(world.read(text(args, 0)?), Value::Bytes))
    }),
    ("fs_write", |world, args| {
        Ok(nothing(
            world.store(text(args, 0)?, text(args, 1)?.as_bytes()),
        ))
    }),
    ("fs_write_bytes", |world, args| {
        Ok(nothing(world.store(text(args, 0)?, bytes(args, 1)?)))
    }),
    ("fs_append", |world, args| {
        Ok(nothing(
            world.append(text(args, 0)?, text(args, 1)?.as_bytes()),
        ))
    }),
    ("fs_append_bytes", |world, args| {
        Ok(nothing(world.append(text(args, 0)?, bytes(args, 1)?)))
    }),
    ("fs_list", |world, args| {
        Ok(gave(world.list(text(args, 0)?), strings))
    }),
    ("fs_exists", |world, args| {
        Ok(Value::Bool(world.exists(text(args, 0)?)))
    }),
    ("fs_info", |world, args| {
        Ok(gave(world.info(text(args, 0)?), Info::value))
    }),
    ("fs_remove", |world, args| {
        Ok(nothing(world.remove(text(args, 0)?)))
    }),
    ("fs_make_directory", |world, args| {
        Ok(nothing(world.make_directory(text(args, 0)?)))
    }),
    ("fs_rename", |world, args| {
        Ok(nothing(world.rename(text(args, 0)?, text(args, 1)?)))
    }),
    ("fs_copy", |world, args| {
        Ok(nothing(world.copy(text(args, 0)?, text(args, 1)?)))
    }),
];
