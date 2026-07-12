//! Tail-call operations for the VM.
//!
//! A tail call reuses the current call frame instead of pushing a new one,
//! so a chain of tail calls runs in constant call-stack space and never
//! trips [`crate::vm::VmError::StackOverflow`]. This is the runtime half of
//! tail-call support; a later compiler phase decides where to emit the
//! `TailCall` / `TailCallClosure` opcodes.
//!
//! Frame reuse and effects: reusing a frame deliberately leaves the handler
//! stack (`handlers`) and the invoke barriers (`handler_barriers`)
//! untouched. A handler whose `boundary_frame_idx` is the reused frame stays
//! installed — the callee is the remainder of that delimited region
//! (dynamic-extent semantics). Continuation capture and resume key only off
//! frame indices and base pointers, both preserved across a reuse, so
//! effects compose with tail calls without any special handling here.
//!
//! Tail-calling a native is special: a native pushes no frame, so a tail
//! call to one is call-then-return. The native runs exactly as it would
//! under `Call`, and on a plain return its value unwinds the current frame
//! (as `Call native; Return` would); a raised exception or host interrupt
//! is delivered at this site, with no return sequence — matching a called
//! native that raises.

use std::sync::Arc;

use ambient_ability::{Value, VmError};

use crate::bytecode::CompiledFunction;

use super::core::Vm;

impl Vm {
    /// Execute a `TailCall` to `hash` with `arg_count` arguments already on
    /// the stack. Returns `Some(value)` when the reused frame was the entry
    /// frame of the enclosing `run_until` region and a native tail call
    /// unwound it (exit the loop with that value), `None` to keep executing.
    pub(super) fn op_tail_call(
        &mut self,
        hash: &blake3::Hash,
        arg_count: u8,
        base_frames: usize,
    ) -> Result<Option<Value>, VmError> {
        // A native pushes no frame: tail-calling one is call-then-return.
        if let Some(&(uuid, param_count)) = self.native_functions.get(hash) {
            return self.tail_call_native(uuid, param_count, arg_count, base_frames);
        }

        let function = self
            .functions
            .get(hash)
            .ok_or(VmError::UnknownFunction(*hash))?
            .clone();
        self.reuse_frame(function, arg_count, Vec::new())?;
        Ok(None)
    }

    /// Execute a `TailCallClosure`: the callee sits just below its
    /// arguments on the stack (`[callee, arg1..argN]`). The callee may be a
    /// `Closure` or a bare `FunctionRef`. Return contract matches
    /// [`Self::op_tail_call`].
    pub(super) fn op_tail_call_closure(
        &mut self,
        arg_count: u8,
        base_frames: usize,
    ) -> Result<Option<Value>, VmError> {
        // Lift the callee out from under the arguments; the remaining top
        // `arg_count` slots are the arguments, exactly as a plain `TailCall`
        // expects them.
        let callee_idx = self
            .stack
            .len()
            .checked_sub(arg_count as usize + 1)
            .ok_or(VmError::StackUnderflow)?;
        let (hash, captures) = match self.stack.remove(callee_idx) {
            Value::Closure(c) => (c.function_hash, c.environment.clone()),
            Value::FunctionRef(hash) => (hash, Vec::new()),
            other => {
                return Err(VmError::TypeError {
                    expected: "closure",
                    got: other.type_name(),
                    operation: "tail_call_closure",
                });
            }
        };

        if let Some(&(uuid, param_count)) = self.native_functions.get(&hash) {
            // A bare function ref may name a native; it has no environment.
            return self.tail_call_native(uuid, param_count, arg_count, base_frames);
        }

        let function = self
            .functions
            .get(&hash)
            .ok_or(VmError::UnknownFunction(hash))?
            .clone();
        self.reuse_frame(function, arg_count, captures)?;
        Ok(None)
    }

    /// Rewrite the current call frame in place to run `function`, reusing
    /// its stack region. The `arg_count` arguments occupying the top of the
    /// value stack become the callee's leading locals; everything between
    /// the frame's base pointer and those arguments (the old locals, and —
    /// for a closure tail call — the already-removed callee slot) is
    /// discarded. No depth check and no new frame: the frame count is
    /// unchanged.
    fn reuse_frame(
        &mut self,
        function: Arc<CompiledFunction>,
        arg_count: u8,
        captures: Vec<Value>,
    ) -> Result<(), VmError> {
        if arg_count != function.param_count {
            return Err(VmError::ArityMismatch {
                expected: function.param_count,
                got: arg_count,
            });
        }

        let bp = self.current_frame()?.bp;

        // Slide the arguments down onto the frame's base pointer, dropping
        // the region in between. Works in both directions: the arguments may
        // sit above a larger local footprint (shift down) or a smaller one.
        let args_start = self.stack.len() - arg_count as usize;
        self.stack.drain(bp..args_start);
        debug_assert_eq!(
            self.stack.len(),
            bp + arg_count as usize,
            "arguments must land exactly on the frame base"
        );

        // Reserve the callee's remaining local slots, mirroring
        // `push_frame_with_captures`.
        let extra_locals = function.local_count as usize - arg_count as usize;
        for _ in 0..extra_locals {
            self.stack.push(Value::Unit);
        }

        // Reuse the frame: same base pointer, fresh function/ip/captures.
        // The handler stack and invoke barriers are intentionally left as
        // they are (see module docs).
        let frame = self.current_frame_mut()?;
        frame.function = function;
        frame.ip = 0;
        frame.captures = captures;
        Ok(())
    }

    /// Tail-call a native: run it as `Call` would, then unwind. On a plain
    /// return the value unwinds the current frame; a raised exception or
    /// host interrupt is delivered at this site with no unwind, exactly as a
    /// *called* native that raises would behave.
    fn tail_call_native(
        &mut self,
        uuid: uuid::Uuid,
        param_count: u8,
        arg_count: u8,
        base_frames: usize,
    ) -> Result<Option<Value>, VmError> {
        match self.run_native(uuid, param_count, arg_count)? {
            Ok(value) => self.unwind_frame(value, base_frames),
            // The exception/interrupt paths mirror `finish_native_call`, but
            // fire at this tail site and run no return sequence: a fired
            // handler drains the current frame into its continuation, and an
            // uncaught fault propagates.
            Err(VmError::Exception(error)) => {
                self.raise_exception(error)?;
                Ok(None)
            }
            Err(VmError::Interrupted { ability_id, method }) => {
                self.deliver_interrupt(ability_id, method)?;
                Ok(None)
            }
            Err(other) => Err(other),
        }
    }
}
