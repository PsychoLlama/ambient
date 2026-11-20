//! The C header a native bundle of a package is written against: what
//! it may call of the runtime, and a prototype for each extern of the
//! package. See "Native bundles" in `docs/spec/externs.md`.
//!
//! The entries come from `ambient_native::ENTRIES`, the one list of
//! them, so a bundle in C and one in Rust are told the same.

use std::fmt::Write;
use std::path::Path;

use ambient_host::native::MACHINE;
use ambient_native::{ENTRIES, Entry, FALSE, LAYOUT, NULL, TRUE, kind};

use super::Session;
use super::linking::{built, plain, symbol};
use crate::surface::DefinitionKind;
use crate::symbol::PackageId;
use crate::workspace::Sources;

/// What C calls a type an entry is written with, if it is one an
/// entry may name.
fn named(rust: &str) -> Option<&'static str> {
    let plain: String = rust.chars().filter(|c| !c.is_whitespace()).collect();
    Some(match plain.as_str() {
        "" => "void",
        "Word" | "u64" => "uint64_t",
        "i64" => "int64_t",
        "*constu8" => "const uint8_t *",
        "*constWord" => "const uint64_t *",
        "*mutCall" => "struct amb_call *",
        "Make" => "amb_make",
        "Dropped" => "amb_dropped",
        "*mutData" => "void *",
        _ => return None,
    })
}

/// The prototype of `entry`, under what it does.
fn prototype(entry: &Entry) -> String {
    let c = |rust| named(rust).unwrap_or("void");
    let mut said = String::new();
    for line in entry.about.lines() {
        let _ = writeln!(said, "//{line}");
    }
    let takes: Vec<String> = entry
        .takes
        .iter()
        .map(|(name, ty)| format!("{} {name}", c(ty)))
        .collect();
    let takes = if takes.is_empty() {
        "void".to_owned()
    } else {
        takes.join(", ")
    };
    let _ = writeln!(said, "{} {}({takes});", c(entry.gives), entry.name);
    said
}

/// The header for the package of `id`, whose externs are `externs`
/// and whose tags and brands are `tags`: each by its module path and
/// name.
#[must_use]
pub fn header(id: &str, externs: &[String], tags: &[String]) -> String {
    let mut said = format!(
        "// What a native bundle of the Ambient package {id} is written\n\
         // against, for layout {LAYOUT}. Written by `ambient header`: do not edit.\n\
         // The command that wrote it links for the platform `{MACHINE}`,\n\
         // which is what a manifest names a bundle for it.\n\
         #ifndef AMBIENT_H\n#define AMBIENT_H\n#include <stdint.h>\n\n\
         // The call a function is in: what the `amb_bundle_` entries take.\n\
         struct amb_call;\n\n\
         // What makes the answer to a wait, on the program's thread, of\n\
         // what a thread of the bundle handed over. Zero fails the task.\n\
         typedef uint64_t (*amb_make)(void *data);\n\n\
         // What is told the number of a handle its program holds no longer.\n\
         typedef void (*amb_dropped)(uint64_t number);\n\n\
         // What says the layout this bundle was built for: a program of\n\
         // another asks for a name this is not, and does not link.\n\
         __attribute__((weak)) const unsigned char {built} = 0;\n\n\
         #define AMB_NULL ((uint64_t){NULL})\n\
         #define AMB_FALSE ((uint64_t){FALSE})\n\
         #define AMB_TRUE ((uint64_t){TRUE})\n\n\
         // What `amb_kind` says a value is.\n",
        built = built(id),
    );
    for (name, number) in kind::NAMED {
        let _ = writeln!(said, "#define AMB_KIND_{name} {number}");
    }
    for entry in ENTRIES {
        let _ = write!(said, "\n{}", prototype(entry));
    }
    said.push_str(
        "\n// The package's externs. Each takes the call it is in and its\n\
         // arguments, a tuple it does not own, and gives a value its caller\n\
         // owns. A bundle defines those a program may reach.\n",
    );
    for path in externs {
        let _ = writeln!(
            said,
            "uint64_t {}(struct amb_call *call, uint64_t args);",
            symbol(id, path)
        );
    }
    if !tags.is_empty() {
        said.push_str(
            "\n// The text of each tag and brand of the package, as `amb_tag` takes\n\
             // it and `amb_string_bytes` gives it: `sizeof` it, less one, is its\n\
             // length.\n",
        );
    }
    for path in tags {
        let name = plain(path);
        let _ = writeln!(said, "#define AMB_TAG_{name} \"{id}\" \"\\0\" \"{path}\"");
    }
    said.push_str("\n#endif\n");
    said
}

impl Session<'_> {
    /// Says the header for the package at `path`.
    pub(super) fn header(&self, path: &Path) -> Result<i32, Vec<String>> {
        let project = self.open(path)?;
        let root = PackageId(project.root.clone());
        let (mut externs, mut tags) = (Vec::new(), Vec::new());
        for read in project
            .modules
            .iter()
            .filter(|read| read.module.package == root)
        {
            let Some(module) = project.workspace.module(&read.module) else {
                continue;
            };
            for definition in module.definitions() {
                let namespace = definition.kind.namespace();
                let identity = read.module.symbol(namespace, &definition.name).identity();
                match definition.kind {
                    DefinitionKind::Extern(_) => externs.push(identity),
                    DefinitionKind::Tag | DefinitionKind::Brand => tags.push(identity),
                    _ => {}
                }
            }
        }
        externs.sort();
        tags.sort();
        let said = header(&project.workspace.identity(&root), &externs, &tags);
        self.world.borrow_mut().write(&said);
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use ambient_native::ENTRIES;

    use super::{header, named};

    #[test]
    fn each_type_an_entry_names_is_one_c_has() {
        for entry in ENTRIES {
            let types = entry.takes.iter().map(|(_, ty)| *ty).chain([entry.gives]);
            for ty in types {
                assert!(named(ty).is_some(), "{}: {ty}", entry.name);
            }
        }
    }

    #[test]
    fn a_header_has_a_prototype_for_each_extern_of_the_package() {
        let said = header("7c1e", &["net.connect".to_owned()], &[]);

        assert!(
            said.contains("uint64_t amb_x_7c1e_net_connect(struct amb_call *call, uint64_t args);")
        );
        assert!(said.contains("uint64_t amb_int_new(int64_t value);"));
        assert!(said.contains("void amb_release(uint64_t value);"));
    }

    #[test]
    fn a_header_has_the_text_of_each_tag_and_brand_of_the_package() {
        let said = header("7c1e", &[], &["net.Stream".to_owned()]);

        assert!(said.contains("#define AMB_TAG_net_Stream \"7c1e\" \"\\0\" \"net.Stream\"\n"));
    }
}
