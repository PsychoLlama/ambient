//! What a build is asked: the questions a tool or a comptime call
//! puts to the engine that runs it. See "Build abilities" in
//! `docs/spec/tools.md`.
//!
//! Each is native code of `ambient.build` that only passes the
//! question on (`Natives::asking`), and the engine answers while the
//! program that asked stands. An answer is a function of what the
//! engine holds, so a run that asked is kept with the answer and made
//! again only when it changes.

use std::collections::BTreeMap;

use ambient_runtime::{
    FALSE, FAULT_PACKAGE, FORSAKEN, Fault, Kind, NULL, SUSPENDING, TRUE, UNWIND, WAITING,
};

use super::Engine;
use crate::backend::native::{Natives, Questions, Running};
use crate::backend::wasm::{self, Wasm};
use crate::checker;
use crate::engine::Parts;
use crate::error::Error;
use crate::symbol::{ModuleId, Namespace, Symbol};
use crate::value::Value;

/// The module of the standard library that declares the build
/// abilities.
pub(super) const MODULE: &str = "build";

/// A record of `fields`.
pub(crate) fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    let named = |(name, value): (&str, Value)| (name.to_owned(), value);
    Value::Record(fields.into_iter().map(named).collect::<BTreeMap<_, _>>())
}

/// The error a build ability gives when it has no answer, as
/// `ambient.Error` describes one: its kind is the tag of
/// `ambient.build` named `kind`.
#[must_use]
pub fn refused(kind: &str, message: String) -> Value {
    let kind = Value::Tag {
        package: FAULT_PACKAGE.to_owned(),
        tag: format!("{MODULE}.{kind}"),
    };
    record([
        ("kind", kind),
        ("message", Value::String(message)),
        ("cause", Value::Null),
        ("trace", Value::List(Vec::new())),
    ])
}

/// The native code that passes each question on to the engine, or to
/// whatever answers in its place and asks the engine the rest.
#[must_use]
pub fn natives() -> Natives {
    // An error nothing caught fails the run, as a fault does, with
    // what the error says.
    let thrown = |args: &[Value]| match args.first() {
        Some(Value::Record(error)) => match error.get("message") {
            Some(Value::String(message)) => Err(message.clone()),
            _ => Err("an error was thrown".to_owned()),
        },
        _ => Err("an error was thrown".to_owned()),
    };
    Natives::new()
        .asking(FAULT_PACKAGE, "build.compile", "compile")
        .asking(FAULT_PACKAGE, "build.annotations", "annotations")
        .asking(FAULT_PACKAGE, "build.signature", "signature")
        .asking(FAULT_PACKAGE, "build.modules", "modules")
        .asking(FAULT_PACKAGE, "build.definitions", "definitions")
        .asking(FAULT_PACKAGE, "build.annotated", "annotated")
        .asking(FAULT_PACKAGE, "build.reach", "reach")
        .asking(FAULT_PACKAGE, "build.doc", "doc")
        .asking(FAULT_PACKAGE, "build.run", "run")
        .asking(FAULT_PACKAGE, "build.fetch", "fetch")
        .asking(FAULT_PACKAGE, "build.keep", "keep")
        .asking(FAULT_PACKAGE, "build.kept", "kept")
        // A digest is a function of the bytes alone.
        .verifiable(FAULT_PACKAGE, "build.digest", |args: &[Value]| {
            match args.first() {
                Some(Value::Bytes(bytes)) => Ok(Value::String(super::reflect::digest(bytes))),
                _ => Err("argument 0 is not bytes".to_owned()),
            }
        })
        // The time is never what it was, so a run that asks is made
        // again on every build.
        .with(FAULT_PACKAGE, "build.now", |_| {
            let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
            let now = since.map_or(0, |since| since.as_millis());
            Ok(Value::Int(i64::try_from(now).unwrap_or(i64::MAX)))
        })
        .verifiable(FAULT_PACKAGE, "build.thrown", thrown)
}

/// The engine, as what native code is handed when it is called again
/// only to see whether what it gave still holds: no program is
/// running, and a question is answered as it was.
pub(super) struct Again<'a, 'e>(pub &'a Engine<'e>);

impl Running for Again<'_, '_> {
    fn load(&mut self, _: &Parts, _: &Natives) -> Result<Value, String> {
        Err(NOWHERE.to_owned())
    }

    fn backend(&self) -> Result<&str, String> {
        Err(NOWHERE.to_owned())
    }

    fn later(&mut self) -> Result<u64, String> {
        Err(NOWHERE.to_owned())
    }

    fn call(&mut self, _: &Value, _: &[Value]) -> Result<Value, String> {
        Err(NOWHERE.to_owned())
    }

    fn perform(&mut self, _: &str, _: &str, _: &[Value]) -> Result<Value, String> {
        Err(NOWHERE.to_owned())
    }

    fn ask(&mut self, question: &str, args: &[Value]) -> Result<Value, String> {
        self.0.answer(question, args)
    }
}

