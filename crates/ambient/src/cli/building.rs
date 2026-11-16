//! The world comptime calls of a package on disk run in: the native
//! side of `system.build`. See "Authority" in `spec/comptime.md`.

use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use super::Session;
use crate::backend::native::Natives;
use crate::engine::Engine;
use crate::host::{Failure, Kind, error};
use crate::project::{Files, Project};
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
    let directories = project.directories.clone();
    Engine::new(&project.workspace, store).with_build_for(system::build(), move |from| {
        natives(&files, directories.get(&from.0).cloned())
    })
}

/// The native code of the build target for one package, which is at
/// `directory`; one that is nowhere reads nothing.
fn natives(files: &Rc<dyn Files>, directory: Option<PathBuf>) -> Natives {
    let files = Rc::clone(files);
    // Reading again says whether what was read still holds, so a run
    // is made again only when a file it read has changed.
    Natives::new().verifiable(system::PACKAGE, "fs_read", move |args| {
        let Some(Value::String(path)) = args.first() else {
            return Err("argument 0 is not text".to_owned());
        };
        let read = within(directory.as_deref(), path).and_then(|path| {
            files.read(&path).map_err(|reason| Failure {
                kind: Kind::NotFound,
                message: format!("{}: {reason}", path.display()),
            })
        });
        Ok(read.map_or_else(|failure| error("fs", &failure), Value::String))
    })
}

/// Where `path` is under a package's directory, if it names something
/// under it: a build reads nothing of the machine but its own package.
fn within(directory: Option<&Path>, path: &str) -> Result<PathBuf, Failure> {
    let inside = Path::new(path)
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    match directory {
        Some(directory) if inside => Ok(directory.join(path)),
        _ => Err(Failure {
            kind: Kind::Denied,
            message: format!("{path} is not in the package"),
        }),
    }
}
