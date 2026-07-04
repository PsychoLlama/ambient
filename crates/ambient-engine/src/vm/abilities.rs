//! Ability handling operations for the VM.
//!
//! This module implements the delimited-continuation semantics of handle
//! expressions:
//!
//! - `Suspend` packages a method call's arguments into a suspended ability.
//! - `Perform` executes it: bytecode handlers capture the delimited
//!   continuation and run the handler arm; host handlers run synchronously.
//! - `Handle` / `HandleWithValue` install handlers that delimit the
//!   *current frame* (the handle expression's body thunk).
//! - `Resume` reinstates a captured continuation, rebased onto the
//!   current stack and frame heights.
//!
//! The delimitation invariants:
//!
//! - A handle expression compiles its body into a thunk closure; the
//!   `Handle` instructions execute inside that thunk, so a handler's
//!   `boundary_frame_idx` is the thunk's own frame.
//! - Performing a handled ability captures `frames[boundary..]`, the value
//!   stack above the boundary frame's base pointer, and every handler
//!   entry delimiting any captured frame. The handler arm then runs *in
//!   place of the thunk call*: if it returns without resuming, its value
//!   lands exactly where the thunk's return value would have (the handle
//!   expression's completion point), and the captured computation is
//!   dropped.
//! - `resume(v)` reinstates the captured frames above the arm's own frame.
//!   When the reinstated thunk eventually returns, its value is delivered
//!   to the arm as the value of the `resume` expression (deep handler
//!   semantics: the arm observes the final result of the rest of the
//!   handled body, and its own return value remains the handle
//!   expression's result).

use std::sync::Arc;

use ambient_ability::{
    CapturedFrame, CapturedHandler, HandlerImpl, SuspendedAbility, Value, VmError,
};

use super::core::{CallFrame, HandlerFrame, Vm};

impl Vm {
    /// Handle the Suspend opcode: create a suspended ability value.
    ///
    /// Pops `arg_count` arguments from the stack and creates a `SuspendedAbility`
    /// value that can later be performed.
    pub(super) fn op_suspend(
        &mut self,
        ability_id: ambient_core::AbilityId,
        method_id: u16,
        arg_count: u8,
    ) -> Result<(), VmError> {
        let mut args = Vec::with_capacity(arg_count as usize);
        for _ in 0..arg_count {
            args.push(self.pop()?);
        }
        args.reverse();
        self.stack
            .push(Value::suspended_ability(ability_id, method_id, args));
        Ok(())
    }

    /// Handle the Perform opcode: execute a suspended ability.
    ///
    /// This is the core of the ability system. It:
    /// 1. Pops the suspended ability from the stack
    /// 2. Looks for a bytecode handler (user-installed handlers take priority)
    /// 3. Falls back to host handlers if no bytecode handler is found
    /// 4. For bytecode handlers, captures the continuation and calls the handler
    pub(super) fn op_perform(&mut self) -> Result<(), VmError> {
        let ability = match self.pop()? {
            Value::SuspendedAbility(a) => a,
            other => {
                return Err(VmError::ExpectedSuspendedAbility {
                    got: other.type_name(),
                });
            }
        };

        // Check for a bytecode handler on the handler stack
        let handler_idx = self
            .handlers
            .iter()
            .rposition(|h| h.ability_id == ability.ability_id);

        if let Some(idx) = handler_idx {
            self.perform_with_bytecode_handler(idx, ability)?;
        } else if let Some(handler) = self
            .host_handlers
            .get(&(ability.ability_id, ability.method_id))
        {
            // Fall back to host handler
            match handler(&ability) {
                Ok(result) => self.stack.push(result),
                // A host handler raising an exception behaves exactly like
                // `Exception.throw!` at the perform site: the caller's frames
                // are intact, so the nearest in-language Exception handler
                // catches it (and may even `resume` the continuation with a
                // substitute value for the failed operation).
                Err(VmError::Exception(error)) => self.raise_exception(error)?,
                Err(other) => return Err(other),
            }
        } else if ability.ability_id == ambient_core::exception::ability_id() {
            // Exception is core language semantics, not a host capability:
            // a throw with no handler in scope is an uncaught exception
            // carrying the thrown value, regardless of host registration.
            let error = ability.args.first().cloned().unwrap_or(Value::Unit);
            return Err(VmError::Exception(error));
        } else {
            return Err(VmError::UnhandledAbility {
                ability_id: ability.ability_id,
                method_id: ability.method_id,
            });
        }

        Ok(())
    }

