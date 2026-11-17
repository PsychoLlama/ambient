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

/// What names this backend ([`Backend::name`]).
pub const NAME: &str = "wasm";

impl Backend for Wasm {
    fn name(&self) -> &'static str {
        NAME
    }

    fn compile(&self, definition: &Open, form: &Form) -> Result<Vec<u8>, String> {
        let function =
            lower(definition, form).map_err(|error| format!("{} are not compiled yet", error.0))?;
        if function.params > MAX_ARITY {
            return Err(format!("more than {MAX_ARITY} parameters"));
        }
        let small = |value: usize| u32::try_from(value).map_err(|error| error.to_string());
        let (locals, params) = (function.locals, small(function.params)?);
        let kept = Kept::of(&function)?;
        // After the lowered function's own locals: a word to hold one
        // while it is tested, a callee while its arguments are pushed,
        // and where a function that may suspend is. Then, if its locals
        // are in memory, their address.
        let own = PARAMS + small(kept.own(&function))?;
        let mut declared = vec![ValType::I64; kept.own(&function) - function.params + 3];
        if matches!(kept, Kept::Memory { .. }) {
            declared.push(ValType::I32);
        }
        let mut emitter = Emitter {
            body: Function::new_with_locals_types(declared),
            locals,
            params,
            kept,
            scratch: own,
            callee: own + 1,
            at: own + 2,
            frame: own + 3,
            slots: function.slots,
            held: 0,
            resumes: function.points > 0,
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
        // Where a point leaves to. A function that has none never gets
        // here.
        if function.points > 0 {
            emitter.save();
            emitter.leave();
        } else {
            emitter.emit(&Instruction::Unreachable);
        }
        emitter.emit(&Instruction::End);
        code::artifact(emitter.body.into_raw_body(), &emitter.calls, emitter.widest)
    }
}

/// How a function keeps the locals of its lowered form.
#[derive(Clone, Copy)]
pub(super) enum Kept {
    /// As locals of its own, its parameters being the first.
    Values,
    /// In memory, in a frame of so many bytes it takes from the
    /// runtime's stack.
    ///
    /// A local is held to the end of its `Let`, so as a value it is
    /// live across everything after it, and an engine takes time that
    /// grows with the product of the two to compile the function. One
    /// with more locals than [`VALUES`] keeps them here, where that
    /// costs it a few bytes at each use and nothing more.
    Memory { size: i32 },
}

/// The most locals a function keeps as values.
pub(super) const VALUES: usize = 256;

impl Kept {
    fn of(function: &lir::Function) -> Result<Self, String> {
        if function.locals <= VALUES {
            return Ok(Self::Values);
        }
        // The frame is kept aligned as the runtime's own are.
        let size = i32::try_from((function.locals * 8).next_multiple_of(16))
            .map_err(|error| error.to_string())?;
        Ok(Self::Memory { size })
    }

    /// How many of the lowered function's locals are locals of the
    /// function emitted: all of them, or only its parameters.
    fn own(self, function: &lir::Function) -> usize {
        match self {
            Self::Values => function.locals,
            Self::Memory { .. } => function.params,
        }
    }
}

pub(super) struct Emitter {
    pub(super) body: Function,
    /// How many locals the lowered function has, and how many of them
    /// are its parameters.
    pub(super) locals: usize,
    pub(super) params: u32,
    kept: Kept,
    /// Where its locals are, if they are in memory: an address.
    frame: u32,
    pub(super) scratch: u32,
    pub(super) callee: u32,
    /// Where a function that may suspend is: see [`super::resume`].
    pub(super) at: u32,
    /// How many slots the definition has; its constants follow them.
    pub(super) slots: usize,
    /// Where the loop the function is keeps its parameters: see
    /// [`Expr::Loop`].
    pub(super) held: usize,
    /// Whether the function may be resuming: it has a point.
    pub(super) resumes: bool,
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

    /// The local of the function emitted that a local of the lowered
    /// one is, where they are values.
    fn own(local: usize) -> u32 {
        PARAMS + u32::try_from(local).unwrap_or(0)
    }

    /// Pushes a local of the lowered function.
    pub(super) fn local(&mut self, local: usize) {
        match self.kept {
            Kept::Values => self.emit(&Instruction::LocalGet(Self::own(local))),
            Kept::Memory { .. } => {
                self.emit(&Instruction::LocalGet(self.frame));
                self.emit(&Instruction::I64Load(memory(Self::offset(local))));
            }
        }
    }

