//! Builds the runtime for WebAssembly as one relocatable object, so the
//! compiler can link into each module the parts of it the module
//! reaches. See `spec/linking.md`. Also names the build, for the
//! store.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

const TARGET: &str = "wasm32-unknown-unknown";

fn run(command: &mut Command, what: &str) {
    let status = command.status();
    assert!(status.is_ok_and(|status| status.success()), "{what} failed");
}

/// The linker the toolchain links WebAssembly with.
fn linker() -> PathBuf {
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(rustc).args(["--print", "sysroot"]).output();
    let sysroot = output.map_or_else(
        |_| String::new(),
        |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    );
    let host = env::var("HOST").unwrap_or_default();
    Path::new(&sysroot)
        .join("lib/rustlib")
        .join(host)
        .join("bin/rust-lld")
}

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

/// Names this build of the compiler: its source, the host's, the
/// runtime's, and the versions of what they depend on. A stored result
/// is only reused by the build that made it (`Engine::stage_if`).
fn identify() {
    let mut hasher = DefaultHasher::new();
    let inputs = [
        "src",
        "../ambient-host/src",
        "../ambient-runtime/src",
        "../../Cargo.lock",
    ];
    for input in inputs {
        println!("cargo::rerun-if-changed={input}");
        feed(Path::new(input), &mut hasher);
    }
    println!("cargo::rustc-env=AMBIENT_BUILD={:016x}", hasher.finish());
}

fn main() {
    identify();
    println!("cargo::rerun-if-changed=../ambient-runtime/src");
    println!("cargo::rerun-if-changed=../ambient-runtime/Cargo.toml");
    println!("cargo::rerun-if-changed=../../Cargo.lock");

    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap_or_default());
    // A target directory of its own: the one this build is running in is
    // locked by the cargo that is running this script.
    let target = out.join("runtime");
    // An archive of objects, the standard library's among them. What a
    // module does not reach is never linked into it, so the runtime is
    // built to be small where it is reached, not to be fast to build.
    run(
        Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["rustc", "--release", "--package", "ambient-runtime"])
            .args(["--target", TARGET, "--crate-type", "staticlib"])
            .arg("--target-dir")
            .arg(&target)
            // Flags meant for the host build do not apply to this one.
            // A panic names the file it is in, and a trap reads no
            // name, so the names are made as short as they go.
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env(
                "RUSTFLAGS",
                "-C opt-level=s -C codegen-units=1 -C panic=abort \
                 --remap-path-prefix=crates/ambient-runtime/src/=",
            ),
        "building the runtime for WebAssembly",
    );
    // One object with one symbol table: the linker resolves every
    // symbol between the archive's members and keeps each function and
    // each datum apart, with what it refers to.
    let archive = target.join(TARGET).join("release/libambient_runtime.a");
    run(
        Command::new(linker())
            .args(["-flavor", "wasm", "-r", "--strip-debug", "--whole-archive"])
            .arg(&archive)
            .arg("-o")
            .arg(out.join("ambient_runtime.o")),
        "linking the runtime into one object",
    );
}