    /// Raise a language-level exception at the current execution point.
    ///
    /// Performs `Exception.throw(error)` against the nearest in-language
    /// Exception handler, exactly as if the currently executing code had
    /// called `Exception.throw!` itself. With no handler in scope the
    /// exception is uncaught and surfaces as [`VmError::Exception`].
    pub(super) fn raise_exception(&mut self, error: Value) -> Result<(), VmError> {
        let ability_id = ambient_core::exception::ability_id();
        let Some(idx) = self
            .handlers
            .iter()
            .rposition(|h| h.ability_id == ability_id)
        else {
            return Err(VmError::Exception(error));
        };

        let throw = Arc::new(SuspendedAbility {
            ability_id,
            method_id: ambient_core::exception::METHOD_THROW,
            args: vec![error],
        });
        self.perform_with_bytecode_handler(idx, throw)
    }

    /// Perform an ability using the bytecode handler at `handler_idx`.
    ///
    /// Captures the delimited continuation (frames, stack segment, and
    /// handler entries above the fired handler's boundary), removes it
    /// from the live VM state, and calls the handler arm in its place.
    fn perform_with_bytecode_handler(
        &mut self,
        handler_idx: usize,
        ability: Arc<SuspendedAbility>,
    ) -> Result<(), VmError> {
        let fired = self.handlers[handler_idx].clone();

        // Determine the arm function (and its environment) by handler kind.
        let (arm_func, arm_captures) = match &fired.handler {
            HandlerImpl::Inline { func, captures } => (*func, captures.clone()),
            HandlerImpl::Value { handler } => match handler.get_method(ability.method_id) {
                Some(func) => (func, handler.captures.clone()),
                None => {
                    return Err(VmError::UnhandledAbility {
                        ability_id: ability.ability_id,
                        method_id: ability.method_id,
                    });
                }
            },
        };

        let boundary = fired.boundary_frame_idx;
        debug_assert!(
            boundary < self.frames.len(),
            "handler boundary out of range"
        );
        let base_stack = self.frames[boundary].bp;

        // Every handler entry delimiting a frame inside the captured region
        // travels with the continuation. Live handler boundaries are
        // monotonically non-decreasing (a live entry's boundary frame is
        // still on the frame stack, so anything installed later sits at the
        // same depth or deeper), so the group is a suffix of the stack.
        let group_start = self
            .handlers
            .iter()
            .position(|h| h.boundary_frame_idx >= boundary)
            .unwrap_or(handler_idx);

        let captured_handlers: Vec<CapturedHandler> = self
            .handlers
            .drain(group_start..)
            .map(|h| CapturedHandler {
                ability_id: h.ability_id,
                handler: h.handler,
                boundary: h.boundary_frame_idx - boundary,
            })
            .collect();

        let captured_stack = self.stack.split_off(base_stack);
        let captured_frames: Vec<CapturedFrame> = self
            .frames
            .drain(boundary..)
            .map(|f| CapturedFrame {
                function_hash: f.function.hash,
                ip: f.ip,
                bp: f.bp - base_stack,
                captures: f.captures,
            })
            .collect();

        // Call the arm in place of the boundary frame's call: its return
        // value (if it never resumes) lands exactly where the handle
        // expression's body thunk would have returned.
        let continuation = Value::continuation(captured_stack, captured_frames, captured_handlers);
        self.stack.push(continuation);
        self.stack.push(Value::SuspendedAbility(ability));
        self.push_frame_with_captures(&arm_func, 2, arm_captures)?;

        Ok(())
    }

