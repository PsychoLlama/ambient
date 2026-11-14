//! The packages under `examples/`, read from disk: each still checks.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use ambient::cli::{Command, Session};
use ambient::host::World;
use ambient::project::Disk;
use ambient::store::MemoryStore;
use support::scripted::Scripted;

#[test]
fn every_example_checks() {
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut checked = 0;
    for entry in std::fs::read_dir(examples).unwrap() {
        let scripted = Rc::new(RefCell::new(Scripted::default()));
        let world: Rc<RefCell<dyn World>> = scripted.clone();
        let store = MemoryStore::new();
        let session = Session {
            files: &Disk,
            store: &store,
            world,
        };
        let path = entry.unwrap().path();
        let code = session.execute(&Command::Check { path });

        assert_eq!((code, scripted.borrow().err.as_str()), (0, ""));
        checked += 1;
    }

    assert!(checked > 0);
}
