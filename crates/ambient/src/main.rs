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

/// The program this file carries, if it is one `ambient build` made.
fn carried(launcher: Option<&PathBuf>) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(launcher?).ok()?;
    ambient::cli::carried(&mut file).ok()?
}

fn main() -> ExitCode {
    let exit = |code: i32| ExitCode::from(u8::try_from(code).unwrap_or(1));
    let launcher = std::env::current_exe().ok();
    // A built program is started with everything it was given: no
    // argument is the command's.
    if let Some(program) = carried(launcher.as_ref()) {
        let args = std::env::args().skip(1).collect();
        let world: Rc<RefCell<dyn World>> = Rc::new(RefCell::new(System::new(args)));
        return exit(ambient::cli::start(&program, &world));
    }
    let cli = Cli::parse();
    let args = match &cli.command {
        Command::Run { args, .. } => args.clone(),
        _ => Vec::new(),
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
        launcher,
    };
    exit(session.execute(&cli.command))
}