    /// Puts the word on the stack in a local of the lowered function.
    pub(super) fn set_local(&mut self, local: usize) {
        match self.kept {
            Kept::Values => self.emit(&Instruction::LocalSet(Self::own(local))),
            Kept::Memory { .. } => {
                self.emit(&Instruction::LocalSet(self.scratch));
                self.emit(&Instruction::LocalGet(self.frame));
                self.emit(&Instruction::LocalGet(self.scratch));
                self.emit(&Instruction::I64Store(memory(Self::offset(local))));
            }
        }
    }

    fn offset(local: usize) -> i32 {
        i32::try_from(local * 8).unwrap_or(0)
    }

    /// What a function does first: one whose locals are in memory makes
    /// room for them, and one that may be resuming takes them back if
    /// it is.
    fn enter(&mut self) {
        if let Kept::Memory { size } = self.kept {
            self.emit(&Instruction::GlobalGet(STACK));
            self.emit(&Instruction::I32Const(size));
            self.emit(&Instruction::I32Sub);
            self.emit(&Instruction::LocalTee(self.frame));
            self.emit(&Instruction::GlobalSet(STACK));
            // Its parameters are the first of its locals, and what
            // resuming gives back is the same.
            for param in 0..self.params {
                self.emit(&Instruction::LocalGet(PARAMS + param));
                self.set_local(param as usize);
            }
        }
        if self.resumes {
            self.restore();
        }
    }

    /// Gives the stack back what the function's locals took, if they
    /// took any. Every way out of the function passes here.
    fn leave(&mut self) {
        if let Kept::Memory { size } = self.kept {
            self.emit(&Instruction::LocalGet(self.frame));
            self.emit(&Instruction::I32Const(size));
            self.emit(&Instruction::I32Add);
            self.emit(&Instruction::GlobalSet(STACK));
        }
    }

    /// Whether a loop's locals may hold something when it starts: only
    /// in memory, which is not made empty.
    pub(super) fn stale(&self) -> bool {
        matches!(self.kept, Kept::Memory { .. })
    }

    /// The value a `Let` binds to `local`, put there.
    pub(super) fn bound(&mut self, local: usize, value: &Expr) {
        self.expr(value);
        if !Self::checked(value) {
            self.check();
        }
        self.set_local(local);
    }

    /// With a word on the stack: leaves for the innermost unwinding target
    /// if the word says a call is unwinding, and otherwise keeps the word.
    ///
    /// The word that says so is what unwinding gives where it lands, so
    /// the branch takes the word with it.
    fn check(&mut self) {
        self.emit(&Instruction::LocalTee(self.scratch));
        self.emit(&Instruction::LocalGet(self.scratch));
        self.emit(&Instruction::I64Eqz);
        self.emit(&Instruction::BrIf(self.unwind_depth()));
    }

    /// Whether what `expr` leaves has been checked already, or never
    /// says a call is unwinding: an operation checks itself.
    fn checked(expr: &Expr) -> bool {
        matches!(expr, Expr::Op(_) | Expr::Point { .. })
    }

    /// Leaves one word on the stack.
    pub(super) fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Point { point, op } => self.point(*point, op),
            // Only a `Let` has one, and binds it itself.
            Expr::Resumed { value, .. } => self.expr(value),
            Expr::Op(op) => {
                self.op(op);
                if op.unwinds() {
                    self.check();
                }
            }
            Expr::Let { local, value, body } => {
                match &**value {
                    Expr::Resumed {
                        points,
                        local,
                        value,
                    } => self.resumed(*points, *local, value),
                    value => self.bound(*local, value),
                }
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
                // the body already; one that is before them has not.
                if recovering.0 < recovering.1 {
                    self.emit(&Instruction::I64Const(0));
                    self.emit(&Instruction::LocalGet(self.at));
                    self.constant(recovering.0 as u64);
                    self.emit(&Instruction::I64GeS);
                    self.emit(&Instruction::BrIf(0));
                    self.emit(&Instruction::Drop);
                }
                self.expr(body);
                if !Self::checked(body) {
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
            Expr::Loop { body, held } => self.looped(body, *held),
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
            Op::Again(args) => self.again(args),
            _ => {
                let steps = op.steps().unwrap_or_default();
                for (position, step) in steps.iter().enumerate() {
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
                    if position == 0 && steps.len() > 1 {
                        self.emit(&Instruction::LocalSet(self.callee));
                    }
                }
                if steps.len() > 1 {
                    self.emit(&Instruction::LocalGet(self.callee));
                }
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
