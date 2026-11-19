//! What `system.code` binds that needs the compiler: packing a
//! function to send, and compiling one that was sent. Loading what was
//! compiled needs none, and is the host's (`ambient_host::host`). See
//! "Code" in `docs/spec/targets.md`.
//!
//! None of them reaches a world. Packing and compiling work over
//! nothing but what a pack holds: a pack leaves nothing
//! out, so an engine with an empty workspace takes one in, and the
//! program's own trees are held as a pack of its entry.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::{Failure, bytes, error, invalid};
use crate::backend::native::{Natives, Running};
use crate::engine::{Backend, Engine, Parts};
use crate::error::Error;
use crate::hash::Hash;
use crate::pack::Pack;
use crate::store::MemoryStore;
use crate::symbol::Symbol;
use crate::system::PACKAGE;
use crate::value::{Function, Value};
use crate::workspace::Workspace;

/// A program under the command-line target, compiled.
pub struct Linked {
    pub parts: Parts,
    /// Its entry and everything that reaches, if the program packs.
    pub trees: Option<Pack>,
}

/// Compiles the program of `root`, which runs `entry`: as one code is
/// loaded into if its row reaches loading, and with its trees if its
/// row reaches packing. A binding the row does not reach is not
/// linked, so the externs say which.
///
/// # Errors
///
/// Fails if anything reached does not check or does not compile.
pub fn link(
    engine: &Engine,
    entry: &Symbol,
    root: &Symbol,
    backend: &dyn Backend,
) -> Result<Linked, Error> {
    let parts = engine.link(root, backend)?;
    let binds = |name: &str| {
        let named =
            |native: &crate::engine::Native| native.package == PACKAGE && native.path == name;
        parts.natives.iter().any(named)
    };
    let trees = binds("code_pack")
        .then(|| engine.pack(entry, &|_| false))
        .transpose()?;
    let parts = if binds("code_load") {
        engine.link_open(root, backend)?
    } else {
        parts
    };
    Ok(Linked { parts, trees })
}

/// Whether a compiled program reaches a handler that needs the
/// compiler: one that packs code, or compiles it.
#[must_use]
pub fn compiles(parts: &Parts) -> bool {
    let needs = |native: &crate::engine::Native| {
        native.package == PACKAGE && ["code_pack", "code_compile"].contains(&native.path.as_str())
    };
    parts.natives.iter().any(needs)
}

/// The packs this program made, by the function each is of: packing
/// one again gives what it gave.
type Packed = RefCell<HashMap<Hash, Vec<u8>>>;

/// The pack of a function of the program whose trees are `trees`.
fn pack(
    trees: Option<&Pack>,
    packed: &Packed,
    function: Option<&Value>,
) -> Result<Vec<u8>, Failure> {
    let Some(Value::Function(Function {
        definition: Some(hash),
        captures,
        ..
    })) = function
    else {
        return Err(invalid(&"what was given is not a function"));
    };
    if !captures.is_empty() {
        return Err(invalid(&"a function that captured something is not packed"));
    }
    if let Some(bytes) = packed.borrow().get(hash) {
        return Ok(bytes.clone());
    }
    let trees = trees.ok_or_else(|| invalid(&"the program was started without its trees"))?;
    let (workspace, store) = (Workspace::default(), MemoryStore::new());
    let engine = Engine::new(&workspace, &store);
    let entry = engine
        .receive(trees, &[])
        .map_err(|error| invalid(&error))?;
    let found = engine
        .pack_reached(&[entry], hash)
        .map_err(|error| invalid(&error))?;
    let found =
        found.ok_or_else(|| invalid(&"the function is not one the program was built with"))?;
    let bytes = found.encode();
    packed.borrow_mut().insert(*hash, bytes.clone());
    Ok(bytes)
}

/// Takes in a pack and compiles what it holds for the program that
/// asked: the bytes of [`Parts::encode`].
fn compile(running: &mut dyn Running, packed: &[u8]) -> Result<Vec<u8>, Failure> {
    let pack = Pack::decode(packed).map_err(|error| invalid(&error))?;
    let (workspace, store) = (Workspace::default(), MemoryStore::new());
    let engine = Engine::new(&workspace, &store);
    let entry = engine
        .receive(&pack, &[])
        .map_err(|error| invalid(&error))?;
    // The program says which backend it was compiled by.
    let name = running.backend().map_err(|error| invalid(&error))?;
    let backend = crate::backend::named(name).map_err(|error| invalid(&error))?;
    let parts = engine.link_to_load(&entry, &*backend);
    Ok(parts.map_err(|error| invalid(&error))?.encode(name))
}

/// `natives`, with what `system.code` binds that needs the compiler,
/// for a program whose trees are `trees`: see [`link`].
#[must_use]
pub fn offer(natives: Natives, trees: Option<Pack>) -> Natives {
    let packed = Rc::new(Packed::default());
    let answer =
        |done: Result<Value, Failure>| done.unwrap_or_else(|failure| error("code", &failure));
    natives
        .with(PACKAGE, "code_pack", move |args| {
            let done = pack(trees.as_ref(), &packed, args.first());
            Ok(answer(done.map(Value::Bytes)))
        })
        .reaching(PACKAGE, "code_compile", move |running, args| {
            let done = compile(running, bytes(args, 0)?);
            Ok(answer(done.map(Value::Bytes)))
        })
}
