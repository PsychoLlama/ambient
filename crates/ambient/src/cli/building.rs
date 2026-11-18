//! The world comptime calls of a package on disk run in: the native
//! side of `ambient.build`. See "Build abilities" in `spec/tools.md`.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ambient_runtime::FAULT_PACKAGE;

use super::Session;
use crate::backend::native::Natives;
use crate::engine::Engine;
use crate::library;
use crate::project::{Contents, Files, Project};
use crate::store::Store;
use crate::value::Value;

impl Session<'_> {
    pub(super) fn engine<'e>(&'e self, project: &'e Project) -> Engine<'e> {
        engine(self.files, self.store, project)
    }
}

/// An engine over a project read from `files`, whose comptime calls
/// read the files of the package each is written in.
#[must_use]
pub fn engine<'e>(files: &dyn Files, store: &'e dyn Store, project: &'e Project) -> Engine<'e> {
    let files = files.shared();
    let packages = project.packages.clone();
    Engine::new(&project.workspace, store).with_build_for(library::build(), move |from| {
        natives(&files, packages.get(&from.0))
    })
}

/// Why a build was not answered: the tag of `ambient.build` for it,
/// and what to say.
struct Refused(&'static str, String);

/// The error a refusal is, as `ambient.Error` describes one.
fn error(Refused(kind, message): Refused) -> Value {
    let kind = Value::Tag {
        package: FAULT_PACKAGE.to_owned(),
        tag: format!("build.{kind}"),
    };
    let fields = [
        ("kind", kind),
        ("message", Value::String(message)),
        ("cause", Value::Null),
        ("trace", Value::List(Vec::new())),
    ];
    let named = |(name, value): (&str, Value)| (name.to_owned(), value);
    Value::Record(fields.into_iter().map(named).collect::<BTreeMap<_, _>>())
}

/// The native code of the build target for one package, whose files
/// are `contents`; one that has none reads nothing.
///
/// Each can be asked again to see whether what it gave still holds,
/// so a run is made again only when something it read has changed.
fn natives(files: &Rc<dyn Files>, contents: Option<&Contents>) -> Natives {
    let reading = |read: fn(&dyn Files, &Path) -> io::Result<Value>| {
        let (files, contents) = (Rc::clone(files), contents.cloned());
        move |args: &[Value]| {
            let Some(Value::String(path)) = args.first() else {
                return Err("argument 0 is not text".to_owned());
            };
            let read = within(contents.as_ref(), path).and_then(|path| {
                let missing =
                    |reason| Refused("not_found", format!("{}: {reason}", path.display()));
                read(&*files, &path).map_err(missing)
            });
            Ok(read.unwrap_or_else(error))
        }
    };
    let text = reading(|files, path| files.read(path).map(Value::String));
    let bytes = reading(|files, path| files.bytes(path).map(Value::Bytes));
    let names = reading(|files, path| {
        let names = files.names(path)?;
        Ok(Value::List(names.into_iter().map(Value::String).collect()))
    });
    Natives::new()
        .verifiable(FAULT_PACKAGE, "build.reading", text)
        .verifiable(FAULT_PACKAGE, "build.reading_bytes", bytes)
        .verifiable(FAULT_PACKAGE, "build.listing", names)
}

/// Where `path` is, if it names a file of the package: a build reads
/// nothing of the machine but its own package, as its snapshot has it.
fn within(contents: Option<&Contents>, path: &str) -> Result<PathBuf, Refused> {
    let held = contents.and_then(|contents| {
        let inside = contents.holds(Path::new(path))?;
        Some(contents.directory.join(inside))
    });
    held.ok_or_else(|| Refused("denied", format!("{path} is not a file of the package")))
}
