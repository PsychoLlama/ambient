//! Putting compiled functions together as a module, with the function
//! that lays out its statics.

use std::collections::BTreeSet;

use wasm_encoder::{
    CodeSection, ConstExpr, DataCountSection, DataSection, ElementSection, Elements, ExportKind,
    ExportSection, FunctionSection, GlobalSection, ImportSection, Module, TypeSection, ValType,
};

use super::code::{self, Calls};
use super::function_type;
use super::layout::{Globals, initializer, reclaimer};
use super::link::{Carried, Space};
pub use super::runtime::HOST;
use super::runtime::{Runtime, Shape, global, global_type, share};
use crate::backend::statics::{self, Base, Statics};
use crate::engine::{Body, Part, Parts};
use crate::hash::Hash;

/// The module name a unit imports what its host keeps under: `unused`,
/// the address of the program's list of units to reclaim, and
/// `natives`, how many externs the program has already.
pub const HOST_MODULE: &str = "host";

/// The module name a unit imports the definitions the program has
/// under: `root:<hash>` and `definition:<hash>`, each an address, or
/// zero for one the program lacks, which the unit then lays out.
pub const KNOWN_MODULE: &str = "known";

/// The types every module starts with: one for each number of
/// arguments a compiled function takes, up to `widest`, then `init`'s
/// and `drop`'s. Returns the index of `init`'s; `drop`'s is the next,
/// and the first free one follows.
fn types(widest: usize) -> Result<(TypeSection, u32), String> {
    let mut types = TypeSection::new();
    for arity in 0..=widest {
        let mut params = vec![ValType::I32, ValType::I64];
        params.extend(vec![ValType::I64; arity]);
        types.ty().function(params, [ValType::I64]);
    }
    types.ty().function([], [ValType::I64]);
    types.ty().function([], []);
    let init = u32::try_from(widest + 1).map_err(|error| error.to_string())?;
    Ok((types, init))
}

/// Whether a part is laid out: a function or a type, which other code
/// refers to by address.
fn placed(part: &Part) -> bool {
    !matches!(part.body, Body::Inert)
}

/// Puts the parts of a program compiled by [`super::Wasm`] together as
/// a module, for the host in this crate.
///
/// The module is the whole program: its own functions, and the pieces
/// of the runtime they and the host reach. It exports its `memory`,
/// those of the runtime's functions its host calls, `init`, which lays
/// out the program's definitions and returns the entry's closure, and
/// `entry`, which takes a context, that closure, and the entry's
/// arguments. For each extern it reaches it exports a global
/// `native:<package>\t<tag>`, and then the package and tag of each
/// ability of its row: its position among them.
///
/// # Errors
///
/// Fails if the program is too large to index.
pub fn module(parts: &Parts) -> Result<Vec<u8>, String> {
    module_for(parts, HOST)
}

