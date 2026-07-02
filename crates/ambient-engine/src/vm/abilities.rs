//! Ability handling operations for the VM.
//!
//! This module contains the complex logic for handling ability operations:
//! - Suspend: Creates a suspended ability value
//! - Perform: Executes an ability, finding handlers and capturing continuations
//! - Handle: Installs an ability handler
//! - Resume: Resumes a captured continuation
//!
//! Extracted from dispatch.rs for better code organization.

use std::sync::Arc;

use ambient_ability::{CapturedFrame, SuspendedAbility, Value, VmError};
use ambient_core::AbilityId;

use super::core::{CallFrame, HandlerFrame, HandlerKind, ReturnAction, Vm};

impl Vm {
    /// Handle the Suspend opcode: create a suspended ability value.
    ///
    /// Pops `arg_count` arguments from the stack and creates a `SuspendedAbility`
    /// value that can later be performed.
    pub(super) fn op_suspend(
        &mut self,
        ability_id: AbilityId,
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
                })
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

    /// Perform an ability using a bytecode handler.
    ///
    /// Captures the continuation (stack and frames from handler point to current)
    /// and calls the handler function with the continuation and suspended ability.
    fn perform_with_bytecode_handler(
        &mut self,
        handler_idx: usize,
        ability: Arc<SuspendedAbility>,
    ) -> Result<(), VmError> {
        let handler = self.handlers[handler_idx].clone();

        // Determine the handler function based on handler kind
        let handler_func = match &handler.handler {
            HandlerKind::Inline { handler_func } => *handler_func,
            HandlerKind::Value { handler_value } => {
                match handler_value.get_method(ability.method_id) {
                    Some(func) => func,
                    None => {
                        return Err(VmError::UnhandledAbility {
                            ability_id: ability.ability_id,
                            method_id: ability.method_id,
                        });
                    }
                }
            }
        };

        // Capture the continuation
        let captured_stack = self.stack.split_off(handler.stack_height);
        let captured_frames: Vec<CapturedFrame> = self.frames[handler.call_frame_idx..]
            .iter()
            .map(|f| CapturedFrame {
                function_hash: f.function.hash,
                ip: f.ip,
                bp: f.bp,
            })
            .collect();

        // Truncate frames and handlers
        self.frames.truncate(handler.call_frame_idx);
        self.handlers.truncate(handler_idx);

        // Create continuation and push arguments for handler
        let continuation = Value::continuation(captured_stack, captured_frames);
        self.stack.push(continuation);
        self.stack.push(Value::SuspendedAbility(ability));

        // Call the handler function
        self.push_frame(&handler_func, 2)?;

        Ok(())
    }

    /// Handle the `Handle` opcode: install an inline ability handler.
    pub(super) fn op_handle(&mut self, ability_id: AbilityId, handler_func: blake3::Hash) {
        self.handlers.push(HandlerFrame {
            ability_id,
            handler: HandlerKind::Inline { handler_func },
            call_frame_idx: self.frames.len() - 1,
            stack_height: self.stack.len(),
        });
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
                })
            }
        };

        self.handlers.push(HandlerFrame {
            ability_id: handler_value.ability_id,
            handler: HandlerKind::Value { handler_value },
            call_frame_idx: self.frames.len() - 1,
            stack_height: self.stack.len(),
        });

        Ok(())
    }

    /// Handle the Resume opcode: resume a captured continuation.
    ///
    /// Restores the captured stack and frames, then pushes the resume value.
    pub(super) fn op_resume(&mut self) -> Result<(), VmError> {
        let value = self.pop()?;
        let continuation = match self.pop()? {
            Value::Continuation(c) => c,
            other => {
                return Err(VmError::ExpectedContinuation {
                    got: other.type_name(),
                })
            }
        };

        // Single-shot enforcement
        if !continuation.mark_resumed() {
            return Err(VmError::ContinuationAlreadyResumed);
        }

        // Restore the captured stack
        self.stack.extend(continuation.stack.iter().cloned());

        // Restore the captured frames
        for captured in &continuation.frames {
            let function = self
                .functions
                .get(&captured.function_hash)
                .ok_or(VmError::UnknownFunction(captured.function_hash))?
                .clone();

            self.frames.push(CallFrame {
                function,
                ip: captured.ip,
                bp: captured.bp,
                captures: Vec::new(),
                return_action: ReturnAction::None,
            });
        }

        // Push the resume value
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
                })
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
