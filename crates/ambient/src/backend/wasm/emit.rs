//! Emitting the body of one lowered function.

use ambient_runtime::layout::{CLOSURE_CAPTURES, CLOSURE_DEFINITION, DEFINITION_CODE};
use ambient_runtime::{Entry, FALSE, NULL, SMALL_MAX, SMALL_MIN, TRUE, Word};
use wasm_encoder::{BlockType, Function, Instruction, MemArg, MemArgTarget, ValType};

use super::code;
use super::function_type;
use super::locals::{Kept, Seen};
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
pub(super) const CLOSURE: u32 = 1;
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
        // Emitted once to see what it reads and whether it turns, and
        // again if knowing that makes it smaller.
        let first = Emitter::emitted(&function, None)?;
        let emitter = match first.smaller() {
            Some(seen) => Emitter::emitted(&function, Some(seen))?,
            None => first,
        };
        code::artifact(emitter.body.into_raw_body(), &emitter.calls, emitter.widest)
    }
}

pub(super) struct Emitter {
    pub(super) body: Function,
    /// How many locals the lowered function has, and how many of them
    /// are its parameters.
    pub(super) locals: usize,
    pub(super) params: u32,
    pub(super) kept: Kept,
    /// Where its locals are, if they are in memory: an address.
    pub(super) frame: u32,
    /// Where its definition is, if it keeps that: an address.
    pub(super) definition: u32,
    /// What this emission has seen of the function, and what one before
    /// it saw.
    pub(super) seen: Seen,
    pub(super) told: Option<Seen>,
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
    /// The function's body, emitted knowing what an emission before
    /// this one saw of it, if one was made.
    fn emitted(function: &lir::Function, told: Option<Seen>) -> Result<Self, String> {
        let small = |value: usize| u32::try_from(value).map_err(|error| error.to_string());
        let kept = Kept::of(function)?;
        let keeps = told.is_some_and(Seen::keeps);
        // After the lowered function's own locals: a word to hold one
        // while it is tested, a callee while its arguments are pushed,
        // and where a function that may suspend is. Then two addresses,
        // for one that has them: its locals in memory, its definition.
        let own = PARAMS + small(kept.own(function))?;
        let memory = u32::from(matches!(kept, Kept::Memory { .. }));
        let mut declared = vec![ValType::I64; kept.own(function) - function.params + 3];
        declared.extend(vec![ValType::I32; (memory + u32::from(keeps)) as usize]);
        let mut emitter = Self {
            body: Function::new_with_locals_types(declared),
            locals: function.locals,
            params: small(function.params)?,
            kept,
            scratch: own,
            callee: own + 1,
            at: own + 2,
            frame: own + 3,
            definition: own + 3 + memory,
            seen: Seen::default(),
            told,
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
        Ok(emitter)
    }

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

    /// With a word on the stack: leaves for the innermost unwinding target
    /// if the word says a call is unwinding, and otherwise keeps the word.
    ///
    /// The word that says so is what unwinding gives where it lands, so
    /// the branch takes the word with it.
    pub(super) fn check(&mut self) {
        self.emit(&Instruction::LocalTee(self.scratch));
        self.emit(&Instruction::LocalGet(self.scratch));
        self.emit(&Instruction::I64Eqz);
        self.emit(&Instruction::BrIf(self.unwind_depth()));
    }

    /// Whether what `expr` leaves has been checked already, or never
    /// says a call is unwinding: an operation checks itself.
    pub(super) fn checked(expr: &Expr) -> bool {
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
        self.widest = self.widest.max(args.len());
        self.emit(&Instruction::CallIndirect {
            type_index: function_type(args.len()),
            table_index: 0,
        });
    }

    /// Replaces the address on the stack with the word at `offset` from it.
    pub(super) fn load(&mut self, offset: i32) {
        self.emit(&Instruction::I32WrapI64);
        self.emit(&Instruction::I64Load(memory(offset)));
    }
}
