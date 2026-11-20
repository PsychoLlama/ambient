//! A native bundle named by its digest: fetched when a program first
//! reaches it, and held to the digest. See "Fetching" in
//! `docs/spec/releases.md` and "Layout" in `docs/spec/packages.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use ambient::backend::native::MACHINE;
use ambient::cli::{Command, Link, Linker};
use ambient::hash::Hash;
use ambient::project::{Commit, OFFLINE, Offline, Snapshots, Source, digest};
use ambient::store::MemoryStore;
use support::packages::Files;
use support::scripted::Scripted;

const URL: &str = "https://example.test/app.o";
const BUNDLE: &[u8] = b"an object";

const SOURCE: &str = "extern fn double(n: int): int;

pub fn main(): int => double(21);

pub fn idle(): int => 1;
";

/// A machine that keeps files by digest, and a network that serves
/// one file.
#[derive(Default)]
struct Mirror {
    served: Vec<u8>,
    kept: RefCell<BTreeMap<String, Vec<u8>>>,
    /// Each URL that was asked for.
    asked: RefCell<Vec<String>>,
}

impl Snapshots for Mirror {
    fn held(&self, _: &Hash) -> Option<PathBuf> {
        None
    }

    fn resolve(&self, source: &Source) -> Result<String, String> {
        Err(format!("nothing reaches {}", source.git))
    }

    fn fetch(&self, commit: Commit) -> Result<Hash, String> {
        Err(format!("nothing reaches {}", commit.git))
    }

    fn bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        self.asked.borrow_mut().push(url.to_owned());
        Ok(self.served.clone())
    }

    fn kept(&self, digest: &str) -> Option<PathBuf> {
        let kept = self.kept.borrow().contains_key(digest);
        kept.then(|| PathBuf::from("kept").join(digest))
    }

    fn keep(&self, digest: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        self.kept
            .borrow_mut()
            .insert(digest.to_owned(), bytes.to_vec());
        Ok(PathBuf::from("kept").join(digest))
    }
}

/// A linker that joins nothing, and keeps what it was asked to.
#[derive(Default)]
struct Asked(RefCell<Vec<Link>>);

impl Linker for Asked {
    fn link(&self, link: &Link) -> Result<Vec<u8>, String> {
        self.0.borrow_mut().push(link.clone());
        Ok(b"linked".to_vec())
    }

    fn run(&self, _: &[u8], _: &[String]) -> Result<i32, String> {
        Ok(0)
    }
}

/// A package whose bundle for this machine is `bundle`, as its
/// manifest says it.
fn package(bundle: &str) -> Files {
    let manifest = format!(
        "name = \"app\"\nid = \"00000000000000000000000000000001\"\nversion = \"1.0.0\"\n\n\
         [native.{MACHINE}]\n{bundle}\n"
    );
    let files = [
        ("app/ambient.toml", manifest.as_str()),
        ("app/src/main.ab", SOURCE),
    ];
    files
        .into_iter()
        .map(|(path, text)| (PathBuf::from(path), text.to_owned()))
        .collect()
}

/// A package whose bundle is the file of [`BUNDLE`]'s digest at [`URL`].
fn listed() -> Files {
    package(&format!("digest = \"{}\"\nurl = \"{URL}\"", digest(BUNDLE)))
}

/// What building `entry` of `files` on `mirror` exits with, the
/// bundles the linker was handed, and what was said went wrong.
fn built(files: &Files, mirror: &dyn Snapshots, entry: &str) -> (i32, Vec<PathBuf>, String) {
    let build = Command::Build {
        path: PathBuf::from("app"),
        entry: entry.to_owned(),
        output: None,
        target: None,
    };
    let linker = Asked::default();
    let (code, world) =
        Scripted::default().execute_linked(files, &MemoryStore::new(), mirror, &linker, &build);
    let bundles = linker
        .0
        .borrow()
        .iter()
        .flat_map(|link| link.bundles.clone())
        .collect();
    (code, bundles, world.err)
}

fn serving(served: &[u8]) -> Mirror {
    Mirror {
        served: served.to_vec(),
        ..Mirror::default()
    }
}

#[test]
fn a_bundle_a_program_reaches_is_fetched_and_kept_by_its_digest() {
    let mirror = serving(BUNDLE);
    let kept = PathBuf::from("kept").join(digest(BUNDLE));

    assert_eq!(
        built(&listed(), &mirror, "main.main"),
        (0, vec![kept.clone()], String::new())
    );
    assert_eq!(
        built(&listed(), &mirror, "main.main"),
        (0, vec![kept], String::new())
    );
    assert_eq!(*mirror.asked.borrow(), [URL]);
}

#[test]
fn a_bundle_no_program_reaches_is_not_fetched() {
    let mirror = serving(BUNDLE);

    assert_eq!(
        built(&listed(), &mirror, "main.idle"),
        (0, vec![], String::new())
    );
    assert!(mirror.asked.borrow().is_empty());
}

#[test]
fn a_file_that_is_not_of_the_digest_is_refused() {
    let mirror = serving(b"some other object");
    let (code, bundles, said) = built(&listed(), &mirror, "main.main");

    assert_eq!((code, bundles), (1, vec![]));
    assert!(said.contains(&format!("not {}", digest(BUNDLE))), "{said}");
    assert!(mirror.kept.borrow().is_empty());
}

#[test]
fn what_a_member_keeps_in_its_directory_is_used_as_it_is() {
    let bundle = format!(
        "digest = \"{}\"\nurl = \"{URL}\"\npath = \"native/app.o\"",
        digest(BUNDLE)
    );
    let mut files = package(&bundle);
    files.insert(
        PathBuf::from("app/native/app.o"),
        "being worked on".to_owned(),
    );
    let mirror = serving(BUNDLE);

    let there = vec![PathBuf::from("app/native/app.o")];
    assert_eq!(
        built(&files, &mirror, "main.main"),
        (0, there, String::new())
    );
    assert!(mirror.asked.borrow().is_empty());
}

#[test]
fn a_bundle_with_nowhere_to_fetch_it_from_is_said_to_have_none() {
    let files = package(&format!("digest = \"{}\"", digest(BUNDLE)));
    let (code, _, said) = built(&files, &serving(BUNDLE), "main.main");

    assert_eq!(code, 1);
    assert!(said.contains("nothing says where to fetch"), "{said}");
}

#[test]
fn what_was_fetched_beforehand_is_built_with_no_network() {
    let mirror = serving(BUNDLE);
    let fetch = Command::Fetch {
        path: PathBuf::from("app"),
        entry: Some("main.main".to_owned()),
        target: None,
    };
    let store = MemoryStore::new();
    let fetched = Scripted::default().execute_from(&listed(), &store, &mirror, &fetch);

    assert_eq!((fetched.0, fetched.1.err.as_str()), (0, ""));
    assert_eq!(built(&listed(), &Offline(&mirror), "main.main").0, 0);
    assert_eq!(*mirror.asked.borrow(), [URL]);
}

#[test]
fn a_build_with_no_network_fails_where_it_would_fetch() {
    let mirror = serving(BUNDLE);
    let (code, _, said) = built(&listed(), &Offline(&mirror), "main.main");

    assert_eq!(code, 1);
    assert!(said.contains(OFFLINE), "{said}");
    assert!(mirror.asked.borrow().is_empty());
}
