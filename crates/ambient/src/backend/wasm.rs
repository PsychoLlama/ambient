//! The WebAssembly backend.
//!
//! A program is two modules: the [runtime](RUNTIME), and one emitted by
//! [`module`] that imports the runtime's memory and entry points. Compiled
//! functions never refer to each other by index, only through closures in
//! memory, so each function's body is compiled on its own and a module is
//! put together by concatenation.

use ambient_runtime::layout::{
    CLOSURE_DEFINITION, CLOSURE_WORDS, DEFINITION_CODE, DEFINITION_SLOTS, DEFINITION_WORDS,
};
use ambient_runtime::{Entry, FALSE, NULL, SMALL_MAX, SMALL_MIN, TRUE, Word};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, ElementSection, Elements, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, ImportSection, Instruction, MemArg, MemArgTarget,
    MemoryType, Module, RefType, TableSection, TableType, TypeSection, ValType,
};

use super::MAX_ARITY;
use super::native::entry_for;
use crate::core::{Open, Tree};
use crate::engine::{Backend, Parts};
use crate::lir::{self, Expr, Op};
use crate::lower::lower;

/// The runtime as a WebAssembly module. It exports its memory and every
/// entry point; instantiate it first and give its exports to the program's
/// module under the name [`RUNTIME_MODULE`].
pub const RUNTIME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ambient_runtime.wasm"));

/// The module name a program imports the runtime under.
pub const RUNTIME_MODULE: &str = "rt";

/// Compiles for WebAssembly.
pub struct Wasm;

/// The function index of an entry point: they are the first imports.
fn import(entry: Entry) -> u32 {
    entry as u32
}

/// The function index of `amb_words`, imported after the entry points.
// There are a handful of entry points; this cannot truncate.
#[allow(clippy::cast_possible_truncation)]
const WORDS: u32 = Entry::ALL.len() as u32;

/// The type index of a compiled function taking `arity` arguments.
fn function_type(arity: usize) -> u32 {
    u32::try_from(arity).unwrap_or(0)
}

fn memory(offset: i32) -> MemArg {
    MemArg {
        offset: u64::from(offset.unsigned_abs()),
        align: 3,
        target: MemArgTarget::Memory(0),
        ordering: None,
    }
}

const CONTEXT: u32 = 0;
const CLOSURE: u32 = 1;
/// The first local of the lowered function.
const LOCALS: u32 = 2;

impl Backend for Wasm {
    fn name(&self) -> &'static str {
        "wasm"
    }

    fn compile(&self, definition: &Open) -> Result<Vec<u8>, String> {
        let Tree::Function(function) = &definition.tree else {
            return Err("not a function".to_owned());
        };
        if function.params.len() > MAX_ARITY {
            return Err(format!("more than {MAX_ARITY} parameters"));
        }
        let function =
            lower(function).map_err(|error| format!("{} are not compiled yet", error.0))?;
        let locals = u32::try_from(function.locals).map_err(|error| error.to_string())?;

        // Two more locals than the function has, to hold a word while it is
        // tested and a callee while its arguments are pushed.
        let declared = function.locals - function.params + 2;
        let mut emitter = Emitter {
            body: Function::new_with_locals_types(vec![ValType::I64; declared]),
            scratch: LOCALS + locals,
            callee: LOCALS + locals + 1,
            frames: Vec::new(),
        };
        emitter.block();
        emitter.frames.push(true);
        emitter.expr(&function.body);
        emitter.frames.pop();
        emitter.emit(&Instruction::End);
        emitter.emit(&Instruction::End);
        Ok(emitter.body.into_raw_body())
    }
}

struct Emitter {
    body: Function,
    scratch: u32,
    callee: u32,
    /// The control constructs open around the current instruction,
    /// innermost last. `true` marks one that unwinding leaves through: the
    /// body of a `Let`, or of the function.
    frames: Vec<bool>,
}

impl Emitter {
    fn emit(&mut self, instruction: &Instruction) {
        self.body.instruction(instruction);
    }

    fn block(&mut self) {
        self.emit(&Instruction::Block(BlockType::Result(ValType::I64)));
    }

    fn constant(&mut self, word: Word) {
        self.emit(&Instruction::I64Const(word.cast_signed()));
    }

