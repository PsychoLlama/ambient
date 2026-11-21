//! The members a workspace's manifest lists: each a directory, or a
//! pattern for several. See "Workspaces" in `docs/spec/packages.md`.
//!
//! A pattern's `*` stands for the whole name of one directory and
//! nothing else is a pattern, so finding what one matches lists only
//! the directories it names, never a tree.

use std::path::{Path, PathBuf};

use super::{Files, MANIFEST};

/// What stands for any one directory's name.
const ANY: &str = "*";

/// Whether a part of a listed member stands for any directory.
fn any(part: &std::ffi::OsStr) -> bool {
    part == ANY
}

/// What is wrong with how a member is listed, if anything is.
pub(super) fn wrong(member: &Path) -> Option<&'static str> {
    let partly = |part: &std::ffi::OsStr| !any(part) && part.to_string_lossy().contains(ANY);
    member
        .iter()
        .any(partly)
        .then_some("has a `*` that is not the whole name of a directory")
}

/// Whether the listed member `pattern` is, or matches, the directory
/// `path`, each written from the workspace's own.
pub(super) fn covers(pattern: &Path, path: &Path) -> bool {
    let same = |(listed, part)| any(listed) || listed == part;
    pattern.iter().count() == path.iter().count() && pattern.iter().zip(path.iter()).all(same)
}

/// The directories the workspace at `root` lists, each from `root`: a
/// member listed as a directory is that directory, there or not, and
/// one listed as a pattern is every directory it matches that holds a
/// manifest, in order.
pub(super) fn expanded(files: &dyn Files, root: &Path, listed: &[PathBuf]) -> Vec<PathBuf> {
    let mut members = Vec::new();
    for member in listed {
        if !member.iter().any(any) {
            members.push(member.clone());
            continue;
        }
        let mut matched = vec![PathBuf::new()];
        for part in member {
            let under = |at: &PathBuf| -> Vec<PathBuf> {
                if !any(part) {
                    return vec![at.join(part)];
                }
                // What cannot be listed holds nothing that matches.
                let names = files.names(&root.join(at)).unwrap_or_default();
                names.iter().map(|name| at.join(name)).collect()
            };
            matched = matched.iter().flat_map(under).collect();
        }
        let package = |at: &PathBuf| files.read(&root.join(at).join(MANIFEST)).is_ok();
        members.extend(matched.into_iter().filter(package));
    }
    members
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use super::{covers, expanded, wrong};

    /// What a workspace at `work` lists as `listed`, where each of
    /// `packages` is a directory there that holds a manifest.
    fn members(packages: &[&str], listed: &[&str]) -> Vec<String> {
        let manifest = |at: &&str| {
            (
                PathBuf::from(format!("work/{at}/ambient.toml")),
                String::new(),
            )
        };
        let mut files: BTreeMap<PathBuf, String> = packages.iter().map(manifest).collect();
        files.insert("work/notes/read.me".into(), String::new());
        let listed: Vec<PathBuf> = listed.iter().map(PathBuf::from).collect();
        let found = expanded(&files, Path::new("work"), &listed);
        found.iter().map(|at| at.display().to_string()).collect()
    }

    #[test]
    fn a_pattern_is_every_directory_it_matches_that_holds_a_manifest() {
        let packages = ["app", "libs/text", "libs/time", "tools/fmt/cli"];

        assert_eq!(members(&packages, &["*"]), ["app"]);
        assert_eq!(members(&packages, &["libs/*"]), ["libs/text", "libs/time"]);
        assert_eq!(members(&packages, &["*/*/cli"]), ["tools/fmt/cli"]);
        assert_eq!(members(&packages, &["absent/*"]), [""; 0]);
    }

    #[test]
    fn a_member_listed_as_a_directory_is_that_directory_there_or_not() {
        assert_eq!(members(&["app"], &["app", "gone"]), ["app", "gone"]);
    }

    #[test]
    fn a_pattern_covers_what_it_would_match() {
        let covers = |pattern: &str, path: &str| covers(Path::new(pattern), Path::new(path));
        assert!(covers("vendor/*", "vendor/numbers") && covers("app", "app"));
        assert!(!covers("vendor/*", "vendor") && !covers("*", "vendor/numbers"));
    }

    #[test]
    fn a_star_is_the_whole_name_of_a_directory() {
        let wrong = |member: &str| wrong(Path::new(member)).is_some();
        assert!(!wrong("libs/*") && !wrong("*/cli") && !wrong("app"));
        assert!(wrong("libs/**") && wrong("lib*") && wrong("*.d/x"));
    }
}
