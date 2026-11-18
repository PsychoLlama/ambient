//! What the command line says of a package as files. See "Identity"
//! in `spec/packages.md`.

use std::path::Path;

use super::Session;

impl Session<'_> {
    /// Says the snapshot of the package at `path`.
    pub(super) fn snapshot(&self, path: &Path) -> Result<i32, Vec<String>> {
        let project = self.open(path)?;
        let snapshot = project.packages[&project.root].snapshot(self.files);
        let snapshot = snapshot.map_err(|error| vec![error.to_string()])?;
        self.world.borrow_mut().write(&format!("{snapshot}\n"));
        Ok(0)
    }
}
