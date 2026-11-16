//! What `ambient build` puts a program after: a file that runs the
//! program it carries, and holds nothing that compiles one. See
//! "Building" in `spec/packages.md`.

use std::process::ExitCode;

use ambient_host::host::{System, World};
use ambient_host::launch;

fn main() -> ExitCode {
    launch::own(launch::start).unwrap_or_else(|| {
        System::new(Vec::new()).warn("error: this file carries no program\n");
        ExitCode::FAILURE
    })
}
