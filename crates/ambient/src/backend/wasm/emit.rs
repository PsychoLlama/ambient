//! Emitting the body of one lowered function.

use ambient_runtime::layout::{
    CLOSURE_CAPTURES, CLOSURE_DEFINITION, DEFINITION_CODE, DEFINITION_METERED, DEFINITION_SLOTS,
};
use ambient_runtime::{
    Context, Entry, FALSE, NULL, RESUMING, SMALL_MAX, SMALL_MIN, SUSPENDING, TRUE, Word,
};
use wasm_encoder::{BlockType, Function, Instruction, MemArg, MemArgTarget, ValType};

use super::{function_type, import};
use crate::backend::MAX_ARITY;
use crate::core::Open;
use crate::engine::Backend;
use crate::lir::{self, Arg, Expr, Op};
use crate::lower::lower;

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

const CONTEXT: u32 = 0;
const CLOSURE: u32 = 1;
/// The first local of the lowered function.
const LOCALS: u32 = 2;

impl Backend for Wasm {
    fn name(&self) -> &'static str {
        "wasm"
    }

    fn compile(&self, definition: &Open, metered: bool) -> Result<Vec<u8>, String> {
        let function = lower(definition, metered)
            .map_err(|error| format!("{} are not compiled yet", error.0))?;
        if function.params > MAX_ARITY {
            return Err(format!("more than {MAX_ARITY} parameters"));
        }
        let locals = u32::try_from(function.locals).map_err(|error| error.to_string())?;

        // Six more locals than the function has: to hold a word while it
        // is tested, a callee while its arguments are pushed, the point a
        // resuming function is going back to, and the three words of the
        // frame a suspending one keeps.
        let declared = function.locals - function.params + 6;
        let mut emitter = Emitter {
            body: Function::new_with_locals_types(vec![ValType::I64; declared]),
            locals,
            scratch: LOCALS + locals,
            callee: LOCALS + locals + 1,
            target: LOCALS + locals + 2,
            saved: LOCALS + locals + 3,
            slots: function.slots,
            frames: Vec::new(),
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
        emitter.emit(&Instruction::Return);
        emitter.emit(&Instruction::End);
        emitter.save();
        emitter.emit(&Instruction::End);
        Ok(emitter.body.into_raw_body())
    }
}

struct Emitter {
    body: Function,
    /// How many locals the lowered function has.
    locals: u32,
    scratch: u32,
    callee: u32,
    /// The point a function that is resuming is going back to, and `-1`
    /// in one that is not.
    target: u32,
    /// The first of three locals a suspending function puts its frame
    /// in: the point it is at, and its definition and position.
    saved: u32,
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

    /// Pushes the context's mode.
    fn mode(&mut self) {
        self.emit(&Instruction::LocalGet(CONTEXT));
        self.emit(&Instruction::I64Load(memory(Context::MODE)));
    }

    /// Calls an entry point that takes the context and what is on the
    /// stack after it.
    fn frame(&mut self, entry: Entry) {
        self.emit(&Instruction::Call(import(entry)));
    }

    /// What a function does first: if its task is resuming, it takes its
    /// locals back and notes where it was.
    fn enter(&mut self) {
        self.emit(&Instruction::I64Const(-1));
        self.emit(&Instruction::LocalSet(self.target));
        self.mode();
        self.constant(RESUMING);
        self.emit(&Instruction::I64Eq);
        self.emit(&Instruction::If(BlockType::Empty));
        self.emit(&Instruction::LocalGet(CONTEXT));
        self.frame(Entry::FramePoint);
        self.emit(&Instruction::LocalSet(self.target));
        for local in 0..self.locals {
            self.emit(&Instruction::LocalGet(CONTEXT));
            self.constant(u64::from(local));
            self.frame(Entry::FrameLoad);
            self.emit(&Instruction::LocalSet(LOCALS + local));
        }
        self.emit(&Instruction::LocalGet(CONTEXT));
        self.frame(Entry::FrameDrop);
        self.emit(&Instruction::End);
    }

    /// Pushes whether the point being resumed to is one of `points`:
    /// first, and one past last.
    fn resuming_to(&mut self, points: (usize, usize)) {
        self.emit(&Instruction::LocalGet(self.target));
        self.constant(points.0 as u64);
        self.emit(&Instruction::I64GeS);
        self.emit(&Instruction::LocalGet(self.target));
        self.constant(points.1 as u64);
        self.emit(&Instruction::I64LtS);
        self.emit(&Instruction::I32And);
    }

    /// An operation the task may suspend in. Leaves one word on the
    /// stack.
    fn point(&mut self, point: usize, op: &Op, frame: Option<(usize, usize)>) {
        // A function that was going back to this point is there.
        self.resuming_to((point, point + 1));
        self.emit(&Instruction::If(BlockType::Empty));
        self.emit(&Instruction::I64Const(-1));
        self.emit(&Instruction::LocalSet(self.target));
        self.emit(&Instruction::End);

        self.op(op);
        self.emit(&Instruction::LocalTee(self.scratch));
        self.emit(&Instruction::I64Eqz);
        self.emit(&Instruction::If(BlockType::Empty));
        self.frames.push(false);
        // It unwound. If the task is suspending, this function keeps its
        // locals for later and leaves at once; if not, it unwinds.
        self.mode();
        self.constant(SUSPENDING);
        self.emit(&Instruction::I64Eq);
        self.emit(&Instruction::If(BlockType::Empty));
        self.constant(point as u64);
        self.emit(&Instruction::LocalSet(self.saved));
        let (definition, position) = match frame {
            Some((definition, position)) => (lir::Value::Constant(definition), position),
            None => (lir::Value::Null, 0),
        };
        self.value(definition);
        self.emit(&Instruction::LocalSet(self.saved + 1));
        self.constant(position as u64);
        self.emit(&Instruction::LocalSet(self.saved + 2));
        // Past this `if` and every construct open around it is the
        // function's outermost block.
        let depth = u32::try_from(self.frames.len()).unwrap_or(0) + 1;
        self.emit(&Instruction::Br(depth));
        self.emit(&Instruction::End);
        self.emit(&Instruction::I64Const(0));
        self.emit(&Instruction::Br(self.unwind_depth()));
        self.frames.pop();
        self.emit(&Instruction::End);
        self.emit(&Instruction::LocalGet(self.scratch));
    }

    /// What every point leaves for to suspend: the function keeps its
    /// frame and its locals, and returns. Every point shares it, so a
    /// function's code grows with its points and not with their product
    /// with its locals.
    fn save(&mut self) {
        self.emit(&Instruction::LocalGet(CONTEXT));
        for word in 0..3 {
            self.emit(&Instruction::LocalGet(self.saved + word));
        }
        self.frame(Entry::FramePush);
        for local in 0..self.locals {
            self.emit(&Instruction::LocalGet(CONTEXT));
            self.emit(&Instruction::LocalGet(LOCALS + local));
            self.frame(Entry::FrameSave);
        }
        self.emit(&Instruction::I64Const(0));
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
                let local = LOCALS + u32::try_from(*local).unwrap_or(0);
                self.emit(&Instruction::LocalGet(local));
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
