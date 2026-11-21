//! Copying a package into a workspace as a member. See "Vendoring" in
//! `docs/spec/releases.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use toml_edit::{Array, DocumentMut, Item, Table, value};

use super::members::{Members, manifest, normal, relative};
use super::{Contents, Files, MANIFEST, Project, ProjectError};
use crate::symbol::PackageId;
use crate::workspace::Sources;

/// The directory of a workspace that vendored packages are copied to.
const VENDOR: &str = "vendor";

/// What vendoring a package comes to.
pub struct Vendored {
    /// Where the copy is.
    pub directory: PathBuf,
    /// Every file to write: the copy, and each manifest that changes.
    pub files: Vec<(PathBuf, Vec<u8>)>,
}

/// The manifests that change, each read once however many changes it
/// takes.
struct Manifests<'a> {
    files: &'a dyn Files,
    read: BTreeMap<PathBuf, DocumentMut>,
}

impl Manifests<'_> {
    /// The manifest in `directory`, to change.
    fn of(&mut self, directory: &Path) -> Result<&mut DocumentMut, ProjectError> {
        let path = directory.join(MANIFEST);
        if !self.read.contains_key(&path) {
            let refuse = |reason: String| ProjectError::Manifest {
                path: path.clone(),
                reason,
            };
            let text = self.files.read(&path);
            let text = text.map_err(|error| refuse(error.to_string()))?;
            let document = text.parse().map_err(|error| refuse(format!("{error}")))?;
            self.read.insert(path.clone(), document);
        }
        self.read
            .get_mut(&path)
            .ok_or(ProjectError::Unnamed { path })
    }
}

/// `path` as a manifest writes one: the same on every machine.
fn written(path: &Path) -> String {
    let parts: Vec<_> = path.iter().map(|part| part.to_string_lossy()).collect();
    parts.join("/")
}

/// Copies the package that the project's own calls `name` into its
/// workspace: under `vendor/`, by the package's name.
///
/// The copy is a member from then on, recorded with the snapshot it
/// was copied from, and every package that was opened and depended on
/// the original by its directory depends on the member instead.
///
/// # Errors
///
/// There is no such dependency, it is in the workspace already, it has
/// been vendored, or it depends on a package by a directory.
pub fn vendored(
    files: &dyn Files,
    project: &Project,
    name: &str,
) -> Result<Vendored, ProjectError> {
    let own = &project.packages[&project.root];
    let refuse = |reason: String| ProjectError::Manifest {
        path: own.directory.join(MANIFEST),
        reason,
    };
    let root = PackageId(project.root.clone());
    let package = project.workspace.dependency(&root, name);
    let origin = package.and_then(|package| project.packages.get(&package.0));
    let origin = origin.ok_or_else(|| refuse(format!("there is no dependency `{name}`")))?;
    let base = project.above.as_ref().unwrap_or(&own.directory);
    if Members::within(files, &origin.directory, base) {
        return Err(refuse(format!("`{name}` is in the workspace already")));
    }
    let said = manifest(files, &origin.directory)?;
    let said = said
        .package
        .ok_or_else(|| refuse(format!("`{name}` is no package")))?;
    // A copy is somewhere else, and its manifest is not changed.
    if said.dependencies.values().any(|on| on.path().is_some()) {
        let reason = format!("`{name}` depends on a package by its directory, so a copy would not");
        return Err(refuse(reason));
    }
    let directory = base.join(VENDOR).join(&said.name);
    if files.read(&directory.join(MANIFEST)).is_ok() {
        return Err(refuse(format!(
            "there is a package at {} already",
            directory.display()
        )));
    }

    let unreadable = |error: std::io::Error| ProjectError::Unreadable {
        path: origin.directory.clone(),
        reason: error.to_string(),
    };
    let mut copied = Vec::new();
    for (_, file) in origin.files(files).map_err(unreadable)? {
        let bytes = files
            .bytes(&origin.directory.join(&file))
            .map_err(unreadable)?;
        copied.push((directory.join(file), bytes));
    }
    let snapshot = origin.snapshot(files).map_err(unreadable)?;

    let mut manifests = Manifests {
        files,
        read: BTreeMap::new(),
    };
    let listed = written(&relative(files, base, &directory));
    let workspace = manifests.of(base)?;
    let workspace = workspace
        .entry("workspace")
        .or_insert(Item::Table(Table::new()));
    let members = &mut workspace["members"];
    if members.is_none() {
        *members = value(Array::new());
    }
    if let Some(members) = members.as_array_mut() {
        members.push(listed.as_str());
    }
    let record = &mut workspace["vendored"];
    if record.is_none() {
        *record = Item::Table(Table::new());
    }
    record[listed.as_str()] = value(snapshot.to_string());
    // Whatever named the original's directory would find two members
    // of one package: it depends on the one in the workspace instead.
    for dependent in project.packages.values() {
        let Some(package) = manifest(files, &dependent.directory)?.package else {
            continue;
        };
        for (local, dependency) in &package.dependencies {
            let path = dependency.path();
            let path = path.map(|path| normal(&dependent.directory.join(path)));
            if !path.is_some_and(|path| Members::same(files, &path, &origin.directory)) {
                continue;
            }
            let declared = &mut manifests.of(&dependent.directory)?["dependencies"][local];
            if let Some(declared) = declared.as_table_like_mut() {
                // A member that is nowhere is named by its ID.
                declared.remove("path");
                declared.insert("id", value(said.id.as_str()));
                declared.insert("version", value(said.version.to_string()));
            }
        }
    }
    let changed = manifests.read.into_iter();
    copied.extend(changed.map(|(path, document)| (path, document.to_string().into_bytes())));
    Ok(Vendored {
        directory,
        files: copied,
    })
}

/// Each vendored member of the project's workspace, by its directory,
/// and whether it is still the snapshot it was copied from.
///
/// # Errors
///
/// A manifest or a file of a vendored member cannot be read.
pub fn kept(files: &dyn Files, project: &Project) -> Result<Vec<(PathBuf, bool)>, ProjectError> {
    let own = &project.packages[&project.root];
    let base = project.above.as_ref().unwrap_or(&own.directory);
    let mut kept = Vec::new();
    for (path, snapshot) in manifest(files, base)?.vendored {
        let directory = normal(&base.join(path));
        let listed = manifest(files, &directory)?.package;
        let contents = Contents {
            directory: directory.clone(),
            listed: listed.map(|package| package.files).unwrap_or_default(),
        };
        let now = contents
            .snapshot(files)
            .map_err(|error| ProjectError::Unreadable {
                path: directory.clone(),
                reason: error.to_string(),
            })?;
        kept.push((directory, now.to_string() == snapshot));
    }
    Ok(kept)
}