    /// Handle the `Handle` opcode: install an inline ability handler.
    ///
    /// Pops the handler arm closure from the stack. The current frame (the
    /// handle expression's body thunk) becomes the delimitation boundary.
    pub(super) fn op_handle(&mut self, ability_id: ambient_core::AbilityId) -> Result<(), VmError> {
        let arm = match self.pop()? {
            Value::Closure(c) => c,
            other => {
                return Err(VmError::TypeError {
                    expected: "closure",
                    got: other.type_name(),
                    operation: "handle",
                });
            }
        };

        self.handlers.push(HandlerFrame {
            ability_id,
            handler: HandlerImpl::Inline {
                func: arm.function_hash,
                captures: arm.environment.clone(),
            },
            boundary_frame_idx: self.frames.len() - 1,
        });
        Ok(())
    }

    /// Handle the `HandleWithValue` opcode: install a handler from a `HandlerValue`.
    ///
    /// Pops the handler value from the stack and installs it as a handler.
    /// The `ability_id` is taken from the handler value itself.
    pub(super) fn op_handle_with_value(&mut self) -> Result<(), VmError> {
        let handler_value = match self.pop()? {
            Value::Handler(h) => h,
            other => {
                return Err(VmError::TypeError {
                    expected: "handler",
                    got: other.type_name(),
                    operation: "handle_with_value",
                });
            }
        };

        self.handlers.push(HandlerFrame {
            ability_id: handler_value.ability_id,
            handler: HandlerImpl::Value {
                handler: handler_value,
            },
            boundary_frame_idx: self.frames.len() - 1,
        });

        Ok(())
    }

    /// Handle the Resume opcode: resume a captured continuation.
    ///
    /// Reinstates the captured stack, frames, and handler entries above the
    /// current (arm) frame, rebased onto the current heights, then pushes
    /// the resume value as the result of the original perform.
    pub(super) fn op_resume(&mut self) -> Result<(), VmError> {
        let value = self.pop()?;
        let continuation = match self.pop()? {
            Value::Continuation(c) => c,
            other => {
                return Err(VmError::ExpectedContinuation {
                    got: other.type_name(),
                });
            }
        };

        // Single-shot enforcement
        if !continuation.mark_resumed() {
            return Err(VmError::ContinuationAlreadyResumed);
        }

        let base_frame = self.frames.len();
        let base_stack = self.stack.len();

        // Restore the captured stack
        self.stack.extend(continuation.stack.iter().cloned());

        // Restore the captured frames, rebasing base pointers
        for captured in &continuation.frames {
            let function = self
                .functions
                .get(&captured.function_hash)
                .ok_or(VmError::UnknownFunction(captured.function_hash))?
                .clone();

            self.frames.push(CallFrame {
                function,
                ip: captured.ip,
                bp: captured.bp + base_stack,
                captures: captured.captures.clone(),
            });
        }

        // Reinstall the captured handler entries, rebasing boundaries.
        // This preserves deep handler semantics: performs in the resumed
        // body dispatch to the same handlers as before capture.
        for captured in &continuation.handlers {
            self.handlers.push(HandlerFrame {
                ability_id: captured.ability_id,
                handler: captured.handler.clone(),
                boundary_frame_idx: captured.boundary + base_frame,
            });
        }

        // Push the resume value as the result of the original perform
        self.stack.push(value);

        Ok(())
    }

    /// Handle the `GetAbilityArg` opcode: get an argument from a suspended ability.
    pub(super) fn op_get_ability_arg(&mut self, arg_index: usize) -> Result<(), VmError> {
        let ability = match self.pop()? {
            Value::SuspendedAbility(a) => a,
            other => {
                return Err(VmError::ExpectedSuspendedAbility {
                    got: other.type_name(),
                });
            }
        };

        if arg_index >= ability.args.len() {
            return Err(VmError::AbilityArgOutOfBounds {
                index: arg_index,
                length: ability.args.len(),
            });
        }

        self.stack.push(ability.args[arg_index].clone());
        Ok(())
    }
}
