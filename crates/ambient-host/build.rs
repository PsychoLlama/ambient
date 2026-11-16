//! Names the build of the host, for compiled parts, and the machine
//! it is for.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::{env, fs};

/// Feeds every file under `path` to `hasher`, in a fixed order.
fn feed(path: &Path, hasher: &mut DefaultHasher) {
    let Ok(entries) = fs::read_dir(path) else {
        fs::read(path).unwrap_or_default().hash(hasher);
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        path.file_name().hash(hasher);
        feed(&path, hasher);
    }
}

/// Names this build of the host: its source, the runtime's, and the
/// versions of what they depend on. Compiled parts are only loaded by
/// the host they were compiled for (`Parts::decode`).
///
/// The compiler's source is no part of it: what compiled code relies
/// on is the runtime and the host, so a change to the compiler alone
/// leaves a host built before it able to load what it compiles.
fn identify() {
    let mut hasher = DefaultHasher::new();
    let inputs = ["src", "../ambient-runtime/src", "../../Cargo.lock"];
    for input in inputs {
        println!("cargo::rerun-if-changed={input}");
        feed(Path::new(input), &mut hasher);
    }
    println!("cargo::rustc-env=AMBIENT_BUILD={:016x}", hasher.finish());
}

fn main() {
    identify();
    // The machine native code is for: what the native backend's name
    // ends with.
    let target = env::var("TARGET").unwrap_or_default();
    println!("cargo::rustc-env=AMBIENT_TARGET={target}");
}
