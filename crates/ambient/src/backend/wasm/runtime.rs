//! What a module has of the runtime: a program its own pieces, linked
//! in, and a unit its program's, borrowed. See `spec/linking.md`.

use std::collections::{BTreeSet, HashMap};

use ambient_runtime::Entry;
use wasm_encoder::{
    CodeSection, ConstExpr, DataSection, ElementSection, Elements, EntityType, ExportKind,
    ExportSection, FunctionSection, GlobalSection, GlobalType, ImportSection, MemorySection,
    MemoryType, Module, RefType, TableSection, TableType, TypeSection, ValType,
};

use super::RUNTIME_MODULE;
use super::link::{Linked, Space, link};
use super::module::FIRST_TYPE;

/// The bytes in a page of memory.
const PAGE: u32 = 1 << 16;

/// The runtime's functions that are not entry points and that a module
/// calls to lay itself out, or to give itself back: the type of each,
/// counted from the first of [`OTHER_TYPES`].
const OTHERS: [(&str, u32); 8] = [
    ("amb_words", 0),
    ("amb_string_pinned", 1),
    ("amb_intern", 1),
    ("amb_tag", 1),
    ("amb_string_new", 1),
    ("amb_words_free", 2),
    ("amb_unit_retain", 3),
    ("amb_unit_release", 3),
];

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
    "amb_intern",
    "amb_tag",
    "amb_sequence_new",
    "amb_sequence_set",
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
];

/// What a host that loads units calls besides.
const LOADING: &[&str] = &["amb_unit_retain", "amb_unit_release", "amb_unit_take"];

/// What a module has of the runtime.
pub(super) enum Runtime {
    /// A program's: the pieces it reaches.
    Own(Linked),
    /// A unit's: the index of each function it imports of its program.
    Borrowed(HashMap<&'static str, u32>),
}

pub(super) fn global(val_type: ValType, mutable: bool) -> EntityType {
    EntityType::Global(GlobalType {
        val_type,
        mutable,
        shared: false,
    })
}

/// What a unit imports of the program it is loaded into: its memory,
/// its table, its stack, and every function of the runtime a unit may
/// call. Returns the index each function has.
fn borrowed(types: &mut TypeSection, imports: &mut ImportSection) -> HashMap<&'static str, u32> {
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
    let mut indices = HashMap::new();
    let mut next_type = FIRST_TYPE;
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
        indices.insert(entry.name(), next_type - FIRST_TYPE);
        next_type += 1;
    }
    let bytes = [ValType::I32, ValType::I64];
    types.ty().function([ValType::I64], [ValType::I32]);
    types.ty().function(bytes, [ValType::I64]);
    types.ty().function(bytes, []);
    types.ty().function([ValType::I64], []);
    for (name, ty) in OTHERS {
        imports.import(RUNTIME_MODULE, name, EntityType::Function(next_type + ty));
        indices.insert(name, u32::try_from(indices.len()).unwrap_or(0));
    }
    indices
}

/// Every function a unit may borrow, and a host that loads units calls.
fn whole() -> impl Iterator<Item = &'static str> {
    let entries = Entry::ALL.iter().map(|entry| entry.name());
    let others = OTHERS.iter().map(|(name, _)| *name);
    entries.chain(others).chain(LOADING.iter().copied())
}

impl Runtime {
    /// A unit's: its imports.
    pub(super) fn borrowed(types: &mut TypeSection, imports: &mut ImportSection) -> Self {
        Self::Borrowed(borrowed(types, imports))
    }

    /// A program's: what its host and its own functions call, by name,
    /// with everything those reach; and all of it if code is loaded
    /// into the program. Adds the types and imports the pieces need.
    pub(super) fn own(
        mut roots: BTreeSet<&'static str>,
        open: bool,
        types: &mut TypeSection,
        imports: &mut ImportSection,
    ) -> Result<Self, String> {
        if open {
            roots.extend(whole());
        }
        let roots = roots.iter().copied().collect();
        let linked = link(&roots, &Space { types: FIRST_TYPE })?;
        for (params, results) in &linked.types {
            let (params, results) = (params.iter().copied(), results.iter().copied());
            types.ty().function(params, results);
        }
        for (module, field, ty) in &linked.imports {
            imports.import(module, field, EntityType::Function(*ty));
        }
        Ok(Self::Own(linked))
    }

    /// The pieces of the runtime the module carries: the name and the
    /// bytes of each.
    pub(super) fn pieces(&self) -> &[(String, usize)] {
        match self {
            Self::Own(linked) => &linked.pieces,
            Self::Borrowed(_) => &[],
        }
    }

    /// The index of one of the runtime's functions.
    pub(super) fn function(&self, name: &str) -> Result<u32, String> {
        match self {
            Self::Own(linked) => linked.function(name),
            Self::Borrowed(indices) => {
                let index = indices.get(name).copied();
                index.ok_or_else(|| format!("a unit borrows no `{name}`"))
            }
        }
    }

    /// How many functions the module has before its own.
    pub(super) fn count(&self) -> usize {
        match self {
            Self::Own(linked) => linked.count(),
            Self::Borrowed(indices) => indices.len(),
        }
    }

    /// Defines the runtime's functions: the module's first.
    pub(super) fn define(&self, functions: &mut FunctionSection, code: &mut CodeSection) {
        let Self::Own(linked) = self else {
            return;
        };
        for (ty, body) in &linked.functions {
            functions.function(*ty);
            code.raw(body);
        }
    }

    /// Gives a program its table and its memory.
    pub(super) fn room(&self, module: &mut Module) {
        let Self::Own(linked) = self else {
            return;
        };
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

    /// Fills the table and memory with what the runtime starts with.
    pub(super) fn fill(&self, elements: &mut ElementSection, data: &mut DataSection) {
        let Self::Own(linked) = self else {
            return;
        };
        if !linked.slots.is_empty() {
            let slots = Elements::Functions(linked.slots.as_slice().into());
            elements.active(None, &ConstExpr::i32_const(1), slots);
        }
        if !linked.data.is_empty() {
            let start = ConstExpr::i32_const(linked.start.cast_signed());
            data.active(0, &start, linked.data.iter().copied());
        }
    }

    /// Defines a program's stack pointer, its first global, and exports
    /// its memory and the functions `host` names; and, if code is
    /// loaded into it, its table, its stack, and the whole runtime.
    pub(super) fn offer(
        &self,
        host: &[&'static str],
        open: bool,
        globals: &mut GlobalSection,
        exports: &mut ExportSection,
    ) -> Result<(), String> {
        let Self::Own(linked) = self else {
            return Ok(());
        };
        // The stack starts at the top of the room left for it.
        let ty = GlobalType {
            val_type: ValType::I32,
            mutable: true,
            shared: false,
        };
        globals.global(ty, &ConstExpr::i32_const(linked.stack.cast_signed()));
        exports.export("memory", ExportKind::Memory, 0);
        let mut named: BTreeSet<&str> = host.iter().copied().collect();
        if open {
            exports.export("__indirect_function_table", ExportKind::Table, 0);
            exports.export("__stack_pointer", ExportKind::Global, 0);
            named.extend(whole());
        }
        for name in named {
            exports.export(name, ExportKind::Func, linked.function(name)?);
        }
        Ok(())
    }
}
