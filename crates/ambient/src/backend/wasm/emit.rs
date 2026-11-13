//! Emitting the body of one lowered function.

use ambient_runtime::layout::{
    CLOSURE_CAPTURES, CLOSURE_DEFINITION, DEFINITION_CODE, DEFINITION_METERED, DEFINITION_SLOTS,
};
use ambient_runtime::{Entry, FALSE, NULL, SMALL_MAX, SMALL_MIN, TRUE, Word};
use wasm_encoder::{BlockType, Function, Instruction, MemArg, MemArgTarget, ValType};

use super::code;
use super::function_type;
use crate::backend::MAX_ARITY;
use crate::core::Open;
use crate::engine::Backend;
use crate::lir::{self, Arg, Expr, Op};
use crate::lower::{Form, lower};

/// Compiles for WebAssembly.
pub struct Wasm;

pub(super) fn memory(offset: i32) -> MemArg {
    MemArg {
        offset: u64::from(offset.unsigned_abs()),
        align: 3,
        target: MemArgTarget::Memory(0),
        ordering: None,
    }
}

pub(super) const CONTEXT: u32 = 0;
const CLOSURE: u32 = 1;
/// The first parameter of the lowered function.
pub(super) const PARAMS: u32 = 2;
/// The runtime's stack pointer, the one global a module imports: a
/// function keeps its locals in a frame it takes from that stack.
pub(super) const STACK: u32 = 0;

impl Backend for Wasm {
    fn name(&self) -> &'static str {
        "wasm"
    }

    fn compile(&self, definition: &Open, form: &Form) -> Result<Vec<u8>, String> {
        let function =
            lower(definition, form).map_err(|error| format!("{} are not compiled yet", error.0))?;
        if function.params > MAX_ARITY {
            return Err(format!("more than {MAX_ARITY} parameters"));
        }
        let small = |value: usize| u32::try_from(value).map_err(|error| error.to_string());
        let (locals, params) = (small(function.locals)?, small(function.params)?);
        // The frame is kept aligned as the runtime's own are.
        let size = i32::try_from((function.locals * 8).next_multiple_of(16))
            .map_err(|error| error.to_string())?;

        // The lowered function's locals are in memory, not locals of
        // the module's function: one whose locals are live across many
        // points takes an engine time that grows with their product to
        // compile. What is declared is six words: one to hold a word
        // while it is tested, a callee while its arguments are pushed,
        // the point a resuming function is going back to, and the three
        // words of the frame a suspending one keeps. Then the address
        // of the locals.
        let mut declared = vec![ValType::I64; 6];
        declared.push(ValType::I32);
        let mut emitter = Emitter {
            body: Function::new_with_locals_types(declared),
            locals,
            params,
            size,
            scratch: PARAMS + params,
            callee: PARAMS + params + 1,
            target: PARAMS + params + 2,
            saved: PARAMS + params + 3,
            frame: PARAMS + params + 6,
            slots: function.slots,
            frames: Vec::new(),
            calls: Vec::new(),
            widest: 0,
        };
        emitter.enter();
        // The outermost block is the one a point leaves to suspend: after
        // it, the function keeps its locals.
        emitter.emit(&Instruction::Block(BlockType::Empty));
        emitter.block();
        emitter.frames.push(true);
        emitter.expr(&function.body);
        emitter.frames.pop();
        emitter.emit(&Instruction::End);
        emitter.leave();
        emitter.emit(&Instruction::Return);
        emitter.emit(&Instruction::End);
        emitter.save();
        emitter.leave();
        emitter.emit(&Instruction::End);
        code::artifact(emitter.body.into_raw_body(), &emitter.calls, emitter.widest)
    }
}

pub(super) struct Emitter {
    pub(super) body: Function,
    /// How many locals the lowered function has, and how many of them
    /// are its parameters.
    pub(super) locals: u32,
    pub(super) params: u32,
    /// The bytes its locals take on the stack.
    pub(super) size: i32,
    /// Where its locals are: an address.
    pub(super) frame: u32,
    pub(super) scratch: u32,
    pub(super) callee: u32,
    /// The point a function that is resuming is going back to, and `-1`
    /// in one that is not.
    pub(super) target: u32,
    /// The first of three locals a suspending function puts its frame
    /// in: the point it is at, and its definition and position.
    pub(super) saved: u32,
    /// How many slots the definition has; its constants follow them.
    pub(super) slots: usize,
    /// The control constructs open around the current instruction,
    /// innermost last. `true` marks one that unwinding leaves through: the
    /// body of a `Let`, or of the function.
    pub(super) frames: Vec<bool>,
    /// Where the body calls the runtime: see [`code`].
    pub(super) calls: Vec<(usize, Entry)>,
    /// The most arguments the body calls a function value with: a
    /// module has a type for each number up to the most any body needs.
    pub(super) widest: usize,
}

impl Emitter {
    pub(super) fn emit(&mut self, instruction: &Instruction) {
        self.body.instruction(instruction);
    }

    /// Calls an entry point of the runtime.
    pub(super) fn runtime(&mut self, entry: Entry) {
        let mut calls = Vec::new();
        code::call(&mut self.body, &mut calls, entry.name());
        self.calls
            .extend(calls.into_iter().map(|(offset, _)| (offset, entry)));
    }

    fn block(&mut self) {
        self.emit(&Instruction::Block(BlockType::Result(ValType::I64)));
    }

    pub(super) fn constant(&mut self, word: Word) {
        self.emit(&Instruction::I64Const(word.cast_signed()));
    }

