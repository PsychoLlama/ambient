//! Putting compiled functions together as a module, with the function
//! that lays out the program's statics.

use ambient_runtime::Entry;
use wasm_encoder::{
    CodeSection, ConstExpr, DataCountSection, DataSection, ElementSection, Elements, EntityType,
    ExportKind, ExportSection, Function, FunctionSection, ImportSection, Instruction, MemoryType,
    Module, RefType, TableSection, TableType, TypeSection, ValType,
};

use super::emit::memory;
use super::{INTERN, RUNTIME_MODULE, STRING_PINNED, WORDS, function_type};
use crate::backend::MAX_ARITY;
use crate::backend::statics::{self, Field, Make};
use crate::engine::{Body, Parts};

/// The types and imports every module has, and the type of `init`.
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
    types.ty().function([ValType::I64], [ValType::I32]);
    imports.import(RUNTIME_MODULE, "amb_words", EntityType::Function(next_type));
    types
        .ty()
        .function([ValType::I32, ValType::I64], [ValType::I64]);
    for name in ["amb_string_pinned", "amb_intern"] {
        imports.import(RUNTIME_MODULE, name, EntityType::Function(next_type + 1));
    }
    let init_type = next_type + 2;
    types.ty().function([], [ValType::I64]);
    Ok((types, imports, init_type))
}

/// Puts the parts of a program compiled by [`Wasm`] together as a module.
///
/// The module exports `init`, which lays out the program's definitions and
/// returns the entry's closure, and `entry`, which takes a context, that
/// closure, and the entry's arguments.
///
/// # Errors
///
/// Fails if the program is too large to index.
pub fn module(parts: &Parts) -> Result<Vec<u8>, String> {
    let index = |value: usize| u32::try_from(value).map_err(|error| error.to_string());

    let (types, imports, init_type) = signatures()?;

    // Functions are numbered, and tabled, in the order of their parts.
    let first = INTERN + 1;
    let mut functions = FunctionSection::new();
    let mut code = CodeSection::new();
    let mut table = Vec::new();
    let mut positions = vec![0u64; parts.parts.len()];
    for (position, part) in parts.parts.iter().enumerate() {
        let Body::Function {
            arity, code: body, ..
        } = &part.body
        else {
            continue;
        };
        positions[position] = table.len() as u64;
        functions.function(function_type(*arity));
        code.raw(&body.0);
        table.push(first + index(table.len())?);
    }
    let entry = first + index(usize::try_from(positions[parts.entry]).unwrap_or(0))?;
    let init = first + index(table.len())?;
    let size = table.len() as u64;
    functions.function(init_type);
    let (initializer, text) = initializer(parts, &positions)?;
    code.function(&initializer);
    let mut data = DataSection::new();
    data.passive(text);

    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        minimum: size,
        maximum: None,
        table64: false,
        shared: false,
    });
    let mut elements = ElementSection::new();
    elements.active(
        Some(0),
        &ConstExpr::i32_const(0),
        Elements::Functions(table.into()),
    );
    let mut exports = ExportSection::new();
    exports.export("init", ExportKind::Func, init);
    exports.export("entry", ExportKind::Func, entry);

    let mut module = Module::new();
    module
        .section(&types)
        .section(&imports)
        .section(&functions)
        .section(&tables)
        .section(&exports)
        .section(&elements)
        .section(&DataCountSection { count: 1 })
        .section(&code)
        .section(&data);
    Ok(module.finish())
}

/// The locals of `initializer`: the address of the block the program's
/// definitions are laid out in, and the address its text was copied to.
const BASE: u32 = 0;
const TEXT: u32 = 1;

/// The function that lays out a program in the runtime's memory and
/// returns the entry's closure, with the bytes it copies in first.
///
/// `positions` is each part's index in the table of functions.
fn initializer(parts: &Parts, positions: &[u64]) -> Result<(Function, Vec<u8>), String> {
    let small = |value: usize| i32::try_from(value).map_err(|error| error.to_string());
    let number = |value: usize| i64::try_from(value).map_err(|error| error.to_string());
    let plan = statics::plan(parts);

    let mut body = Function::new_with_locals_types([ValType::I32, ValType::I32]);
    body.instruction(&Instruction::I64Const(number(plan.words)?));
    body.instruction(&Instruction::Call(WORDS));
    body.instruction(&Instruction::LocalSet(BASE));
    // The text goes into memory the runtime allocated, since the runtime
    // owns the memory.
    body.instruction(&Instruction::I64Const(number(plan.text.len().div_ceil(8))?));
    body.instruction(&Instruction::Call(WORDS));
    body.instruction(&Instruction::LocalTee(TEXT));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(small(plan.text.len())?));
    body.instruction(&Instruction::MemoryInit {
        mem: 0,
        data_index: 0,
    });

    let address = |body: &mut Function, word: usize| -> Result<(), String> {
        body.instruction(&Instruction::LocalGet(BASE));
        body.instruction(&Instruction::I64ExtendI32U);
        body.instruction(&Instruction::I64Const(i64::from(small(word * 8)?)));
        body.instruction(&Instruction::I64Add);
        Ok(())
    };
    for (word, field) in &plan.fields {
        body.instruction(&Instruction::LocalGet(BASE));
        match *field {
            Field::Word(value) => {
                body.instruction(&Instruction::I64Const(value.cast_signed()));
            }
            Field::Address(target) => address(&mut body, target)?,
            // Code is an index into the table.
            Field::Code(part) => {
                body.instruction(&Instruction::I64Const(positions[part].cast_signed()));
            }
            Field::Text {
                make,
                offset,
                length,
            } => {
                body.instruction(&Instruction::LocalGet(TEXT));
                body.instruction(&Instruction::I32Const(small(offset)?));
                body.instruction(&Instruction::I32Add);
                body.instruction(&Instruction::I64Const(number(length)?));
                body.instruction(&Instruction::Call(match make {
                    Make::String => STRING_PINNED,
                    Make::Field => INTERN,
                }));
            }
        }
        body.instruction(&Instruction::I64Store(memory(small(word * 8)?)));
    }

    address(&mut body, plan.entry)?;
    body.instruction(&Instruction::End);
    Ok((body, plan.text))
}
