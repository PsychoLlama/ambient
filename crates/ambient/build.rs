//! Builds the runtime for WebAssembly as one relocatable object, so the
//! compiler can link into each module the parts of it the module
//! reaches. See `spec/linking.md`.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn main() {
    println!("cargo::rerun-if-changed=../ambient-runtime/src");
    println!("cargo::rerun-if-changed=../ambient-runtime/Cargo.toml");

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
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env(
                "RUSTFLAGS",
                "-C opt-level=s -C codegen-units=1 -C panic=abort",
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
