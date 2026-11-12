//! Putting compiled functions together as a module, with the function
//! that lays out its statics.

use ambient_runtime::Entry;
use wasm_encoder::{
    CodeSection, ConstExpr, DataCountSection, DataSection, ElementSection, Elements, EntityType,
    ExportKind, ExportSection, FunctionSection, GlobalSection, GlobalType, ImportSection,
    MemoryType, Module, RefType, TableType, TypeSection, ValType,
};

use super::layout::{Globals, initializer, reclaimer};
use super::{FIRST, RUNTIME_MODULE, function_type};
use crate::backend::MAX_ARITY;
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

/// The types and imports every module has, and the first type after
/// them.
fn signatures() -> Result<(TypeSection, ImportSection, u32), String> {
    let index = |value: usize| u32::try_from(value).map_err(|error| error.to_string());
    let mut types = TypeSection::new();
    for arity in 0..=MAX_ARITY {
        let mut params = vec![ValType::I32, ValType::I64];
        params.extend(vec![ValType::I64; arity]);
        types.ty().function(params, [ValType::I64]);
    }
    let mut next_type = index(MAX_ARITY + 1)?;
    let mut imports = ImportSection::new();
    imports.import(
        RUNTIME_MODULE,
        "memory",
        EntityType::Memory(MemoryType {
            minimum: 0,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        }),
    );
    // Compiled functions go in the runtime's table, so that the runtime
    // can call them and they can call each other the same way.
    imports.import(
        RUNTIME_MODULE,
        "__indirect_function_table",
        EntityType::Table(TableType {
            element_type: RefType::FUNCREF,
            minimum: 0,
            maximum: None,
            table64: false,
            shared: false,
        }),
    );
    // The runtime's stack, where a function keeps its locals.
    imports.import(
        RUNTIME_MODULE,
        "__stack_pointer",
        global(ValType::I32, true),
    );
    for entry in Entry::ALL {
        let mut params = Vec::new();
        if entry.takes_context() {
            params.push(ValType::I32);
        }
        params.extend(vec![ValType::I64; entry.arity()]);
        let results: &[ValType] = if entry.returns() {
            &[ValType::I64]
        } else {
            &[]
        };
        types.ty().function(params, results.iter().copied());
        imports.import(
            RUNTIME_MODULE,
            entry.name(),
            EntityType::Function(next_type),
        );
        next_type += 1;
    }
    // In the order of the indices `super` gives them.
    let bytes = [ValType::I32, ValType::I64];
    types.ty().function([ValType::I64], [ValType::I32]);
    types.ty().function(bytes, [ValType::I64]);
    types.ty().function(bytes, []);
    types.ty().function([ValType::I64], []);
    let others = [
        ("amb_words", 0),
        ("amb_string_pinned", 1),
        ("amb_intern", 1),
        ("amb_tag", 1),
        ("amb_string_new", 1),
        ("amb_words_free", 2),
        ("amb_unit_retain", 3),
        ("amb_unit_release", 3),
    ];
    for (name, ty) in others {
        imports.import(RUNTIME_MODULE, name, EntityType::Function(next_type + ty));
    }
    Ok((types, imports, next_type + 4))
}

fn global(val_type: ValType, mutable: bool) -> EntityType {
    EntityType::Global(GlobalType {
        val_type,
        mutable,
        shared: false,
    })
}

/// Whether a part is laid out: a function or a type, which other code
/// refers to by address.
fn placed(part: &Part) -> bool {
    !matches!(part.body, Body::Inert)
}

/// Puts the parts of a program compiled by [`super::Wasm`] together as
/// a module.
///
/// The module exports `init`, which lays out the program's definitions and
/// returns the entry's closure, and `entry`, which takes a context, that
/// closure, and the entry's arguments. For each extern it reaches it
/// exports a global `native:<package>\t<tag>`, its position among them.
///
/// # Errors
///
/// Fails if the program is too large to index.
pub fn module(parts: &Parts) -> Result<Vec<u8>, String> {
    assemble(parts, Shape::Program { open: false })
}

