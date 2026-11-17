//! Where a function keeps the locals of its lowered form and reaches
//! its definition, and what emitting it once says of how to.

use ambient_runtime::layout::{CLOSURE_DEFINITION, DEFINITION_SLOTS};
use wasm_encoder::Instruction;

use super::emit::{CLOSURE, Emitter, PARAMS, STACK, memory};
use crate::lir::{self, Expr};

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
    pub(super) fn of(function: &lir::Function) -> Result<Self, String> {
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
    pub(super) fn own(self, function: &lir::Function) -> usize {
        match self {
            Self::Values => function.locals,
            Self::Memory { .. } => function.params,
        }
    }
}

/// What emitting a function says of it, that emitting it again can
/// use.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Seen {
    /// How many times it reads its definition, for a slot or a
    /// constant.
    pub(super) reads: usize,
    /// Whether it is a loop: see [`Expr::Loop`].
    pub(super) turns: bool,
}

impl Seen {
    /// Whether a function that reads its definition so often keeps
    /// where it is: finding it once costs what two reads save.
    pub(super) fn keeps(self) -> bool {
        self.reads > 2
    }
}

impl Emitter {
    /// What this emission saw, if emitting again with it would come out
    /// smaller.
    pub(super) fn smaller(&self) -> Option<Seen> {
        let fewer = self.resumes && !self.seen.turns && self.params > 0;
        (self.seen.keeps() || fewer).then_some(self.seen)
    }

    /// The local of the function emitted that a local of the lowered
    /// one is, where they are values.
    fn own_local(local: usize) -> u32 {
        PARAMS + u32::try_from(local).unwrap_or(0)
    }

    /// Pushes a local of the lowered function.
    pub(super) fn local(&mut self, local: usize) {
        match self.kept {
            Kept::Values => self.emit(&Instruction::LocalGet(Self::own_local(local))),
            Kept::Memory { .. } => {
                self.emit(&Instruction::LocalGet(self.frame));
                self.emit(&Instruction::I64Load(memory(Self::offset(local))));
            }
        }
    }

    /// Puts the word on the stack in a local of the lowered function.
    pub(super) fn set_local(&mut self, local: usize) {
        match self.kept {
            Kept::Values => self.emit(&Instruction::LocalSet(Self::own_local(local))),
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
    pub(super) fn enter(&mut self) {
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
        if self.told.is_some_and(Seen::keeps) {
            self.emit(&Instruction::LocalGet(CLOSURE));
            self.emit(&Instruction::I32WrapI64);
            self.emit(&Instruction::I64Load(memory(CLOSURE_DEFINITION)));
            self.emit(&Instruction::I32WrapI64);
            self.emit(&Instruction::LocalSet(self.definition));
        }
        if self.resumes {
            self.restore();
        }
    }

    /// The locals a function that suspends keeps: all of them if it
    /// turns, since a turn changes its parameters, and otherwise all
    /// but its parameters, which what calls it again hands it again.
    pub(super) fn saved(&self) -> std::ops::Range<usize> {
        match self.told {
            Some(Seen { turns: false, .. }) => self.params as usize..self.locals,
            _ => 0..self.locals,
        }
    }

    /// Gives the stack back what the function's locals took, if they
    /// took any. Every way out of the function passes here.
    pub(super) fn leave(&mut self) {
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

    /// Pushes the word at `position` after the fixed part of this
    /// definition: a slot, or past the slots a constant.
    pub(super) fn in_definition(&mut self, position: usize) {
        self.seen.reads += 1;
        let offset = DEFINITION_SLOTS + i32::try_from(position * 8).unwrap_or(0);
        if self.told.is_some_and(Seen::keeps) {
            self.emit(&Instruction::LocalGet(self.definition));
            self.emit(&Instruction::I64Load(memory(offset)));
        } else {
            self.emit(&Instruction::LocalGet(CLOSURE));
            self.load(CLOSURE_DEFINITION);
            self.load(offset);
        }
    }
}
