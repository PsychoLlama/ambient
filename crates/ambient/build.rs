//! Builds the runtime for WebAssembly, so the compiler can hand it out
//! beside the modules its WebAssembly backend emits.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=../ambient-runtime/src");
    println!("cargo::rerun-if-changed=../ambient-runtime/Cargo.toml");

    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap_or_default());
    // A target directory of its own: the one this build is running in is
    // locked by the cargo that is running this script.
    let target = out.join("runtime");
    let status = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["build", "--release", "--package", "ambient-runtime"])
        .args(["--target", "wasm32-unknown-unknown"])
        .arg("--target-dir")
        .arg(&target)
        // Flags meant for the host build do not apply to this one. Its own
        // make the runtime's function table one a program can add its
        // functions to, which is what lets the runtime call them, and
        // share its stack, which is where a function keeps its locals.
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env(
            "RUSTFLAGS",
            "-C link-arg=--export-table -C link-arg=--growable-table \
             -C link-arg=--export=__stack_pointer",
        )
        .status();
    assert!(
        status.is_ok_and(|status| status.success()),
        "building the runtime for wasm32-unknown-unknown failed"
    );

    let built = target.join("wasm32-unknown-unknown/release/ambient_runtime.wasm");
    if let Err(error) = std::fs::copy(&built, out.join("ambient_runtime.wasm")) {
        panic!("copying {}: {error}", built.display());
    }
}