    /// How many constructs a branch must leave to reach the innermost one
    /// unwinding leaves through.
    pub(super) fn unwind_depth(&self) -> u32 {
        let inside = self
            .frames
            .iter()
            .rev()
            .take_while(|target| !**target)
            .count();
        u32::try_from(inside).unwrap_or(0)
    }

    pub(super) fn value(&mut self, value: lir::Value) {
        match value {
            lir::Value::Local(local) => self.local(local),
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

    /// Pushes a local of the lowered function.
    pub(super) fn local(&mut self, local: usize) {
        self.emit(&Instruction::LocalGet(self.frame));
        self.emit(&Instruction::I64Load(memory(Self::offset(local))));
    }

    /// Puts the word on the stack in a local of the lowered function.
    pub(super) fn set_local(&mut self, local: usize) {
        self.emit(&Instruction::LocalSet(self.scratch));
        self.emit(&Instruction::LocalGet(self.frame));
        self.emit(&Instruction::LocalGet(self.scratch));
        self.emit(&Instruction::I64Store(memory(Self::offset(local))));
    }

    fn offset(local: usize) -> i32 {
        i32::try_from(local * 8).unwrap_or(0)
    }

    /// Gives the stack back what the function's locals took. Every way
    /// out of the function passes here.
    fn leave(&mut self) {
        self.emit(&Instruction::LocalGet(self.frame));
        self.emit(&Instruction::I32Const(self.size));
        self.emit(&Instruction::I32Add);
        self.emit(&Instruction::GlobalSet(STACK));
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
            Expr::Point { point, op, frame } => self.point(*point, op, *frame),
            Expr::Resumed {
                points,
                local,
                value,
            } => {
                // Unless the function is on its way to a point elsewhere,
                // in which case the local has its value already.
                self.emit(&Instruction::LocalGet(self.target));
                self.emit(&Instruction::I64Const(0));
                self.emit(&Instruction::I64LtS);
                self.resuming_to(*points);
                self.emit(&Instruction::I32Or);
                self.emit(&Instruction::If(BlockType::Result(ValType::I64)));
                self.frames.push(false);
                self.expr(value);
                self.emit(&Instruction::Else);
                self.local(*local);
                self.frames.pop();
                self.emit(&Instruction::End);
            }
            Expr::Op(op) => {
                self.op(op);
                if op.unwinds() {
                    self.check();
                }
            }
            Expr::Let { local, value, body } => {
                self.expr(value);
                if !matches!(**value, Expr::Op(_)) {
                    self.check();
                }
                self.set_local(*local);
                self.block();
                self.frames.push(true);
                self.expr(body);
                self.frames.pop();
                self.emit(&Instruction::End);
                self.local(*local);
                self.runtime(Entry::Release);
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
            Expr::Try {
                body,
                recover,
                recovering,
            } => {
                // The inner block is where the body's unwinding lands; the
                // outer one is where both ways out meet.
                self.block();
                self.frames.push(false);
                self.block();
                self.frames.push(true);
                // A function going back to a point in `recover` had left
                // the body already.
                self.resuming_to(*recovering);
                self.emit(&Instruction::If(BlockType::Empty));
                self.emit(&Instruction::I64Const(0));
                self.emit(&Instruction::Br(1));
                self.emit(&Instruction::End);
                self.expr(body);
                if !matches!(**body, Expr::Op(_)) {
                    self.check();
                }
                self.emit(&Instruction::Br(1));
                self.frames.pop();
                self.emit(&Instruction::End);
                self.emit(&Instruction::Drop);
                self.expr(recover);
                self.frames.pop();
                self.emit(&Instruction::End);
            }
            Expr::Fault(fault) => {
                self.emit(&Instruction::LocalGet(CONTEXT));
                self.constant(*fault as u64);
                self.runtime(Entry::Fault);
                self.emit(&Instruction::Br(self.unwind_depth()));
            }
        }
    }

    pub(super) fn op(&mut self, op: &Op) {
        match op {
            Op::Copy(value) => {
                if value.counted() {
                    self.value(*value);
                    self.runtime(Entry::Retain);
                }
                self.value(*value);
            }
            Op::Int(value) => {
                if (SMALL_MIN..=SMALL_MAX).contains(value) {
                    self.value(lir::Value::Int(*value));
                } else {
                    self.emit(&Instruction::I64Const(*value));
                    self.runtime(Entry::IntNew);
                }
            }
            Op::Float(bits) => {
                self.constant(*bits);
                self.runtime(Entry::FloatNew);
            }
            Op::Slot(slot) => {
                self.in_definition(*slot);
                self.runtime(Entry::Retain);
                self.in_definition(*slot);
            }
            Op::Call {
                slot,
                args,
                metered,
            } => {
                self.emit(&Instruction::LocalGet(CONTEXT));
                self.in_definition(*slot);
                self.call(args, *metered);
            }
            Op::CallValue {
                callee,
                args,
                metered,
            } => {
                self.emit(&Instruction::LocalGet(CONTEXT));
                self.value(*callee);
                self.call(args, *metered);
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
                    self.runtime(step.entry);
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
    fn call(&mut self, args: &[lir::Value], metered: bool) {
        self.emit(&Instruction::LocalTee(self.callee));
        for arg in args {
            self.value(*arg);
        }
        // Its code: an index into the table.
        self.emit(&Instruction::LocalGet(self.callee));
        self.load(CLOSURE_DEFINITION);
        self.load(if metered {
            DEFINITION_METERED
        } else {
            DEFINITION_CODE
        });
        self.emit(&Instruction::I32WrapI64);
        self.widest = self.widest.max(args.len());
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
