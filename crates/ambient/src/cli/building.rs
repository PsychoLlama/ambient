//! The world comptime calls of a package on disk run in: the native
//! side of `ambient.build`. See "Build abilities" in `docs/spec/tools.md`.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ambient_runtime::FAULT_PACKAGE;

use super::Session;
use super::linking::allowing;
use crate::backend::native::Natives;
use crate::engine::{Engine, refused};
use crate::library;
use crate::project::{Contents, Files, Project};
use crate::store::Store;
use crate::symbol::PackageId;
use crate::value::Value;
use crate::workspace::{Sources, native};

impl Session<'_> {
    pub(super) fn engine<'e>(&'e self, project: &'e Project) -> Engine<'e> {
        // What a build fetches, it fetches as the machine fetches a
        // snapshot.
        engine(self.files, self.store, project).with_fetch(|url| self.snapshots.bytes(url))
    }
}

/// An engine over a project read from `files`, whose comptime calls
/// and tools read the files of the package each is written in.
#[must_use]
pub fn engine<'e>(files: &dyn Files, store: &'e dyn Store, project: &'e Project) -> Engine<'e> {
    let files = files.shared();
    let packages = project.packages.clone();
    // Host code is asked for by the ID of the package that keeps it.
    let mut hosts: Hosts = BTreeMap::new();
    for (name, contents) in &packages {
        let package = PackageId(name.clone());
        let id = project.workspace.identity(&package);
        // Host code is native code: a package that may bring none
        // has none to give, and says how it would.
        let host = if native(&project.workspace, &package) {
            Ok(contents.clone())
        } else {
            Err(allowing(project, name))
        };
        hosts.entry(id).or_default().push(host);
    }
    let hosts = Rc::new(hosts);
    Engine::new(&project.workspace, store).with_build_for(library::build(), move |from| {
        natives(&files, packages.get(&from.0), &hosts)
    })
}

/// The files of each package of a project, by the package's ID: two
/// snapshots of one package are two sets of them.
type Hosts = BTreeMap<String, Vec<Result<Contents, String>>>;

/// Why a build was not answered: the tag of `ambient.build` for it,
/// and what to say.
struct Refused(&'static str, String);

/// The error a refusal is, as `ambient.Error` describes one.
fn error(Refused(kind, message): Refused) -> Value {
    refused(kind, message)
}

/// The native code of the build target for one package, whose files
/// are `contents`; one that has none reads nothing.
///
/// Each can be asked again to see whether what it gave still holds,
/// so a run is made again only when something it read has changed.
fn natives(files: &Rc<dyn Files>, contents: Option<&Contents>, hosts: &Rc<Hosts>) -> Natives {
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
    let hosting = {
        let (files, hosts) = (Rc::clone(files), Rc::clone(hosts));
        move |args: &[Value]| {
            let [
                Value::String(package),
                Value::String(platform),
                Value::String(path),
            ] = args
            else {
                return Err("the arguments are not three texts".to_owned());
            };
            let path = Path::new(platform).join(path);
            let kept = hosts.get(package).map(Vec::as_slice).unwrap_or_default();
            // Snapshots of one package declare one extern, so the
            // host code of any of them that has it is the extern's.
            let read = kept.iter().flatten().find_map(|contents| {
                let inside = contents.holds(&path)?;
                files.read(&contents.directory.join(inside)).ok()
            });
            let missing = || {
                let barred = kept.iter().find_map(|host| host.as_ref().err());
                let why = match barred {
                    Some(allowing) => format!(
                        "the package `{package}` may bring no native code, which host code is.{allowing}"
                    ),
                    None => format!(
                        "the package `{package}` has no host code at {}",
                        path.display()
                    ),
                };
                Refused("not_found", why)
            };
            Ok(read
                .map(Value::String)
                .ok_or_else(missing)
                .unwrap_or_else(error))
        }
    };
    Natives::new()
        .verifiable(FAULT_PACKAGE, "build.hosting", hosting)
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