/// A program as [`module`] makes it, that code can be loaded into.
///
/// It also exports where it laid everything out, for its host to hand
/// to a [`unit`]: `base`, the address of its block, and for each
/// definition `root:<hash>` and `definition:<hash>`, offsets from that,
/// and `metered:<hash>` if the definition has a metered form.
///
/// # Errors
///
/// Fails if the program is too large to index.
pub fn open_module(parts: &Parts) -> Result<Vec<u8>, String> {
    assemble(parts, Shape::Program { open: true })
}

/// Puts parts made by [`crate::engine::Engine::link_to_load`] together
/// as a module to instantiate over a running program's memory and
/// table.
///
/// Besides what a program imports, it imports what [`HOST_MODULE`] and
/// [`KNOWN_MODULE`] describe. It exports what [`open_module`] does, but
/// for `entry`, and `drop`, which gives back everything `init` took and
/// is called once the runtime says nothing refers to the unit.
///
/// # Errors
///
/// Fails if the parts are too large to index.
pub fn unit(parts: &Parts) -> Result<Vec<u8>, String> {
    assemble(parts, Shape::Unit)
}

fn assemble(parts: &Parts, shape: Shape) -> Result<Vec<u8>, String> {
    let index = |value: usize| u32::try_from(value).map_err(|error| error.to_string());
    let loaded = shape == Shape::Unit;
    let open = shape != Shape::Program { open: false };

    let (mut types, mut imports, next_type) = signatures()?;
    types.ty().function([], [ValType::I64]);
    types.ty().function([], []);
    let (init_type, drop_type) = (next_type, next_type + 1);

    // The stack pointer is every module's first global.
    let (mut globals, mut imported) = (Globals::default(), 1);
    if loaded {
        (globals, imported) = told(parts, &mut imports);
    }

    // Functions are numbered, and tabled, in the order of their parts.
    let mut functions = FunctionSection::new();
    let mut code = CodeSection::new();
    let mut table = Vec::new();
    let codes = parts.codes();
    let mut positions = vec![0u64; codes.len()];
    for (position, unit) in codes.iter().enumerate() {
        let Some((arity, body)) = unit else {
            continue;
        };
        positions[position] = table.len() as u64;
        functions.function(function_type(*arity));
        code.raw(&body.0);
        table.push(FIRST + index(table.len())?);
    }
    // A part's plain code comes before its metered code.
    let entry = usize::try_from(positions[parts.entry * 2]).unwrap_or(0);
    let entry = FIRST + index(entry)?;
    let init = FIRST + index(table.len())?;
    let size = index(table.len())?;

    let plan = statics::plan(
        parts,
        &Base {
            loaded,
            ..Base::none()
        },
    );
    let mut exports = ExportSection::new();
    let mut defined = GlobalSection::new();
    let mut next_global = imported;
    if open {
        // Where `init` put the block, and the first of its functions.
        for name in ["base", "table"] {
            let ty = GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            };
            defined.global(ty, &ConstExpr::i32_const(0));
            exports.export(name, ExportKind::Global, next_global);
            next_global += 1;
        }
        (globals.base, globals.table) = (Some(imported), Some(imported + 1));
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

    functions.function(init_type);
    code.function(&initializer(&plan, &positions, size, &globals)?);
    exports.export("init", ExportKind::Func, init);
    if loaded {
        functions.function(drop_type);
        code.function(&reclaimer(&plan, size, &globals)?);
        exports.export("drop", ExportKind::Func, init + 1);
    } else {
        exports.export("entry", ExportKind::Func, entry);
    }
    let mut data = DataSection::new();
    data.passive(plan.text);

    // `init` copies these into the table, after whatever it holds.
    let mut elements = ElementSection::new();
    elements.passive(Elements::Functions(table.into()));

    let mut module = Module::new();
    module
        .section(&types)
        .section(&imports)
        .section(&functions)
        .section(&defined)
        .section(&exports)
        .section(&elements)
        .section(&DataCountSection { count: 1 })
        .section(&code)
        .section(&data);
    Ok(module.finish())
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
