//! Builds the runtime for WebAssembly as one relocatable object, so the
//! compiler can link into each module the parts of it the module
//! reaches. See `docs/spec/linking.md`. Also names the build, for the
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

/// What a build of the launcher is told, whatever it is building.
fn launching(cargo: &std::ffi::OsStr, verb: &str, target: &Path) -> Command {
    let machine = env::var("TARGET").unwrap_or_default();
    let mut build = Command::new(cargo);
    build
        .args([verb, "--locked", "--package", "ambient-launcher"])
        .args(["--target", &machine, "--target-dir"])
        .arg(target)
        // Flags meant for this build's own scripts do not apply.
        .env_remove("CARGO_ENCODED_RUSTFLAGS");
    // The command holds what this makes, twice over, so what says
    // where its lines came from is left out of even a build to debug.
    build.args(["--config", "profile.dev.debug=false"]);
    if env::var("PROFILE").is_ok_and(|profile| profile == "release") {
        build.args(["--release", "--config", "profile.release.lto=true"]);
        build.args(["--config", "profile.release.codegen-units=1"]);
        build.args(["--config", "profile.release.strip=true"]);
    }
    build
}

/// Builds the launcher for the machine the command is being built for
/// and as the command is: a release build of one is of the other,
/// stripped and optimized as a whole, since every executable carries
/// it.
///
/// It is built as an archive for the machine's linker to join with
/// what a program reaches, beside the libraries of the system such a
/// link needs: see "Native code" in `docs/spec/linking.md`.
fn launcher(cargo: &std::ffi::OsStr, out: &Path) {
    println!("cargo::rerun-if-changed=../ambient-launcher");
    let target = out.join("launching");
    let machine = env::var("TARGET").unwrap_or_default();
    let release = env::var("PROFILE").is_ok_and(|profile| profile == "release");
    let profile = if release { "release" } else { "debug" };
    let made = target.join(machine).join(profile);
    // The archive is the library made as nothing else: a release
    // build is then optimized as a whole, and holds what the entry
    // points reach in place of every crate's code and bitcode. The
    // compiler says which libraries an archive is linked with, and
    // only when asked as it makes one.
    let mut asking = launching(cargo, "rustc", &target);
    asking.args(["--lib", "--crate-type", "staticlib"]);
    asking.args(["--", "--print", "native-static-libs"]);
    let asked = asking.output();
    let built = asked.as_ref().is_ok_and(|ran| ran.status.success());
    let said = asked.map(|ran| ran.stderr).unwrap_or_default();
    let said = String::from_utf8_lossy(&said);
    assert!(built, "building the launcher failed:\n{said}");
    packed(
        &made.join("libambient_launcher.a"),
        &out.join("launcher.a.gz"),
    );
    let named = |line: &str| {
        let (_, libraries) = line.split_once("native-static-libs: ")?;
        Some(libraries.trim().to_owned())
    };
    let libraries = said.lines().find_map(named);
    assert!(
        libraries.is_some(),
        "the compiler did not say what the launcher links with:\n{said}"
    );
    let written = fs::write(
        out.join("launcher.libraries"),
        libraries.unwrap_or_default(),
    );
    assert!(written.is_ok(), "the launcher's libraries were not written");
}

/// Writes the archive at `from` to `to`, compressed: the command
/// holds it, and it is several times the command's own size.
fn packed(from: &Path, to: &Path) {
    use std::io::Write;

    let archive = fs::read(from);
    assert!(
        archive.is_ok(),
        "the launcher's archive was not where it was built"
    );
    let mut packed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    let written = packed.write_all(&archive.unwrap_or_default());
    let written = written.and_then(|()| packed.finish());
    let written = written.and_then(|packed| fs::write(to, packed));
    assert!(written.is_ok(), "the launcher's archive was not written");
}

fn main() {
    identify();
    println!("cargo::rerun-if-changed=../ambient-runtime/src");
    println!("cargo::rerun-if-changed=../ambient-runtime/Cargo.toml");
    println!("cargo::rerun-if-changed=../../Cargo.lock");

    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap_or_default());
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    launcher(&cargo, &out);
    // A target directory of its own: the one this build is running in is
    // locked by the cargo that is running this script.
    let target = out.join("runtime");
    // An archive of objects, the standard library's among them. What a
    // module does not reach is never linked into it, so the runtime is
    // built to be small where it is reached, not to be fast to build.
    run(
        Command::new(&cargo)
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
