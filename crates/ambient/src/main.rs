//! The `ambient` command.

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use ambient::cli::{Cli, Command, Session};
use ambient::host::{System, World};
use ambient::project::Disk;
use ambient::store::{DiskStore, MemoryStore, Store};
use clap::Parser;

/// Where built definitions are kept between runs: one store for every
/// project, since a definition is kept by its content.
fn cache() -> Option<PathBuf> {
    let home = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let base = home("XDG_CACHE_HOME").or_else(|| home("HOME").map(|home| home.join(".cache")))?;
    Some(base.join("ambient").join("store"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let args = match &cli.command {
        Command::Run { args, .. } => args.clone(),
        Command::Check { .. } | Command::Test { .. } => Vec::new(),
    };
    // Without somewhere to keep it, a build is kept for this run alone.
    let store: Box<dyn Store> = match cache().and_then(|root| DiskStore::open(root).ok()) {
        Some(store) => Box::new(store),
        None => Box::new(MemoryStore::new()),
    };
    let world: Rc<RefCell<dyn World>> = Rc::new(RefCell::new(System::new(args)));
    let session = Session {
        files: &Disk,
        store: store.as_ref(),
        world,
    };
    let code = session.execute(&cli.command);
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
