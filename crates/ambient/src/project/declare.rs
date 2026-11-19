//! Writing a dependency into a manifest, so that an ID is copied by
//! the command and never by hand. See "Dependencies" in
//! `docs/spec/packages.md`.

use std::path::Path;

use toml_edit::{DocumentMut, InlineTable, Item, Table, value};

use super::members::{manifest, relative};
use super::{Files, MANIFEST, ProjectError};

/// The text of the manifest of the package at `directory`, once it
/// depends on the package at `on` under the local name `name`, or under
/// that package's own if none is given. Nothing else in the text
/// changes.
///
/// # Errors
///
/// Either manifest is missing or is no package's, or the name is taken.
pub fn declared(
    files: &dyn Files,
    directory: &Path,
    on: &Path,
    name: Option<&str>,
) -> Result<String, ProjectError> {
    let path = directory.join(MANIFEST);
    let refuse = |reason: String| ProjectError::Manifest {
        path: path.clone(),
        reason,
    };
    let no_package = |at: &Path| ProjectError::Manifest {
        path: at.join(MANIFEST),
        reason: "this is a workspace's manifest, not a package's".to_owned(),
    };
    let own = manifest(files, directory)?.package;
    let own = own.ok_or_else(|| no_package(directory))?;
    let package = manifest(files, on)?.package.ok_or_else(|| no_package(on))?;
    let name = name.unwrap_or(&package.name);
    if own.dependencies.contains_key(name) {
        return Err(refuse(format!("there is a dependency `{name}` already")));
    }
    // A path is written one way on every machine.
    let leads: Vec<String> = relative(files, directory, on)
        .iter()
        .map(|part| part.to_string_lossy().into_owned())
        .collect();
    let mut dependency = InlineTable::new();
    dependency.insert("id", package.id.into());
    dependency.insert("path", leads.join("/").into());

    let text = files
        .read(&path)
        .map_err(|error| refuse(error.to_string()))?;
    let mut document: DocumentMut = text.parse().map_err(|error| refuse(format!("{error}")))?;
    let dependencies = document
        .entry("dependencies")
        .or_insert(Item::Table(Table::new()));
    dependencies[name] = value(dependency);
    Ok(document.to_string())
}
