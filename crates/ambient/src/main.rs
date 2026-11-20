//! The `ambient` command.

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use ambient::cli::{Cli, Command, Linker, Machine, Session, Unlinked};
use ambient::host::{System, World};
use ambient::project::{Disk, Kept, Nowhere, Offline, Snapshots};
use ambient::store::{DiskStore, MemoryStore, Store};
use clap::Parser;

/// Where what is built and what is fetched are kept between runs: one
/// place for every project, since each is kept by its content.
fn cache() -> Option<PathBuf> {
    let home = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let base = home("XDG_CACHE_HOME").or_else(|| home("HOME").map(|home| home.join(".cache")))?;
    Some(base.join("ambient"))
}

fn main() -> ExitCode {
    let exit = |code: i32| ExitCode::from(u8::try_from(code).unwrap_or(1));
    // The command is the launcher of a program that compiles: one
    // such is this file with the program after it.
    if let Some(code) = ambient_host::launch::own(ambient::cli::start) {
        return code;
    }
    let launcher = std::env::current_exe().ok();
    let cli = Cli::parse();
    let args = match &cli.command {
        Command::Run { args, .. } => args.clone(),
        _ => Vec::new(),
    };
    // Without somewhere to keep it, a build is kept for this run alone.
    let built = cache().map(|root| root.join("store"));
    let store: Box<dyn Store> = match built.and_then(|root| DiskStore::open(root).ok()) {
        Some(store) => Box::new(store),
        None => Box::new(MemoryStore::new()),
    };
    let world: Rc<RefCell<dyn World>> = Rc::new(RefCell::new(System::new(args)));
    // Without somewhere to keep it, nothing is fetched.
    let snapshots: Box<dyn Snapshots> = match cache() {
        Some(root) => Box::new(Kept::under(root.join("snapshots"))),
        None => Box::new(Nowhere),
    };
    let offline = Offline(snapshots.as_ref());
    let snapshots: &dyn Snapshots = if cli.offline {
        &offline
    } else {
        snapshots.as_ref()
    };
    // Without somewhere to keep it, nothing is linked.
    let linker: Box<dyn Linker> = match cache() {
        Some(root) => Box::new(Machine::under(root.join("linked"))),
        None => Box::new(Unlinked),
    };
    let session = Session {
        files: &Disk,
        store: store.as_ref(),
        snapshots,
        world,
        launcher,
        linker: linker.as_ref(),
    };
    exit(session.execute(&cli.command))
}
