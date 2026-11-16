//! What a module has of the runtime: the pieces it reaches, linked in.
//! A program has them at places known when it is put together; a unit
//! brings the ones its program lacks and imports the rest. See
//! `spec/linking.md`.

use std::collections::BTreeSet;

use wasm_encoder::{
    CodeSection, ConstExpr, DataSection, ElementSection, Elements, EntityType, ExportKind,
    ExportSection, FunctionSection, GlobalSection, GlobalType, ImportSection, MemorySection,
    MemoryType, Module, RefType, TableSection, TableType, TypeSection, ValType,
};

use super::RUNTIME_MODULE;
use super::link::{Carried, For, Linked, Offer, STATE_MODULE, Space, link};
use crate::hash::Hash;

/// The bytes in a page of memory.
const PAGE: u32 = 1 << 16;

/// The runtime's functions the host in this crate calls: what
/// [`super::Program`] starts a call with, and makes and reads values
/// with. See "Hosts" in `spec/linking.md`.
pub const HOST: &[&str] = &[
    "amb_words",
    "amb_words_free",
    "amb_kind",
    "amb_int_value",
    "amb_float_bits",
    "amb_closure_hash",
    "amb_closure_arity",
    "amb_length",
    "amb_word_at",
    "amb_string_bytes",
    "amb_int_new",
    "amb_float_new",
    "amb_string_new",
    "amb_bytes_new",
    "amb_intern",
    "amb_tag",
    "amb_handle_new",
    "amb_handle_next",
    "amb_handle_number",
    "amb_sequence_new",
    "amb_sequence_set",
    "amb_iterator_of",
    "amb_iterator_hold",
    "amb_record_new",
    "amb_record_set",
    "amb_closure_new",
    "amb_closure_set",
    "amb_method_mark",
    "amb_release",
    "amb_fault",
    "amb_live",
    "amb_context_new",
    "amb_context_mode",
    "amb_context_fault",
    "amb_context_error",
    "amb_context_free",
    "amb_start",
    "amb_schedule",
    "amb_await",
    "amb_answer",
    "amb_refuse",
    "amb_strand",
    "amb_call_back",
    "amb_call_among",
    "amb_budget_switch",
    "amb_budget_back",
];

/// What a host calls only of a program that reaches a function: what
/// gives suspended tasks their turns, of a program that can suspend.
/// One that cannot is linked without it. A program code is loaded into
/// has it regardless, since what is loaded may suspend. Likewise what
/// answers native code that answers later, of a program that reaches
/// such native code; and what native code calls back through, of a
/// program that reaches native code at all, which gives other tasks
/// their turns only in a program that can suspend.
const OFFERED: &[(&str, &[&str])] = &[
    ("amb_schedule", &["amb_suspend"]),
    ("amb_await", &["amb_native_wait"]),
    ("amb_answer", &["amb_native_wait"]),
    ("amb_refuse", &["amb_native_wait"]),
    ("amb_strand", &["amb_native_wait"]),
    ("amb_call_back", &["amb_native"]),
    ("amb_call_among", &["amb_native", "amb_suspend"]),
    ("amb_budget_switch", &["amb_native", "amb_budget_enter"]),
    ("amb_budget_back", &["amb_native", "amb_budget_enter"]),
];

/// What a host that loads units calls besides.
const LOADING: &[&str] = &["amb_unit_retain", "amb_unit_release", "amb_unit_take"];

/// What kind of module the runtime is linked into.
#[derive(Clone, Copy)]
pub(super) enum Shape<'a> {
    /// A program, and whether code can be loaded into it.
    Program { open: bool },
    /// Code to load into a program that has the pieces of these hashes.
    Unit(&'a BTreeSet<Hash>),
}