    /// How many constructs a branch must leave to reach the innermost one
    /// unwinding leaves through.
    fn unwind_depth(&self) -> u32 {
        let inside = self
            .frames
            .iter()
            .rev()
            .take_while(|target| !**target)
            .count();
        u32::try_from(inside).unwrap_or(0)
    }

    fn value(&mut self, value: lir::Value) {
        match value {
            lir::Value::Local(local) => {
                let local = u32::try_from(local).unwrap_or(0);
                self.emit(&Instruction::LocalGet(LOCALS + local));
            }
            lir::Value::Null => self.constant(NULL),
            lir::Value::Bool(true) => self.constant(TRUE),
            lir::Value::Bool(false) => self.constant(FALSE),
            lir::Value::Int(value) => self.emit(&Instruction::I64Const((value << 1) | 1)),
        }
    }

    /// With a word on the stack: leaves for the innermost unwinding target
    /// if the word says a call is unwinding, and otherwise keeps the word.
    fn check(&mut self) {
        self.emit(&Instruction::LocalTee(self.scratch));
        self.emit(&Instruction::I64Eqz);
        self.emit(&Instruction::If(BlockType::Empty));
        self.frames.push(false);
        self.emit(&Instruction::I64Const(0));
        self.emit(&Instruction::Br(self.unwind_depth()));
        self.frames.pop();
        self.emit(&Instruction::End);
        self.emit(&Instruction::LocalGet(self.scratch));
    }

    /// Leaves one word on the stack.
    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Op(op) => {
                self.op(op);
                if op.unwinds() {
                    self.check();
                }
            }
            Expr::Let { local, value, body } => {
                let local = LOCALS + u32::try_from(*local).unwrap_or(0);
                self.expr(value);
                if !matches!(**value, Expr::Op(_)) {
                    self.check();
                }
                self.emit(&Instruction::LocalSet(local));
                self.block();
                self.frames.push(true);
                self.expr(body);
                self.frames.pop();
                self.emit(&Instruction::End);
                self.emit(&Instruction::LocalGet(local));
                self.emit(&Instruction::Call(import(Entry::Release)));
            }
            Expr::If {
                condition,
                then,
                otherwise,
            } => {
                self.value(*condition);
                self.constant(TRUE);
                self.emit(&Instruction::I64Eq);
                self.emit(&Instruction::If(BlockType::Result(ValType::I64)));
                self.frames.push(false);
                self.expr(then);
                self.emit(&Instruction::Else);
                self.expr(otherwise);
                self.frames.pop();
                self.emit(&Instruction::End);
            }
            Expr::Fault(fault) => {
                self.emit(&Instruction::LocalGet(CONTEXT));
                self.constant(*fault as u64);
                self.emit(&Instruction::Call(import(Entry::Fault)));
                self.emit(&Instruction::Br(self.unwind_depth()));
            }
        }
    }

    fn op(&mut self, op: &Op) {
        match op {
            Op::Copy(value) => {
                if matches!(value, lir::Value::Local(_)) {
                    self.value(*value);
                    self.emit(&Instruction::Call(import(Entry::Retain)));
                }
                self.value(*value);
            }
            Op::Int(value) => {
                if (SMALL_MIN..=SMALL_MAX).contains(value) {
                    self.value(lir::Value::Int(*value));
                } else {
                    self.emit(&Instruction::I64Const(*value));
                    self.emit(&Instruction::Call(import(Entry::IntNew)));
                }
            }
            Op::Float(bits) => {
                self.constant(*bits);
                self.emit(&Instruction::Call(import(Entry::FloatNew)));
            }
            Op::Call { slot, args } => {
                let slot = DEFINITION_SLOTS + i32::try_from(*slot * 8).unwrap_or(0);
                self.emit(&Instruction::LocalGet(CONTEXT));
                // The callee: a closure in a slot of this definition.
                self.emit(&Instruction::LocalGet(CLOSURE));
                self.load(CLOSURE_DEFINITION);
                self.load(slot);
                self.emit(&Instruction::LocalTee(self.callee));
                for arg in args {
                    self.value(*arg);
                }
                // Its code: an index into the table.
                self.emit(&Instruction::LocalGet(self.callee));
                self.load(CLOSURE_DEFINITION);
                self.load(DEFINITION_CODE);
                self.emit(&Instruction::I32WrapI64);
                self.emit(&Instruction::CallIndirect {
                    type_index: function_type(args.len()),
                    table_index: 0,
                });
            }
            Op::Primitive { operation, args } => {
                let entry = entry_for(*operation);
                if entry.takes_context() {
                    self.emit(&Instruction::LocalGet(CONTEXT));
                }
                for arg in args {
                    self.value(*arg);
                }
                self.emit(&Instruction::Call(import(entry)));
            }
        }
    }

    /// Replaces the address on the stack with the word at `offset` from it.
    fn load(&mut self, offset: i32) {
        self.emit(&Instruction::I32WrapI64);
        self.emit(&Instruction::I64Load(memory(offset)));
    }
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
    let init_type = next_type + 1;
    types.ty().function([], [ValType::I64]);

    let first = WORDS + 1;
    let mut functions = FunctionSection::new();
    let mut code = CodeSection::new();
    let mut table = Vec::new();
    for (position, part) in parts.parts.iter().enumerate() {
        functions.function(function_type(part.arity));
        code.raw(&part.code.0);
        table.push(first + index(position)?);
    }
    let init = first + index(parts.parts.len())?;
    functions.function(init_type);
    code.function(&initializer(parts)?);

    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        minimum: parts.parts.len() as u64,
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
    exports.export("entry", ExportKind::Func, first + index(parts.entry)?);

    let mut module = Module::new();
    module
        .section(&types)
        .section(&imports)
        .section(&functions)
        .section(&tables)
        .section(&exports)
        .section(&elements)
        .section(&code);
    Ok(module.finish())
}

