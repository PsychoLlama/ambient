// Included by the build scripts of `ambient-host`, which names the
// platform, and of `ambient-archive`, which builds the launcher for it.

/// The platform a command built for `target` builds programs for: what
/// a native bundle says it is for, and what the launcher is built for.
///
/// It is the command's own, but for a command linked with a C library
/// of its own so as to run on any Linux: a program is linked on the
/// machine that builds it, with that machine's C library, as a program
/// in C is. See "Native code" in `docs/spec/linking.md`.
fn platform(target: &str) -> String {
    match target.strip_suffix("-linux-musl") {
        Some(machine) => format!("{machine}-linux-gnu"),
        None => target.to_owned(),
    }
}
