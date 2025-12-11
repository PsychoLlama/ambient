//! Core VM structures and infrastructure.

use std::collections::HashMap;
use std::sync::Arc;

use crate::bytecode::{CompiledFunction, Opcode};
use crate::value::{SuspendedAbility, Value};

use super::error::VmError;

/// Action to perform when a call frame returns.
///
/// This enables "continuation frames" for operations like `Option.map` that need
/// to call a closure and then wrap the result in an enum.
#[derive(Debug, Clone, Default)]
pub(super) enum ReturnAction {
    /// Normal return - just push the result onto the caller's stack.
    #[default]
    None,

    /// Wrap the result in `Some(result)` for `Option.map`.
    WrapSome,

    /// For `Option.and_then` - the closure returns `Option<U>`, pass through as-is.
    /// This is essentially the same as `None` but documents intent.
    PassThrough,

    /// Wrap the result in `Ok(result)` for `Result.map`.
    WrapOk,

    /// Wrap the result in `Err(result)` for `Result.map_err`.
    WrapErr,
}

/// A single stack frame representing an active function call.
#[derive(Debug, Clone)]
pub(super) struct CallFrame {
    /// The function being executed.
    pub function: Arc<CompiledFunction>,

    /// Instruction pointer (offset into bytecode).
    pub ip: usize,

    /// Base pointer into the value stack for this frame's locals.
    pub bp: usize,

    /// Captured environment for closures.
    /// Empty for regular function calls, contains captured values for closure calls.
    pub captures: Vec<Value>,

    /// Action to perform when this frame returns.
    /// Used by operations like `Option.map` that call a closure and wrap the result.
    pub return_action: ReturnAction,
}

/// The kind of handler installed in a handler frame.
#[derive(Debug, Clone)]
pub(super) enum HandlerKind {
    /// An inline handler with a single function for all methods.
    /// The function receives (continuation, `suspended_ability`) and must dispatch by method.
    Inline { handler_func: blake3::Hash },

    /// A handler value with separate functions for each method.
    /// When an ability is performed, the `method_id` is used to look up the function.
    Value {
        handler_value: Arc<crate::value::HandlerValue>,
    },
}

/// An installed ability handler that can intercept ability operations.
#[derive(Debug, Clone)]
pub(super) struct HandlerFrame {
    /// The ability ID this handler handles.
    pub ability_id: u16,

    /// The handler implementation (inline function or handler value).
    pub handler: HandlerKind,

    /// The call frame index where this handler was installed.
    pub call_frame_idx: usize,

    /// The stack height when the handler was installed.
    pub stack_height: usize,
}

/// A host-provided ability handler callback.
///
/// Must be Send + Sync to allow the VM to be used across threads.
pub type HostHandler = Box<dyn Fn(&SuspendedAbility) -> Result<Value, VmError> + Send + Sync>;

/// The Ambient virtual machine.
pub struct Vm {
    /// The value stack.
    pub(super) stack: Vec<Value>,

    /// The call stack.
    pub(super) frames: Vec<CallFrame>,

    /// The handler stack for installed ability handlers.
    pub(super) handlers: Vec<HandlerFrame>,

    /// Host-provided ability handlers (for abilities like Console, Filesystem).
    /// Maps `(ability_id, method_id)` to handler functions.
    pub(super) host_handlers: HashMap<(u16, u16), HostHandler>,

    /// Content-addressed function store.
    pub(super) functions: HashMap<blake3::Hash, Arc<CompiledFunction>>,