const NOWHERE: &str = "no program is running";

impl Questions for Engine<'_> {
    fn answer(&self, question: &str, args: &[Value]) -> Result<Value, String> {
        match question {
            "compile" => {
                let (entry, target) = (definition(args, 0)?, definition(args, 1)?);
                let compiled = self.module_of(&entry, &target);
                Ok(compiled.unwrap_or_else(|error| refused("failed", error.to_string())))
            }
            "annotations" => {
                let from = definition(args, 0)?;
                let Some(Value::Tag { package, tag }) = args.get(1) else {
                    return Err("an annotation's key is a tag".to_owned());
                };
                let found = self.annotations(&from, package, tag);
                Ok(found.unwrap_or_else(|error| refused("failed", error.to_string())))
            }
            "signature" => {
                let tree = self.signature_tree(&definition(args, 0)?);
                Ok(tree.unwrap_or_else(|error| refused("failed", error.to_string())))
            }
            _ => self
                .told(question, args)
                .unwrap_or_else(|| Err(format!("a build has no answer to `{question}`"))),
        }
    }
}

/// The definition a `tree.Reference` among `args` refers to.
///
/// # Errors
///
/// Fails if what is at `position` is no reference to a definition.
pub fn definition(args: &[Value], position: usize) -> Result<Symbol, String> {
    let wrong = || format!("argument {position} is not a reference to a definition");
    let Some(Value::Record(fields)) = args.get(position) else {
        return Err(wrong());
    };
    let text = |name: &str| match fields.get(name) {
        Some(Value::String(text)) => Ok(text.as_str()),
        _ => Err(wrong()),
    };
    let Some(Value::List(module)) = fields.get("module") else {
        return Err(wrong());
    };
    let mut path = Vec::new();
    for part in module {
        let Value::String(part) = part else {
            return Err(wrong());
        };
        path.push(part.as_str());
    }
    let namespace = match text("kind")? {
        "type" => Namespace::Type,
        "value" => Namespace::Value,
        "ability" => Namespace::Ability,
        "contract" => Namespace::Contract,
        _ => return Err(wrong()),
    };
    let module = ModuleId::new(text("package")?, &path.join("."));
    Ok(module.symbol(namespace, text("name")?))
}

/// A name and the number it stands for, as `build.Word` describes
/// one.
fn word(name: &str, number: u64) -> Value {
    record([
        ("name", Value::String(name.to_owned())),
        ("word", Value::Int(number.cast_signed())),
    ])
}

/// What the runtime defines that a host of a module must agree with:
/// the words it gives a meaning of their own, the modes of a context,
/// the fault native code that fails is, and the index native code is
/// told a task left a wait by.
fn words() -> Value {
    let words = [
        ("unwind", UNWIND),
        ("null", NULL),
        ("false", FALSE),
        ("true", TRUE),
        ("suspending", SUSPENDING),
        ("waiting", WAITING),
        ("native", Fault::Native as u64),
        ("forsaken", FORSAKEN),
    ];
    Value::List(words.map(|(name, number)| word(name, number)).into())
}

/// What kind of value a word is, as the runtime says it.
fn kinds() -> Value {
    let kinds = [
        ("null", Kind::Null),
        ("bool", Kind::Bool),
        ("int", Kind::Int),
        ("float", Kind::Float),
        ("string", Kind::String),
        ("tuple", Kind::Tuple),
        ("list", Kind::List),
        ("record", Kind::Record),
        ("function", Kind::Function),
        ("cell", Kind::Cell),
        ("tag", Kind::Tag),
        ("iterator", Kind::Iterator),
        ("bytes", Kind::Bytes),
        ("handle", Kind::Handle),
    ];
    Value::List(kinds.map(|(name, kind)| word(name, kind as u64)).into())
}

impl Engine<'_> {
    /// `entry` under `target` as a WebAssembly module, with the externs
    /// it imports and what its host must agree with: a
    /// `build.Compiled`.
    fn module_of(&self, entry: &Symbol, target: &Symbol) -> Result<Value, Error> {
        let root = self.program(entry, target)?;
        let parts = self.link(&root, &Wasm)?;
        let module = wasm::module(&parts).map_err(|message| Error::Backend {
            symbol: entry.clone(),
            message,
        })?;
        let declared = self.externs(&root)?;
        let externs = parts.natives.iter().map(|native| {
            let declared = declared.get(&(native.package.clone(), native.path.clone()));
            record([
                ("package", Value::String(native.package.clone())),
                ("path", Value::String(native.path.clone())),
                // What is imported is declared by something reached.
                (
                    "definition",
                    declared.map_or(Value::Null, checker::reference),
                ),
            ])
        });
        Ok(record([
            ("module", Value::Bytes(module)),
            ("externs", Value::List(externs.collect())),
            ("words", words()),
            ("kinds", kinds()),
        ]))
    }
}
