//! Putting compiled functions together as a module, with the function
//! that lays out its statics.

use wasm_encoder::{
    CodeSection, ConstExpr, DataCountSection, DataSection, ElementSection, Elements, ExportKind,
    ExportSection, FunctionSection, GlobalSection, GlobalType, ImportSection, Module, TypeSection,
    ValType,
};

use super::code::{self, Calls};
use super::function_type;
use super::layout::{Globals, initializer, reclaimer};
pub use super::runtime::HOST;
use super::runtime::{Runtime, global};
use crate::backend::statics::{self, Base, Statics};
use crate::engine::{Body, Part, Parts};

/// The module name a unit imports what its host keeps under: `unused`,
/// the address of the program's list of units to reclaim, and
/// `natives`, how many externs the program has already.
pub const HOST_MODULE: &str = "host";

/// The module name a unit imports the definitions the program has
/// under: `root:<hash>` and `definition:<hash>`, each an address, or
/// zero for one the program lacks, which the unit then lays out.
pub const KNOWN_MODULE: &str = "known";

/// What kind of module is being put together.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A program, and whether code can be loaded into it.
    Program { open: bool },
    /// Code to load into a program.
    Unit,
}

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
/// `native:<package>\t<tag>`, its position among them.
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
/// It has the whole runtime, since what is loaded may call any of it,
/// and exports all of it with its table and its stack. It also exports
/// where it laid everything out, for its host to hand to a [`unit`]:
/// `base`, the address of its block, and for each definition
/// `root:<hash>` and `definition:<hash>`, offsets from that, and
/// `metered:<hash>` if the definition has a metered form.
///
/// # Errors
///
/// Fails if the program is too large to index.
pub fn open_module(parts: &Parts) -> Result<Vec<u8>, String> {
    assemble(parts, Shape::Program { open: true }, HOST)
}

/// Puts parts made by [`crate::engine::Engine::link_to_load`] together
/// as a module to instantiate over a running program's memory and
/// table.
///
/// It imports the program's memory, table, stack, and runtime, and
/// what [`HOST_MODULE`] and [`KNOWN_MODULE`] describe. It exports what
/// [`open_module`] does of itself, but for `entry`, and `drop`, which
/// gives back everything `init` took and is called once the runtime
/// says nothing refers to the unit.
///
/// # Errors
///
/// Fails if the parts are too large to index.
pub fn unit(parts: &Parts) -> Result<Vec<u8>, String> {
    assemble(parts, Shape::Unit, &[])
}

/// The functions a module defines besides the runtime's: each a body
/// and where it calls the runtime, with its type.
struct Own {
    bodies: Vec<(Vec<u8>, Calls, u32)>,
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

fn assemble(parts: &Parts, shape: Shape, host: &[&'static str]) -> Result<Vec<u8>, String> {
    Ok(assembled(parts, shape, host)?.0)
}

/// The pieces of the runtime a program carries for a host that calls
/// `host`: what the runtime's source calls each, and its bytes. See
/// "Pieces" in `spec/linking.md`.
///
/// # Errors
///
/// Fails as [`module_for`] does.
pub fn pieces(parts: &Parts, host: &[&'static str]) -> Result<Vec<(String, usize)>, String> {
    Ok(assembled(parts, Shape::Program { open: false }, host)?.1)
}

/// A module, and the pieces of the runtime it carries.
type Assembled = (Vec<u8>, Vec<(String, usize)>);

fn assembled(parts: &Parts, shape: Shape, host: &[&'static str]) -> Result<Assembled, String> {
    let loaded = shape == Shape::Unit;
    let open = shape != Shape::Program { open: false };

    let mut own = compiled(parts)?;
    let (mut types, init_type) = types(own.widest)?;
    let first_type = init_type + 2;
    let mut imports = ImportSection::new();
    // The stack pointer is every module's first global.
    let (mut globals, mut next_global) = (Globals::default(), 1);
    let mut borrowed = None;
    if loaded {
        borrowed = Some(Runtime::borrowed(&mut types, &mut imports, first_type));
        (globals, next_global) = told(parts, &mut imports);
    }
    if open {
        // Where `init` puts the block, and the first of its functions.
        (globals.base, globals.table) = (Some(next_global), Some(next_global + 1));
    }

    let base = Base {
        loaded,
        ..Base::none()
    };
    let plan = statics::plan(parts, &base);
    let (init, calls) = initializer(&plan, &own.positions, own.compiled, &globals)?;
    own.bodies.push((init.into_raw_body(), calls, init_type));
    if loaded {
        let (drop, calls) = reclaimer(&plan, own.compiled, &globals)?;
        own.bodies
            .push((drop.into_raw_body(), calls, init_type + 1));
    }

    // A unit borrows its program's runtime, and a program has its own:
    // what its host and its functions call.
    let runtime = if let Some(borrowed) = borrowed {
        borrowed
    } else {
        let called = own.bodies.iter().flat_map(|(_, calls, _)| calls);
        let roots = host.iter().copied().chain(called.map(|(_, name)| *name));
        Runtime::own(roots.collect(), open, first_type, &mut types, &mut imports)?
    };
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
    }
    for (position, native) in parts.natives.iter().enumerate() {
        let name = format!("native:{}\t{}", native.package, native.tag);
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
        // A part's plain code comes before its metered code.
        let entry = u32::try_from(own.positions[parts.entry * 2]).unwrap_or(0);
        exports.export("entry", ExportKind::Func, first + entry);
    }

    // `init` copies the first of these into the table, after whatever
    // it holds.
    let mut elements = ElementSection::new();
    elements.passive(Elements::Functions(table.into()));
    let mut data = DataSection::new();
    data.passive(plan.text);
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

fn global_type(val_type: ValType, mutable: bool) -> GlobalType {
    GlobalType {
        val_type,
        mutable,
        shared: false,
    }
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
    let ty = GlobalType {
        val_type: ValType::I64,
        mutable: false,
        shared: false,
    };
    defined.global(ty, &ConstExpr::i64_const(value));
    exports.export(name, ExportKind::Global, *next);
    *next += 1;
    Ok(())
}

/// Exports where the block has each part, as offsets from its base, and
/// which functions have a metered form.
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
            Body::Type(_) | Body::Inert => 0,
        };
        constant(
            defined,
            exports,
            next,
            &format!("definition:{hash}"),
            definition,
        )?;
        if matches!(&part.body, Body::Function { metered, .. } if metered.is_some()) {
            constant(defined, exports, next, &format!("metered:{hash}"), 1)?;
        }
    }
    Ok(())
}