/// A program as [`module`] makes it, for a host that calls only the
/// functions of the runtime named `host`.
///
/// # Errors
///
/// Fails if the program is too large to index, or the runtime has no
/// function of one of those names.
pub fn module_for(parts: &Parts, host: &[&'static str]) -> Result<Vec<u8>, String> {
    assemble(parts, Shape::Program { open: false }, host)
}

/// A program as [`module`] makes it, that code can be loaded into.
///
/// It carries no more of the runtime than [`module`] does, but for
/// what loading calls. It exports its table and its stack, and what it
/// has, for its host to hand to a [`unit`]: each function of the
/// runtime as `piece:<hash>` and the address of each state as
/// `state:<hash>`; `base`, the address of its block; and for each
/// definition `root:<hash>` and `definition:<hash>`, offsets from
/// that.
///
/// # Errors
///
/// Fails if the program is too large to index, or was not linked with
/// [`crate::engine::Engine::link_open`].
pub fn open_module(parts: &Parts) -> Result<Vec<u8>, String> {
    // What is loaded may suspend in the middle of a call the program
    // made, and only an open program is compiled for that.
    if !parts.open {
        return Err(crate::backend::CLOSED.to_owned());
    }
    assemble(parts, Shape::Program { open: true }, HOST)
}

/// Puts parts made by [`crate::engine::Engine::link_to_load`] together
/// as a module to instantiate over a running program's memory and
/// table.
///
/// `has` is the pieces of the runtime its target has, by hash: what
/// the program and the units loaded into it export as `piece:<hash>`.
/// The unit imports those, and carries the rest of what it reaches;
/// told of nothing, it loads into any program. See "Loading" in
/// `docs/spec/linking.md`.
///
/// It imports the program's memory, table, and stack, and what
/// [`HOST_MODULE`] and [`KNOWN_MODULE`] describe. It exports what
/// [`open_module`] does of itself, but for `entry`, and `drop`, which
/// gives back what `init` took for the unit's definitions and is called
/// once the runtime says nothing refers to the unit.
///
/// # Errors
///
/// Fails if the parts are too large to index.
pub fn unit(parts: &Parts, has: &BTreeSet<Hash>) -> Result<Vec<u8>, String> {
    assemble(parts, Shape::Unit(has), &[])
}

/// A function a module defines besides the runtime's: its body and
/// where that calls the runtime, with its type.
type Defined = (Vec<u8>, Calls, u32);

/// The functions a module defines besides the runtime's.
struct Own {
    bodies: Vec<Defined>,
    /// Where each of [`Parts::codes`] is among them.
    positions: Vec<u64>,
    /// How many of them are compiled functions: the rest lay the
    /// module out and give it back.
    compiled: u32,
    /// The most arguments any of them takes or calls a function
    /// value with.
    widest: usize,
}

/// Reads the compiled functions of `parts`, in the order of the parts.
fn compiled(parts: &Parts) -> Result<Own, String> {
    let codes = parts.codes();
    let mut own = Own {
        bodies: Vec::new(),
        positions: vec![0u64; codes.len()],
        compiled: 0,
        widest: 0,
    };
    for (position, unit) in codes.iter().enumerate() {
        let Some((arity, artifact)) = unit else {
            continue;
        };
        own.positions[position] = own.bodies.len() as u64;
        let (body, calls, widest) = code::read(&artifact.0)?;
        own.widest = own.widest.max(widest).max(*arity);
        own.bodies
            .push((body.to_vec(), calls, function_type(*arity)));
    }
    own.compiled = u32::try_from(own.bodies.len()).map_err(|error| error.to_string())?;
    Ok(own)
}

impl Own {
    /// Adds the functions that lay the module out, and gives the
    /// module's first data.
    fn lay(&mut self, plan: &Statics, globals: &Globals, ty: u32) -> Result<Vec<u8>, String> {
        let (bodies, held) = layout(plan, self, globals, ty)?;
        self.bodies.extend(bodies);
        Ok(held)
    }
}

fn assemble(parts: &Parts, shape: Shape, host: &[&'static str]) -> Result<Vec<u8>, String> {
    Ok(assembled(parts, shape, host)?.0)
}

/// The pieces of the runtime a program carries for a host that calls
/// `host`. See "Pieces" in `docs/spec/linking.md`.
///
/// # Errors
///
/// Fails as [`module_for`] does.
pub fn pieces(parts: &Parts, host: &[&'static str]) -> Result<Vec<Carried>, String> {
    Ok(assembled(parts, Shape::Program { open: false }, host)?.1)
}

/// A module, and the pieces of the runtime it carries.
type Assembled = (Vec<u8>, Vec<Carried>);

/// The functions that lay a module out, and give a unit back: each
/// a body and where it calls the runtime, with its type. With them,
/// the module's first data: the text of the plan, and after it
/// whatever the first of those functions reads.
fn layout(
    plan: &Statics,
    own: &Own,
    globals: &Globals,
    init_type: u32,
) -> Result<(Vec<Defined>, Vec<u8>), String> {
    let init = initializer(plan, &own.positions, own.compiled, globals)?;
    let mut bodies = vec![(init.body.into_raw_body(), init.calls, init_type)];
    if plan.loaded {
        let (drop, calls) = reclaimer(plan, own.compiled, globals)?;
        bodies.push((drop.into_raw_body(), calls, init_type + 1));
    }
    Ok((bodies, [plan.text.as_slice(), &init.stream].concat()))
}

fn assembled(parts: &Parts, shape: Shape, host: &[&'static str]) -> Result<Assembled, String> {
    let (loaded, open) = (shape.loaded(), shape.open());

    let mut own = compiled(parts)?;
    let (mut types, init_type) = types(own.widest)?;
    let mut imports = ImportSection::new();
    let (mut globals, mut imported) = (Globals::default(), 0);
    if loaded {
        share(&mut imports);
        (globals, imported) = told(parts, &mut imports);
    }

    let base = Base {
        loaded,
        ..Base::none()
    };
    let plan = statics::plan(parts, &base);
    // What laying the module out calls does not depend on where its
    // globals are, and where they are depends on what is linked: so
    // it is written once to see what it calls, and again once that is
    // linked.
    let draft = Globals {
        base: Some(0),
        table: Some(0),
        ..globals.clone()
    };
    let (draft, _) = layout(&plan, &own, &draft, init_type)?;
    let called = own
        .bodies
        .iter()
        .chain(&draft)
        .flat_map(|(_, calls, _)| calls);
    let roots = shape
        .roots(host)
        .into_iter()
        .chain(called.map(|(_, name)| *name));
    let space = Space {
        types: init_type + 2,
        globals: imported,
        thunk: init_type + 1,
    };
    let runtime = Runtime::link(roots.collect(), shape, &space, &mut types, &mut imports)?;
    let mut next_global = runtime.globals();
    if open {
        // Where `init` puts the block, and the first of its functions.
        (globals.base, globals.table) = (Some(next_global), Some(next_global + 1));
    }
    globals.setup = runtime.setup();
    let held = own.lay(&plan, &globals, init_type)?;

    let mut functions = FunctionSection::new();
    let mut code = CodeSection::new();
    runtime.define(&mut functions, &mut code);
    let called = |name: &str| runtime.function(name);
    for (body, calls, ty) in &own.bodies {
        functions.function(*ty);
        code.raw(&code::numbered(body, calls, &called)?);
    }
    let first = u32::try_from(runtime.count()).map_err(|error| error.to_string())?;
    let table: Vec<u32> = (first..first + own.compiled).collect();
    let init = first + own.compiled;

    let mut exports = ExportSection::new();
    let mut defined = GlobalSection::new();
    runtime.offer(host, open, &mut defined, &mut exports)?;
    if open {
        for name in ["base", "table"] {
            defined.global(global_type(ValType::I32, true), &ConstExpr::i32_const(0));
            exports.export(name, ExportKind::Global, next_global);
            next_global += 1;
        }
        describe(parts, &plan, &mut defined, &mut exports, &mut next_global)?;
        runtime.tell(&mut defined, &mut exports, &mut next_global);
    }
    for (position, native) in parts.natives.iter().enumerate() {
        // Each ability of its row follows, as a package and a path.
        let mut name = format!("native:{}\t{}", native.package, native.path);
        for (package, path) in &native.row {
            name = format!("{name}\t{package}\t{path}");
        }
        constant(
            &mut defined,
            &mut exports,
            &mut next_global,
            &name,
            position,
        )?;
    }
    exports.export("init", ExportKind::Func, init);
    if loaded {
        exports.export("drop", ExportKind::Func, init + 1);
    } else {
        let entry = u32::try_from(own.positions[parts.entry]).unwrap_or(0);
        exports.export("entry", ExportKind::Func, first + entry);
    }

    // `init` copies the first of these into the table, after whatever
    // it holds.
    let mut elements = ElementSection::new();
    elements.passive(Elements::Functions(table.into()));
    let mut data = DataSection::new();
    data.passive(held);
    runtime.fill(&mut elements, &mut data);

    let mut module = Module::new();
    module.section(&types).section(&imports).section(&functions);
    runtime.room(&mut module);
    module
        .section(&defined)
        .section(&exports)
        .section(&elements)
        .section(&DataCountSection { count: data.len() })
        .section(&code)
        .section(&data);
    Ok((module.finish(), runtime.pieces().to_vec()))
}

/// Imports what a unit is told: what its host keeps, and where the
/// program has each part the unit would lay out. Returns the globals
/// those are, and how many globals the module now imports.
fn told(parts: &Parts, imports: &mut ImportSection) -> (Globals, u32) {
    for name in ["unused", "natives"] {
        imports.import(HOST_MODULE, name, global(ValType::I64, false));
    }
    let mut globals = Globals {
        unused: 1,
        natives: 2,
        ..Globals::default()
    };
    let mut imported = 3;
    for part in &parts.parts {
        globals.known.push(placed(part).then_some(imported));
        if placed(part) {
            for what in ["root", "definition"] {
                let name = format!("{what}:{}", part.hash);
                imports.import(KNOWN_MODULE, &name, global(ValType::I64, false));
            }
            imported += 2;
        }
    }
    (globals, imported)
}

/// Exports a global that holds `value`, under `name`.
fn constant(
    defined: &mut GlobalSection,
    exports: &mut ExportSection,
    next: &mut u32,
    name: &str,
    value: usize,
) -> Result<(), String> {
    let value = i64::try_from(value).map_err(|error| error.to_string())?;
    let ty = global_type(ValType::I64, false);
    defined.global(ty, &ConstExpr::i64_const(value));
    exports.export(name, ExportKind::Global, *next);
    *next += 1;
    Ok(())
}

/// Exports where the block has each part, as offsets from its base.
fn describe(
    parts: &Parts,
    plan: &Statics,
    defined: &mut GlobalSection,
    exports: &mut ExportSection,
    next: &mut u32,
) -> Result<(), String> {
    for (part, placed) in parts.parts.iter().zip(&plan.placed) {
        let Some((root, definition)) = *placed else {
            continue;
        };
        let hash = part.hash;
        constant(defined, exports, next, &format!("root:{hash}"), root * 8)?;
        let definition = match part.body {
            Body::Function { .. } => definition * 8,
            // Only a function has one.
            Body::Type(_) | Body::Constant(_) | Body::Inert => 0,
        };
        constant(
            defined,
            exports,
            next,
            &format!("definition:{hash}"),
            definition,
        )?;
    }
    Ok(())
}