    /// Maximum call stack depth to prevent infinite recursion.
    pub(super) max_call_depth: usize,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    /// Create a new VM instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            handlers: Vec::with_capacity(16),
            host_handlers: HashMap::new(),
            functions: HashMap::new(),
            max_call_depth: 1000,
        }
    }

    /// Load a compiled function into the VM.
    pub fn load_function(&mut self, func: CompiledFunction) {
        let hash = func.hash;
        self.functions.insert(hash, Arc::new(func));
    }

    /// Register a host-provided ability handler.
    ///
    /// Host handlers are called synchronously when an ability is performed
    /// and no bytecode handler is installed.
    pub fn register_host_handler(&mut self, ability_id: u16, method_id: u16, handler: HostHandler) {
        self.host_handlers.insert((ability_id, method_id), handler);
    }

    /// Call a function by its hash with the given arguments.
    pub fn call(&mut self, hash: &blake3::Hash, args: Vec<Value>) -> Result<Value, VmError> {
        // Reset state
        self.stack.clear();
        self.frames.clear();
        self.handlers.clear();

        let arg_count = args.len() as u8;

        // Push arguments onto stack
        for arg in args {
            self.stack.push(arg);
        }

        // Set up initial call frame
        self.push_frame(hash, arg_count)?;

        // Run the execution loop
        self.run()
    }

    /// Call a function and return a `RuntimeError` with stack trace on failure.
    ///
    /// This is the preferred method for calling functions when you want
    /// rich error messages with source locations.
    pub fn call_with_trace(
        &mut self,
        hash: &blake3::Hash,
        args: Vec<Value>,
    ) -> Result<Value, super::error::RuntimeError> {
        self.call(hash, args).map_err(|e| self.runtime_error(e))
    }

    /// Push a new call frame for the given function.
    pub(super) fn push_frame(&mut self, hash: &blake3::Hash, arg_count: u8) -> Result<(), VmError> {
        self.push_frame_with_captures(hash, arg_count, Vec::new())
    }

    /// Push a new call frame for a closure call with captured environment.
    pub(super) fn push_frame_with_captures(
        &mut self,
        hash: &blake3::Hash,
        arg_count: u8,
        captures: Vec<Value>,
    ) -> Result<(), VmError> {
        if self.frames.len() >= self.max_call_depth {
            return Err(VmError::StackOverflow);
        }

        let function = self
            .functions
            .get(hash)
            .ok_or(VmError::UnknownFunction(*hash))?
            .clone();

        if arg_count != function.param_count {
            return Err(VmError::ArityMismatch {
                expected: function.param_count,
                got: arg_count,
            });
        }

        // Base pointer is where arguments start on the stack
        let bp = self.stack.len() - arg_count as usize;

        // Reserve space for locals (args are already there, just need remaining slots)
        let extra_locals = function.local_count as usize - arg_count as usize;
        for _ in 0..extra_locals {
            self.stack.push(Value::Unit);
        }

        self.frames.push(CallFrame {
            function,
            ip: 0,
            bp,
            captures,
            return_action: ReturnAction::None,
        });

        Ok(())
    }

    /// Push a new call frame for a closure call with a return action.
    ///
    /// The return action specifies what to do with the closure's return value,
    /// e.g., wrap it in `Some` for `Option.map`.
    pub(super) fn push_frame_with_return_action(
        &mut self,
        hash: &blake3::Hash,
        arg_count: u8,
        captures: Vec<Value>,
        return_action: ReturnAction,
    ) -> Result<(), VmError> {
        if self.frames.len() >= self.max_call_depth {
            return Err(VmError::StackOverflow);
        }

        let function = self
            .functions
            .get(hash)
            .ok_or(VmError::UnknownFunction(*hash))?
            .clone();

        if arg_count != function.param_count {
            return Err(VmError::ArityMismatch {
                expected: function.param_count,
                got: arg_count,
            });
        }

        // Base pointer is where arguments start on the stack
        let bp = self.stack.len() - arg_count as usize;

        // Reserve space for locals (args are already there, just need remaining slots)
        let extra_locals = function.local_count as usize - arg_count as usize;
        for _ in 0..extra_locals {
            self.stack.push(Value::Unit);
        }

        self.frames.push(CallFrame {
            function,
            ip: 0,
            bp,
            captures,
            return_action,
        });

        Ok(())
    }

    /// Perform a single suspended ability using a host handler.
    ///
    /// Returns an error if no host handler is registered for this ability.
    /// Note: This does not support bytecode handlers - only host handlers.
    pub(super) fn perform_ability_host(
        &self,
        ability: &SuspendedAbility,
    ) -> Result<Value, VmError> {
        if let Some(handler) = self
            .host_handlers
            .get(&(ability.ability_id, ability.method_id))
        {
            handler(ability)
        } else {
            Err(VmError::UnhandledAbility {
                ability_id: ability.ability_id,
                method_id: ability.method_id,
            })
        }
    }

    /// Perform all abilities concurrently and collect results.
    ///
    /// Uses `std::thread::scope` for safe parallelism. Each ability is executed
    /// in its own thread, and results are collected in order.
    pub(super) fn perform_all_abilities(
        &self,
        abilities: &[Arc<SuspendedAbility>],
    ) -> Result<Vec<Value>, VmError> {
        if abilities.is_empty() {
            return Ok(Vec::new());
        }

        // For a single ability, no need for threading overhead
        if abilities.len() == 1 {
            return Ok(vec![self.perform_ability_host(&abilities[0])?]);
        }

        // Use thread::scope for safe concurrent execution
        std::thread::scope(|s| {
            // Spawn a thread for each ability
            let handles: Vec<_> = abilities
                .iter()
                .map(|ability| s.spawn(|| self.perform_ability_host(ability)))
                .collect();

            // Collect results in order
            let mut results = Vec::with_capacity(handles.len());
            for handle in handles {
                let result = handle.join().map_err(|_| VmError::StackOverflow)?;
                results.push(result?);
            }
            Ok(results)
        })
    }

    /// Race abilities concurrently and return the first result.
    ///
    /// Uses `std::thread::scope` with channels for true racing. The first
    /// ability to complete wins, and its result is returned. Other threads
    /// continue to completion but their results are discarded.
    ///
    /// Note: True cancellation would require cooperative cancellation tokens
    /// in the ability handlers. For now, we let threads complete but only
    /// use the first result.
    pub(super) fn perform_race_abilities(
        &self,
        abilities: &[Arc<SuspendedAbility>],
    ) -> Result<Value, VmError> {
        if abilities.is_empty() {
            return Err(VmError::StackUnderflow);
        }

        // For a single ability, no need for threading overhead
        if abilities.len() == 1 {
            return self.perform_ability_host(&abilities[0]);
        }

        // Use a channel to receive the first result
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::scope(|s| {
            // Spawn a thread for each ability
            for (idx, ability) in abilities.iter().enumerate() {
                let tx = tx.clone();
                s.spawn(move || {
                    let result = self.perform_ability_host(ability);
                    // Send result with index (for debugging/ordering if needed)
                    let _ = tx.send((idx, result));
                });
            }

            // Drop our sender so the channel closes when all threads complete
            drop(tx);

            // Return the first result we receive
            match rx.recv() {
                Ok((_idx, result)) => result,
                Err(_) => Err(VmError::StackUnderflow), // All threads failed to send
            }
        })
    }

    /// Fetch the next opcode from the current frame's bytecode.
    pub(super) fn fetch_opcode(&mut self) -> Result<Opcode, VmError> {
        let frame = self.current_frame_mut()?;
        if frame.ip >= frame.function.bytecode.len() {
            return Err(VmError::InstructionOutOfBounds);
        }
        let byte = frame.function.bytecode[frame.ip];
        frame.ip += 1;
        Opcode::from_byte(byte).ok_or(VmError::InvalidOpcode(byte))
    }

    /// Read a u8 operand from the bytecode.
    pub(super) fn read_u8(&mut self) -> Result<u8, VmError> {
        let frame = self.current_frame_mut()?;
        if frame.ip >= frame.function.bytecode.len() {
            return Err(VmError::InstructionOutOfBounds);
        }
        let byte = frame.function.bytecode[frame.ip];
        frame.ip += 1;
        Ok(byte)
    }

    /// Read a u16 operand from the bytecode (little-endian).
    pub(super) fn read_u16(&mut self) -> Result<u16, VmError> {
        let frame = self.current_frame_mut()?;
        if frame.ip + 1 >= frame.function.bytecode.len() {
            return Err(VmError::InstructionOutOfBounds);
        }
        let lo = frame.function.bytecode[frame.ip];
        let hi = frame.function.bytecode[frame.ip + 1];
        frame.ip += 2;
        Ok(u16::from_le_bytes([lo, hi]))
    }

    /// Read an i16 operand from the bytecode (little-endian).
    pub(super) fn read_i16(&mut self) -> Result<i16, VmError> {
        let frame = self.current_frame_mut()?;
        if frame.ip + 1 >= frame.function.bytecode.len() {
            return Err(VmError::InstructionOutOfBounds);
        }
        let lo = frame.function.bytecode[frame.ip];
        let hi = frame.function.bytecode[frame.ip + 1];
        frame.ip += 2;
        Ok(i16::from_le_bytes([lo, hi]))
    }

    /// Get the current call frame.
    pub(super) fn current_frame(&self) -> Result<&CallFrame, VmError> {
        self.frames.last().ok_or(VmError::StackUnderflow)
    }

    /// Get the current call frame mutably.
    pub(super) fn current_frame_mut(&mut self) -> Result<&mut CallFrame, VmError> {
        self.frames.last_mut().ok_or(VmError::StackUnderflow)
    }

    /// Get a constant from the current function's constant pool.
    pub(super) fn get_constant(&self, idx: u16) -> Result<Value, VmError> {
        let frame = self.current_frame()?;
        frame
            .function
            .constants
            .get(idx as usize)
            .cloned()
            .ok_or(VmError::InvalidConstant(idx))
    }

    /// Get a local variable from the current frame.
    pub(super) fn get_local(&self, slot: u16) -> Result<Value, VmError> {
        let frame = self.current_frame()?;
        let idx = frame.bp + slot as usize;
        self.stack
            .get(idx)
            .cloned()
            .ok_or(VmError::InvalidLocal(slot))
    }

    /// Set a local variable in the current frame.
    pub(super) fn set_local(&mut self, slot: u16, value: Value) -> Result<(), VmError> {
        let frame = self.current_frame()?;
        let idx = frame.bp + slot as usize;
        if idx >= self.stack.len() {
            return Err(VmError::InvalidLocal(slot));
        }
        self.stack[idx] = value;
        Ok(())
    }

    /// Pop a value from the stack.
    pub(super) fn pop(&mut self) -> Result<Value, VmError> {
        self.stack.pop().ok_or(VmError::StackUnderflow)
    }

    /// Peek at the top of the stack without popping.
    pub(super) fn peek(&self) -> Result<&Value, VmError> {
        self.stack.last().ok_or(VmError::StackUnderflow)
    }

    /// Pop a typed value from the stack or return a type error.
    ///
    /// This is the generic implementation used by the type-specific pop methods.
    fn pop_typed<T>(
        &mut self,
        expected: &'static str,
        extract: impl FnOnce(Value) -> Option<T>,
        operation: &'static str,
    ) -> Result<T, VmError> {
        let value = self.pop()?;
        extract(value.clone()).ok_or_else(|| VmError::TypeError {
            expected,
            got: value.type_name(),
            operation,
        })
    }

    /// Pop a number from the stack or return a type error.
    pub(super) fn pop_number(&mut self, operation: &'static str) -> Result<f64, VmError> {
        self.pop_typed("number", |v| v.as_number(), operation)
    }

    /// Pop a bool from the stack or return a type error.
    pub(super) fn pop_bool(&mut self, operation: &'static str) -> Result<bool, VmError> {
        self.pop_typed("bool", |v| v.as_bool(), operation)
    }

    /// Pop a string from the stack or return a type error.
    pub(super) fn pop_string(&mut self, operation: &'static str) -> Result<Arc<String>, VmError> {
        self.pop_typed("string", Value::into_string, operation)
    }

    /// Execute a unary operation on a number.
    pub(super) fn unary_number_op(
        &mut self,
        op: impl FnOnce(f64) -> f64,
        name: &'static str,
    ) -> Result<(), VmError> {
        let n = self.pop_number(name)?;
        self.stack.push(Value::Number(op(n)));
        Ok(())
    }

    /// Execute a binary operation on numbers.
    pub(super) fn binary_number_op(
        &mut self,
        op: impl FnOnce(f64, f64) -> f64,
        name: &'static str,
    ) -> Result<(), VmError> {
        let b = self.pop_number(name)?;
        let a = self.pop_number(name)?;
        self.stack.push(Value::Number(op(a, b)));
        Ok(())
    }

    /// Execute a comparison operation on numbers.
    pub(super) fn comparison_op(
        &mut self,
        op: impl FnOnce(f64, f64) -> bool,
        name: &'static str,
    ) -> Result<(), VmError> {
        let b = self.pop_number(name)?;
        let a = self.pop_number(name)?;
        self.stack.push(Value::Bool(op(a, b)));
        Ok(())
    }

    /// Jump relative to current instruction pointer.
    pub(super) fn jump_relative(&mut self, offset: i16) -> Result<(), VmError> {
        let frame = self.current_frame_mut()?;
        let new_ip = frame.ip as isize + offset as isize;
        if new_ip < 0 || new_ip > frame.function.bytecode.len() as isize {
            return Err(VmError::InstructionOutOfBounds);
        }
        frame.ip = new_ip as usize;
        Ok(())
    }

    /// Capture the current call stack as a stack trace.
    ///
    /// This collects information from all active call frames, using debug info
    /// when available to provide source locations.
    pub fn capture_stack_trace(&self) -> Vec<super::error::StackTraceFrame> {
        self.frames
            .iter()
            .rev() // Most recent frame first
            .map(|frame| {
                let function = &frame.function;
                let bytecode_offset = frame.ip.saturating_sub(1); // Point to the instruction that caused the error

                // Try to get source location from debug info
                let (source_file, function_name, line, column) =
                    if let Some(ref debug_info) = function.debug_info {
                        let mapping = debug_info.find_source_location(bytecode_offset);
                        (
                            debug_info.source_file.clone(),
                            debug_info.function_name.clone(),
                            mapping.map(|m| m.line),
                            mapping.map(|m| m.column),
                        )
                    } else {
                        (None, None, None, None)
                    };

                super::error::StackTraceFrame {
                    function_name,
                    source_file,
                    line,
                    column,
                    function_hash: function.hash,
                    bytecode_offset,
                }
            })
            .collect()
    }

    /// Create a `RuntimeError` with the current stack trace.
    pub fn runtime_error(&self, error: VmError) -> super::error::RuntimeError {
        super::error::RuntimeError::with_stack_trace(error, self.capture_stack_trace())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Option/Result closure helpers
    // ─────────────────────────────────────────────────────────────────────────

    /// Pop a closure from the stack, returning a type error if not a closure.
    pub(super) fn pop_closure(
        &mut self,
        operation: &'static str,
    ) -> Result<Arc<crate::value::Closure>, VmError> {
        match self.pop()? {
            Value::Closure(c) => Ok(c),
            other => Err(VmError::TypeError {
                expected: "closure",
                got: other.type_name(),
                operation,
            }),
        }
    }

    /// Pop an enum value from the stack and check it matches the expected type.
    pub(super) fn pop_enum(
        &mut self,
        expected_type: &'static str,
        operation: &'static str,
    ) -> Result<Arc<crate::value::EnumValue>, VmError> {
        match self.pop()? {
            Value::Enum(e) if &*e.type_name == expected_type => Ok(e),
            other => Err(VmError::TypeError {
                expected: expected_type,
                got: other.type_name(),
                operation,
            }),
        }
    }

    /// Apply a closure to an enum variant's payload and set up the return action.
    ///
    /// This is used by `Option.map`, `Option.and_then`, `Result.map`, etc.
    /// - If the enum has the matching tag, calls the closure with the payload
    /// - Otherwise, pushes the fallback value to the stack
    ///
    /// Returns `true` if a call frame was pushed (closure is being called),
    /// `false` if the fallback value was pushed instead.
    pub(super) fn apply_closure_to_enum(
        &mut self,
        closure: &crate::value::Closure,
        enum_val: Arc<crate::value::EnumValue>,
        active_tag: u16,
        return_action: ReturnAction,
        type_name: &str,
        variant_name: &str,
    ) -> Result<bool, VmError> {
        if enum_val.tag == active_tag {
            // Active variant - extract payload and call closure
            let payload =
                enum_val
                    .payload
                    .as_deref()
                    .ok_or_else(|| VmError::EnumPayloadMissing {
                        type_name: type_name.to_string(),
                        variant_name: variant_name.to_string(),
                    })?;
            self.stack.push(payload.clone());
            self.push_frame_with_return_action(
                &closure.function_hash,
                1,
                closure.environment.clone(),
                return_action,
            )?;
            Ok(true)
        } else {
            // Inactive variant - return unchanged
            self.stack.push(Value::Enum(enum_val));
            Ok(false)
        }
    }
}