impl Shape<'_> {
    /// What of `host`, the functions a host calls, the module is
    /// linked for whatever it reaches. A program nothing is loaded
    /// into is not linked for what it is only offered: it has those
    /// if it reaches what they need, or if its own code calls them.
    pub(super) fn roots(&self, host: &[&'static str]) -> Vec<&'static str> {
        let closed = matches!(self, Self::Program { open: false });
        let offered = |name: &str| OFFERED.iter().any(|(offered, _)| *offered == name);
        let asked = host.iter().copied();
        asked.filter(|name| !(closed && offered(name))).collect()
    }

    pub(super) fn loaded(&self) -> bool {
        matches!(self, Self::Unit(_))
    }

    /// Whether the module says where it put things, for code loaded
    /// after it.
    pub(super) fn open(&self) -> bool {
        !matches!(self, Self::Program { open: false })
    }
}

/// What a module has of the runtime.
pub(super) struct Runtime {
    linked: Linked,
    loaded: bool,
    /// The first global the module defines.
    first: u32,
}

pub(super) fn global(val_type: ValType, mutable: bool) -> EntityType {
    EntityType::Global(global_type(val_type, mutable))
}

pub(super) fn global_type(val_type: ValType, mutable: bool) -> GlobalType {
    GlobalType {
        val_type,
        mutable,
        shared: false,
    }
}

/// Imports what a unit shares with the program it is loaded into: its
/// memory, its table, and its stack, the unit's first global.
pub(super) fn share(imports: &mut ImportSection) {
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
    // Compiled functions go in the program's table, so that the runtime
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
    // The program's stack, where a function keeps its locals.
    imports.import(
        RUNTIME_MODULE,
        "__stack_pointer",
        global(ValType::I32, true),
    );
}

impl Runtime {
    /// Links what a module's host and its own functions call, by name,
    /// with everything those reach. Adds the types and imports the
    /// pieces need, after the `globals` globals the module imports.
    pub(super) fn link(
        mut roots: BTreeSet<&'static str>,
        shape: Shape,
        space: &Space,
        types: &mut TypeSection,
        imports: &mut ImportSection,
    ) -> Result<Self, String> {
        let target = match shape {
            Shape::Program { open: false } => For::Program(OFFERED),
            Shape::Program { open: true } => {
                roots.extend(LOADING);
                For::Program(&[])
            }
            Shape::Unit(has) => For::Unit(has),
        };
        let roots = roots.iter().copied().collect();
        let linked = link(&roots, &target, space)?;
        for (params, results) in &linked.types {
            let (params, results) = (params.iter().copied(), results.iter().copied());
            types.ty().function(params, results);
        }
        for state in &linked.states {
            let name = state.to_string();
            imports.import(STATE_MODULE, &name, global(ValType::I32, false));
        }
        for (module, field, ty) in &linked.imports {
            imports.import(module, field, EntityType::Function(*ty));
        }
        let states = u32::try_from(linked.states.len()).map_err(|error| error.to_string())?;
        Ok(Self {
            linked,
            loaded: shape.loaded(),
            first: space.globals + states,
        })
    }

    /// The pieces of the runtime the module carries.
    pub(super) fn pieces(&self) -> &[Carried] {
        &self.linked.pieces
    }

    /// The index of one of the runtime's functions.
    pub(super) fn function(&self, name: &str) -> Result<u32, String> {
        self.linked.function(name)
    }

    /// The function a unit's `init` calls first.
    pub(super) fn setup(&self) -> Option<u32> {
        self.linked.setup
    }

    /// How many functions the module has before its own.
    pub(super) fn count(&self) -> usize {
        self.linked.count()
    }

    /// The first global the module is free to define after the
    /// runtime's: a program's stack pointer, and what a unit reaches
    /// its pieces through.
    pub(super) fn globals(&self) -> u32 {
        self.first + if self.loaded { self.linked.globals } else { 1 }
    }

    /// Defines the runtime's functions: the module's first.
    pub(super) fn define(&self, functions: &mut FunctionSection, code: &mut CodeSection) {
        for (ty, body) in &self.linked.functions {
            functions.function(*ty);
            code.raw(body);
        }
    }

    /// Gives a program its table and its memory.
    pub(super) fn room(&self, module: &mut Module) {
        if self.loaded {
            return;
        }
        let linked = &self.linked;
        // The runtime's own functions are tabled from the second place
        // on: nothing is at the first, which stands for no function.
        let mut tables = TableSection::new();
        tables.table(TableType {
            element_type: RefType::FUNCREF,
            minimum: 1 + linked.slots.len() as u64,
            maximum: None,
            table64: false,
            shared: false,
        });
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: u64::from(linked.end.max(linked.stack).div_ceil(PAGE)),
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&tables).section(&memories);
    }

    /// What the runtime's pieces start with: in a program, in the
    /// table and memory already; in a unit, for its setup to copy to
    /// wherever it finds room.
    pub(super) fn fill(&self, elements: &mut ElementSection, data: &mut DataSection) {
        let linked = &self.linked;
        if !linked.slots.is_empty() {
            let slots = Elements::Functions(linked.slots.as_slice().into());
            if self.loaded {
                elements.passive(slots);
            } else {
                elements.active(None, &ConstExpr::i32_const(1), slots);
            }
        }
        if !linked.data.is_empty() {
            let bytes = linked.data.iter().copied();
            if self.loaded {
                data.passive(bytes);
            } else {
                let start = ConstExpr::i32_const(linked.start.cast_signed());
                data.active(0, &start, bytes);
            }
        }
    }

    /// Defines the module's first globals, a program's stack pointer or
    /// what a unit reaches its pieces through, and exports of a
    /// program its memory and the functions `host` names; and, if code
    /// is loaded into it, its table, its stack, and what loading calls.
    pub(super) fn offer(
        &self,
        host: &[&'static str],
        open: bool,
        globals: &mut GlobalSection,
        exports: &mut ExportSection,
    ) -> Result<(), String> {
        let linked = &self.linked;
        let ty = global_type(ValType::I32, true);
        if self.loaded {
            for _ in 0..linked.globals {
                globals.global(ty, &ConstExpr::i32_const(0));
            }
            return Ok(());
        }
        // The stack starts at the top of the room left for it.
        globals.global(ty, &ConstExpr::i32_const(linked.stack.cast_signed()));
        exports.export("memory", ExportKind::Memory, 0);
        let mut named: BTreeSet<&str> = host.iter().copied().collect();
        if open {
            exports.export("__indirect_function_table", ExportKind::Table, 0);
            exports.export("__stack_pointer", ExportKind::Global, 0);
            named.extend(LOADING);
        }
        for name in named {
            let offered = OFFERED.iter().any(|(offered, _)| *offered == name);
            if !offered || linked.has(name) {
                exports.export(name, ExportKind::Func, linked.function(name)?);
            }
        }
        Ok(())
    }

    /// Exports what the module carries of the runtime, for code loaded
    /// after it: each function as `piece:<hash>`, and the address of
    /// each state as `state:<hash>`.
    pub(super) fn tell(
        &self,
        globals: &mut GlobalSection,
        exports: &mut ExportSection,
        next: &mut u32,
    ) {
        // Two pieces of one hash are the same piece twice over, and
        // either stands for both.
        let mut told = BTreeSet::new();
        for (hash, offer) in &self.linked.offers {
            if !told.insert(*hash) {
                continue;
            }
            match *offer {
                Offer::Function(index) => {
                    exports.export(&format!("piece:{hash}"), ExportKind::Func, index);
                }
                Offer::Global(global) => {
                    exports.export(&format!("state:{hash}"), ExportKind::Global, global);
                }
                Offer::State(address) => {
                    let ty = global_type(ValType::I32, false);
                    globals.global(ty, &ConstExpr::i32_const(address.cast_signed()));
                    exports.export(&format!("state:{hash}"), ExportKind::Global, *next);
                    *next += 1;
                }
            }
        }
    }
}
