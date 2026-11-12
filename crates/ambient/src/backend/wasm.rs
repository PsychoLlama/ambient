//! The WebAssembly backend.
//!
//! A program is two modules: the [runtime](RUNTIME), and one emitted by
//! [`module`] that imports the runtime's memory and entry points. Compiled
//! functions never refer to each other by index, only through closures in
//! memory, so each function's body is compiled on its own and a module is
//! put together by concatenation.

use ambient_runtime::layout::{
    CLOSURE_CAPTURES, CLOSURE_DEFINITION, DEFINITION_CODE, DEFINITION_SLOTS,
};
use ambient_runtime::{Entry, FALSE, NULL, SMALL_MAX, SMALL_MIN, TRUE, Word};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataCountSection, DataSection, ElementSection, Elements,
    EntityType, ExportKind, ExportSection, Function, FunctionSection, ImportSection, Instruction,
    MemArg, MemArgTarget, MemoryType, Module, RefType, TableSection, TableType, TypeSection,
    ValType,
};

use super::MAX_ARITY;
use super::statics::{self, Field, Make};
use crate::core::Open;
use crate::engine::{Backend, Body, Parts};
use crate::lir::{self, Arg, Expr, Op};
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

/// The function indices of the imports that make constants.
const STRING_PINNED: u32 = WORDS + 1;
const INTERN: u32 = WORDS + 2;

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
        let function =
            lower(definition).map_err(|error| format!("{} are not compiled yet", error.0))?;
        if function.params > MAX_ARITY {
            return Err(format!("more than {MAX_ARITY} parameters"));
        }
        let locals = u32::try_from(function.locals).map_err(|error| error.to_string())?;

        // Two more locals than the function has, to hold a word while it is
        // tested and a callee while its arguments are pushed.
        let declared = function.locals - function.params + 2;
        let mut emitter = Emitter {
            body: Function::new_with_locals_types(vec![ValType::I64; declared]),
            scratch: LOCALS + locals,
            callee: LOCALS + locals + 1,
            slots: function.slots,
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
    /// How many slots the definition has; its constants follow them.
    slots: usize,
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
            lir::Value::Captured(position) => {
                self.emit(&Instruction::LocalGet(CLOSURE));
                self.load(CLOSURE_CAPTURES + i32::try_from(position * 8).unwrap_or(0));
            }
            lir::Value::Constant(position) => self.in_definition(self.slots + position),
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
            Op::Slot(slot) => self.in_definition(*slot),
            Op::Call { slot, args } => {
                self.emit(&Instruction::LocalGet(CONTEXT));
                self.in_definition(*slot);
                self.call(args);
            }
            Op::CallValue { callee, args } => {
                self.emit(&Instruction::LocalGet(CONTEXT));
                self.value(*callee);
                self.call(args);
            }
            _ => {
                for (position, step) in op.steps().unwrap_or_default().iter().enumerate() {
                    if step.entry.takes_context() {
                        self.emit(&Instruction::LocalGet(CONTEXT));
                    }
                    for arg in &step.args {
                        match arg {
                            Arg::Value(value) => self.value(*value),
                            Arg::Raw(raw) => self.constant(*raw),
                            Arg::Object => self.emit(&Instruction::LocalGet(self.callee)),
                            Arg::Definition(slot) => {
                                self.in_definition(*slot);
                                self.load(CLOSURE_DEFINITION);
                            }
                        }
                    }
                    self.emit(&Instruction::Call(import(step.entry)));
                    // What the first call returns is the result, and what
                    // the others fill in.
                    if position == 0 {
                        self.emit(&Instruction::LocalSet(self.callee));
                    }
                }
                self.emit(&Instruction::LocalGet(self.callee));
            }
        }
    }

    /// Pushes the word at `position` after the fixed part of this
    /// definition: a slot, or past the slots a constant.
    fn in_definition(&mut self, position: usize) {
        self.emit(&Instruction::LocalGet(CLOSURE));
        self.load(CLOSURE_DEFINITION);
        self.load(DEFINITION_SLOTS + i32::try_from(position * 8).unwrap_or(0));
    }

    /// With the context and a closure on the stack: calls the closure with
    /// `args`.
    fn call(&mut self, args: &[lir::Value]) {
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

    /// Replaces the address on the stack with the word at `offset` from it.
    fn load(&mut self, offset: i32) {
        self.emit(&Instruction::I32WrapI64);
        self.emit(&Instruction::I64Load(memory(offset)));
    }
}

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
