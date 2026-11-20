//! What is linked for a program that reaches native bundles: the
//! functions of them it reaches, and the `main` that hands those to
//! what loads the program. See "Native code" in
//! `docs/spec/linking.md` and "Distribution" in `docs/spec/externs.md`.
//!
//! Nothing here runs a linker: [`Link`] says what one is to join, and a
//! [`Linker`] is what does.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::io::Read;
use std::path::PathBuf;

use ambient_host::native::MACHINE;

use super::Session;
use crate::engine::Parts;
use crate::project::{Project, fetched};
use crate::symbol::PackageId;
use crate::workspace::Sources;

/// What the launcher is as an archive, compressed: what a link joins
/// native bundles with, once [`archive`] has made it what it was.
const PACKED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/launcher.a.gz"));

/// The launcher as an archive.
///
/// # Errors
///
/// Fails if what the command holds is not what its build wrote.
pub fn archive() -> std::io::Result<Vec<u8>> {
    let mut archive = Vec::new();
    flate2::read::GzDecoder::new(PACKED).read_to_end(&mut archive)?;
    Ok(archive)
}

/// The libraries of the system an archive of the launcher is linked
/// with, as the compiler that made it names them to a linker.
pub const LIBRARIES: &str = include_str!(concat!(env!("OUT_DIR"), "/launcher.libraries"));

/// What a linker is to join into what loads one program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// The `main` of the executable, in C: it lists each function of a
    /// native bundle the program reaches, and starts the launcher with
    /// them.
    pub main: String,
    /// The native bundles those functions are in.
    pub bundles: Vec<PathBuf>,
    /// The libraries of the system the bundles say they need.
    pub libraries: Vec<String>,
}

/// What joins native bundles with the launcher, and runs what it made.
pub trait Linker {
    /// The executable `link` describes, with no program after it.
    ///
    /// # Errors
    ///
    /// Fails with what the linker said, or that there is none.
    fn link(&self, link: &Link) -> Result<Vec<u8>, String>;

    /// Runs `executable` with `args` in the place of this process, and
    /// gives what it exited with.
    ///
    /// # Errors
    ///
    /// Fails if it could not be started.
    fn run(&self, executable: &[u8], args: &[String]) -> Result<i32, String>;
}

/// No linker: what a session that is to link nothing is given.
pub struct Unlinked;

impl Linker for Unlinked {
    fn link(&self, _: &Link) -> Result<Vec<u8>, String> {
        Err("nothing here links a native bundle".to_owned())
    }

    fn run(&self, _: &[u8], _: &[String]) -> Result<i32, String> {
        Err("nothing here runs what was linked".to_owned())
    }
}

/// The name a native bundle has for the function of an extern: the ID
/// of the package that declares it, then its module path and name,
/// with whatever a C name cannot hold made an underscore.
#[must_use]
pub fn symbol(id: &str, path: &str) -> String {
    format!("{EXTERN}{}_{}", plain(id), plain(path))
}

/// What begins the name of every function of an extern.
pub(super) const EXTERN: &str = "amb_x_";

/// What begins the name that says what layout a bundle was built for.
pub(super) const BUILT: &str = "amb_built_";

/// `text` with whatever a C name cannot hold made an underscore.
pub(super) fn plain(text: &str) -> String {
    let plain = |c: char| if c.is_ascii_alphanumeric() { c } else { '_' };
    text.chars().map(plain).collect()
}

/// The name a native bundle of the package of `id` defines to say it
/// was built for this layout.
#[must_use]
pub fn built(id: &str) -> String {
    format!("{BUILT}{}_{}", plain(id), ambient_native::LAYOUT)
}

/// One function a program reaches of a native bundle.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Root {
    /// The ID of the package that declares the extern, which is how
    /// a program names it.
    package: String,
    path: String,
    symbol: String,
}

/// C text of `text`, which is a name: it holds nothing to escape but
/// what a string could not.
fn quoted(text: &str) -> String {
    let plain = |c: &char| !matches!(c, '"' | '\\') && !c.is_control();
    format!("\"{}\"", text.chars().filter(plain).collect::<String>())
}

