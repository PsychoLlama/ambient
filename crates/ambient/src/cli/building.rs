//! The world comptime calls of a package on disk run in: the native
//! side of `system.build`. See "Authority" in `spec/comptime.md`.

use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::Session;
use crate::backend::native::Natives;
use crate::engine::Engine;
use crate::host::{Failure, Kind, error};
use crate::project::{Contents, Files, Project};
use crate::store::Store;
use crate::system;
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
    Engine::new(&project.workspace, store).with_build_for(system::build(), move |from| {
        natives(&files, packages.get(&from.0))
    })
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
                read(&*files, &path).map_err(|reason| Failure {
                    kind: Kind::NotFound,
                    message: format!("{}: {reason}", path.display()),
                })
            });
            Ok(read.unwrap_or_else(|failure| error("fs", &failure)))
        }
    };
    let text = reading(|files, path| files.read(path).map(Value::String));
    let names = reading(|files, path| {
        let names = files.names(path)?;
        Ok(Value::List(names.into_iter().map(Value::String).collect()))
    });
    Natives::new()
        .verifiable(system::PACKAGE, "fs_read", text)
        .verifiable(system::PACKAGE, "fs_list", names)
}

/// Where `path` is, if it names a file of the package: a build reads
/// nothing of the machine but its own package, as its snapshot has it.
fn within(contents: Option<&Contents>, path: &str) -> Result<PathBuf, Failure> {
    let held = contents.and_then(|contents| {
        let inside = contents.holds(Path::new(path))?;
        Some(contents.directory.join(inside))
    });
    held.ok_or_else(|| Failure {
        kind: Kind::Denied,
        message: format!("{path} is not a file of the package"),
    })
}
