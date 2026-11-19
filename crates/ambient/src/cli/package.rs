//! What the command line says of a package as files. See "Identity"
//! in `docs/spec/packages.md`.

use std::path::Path;

use super::Session;
use super::init::names;
use crate::project::{self, MANIFEST, Update};

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

    /// Reads again the branch or tag the dependency `name` of the
    /// package at `path` is on, or each one in its workspace, and says
    /// whether the lock moved.
    pub(super) fn update(&self, path: &Path, name: Option<&str>) -> Result<i32, Vec<String>> {
        let update = name.map_or(Update::Every, |name| Update::One(name.to_owned()));
        let project = self.open_with(path, &update)?;
        let said = match &project.locked {
            Some((lock, _)) => format!("wrote {}\n", lock.display()),
            None => "nothing moved\n".to_owned(),
        };
        self.world.borrow_mut().write(&said);
        Ok(0)
    }

    /// Copies the dependency `name` of the package at `path` into its
    /// workspace, or says of each copy there whether it was changed.
    pub(super) fn vendor(&self, path: &Path, name: Option<&str>) -> Result<i32, Vec<String>> {
        let project = self.open(path)?;
        let refused = |error: project::ProjectError| vec![error.to_string()];
        let mut world = self.world.borrow_mut();
        let Some(name) = name else {
            for (directory, same) in project::kept(self.files, &project).map_err(refused)? {
                let state = if same { "as copied" } else { "changed" };
                world.write(&format!("{}: {state}\n", directory.display()));
            }
            return Ok(0);
        };
        let vendored = project::vendored(self.files, &project, name).map_err(refused)?;
        for (file, bytes) in &vendored.files {
            world
                .store(&file.to_string_lossy(), bytes)
                .map_err(|failure| vec![format!("{}: {}", file.display(), failure.message)])?;
        }
        let copy = vendored.directory.display();
        world.write(&format!("copied `{name}` to {copy}\n"));
        Ok(0)
    }
}
