//! What `ambient build` puts a program after: a file that runs the
//! program it carries, and holds nothing that compiles one. See
//! "Building" in `docs/spec/packages.md`.

use std::process::ExitCode;

fn main() -> ExitCode {
    ambient_launcher::launched(
        std::env::args_os().skip(1),
        ambient_host::native::Natives::new,
    )
    .map_or(ExitCode::FAILURE, |code| {
        ExitCode::from(u8::try_from(code).unwrap_or(1))
    })
}
