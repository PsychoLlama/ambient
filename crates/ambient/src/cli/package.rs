//! What the command line says of a package as files. See "Identity"
//! in `spec/packages.md`.

use std::path::Path;

use super::Session;
use super::init::names;
use crate::project::{self, MANIFEST};

impl Session<'_> {
    /// Says the snapshot of the package at `path`.
    pub(super) fn snapshot(&self, path: &Path) -> Result<i32, Vec<String>> {
        let project = self.open(path)?;
        let snapshot = project.packages[&project.root].snapshot(self.files);
        let snapshot = snapshot.map_err(|error| vec![error.to_string()])?;
        self.world.borrow_mut().write(&format!("{snapshot}\n"));
        Ok(0)
    }

    /// Makes the package at `path` depend on the one at `on`.
    pub(super) fn add(
        &self,
        path: &Path,
        on: &Path,
        name: Option<&str>,
    ) -> Result<i32, Vec<String>> {
        if let Some(name) = name.filter(|name| !names(name)) {
            let said = format!("`{name}` is no name: letters, digits, and `_`, a letter first");
            return Err(vec![said]);
        }
        let manifest = project::declared(self.files, path, on, name);
        let manifest = manifest.map_err(|error| vec![error.to_string()])?;
        let file = path.join(MANIFEST);
        let mut world = self.world.borrow_mut();
        world
            .store(&file.to_string_lossy(), manifest.as_bytes())
            .map_err(|failure| vec![format!("{}: {}", file.display(), failure.message)])?;
        Ok(0)
    }
}