/// The `main` that starts the launcher with `roots`.
fn main(roots: &BTreeSet<Root>) -> String {
    let mut main = String::from(
        "#include <stdint.h>\n\
         struct amb_call;\n\
         typedef uint64_t (*amb_extern)(struct amb_call *, uint64_t);\n\
         struct amb_root { const char *package; const char *path; amb_extern function; \
         const unsigned char *built; };\n\
         int ambient_launch(const struct amb_root *, uintptr_t, int, char **);\n",
    );
    for root in roots {
        let _ = writeln!(
            main,
            "uint64_t {}(struct amb_call *, uint64_t);",
            root.symbol
        );
    }
    let packages: BTreeSet<&String> = roots.iter().map(|root| &root.package).collect();
    for package in packages {
        let _ = writeln!(main, "extern const unsigned char {};", built(package));
    }
    main.push_str("static const struct amb_root roots[] = {\n");
    for root in roots {
        let (package, path) = (quoted(&root.package), quoted(&root.path));
        let built = built(&root.package);
        let _ = writeln!(
            main,
            "  {{ {package}, {path}, {}, &{built} }},",
            root.symbol
        );
    }
    let _ = write!(
        main,
        "}};\nint main(int argc, char **argv) {{\n  \
         return ambient_launch(roots, {}, argc, argv);\n}}\n",
        roots.len()
    );
    main
}

impl Session<'_> {
    /// What is to be linked for the program of `parts`, if it reaches
    /// an extern of a package that has native bundles.
    ///
    /// # Errors
    ///
    /// Fails if such a package has no bundle for this machine, or none
    /// that is there.
    pub(super) fn linking(&self, project: &Project, parts: &Parts) -> Result<Option<Link>, String> {
        let mut roots = BTreeSet::new();
        let mut bundles = BTreeSet::new();
        let mut libraries = Vec::new();
        // An extern names its package by ID, as a tag does.
        let identity = |name: &String| project.workspace.identity(&PackageId(name.clone()));
        let offering: BTreeMap<String, &String> = project
            .natives
            .keys()
            .map(|name| (identity(name), name))
            .collect();
        for native in &parts.natives {
            let Some(package) = offering.get(&native.package).copied() else {
                continue;
            };
            let bundle = project.natives[package].get(MACHINE).ok_or_else(|| {
                // A platform is named as the compiler names it, which
                // is seldom what its author guessed.
                let has: Vec<&str> = project.natives[package]
                    .keys()
                    .map(String::as_str)
                    .collect();
                let said = "has no native bundle for";
                format!(
                    "package `{package}` {said} `{MACHINE}`, only for `{}`",
                    has.join("`, `")
                )
            })?;
            let directory = &project.packages[package].directory;
            // What a member keeps in its directory is used as it is;
            // anything else is the file of the digest.
            let place = bundle.path.as_ref().map(|path| directory.join(path));
            let place = place.filter(|place| self.files.bytes(place).is_ok());
            let place = match (place, &bundle.digest) {
                (Some(place), _) => place,
                (None, Some(digest)) => fetched(self.snapshots, digest, bundle.url.as_deref())
                    .map_err(|reason| {
                        format!("the native bundle of package `{package}`: {reason}")
                    })?,
                (None, None) => {
                    let said = "the native bundle of package";
                    return Err(format!("{said} `{package}` for `{MACHINE}` is not there"));
                }
            };
            if bundles.insert(place) {
                libraries.extend(bundle.libraries.iter().cloned());
            }
            roots.insert(Root {
                package: native.package.clone(),
                path: native.path.clone(),
                symbol: symbol(&native.package, &native.path),
            });
        }
        Ok((!roots.is_empty()).then(|| Link {
            main: main(&roots),
            bundles: bundles.into_iter().collect(),
            libraries,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{Root, built, main, symbol};

    #[test]
    fn a_function_is_named_for_its_package_and_its_extern() {
        assert_eq!(
            symbol("7c1e", "net.tls.connect"),
            "amb_x_7c1e_net_tls_connect"
        );
        assert_eq!(symbol("a-b", "open"), "amb_x_a_b_open");
    }

    #[test]
    fn a_main_lists_each_function_and_starts_the_launcher() {
        let root = Root {
            package: "hue".to_owned(),
            path: "tls.connect".to_owned(),
            symbol: symbol("7c1e", "tls.connect"),
        };
        let main = main(&BTreeSet::from([root]));

        assert!(main.contains("uint64_t amb_x_7c1e_tls_connect(struct amb_call *, uint64_t);"));
        let built = built("hue");
        assert!(main.contains(&format!("extern const unsigned char {built};")));
        assert!(main.contains(&format!(
            "{{ \"hue\", \"tls.connect\", amb_x_7c1e_tls_connect, &{built} }},"
        )));
        assert!(main.contains("return ambient_launch(roots, 1, argc, argv);"));
    }
}
