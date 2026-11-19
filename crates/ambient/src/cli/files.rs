//! Writing the files of a build into a directory the command owns.
//! See "Builds" in `docs/spec/tools.md`.

use std::collections::BTreeSet;
use std::path::{Component, Path};

use super::Session;
use crate::engine::Files;

/// The file in a build's directory that lists what the last build
/// wrote there, a path to a line: what the next build takes away if
/// it does not give it again.
const WRITTEN: &str = ".ambient-build";

/// A path a build gives, if it names something under the directory
/// the build is written into.
fn inside(path: &str) -> Option<&str> {
    let normal = |part| matches!(part, Component::Normal(_));
    let under = !path.is_empty() && path != WRITTEN && Path::new(path).components().all(normal);
    under.then_some(path)
}

impl Session<'_> {
    /// Writes `files` into `directory`, and takes away what the build
    /// before this one wrote there and this one does not give.
    /// Nothing else in the directory is touched.
    pub(super) fn written(&self, directory: &Path, files: &Files) -> Result<i32, Vec<String>> {
        let mut world = self.world.borrow_mut();
        let within = |path: &str| directory.join(path).to_string_lossy().into_owned();
        let failed = |path: &str, failure: crate::host::Failure| {
            vec![format!("{}: {}", within(path), failure.message)]
        };
        let mut given = BTreeSet::new();
        for (path, _) in files {
            let Some(path) = inside(path) else {
                return Err(vec![format!(
                    "the build gives `{path}`, which is not a file under its directory"
                )]);
            };
            if !given.insert(path) {
                return Err(vec![format!("the build gives `{path}` twice")]);
            }
        }
        let before = world.read(&within(WRITTEN)).unwrap_or_default();
        let before = String::from_utf8_lossy(&before).into_owned();
        for stale in before.lines().filter_map(inside) {
            if !given.contains(stale) && world.exists(&within(stale)) {
                world
                    .remove(&within(stale))
                    .map_err(|failure| failed(stale, failure))?;
            }
        }
        for (path, bytes) in files {
            let parent = Path::new(&within(path)).parent().map(Path::to_owned);
            let parent = parent.unwrap_or_else(|| directory.to_owned());
            world
                .make_directory(&parent.to_string_lossy())
                .and_then(|()| world.store(&within(path), bytes))
                .map_err(|failure| failed(path, failure))?;
        }
        let listed = given
            .iter()
            .fold(String::new(), |listed, path| listed + path + "\n");
        world
            .store(&within(WRITTEN), listed.as_bytes())
            .map_err(|failure| failed(WRITTEN, failure))?;
        let size: usize = files.iter().map(|(_, bytes)| bytes.len()).sum();
        world.write(&format!(
            "built {}: {} files, {size} bytes\n",
            directory.display(),
            files.len()
        ));
        Ok(0)
    }
}
