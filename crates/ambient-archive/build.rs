//! Builds the launcher for the platform the command being built makes
//! programs for, as an archive, and compresses it for the command to
//! hold.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
use std::{env, fs};

include!("../ambient-host/platform.rs");

/// Everything the launcher is built from. Cargo wakes this script when
/// any of it changes, and the build it runs decides what to make again,
/// so naming too much costs a build that does nothing and never a
/// stale archive.
const SOURCES: &[&str] = &[
    "../ambient",
    "../ambient-host",
    "../ambient-launcher",
    "../ambient-native",
    "../ambient-runtime",
    "../../Cargo.lock",
    "../../Cargo.toml",
];

/// What a build of the launcher is told, whatever it is building.
fn launching(cargo: &std::ffi::OsStr, verb: &str, target: &Path, release: bool) -> Command {
    let machine = platform(&env::var("TARGET").unwrap_or_default());
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
    if release {
        build.args(["--release", "--config", "profile.release.lto=true"]);
        build.args(["--config", "profile.release.codegen-units=1"]);
        build.args(["--config", "profile.release.strip=true"]);
    }
    build
}

/// Builds the launcher as the command is: a release build of one is
/// of the other, stripped and optimized as a whole, since every
/// executable carries it.
///
/// It is built as an archive for the machine's linker to join with
/// what a program reaches, beside the libraries of the system such a
/// link needs: see "Native code" in `docs/spec/linking.md`.
fn main() {
    for source in SOURCES {
        println!("cargo::rerun-if-changed={source}");
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap_or_default());
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    // A target directory of its own: the one this build is running in
    // is locked by the cargo that is running this script.
    let target = out.join("launching");
    let machine = platform(&env::var("TARGET").unwrap_or_default());
    let release = env::var("PROFILE").is_ok_and(|profile| profile == "release");
    let profile = if release { "release" } else { "debug" };
    let made = target.join(machine).join(profile);
    // The archive is the library made as nothing else: a release
    // build is then optimized as a whole, and holds what the entry
    // points reach in place of every crate's code and bitcode. The
    // compiler says which libraries an archive is linked with, and
    // only when asked as it makes one.
    let mut asking = launching(&cargo, "rustc", &target, release);
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
        release,
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

/// When `path` was last written, and how long it is, as text.
fn stamp(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    let since = metadata
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH);
    Some(format!("{} {}", since.ok()?.as_nanos(), metadata.len()))
}

/// Writes the archive at `from` to `to`, compressed: the command
/// holds it, and it is several times the command's own size. A
/// release is compressed as far as it goes; a build to debug is
/// rebuilt on every change to the compiler, and the best compression
/// takes far longer than the rest of that. An archive the build left
/// as it was is not compressed again.
fn packed(from: &Path, to: &Path, release: bool) {
    use std::io::Write;

    let stamped = to.with_extension("stamp");
    let now = stamp(from);
    if to.exists() && now.is_some() && fs::read_to_string(&stamped).ok() == now {
        return;
    }
    let archive = fs::read(from);
    assert!(
        archive.is_ok(),
        "the launcher's archive was not where it was built"
    );
    let level = if release {
        flate2::Compression::best()
    } else {
        flate2::Compression::fast()
    };
    let mut packed = flate2::write::GzEncoder::new(Vec::new(), level);
    let written = packed.write_all(&archive.unwrap_or_default());
    let written = written.and_then(|()| packed.finish());
    let written = written.and_then(|packed| fs::write(to, packed));
    assert!(written.is_ok(), "the launcher's archive was not written");
    let _ = fs::write(&stamped, now.unwrap_or_default());
}