/// What `initializer` writes into one word.
enum Field {
    Word(u64),
    /// The address of another word of the block being laid out.
    Address(usize),
}

/// The local of `initializer` that holds the address of the block.
const BASE: u32 = 0;

/// The function that lays out every definition and its closure in the
/// runtime's memory and returns the entry's closure.
fn initializer(parts: &Parts) -> Result<Function, String> {
    let offset = |word: usize| i32::try_from(word * 8).map_err(|error| error.to_string());

    // Each part's definition, then its closure, as the native loader does.
    let mut definitions = Vec::new();
    let mut words = 0usize;
    for part in &parts.parts {
        definitions.push(words);
        words += DEFINITION_WORDS + part.slots.len() + CLOSURE_WORDS;
    }
    let closure =
        |part: usize| definitions[part] + DEFINITION_WORDS + parts.parts[part].slots.len();

    let mut body = Function::new_with_locals_types([ValType::I32]);
    body.instruction(&Instruction::I64Const(
        i64::try_from(words).map_err(|error| error.to_string())?,
    ));
    body.instruction(&Instruction::Call(WORDS));
    body.instruction(&Instruction::LocalSet(BASE));

    let store = |body: &mut Function, word: usize, field: Field| -> Result<(), String> {
        body.instruction(&Instruction::LocalGet(BASE));
        match field {
            Field::Word(value) => {
                body.instruction(&Instruction::I64Const(value.cast_signed()));
            }
            Field::Address(target) => {
                body.instruction(&Instruction::LocalGet(BASE));
                body.instruction(&Instruction::I64ExtendI32U);
                body.instruction(&Instruction::I64Const(i64::from(offset(target)?)));
                body.instruction(&Instruction::I64Add);
            }
        }
        body.instruction(&Instruction::I64Store(memory(offset(word)?)));
        Ok(())
    };

    let mut header = [0u64; CLOSURE_WORDS];
    ambient_runtime::pinned_closure(&mut header, 0);
    for (position, part) in parts.parts.iter().enumerate() {
        let definition = definitions[position];
        // The code is the function's index in the table, which is its
        // position among the parts.
        store(&mut body, definition, Field::Word(position as u64))?;
        store(&mut body, definition + 1, Field::Word(part.arity as u64))?;
        for (slot, target) in part.slots.iter().enumerate() {
            let field = target.map_or(Field::Word(0), |target| Field::Address(closure(target)));
            store(&mut body, definition + DEFINITION_WORDS + slot, field)?;
        }
        let at = closure(position);
        store(&mut body, at, Field::Word(header[0]))?;
        store(&mut body, at + 1, Field::Word(header[1]))?;
        store(&mut body, at + 2, Field::Address(definition))?;
    }

    body.instruction(&Instruction::LocalGet(BASE));
    body.instruction(&Instruction::I64ExtendI32U);
    body.instruction(&Instruction::I64Const(i64::from(offset(closure(
        parts.entry,
    ))?)));
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::End);
    Ok(body)
}
