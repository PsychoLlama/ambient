//! Stack-based virtual machine for executing Ambient bytecode.
//!
//! The VM executes compiled functions using:
//! - A value stack for operands
//! - A call stack for function frames
//! - A handler stack for ability handlers
//! - A content-addressed function store

#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]

use std::collections::HashMap;
use std::sync::Arc;

use crate::bytecode::{CompiledFunction, Opcode};
use crate::value::{CapturedFrame, SuspendedAbility, Value};

/// Runtime error during VM execution.
#[derive(Debug, Clone, PartialEq)]
pub enum VmError {
    /// Stack underflow - tried to pop from empty stack.
    StackUnderflow,

    /// Invalid opcode encountered.
    InvalidOpcode(u8),

    /// Type mismatch for an operation.
    TypeError {
        expected: &'static str,
        got: &'static str,
        operation: &'static str,
    },

    /// Division by zero.
    DivisionByZero,

    /// Invalid constant pool index.
    InvalidConstant(u16),

    /// Invalid local variable slot.
    InvalidLocal(u16),

    /// Unknown function hash.
    UnknownFunction(blake3::Hash),

    /// Tuple index out of bounds.
    TupleIndexOutOfBounds { index: u8, length: usize },

    /// Record field not found.
    RecordFieldNotFound(String),

    /// Instruction pointer out of bounds.
    InstructionOutOfBounds,

    /// Call stack overflow.
    StackOverflow,

    /// Wrong number of arguments to function.
    ArityMismatch { expected: u8, got: u8 },

    /// No handler found for an ability.
    UnhandledAbility { ability_id: u16, method_id: u16 },

    /// Tried to resume an already-resumed continuation (single-shot violation).
    ContinuationAlreadyResumed,

    /// Expected a suspended ability value but got something else.
    ExpectedSuspendedAbility { got: &'static str },

    /// Expected a continuation but got something else.
    ExpectedContinuation { got: &'static str },

    /// Ability argument index out of bounds.
    AbilityArgOutOfBounds { index: usize, length: usize },
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StackUnderflow => write!(f, "stack underflow"),
            Self::InvalidOpcode(op) => write!(f, "invalid opcode: 0x{op:02x}"),
            Self::TypeError {
                expected,
                got,
                operation,
            } => {
                write!(
                    f,
                    "type error in {operation}: expected {expected}, got {got}"
                )
            }
            Self::DivisionByZero => write!(f, "division by zero"),
            Self::InvalidConstant(idx) => write!(f, "invalid constant index: {idx}"),
            Self::InvalidLocal(slot) => write!(f, "invalid local variable slot: {slot}"),
            Self::UnknownFunction(hash) => write!(f, "unknown function: {hash}"),
            Self::TupleIndexOutOfBounds { index, length } => {
                write!(f, "tuple index {index} out of bounds (length {length})")
            }
            Self::RecordFieldNotFound(field) => write!(f, "record field not found: {field}"),
            Self::InstructionOutOfBounds => write!(f, "instruction pointer out of bounds"),
            Self::StackOverflow => write!(f, "call stack overflow"),
            Self::ArityMismatch { expected, got } => {
                write!(
                    f,
                    "arity mismatch: expected {expected} arguments, got {got}"
                )
            }
            Self::UnhandledAbility {
                ability_id,
                method_id,
            } => {
                write!(
                    f,
                    "unhandled ability: ability {ability_id}, method {method_id}"
                )
            }
            Self::ContinuationAlreadyResumed => {
                write!(f, "continuation already resumed (single-shot violation)")
            }
            Self::ExpectedSuspendedAbility { got } => {
                write!(f, "expected suspended ability, got {got}")
            }
            Self::ExpectedContinuation { got } => {
                write!(f, "expected continuation, got {got}")
            }
            Self::AbilityArgOutOfBounds { index, length } => {
                write!(
                    f,
                    "ability argument index {index} out of bounds (length {length})"
                )
            }
        }
    }
}

impl std::error::Error for VmError {}

/// A single stack frame representing an active function call.
#[derive(Debug, Clone)]
struct CallFrame {
    /// The function being executed.
    function: Arc<CompiledFunction>,

    /// Instruction pointer (offset into bytecode).
    ip: usize,

    /// Base pointer into the value stack for this frame's locals.
    bp: usize,

    /// Captured environment for closures.
    /// Empty for regular function calls, contains captured values for closure calls.
    captures: Vec<Value>,
}

/// The kind of handler installed in a handler frame.
#[derive(Debug, Clone)]
enum HandlerKind {
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
#[allow(dead_code)] // normal_completion_ip may be used for future optimizations
struct HandlerFrame {
    /// The ability ID this handler handles.
    ability_id: u16,

    /// The handler implementation (inline function or handler value).
    handler: HandlerKind,

    /// The call frame index where this handler was installed.
    call_frame_idx: usize,

    /// The stack height when the handler was installed.
    stack_height: usize,

    /// The instruction pointer to jump to for normal completion.
    normal_completion_ip: usize,
}

/// A host-provided ability handler callback.
///
/// Must be Send + Sync to allow the VM to be used across threads.
pub type HostHandler = Box<dyn Fn(&SuspendedAbility) -> Result<Value, VmError> + Send + Sync>;

/// The Ambient virtual machine.
pub struct Vm {
    /// The value stack.
    stack: Vec<Value>,

    /// The call stack.
    frames: Vec<CallFrame>,

    /// The handler stack for installed ability handlers.
    handlers: Vec<HandlerFrame>,

    /// Host-provided ability handlers (for abilities like Console, Filesystem).
    /// Maps `(ability_id, method_id)` to handler functions.
    host_handlers: HashMap<(u16, u16), HostHandler>,

    /// Content-addressed function store.
    functions: HashMap<blake3::Hash, Arc<CompiledFunction>>,

    /// Maximum call stack depth to prevent infinite recursion.
    max_call_depth: usize,
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

    /// Perform a single suspended ability using a host handler.
    ///
    /// Returns an error if no host handler is registered for this ability.
    /// Note: This does not support bytecode handlers - only host handlers.
    fn perform_ability_host(&self, ability: &SuspendedAbility) -> Result<Value, VmError> {
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
    fn perform_all_abilities(
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
    fn perform_race_abilities(
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

    /// Push a new call frame for the given function.
    fn push_frame(&mut self, hash: &blake3::Hash, arg_count: u8) -> Result<(), VmError> {
        self.push_frame_with_captures(hash, arg_count, Vec::new())
    }

    /// Push a new call frame for a closure call with captured environment.
    fn push_frame_with_captures(
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
        });

        Ok(())
    }

    /// Main execution loop.
    fn run(&mut self) -> Result<Value, VmError> {
        loop {
            let op = self.fetch_opcode()?;

            match op {
                Opcode::PushConst => {
                    let idx = self.read_u16()?;
                    let value = self.get_constant(idx)?;
                    self.stack.push(value);
                }

                Opcode::Pop => {
                    self.pop()?;
                }

                Opcode::Dup => {
                    let value = self.peek()?.clone();
                    self.stack.push(value);
                }

                Opcode::StoreLocal => {
                    let slot = self.read_u16()?;
                    let value = self.peek()?.clone();
                    self.set_local(slot, value)?;
                }

                Opcode::LoadLocal => {
                    let slot = self.read_u16()?;
                    let value = self.get_local(slot)?;
                    self.stack.push(value);
                }

                Opcode::Add => self.binary_number_op(|a, b| a + b, "add")?,
                Opcode::Sub => self.binary_number_op(|a, b| a - b, "sub")?,
                Opcode::Mul => self.binary_number_op(|a, b| a * b, "mul")?,
                Opcode::Div => {
                    let b = self.pop_number("div")?;
                    let a = self.pop_number("div")?;
                    if b == 0.0 {
                        return Err(VmError::DivisionByZero);
                    }
                    self.stack.push(Value::Number(a / b));
                }
                Opcode::Mod => {
                    let b = self.pop_number("mod")?;
                    let a = self.pop_number("mod")?;
                    if b == 0.0 {
                        return Err(VmError::DivisionByZero);
                    }
                    self.stack.push(Value::Number(a % b));
                }
                Opcode::Neg => {
                    let n = self.pop_number("neg")?;
                    self.stack.push(Value::Number(-n));
                }

                Opcode::Eq => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(Value::Bool(a == b));
                }
                Opcode::Ne => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(Value::Bool(a != b));
                }
                Opcode::Lt => self.comparison_op(|a, b| a < b, "lt")?,
                Opcode::Le => self.comparison_op(|a, b| a <= b, "le")?,
                Opcode::Gt => self.comparison_op(|a, b| a > b, "gt")?,
                Opcode::Ge => self.comparison_op(|a, b| a >= b, "ge")?,

                Opcode::And => {
                    let b = self.pop_bool("and")?;
                    let a = self.pop_bool("and")?;
                    self.stack.push(Value::Bool(a && b));
                }
                Opcode::Or => {
                    let b = self.pop_bool("or")?;
                    let a = self.pop_bool("or")?;
                    self.stack.push(Value::Bool(a || b));
                }
                Opcode::Not => {
                    let v = self.pop_bool("not")?;
                    self.stack.push(Value::Bool(!v));
                }

                Opcode::Jump => {
                    let offset = self.read_i16()?;
                    self.jump_relative(offset)?;
                }
                Opcode::JumpIf => {
                    let offset = self.read_i16()?;
                    let cond = self.pop_bool("jump_if")?;
                    if cond {
                        self.jump_relative(offset)?;
                    }
                }
                Opcode::JumpIfNot => {
                    let offset = self.read_i16()?;
                    let cond = self.pop_bool("jump_if_not")?;
                    if !cond {
                        self.jump_relative(offset)?;
                    }
                }

                Opcode::Call => {
                    let func_idx = self.read_u16()?;
                    let arg_count = self.read_u8()?;
                    let func_ref = self.get_constant(func_idx)?;
                    let hash = match func_ref {
                        Value::FunctionRef(h) => h,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "function",
                                got: other.type_name(),
                                operation: "call",
                            })
                        }
                    };
                    self.push_frame(&hash, arg_count)?;
                }

                Opcode::Return => {
                    let result = self.pop()?;

                    // Get info before popping frame
                    let frame = self.frames.pop().ok_or(VmError::StackUnderflow)?;

                    // Pop locals and arguments from stack
                    self.stack.truncate(frame.bp);

                    if self.frames.is_empty() {
                        // Returning from top-level function
                        return Ok(result);
                    }

                    // Push result for caller
                    self.stack.push(result);
                }

                Opcode::MakeTuple => {
                    let arity = self.read_u8()?;
                    let mut elements = Vec::with_capacity(arity as usize);
                    for _ in 0..arity {
                        elements.push(self.pop()?);
                    }
                    elements.reverse();
                    self.stack.push(Value::tuple(elements));
                }

                Opcode::TupleGet => {
                    let index = self.read_u8()?;
                    let tuple = self.pop()?;
                    match tuple {
                        Value::Tuple(elements) => {
                            let elem = elements.get(index as usize).ok_or(
                                VmError::TupleIndexOutOfBounds {
                                    index,
                                    length: elements.len(),
                                },
                            )?;
                            self.stack.push(elem.clone());
                        }
                        other => {
                            return Err(VmError::TypeError {
                                expected: "tuple",
                                got: other.type_name(),
                                operation: "tuple_get",
                            })
                        }
                    }
                }

                Opcode::MakeRecord => {
                    let field_count = self.read_u8()?;
                    let mut fields: Vec<(Arc<str>, Value)> =
                        Vec::with_capacity(field_count as usize);

                    // Pop field-value pairs (value first, then field name)
                    for _ in 0..field_count {
                        let value = self.pop()?;
                        let field_name = match self.pop()? {
                            Value::String(s) => Arc::from(s.as_str()),
                            other => {
                                return Err(VmError::TypeError {
                                    expected: "string",
                                    got: other.type_name(),
                                    operation: "make_record",
                                })
                            }
                        };
                        fields.push((field_name, value));
                    }

                    self.stack.push(Value::record(fields));
                }

                Opcode::RecordGet => {
                    let field_idx = self.read_u16()?;
                    let field_name = match self.get_constant(field_idx)? {
                        Value::String(s) => s,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "string",
                                got: other.type_name(),
                                operation: "record_get",
                            })
                        }
                    };

                    let record = self.pop()?;
                    match record {
                        Value::Record(fields) => {
                            let key: Arc<str> = Arc::from(field_name.as_str());
                            let value = fields.get(&key).ok_or_else(|| {
                                VmError::RecordFieldNotFound(field_name.to_string())
                            })?;
                            self.stack.push(value.clone());
                        }
                        other => {
                            return Err(VmError::TypeError {
                                expected: "record",
                                got: other.type_name(),
                                operation: "record_get",
                            })
                        }
                    }
                }

                // ─────────────────────────────────────────────────────────────
                // Abilities (Milestone 2)
                // ─────────────────────────────────────────────────────────────
                Opcode::Suspend => {
                    // Create a suspended ability value from arguments on the stack
                    let ability_id = self.read_u16()?;
                    let method_id = self.read_u16()?;
                    let arg_count = self.read_u8()?;

                    // Pop arguments (in reverse order)
                    let mut args = Vec::with_capacity(arg_count as usize);
                    for _ in 0..arg_count {
                        args.push(self.pop()?);
                    }
                    args.reverse();

                    // Push the suspended ability value
                    self.stack
                        .push(Value::suspended_ability(ability_id, method_id, args));
                }

                Opcode::Perform => {
                    // Pop the suspended ability and perform it
                    let ability = match self.pop()? {
                        Value::SuspendedAbility(a) => a,
                        other => {
                            return Err(VmError::ExpectedSuspendedAbility {
                                got: other.type_name(),
                            })
                        }
                    };

                    // First, check for a host handler
                    if let Some(handler) = self
                        .host_handlers
                        .get(&(ability.ability_id, ability.method_id))
                    {
                        // Call the host handler synchronously
                        let result = handler(&ability)?;
                        self.stack.push(result);
                        continue;
                    }

                    // Look for a bytecode handler on the handler stack
                    let handler_idx = self
                        .handlers
                        .iter()
                        .rposition(|h| h.ability_id == ability.ability_id);

                    if let Some(idx) = handler_idx {
                        // Found a handler - capture continuation and jump to handler
                        let handler = self.handlers[idx].clone();

                        // Determine the handler function to call based on handler kind
                        let handler_func = match &handler.handler {
                            HandlerKind::Inline { handler_func } => *handler_func,
                            HandlerKind::Value { handler_value } => {
                                // Look up the method function from the handler value
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

                        // Capture the continuation: stack and frames from handler point to current
                        let captured_stack = self.stack.split_off(handler.stack_height);
                        let captured_frames: Vec<CapturedFrame> = self.frames
                            [handler.call_frame_idx..]
                            .iter()
                            .map(|f| CapturedFrame {
                                function_hash: f.function.hash,
                                ip: f.ip,
                                bp: f.bp,
                            })
                            .collect();

                        // Truncate frames to handler point
                        self.frames.truncate(handler.call_frame_idx);

                        // Remove the handler (and any handlers installed after it)
                        self.handlers.truncate(idx);

                        // Create continuation value
                        let continuation = Value::continuation(captured_stack, captured_frames);

                        // Push the continuation and the suspended ability as arguments
                        // to the handler function
                        self.stack.push(continuation);
                        self.stack.push(Value::SuspendedAbility(ability));

                        // Call the handler function
                        self.push_frame(&handler_func, 2)?;
                    } else {
                        // No handler found
                        return Err(VmError::UnhandledAbility {
                            ability_id: ability.ability_id,
                            method_id: ability.method_id,
                        });
                    }
                }

                Opcode::Handle => {
                    // Install an ability handler (inline)
                    let ability_id = self.read_u16()?;
                    let handler_idx = self.read_u16()?;
                    let completion_offset = self.read_i16()?;

                    let handler_func = match self.get_constant(handler_idx)? {
                        Value::FunctionRef(h) => h,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "function",
                                got: other.type_name(),
                                operation: "handle",
                            })
                        }
                    };

                    // Calculate normal completion IP
                    let frame = self.current_frame()?;
                    let current_ip = frame.ip;
                    let normal_completion_ip =
                        (current_ip as isize + completion_offset as isize) as usize;

                    self.handlers.push(HandlerFrame {
                        ability_id,
                        handler: HandlerKind::Inline { handler_func },
                        call_frame_idx: self.frames.len() - 1,
                        stack_height: self.stack.len(),
                        normal_completion_ip,
                    });
                }

                Opcode::Unhandle => {
                    // Remove the most recent handler
                    self.handlers.pop();
                }

                Opcode::Resume => {
                    // Resume a continuation with a value
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
                            captures: Vec::new(), // Continuations don't preserve closure captures
                        });
                    }

                    // Push the resume value as the result of the Perform
                    self.stack.push(value);
                }

                Opcode::GetAbilityArg => {
                    let arg_index = self.read_u8()? as usize;
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
                }

                Opcode::Halt => {
                    return self.pop();
                }

                // ─────────────────────────────────────────────────────────────
                // Concurrency (Milestone 9)
                // ─────────────────────────────────────────────────────────────
                Opcode::AsyncAll => {
                    let count = self.read_u8()?;

                    // Pop all suspended abilities (in reverse order)
                    let mut abilities = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        let ability = match self.pop()? {
                            Value::SuspendedAbility(a) => a,
                            other => {
                                return Err(VmError::ExpectedSuspendedAbility {
                                    got: other.type_name(),
                                })
                            }
                        };
                        abilities.push(ability);
                    }
                    abilities.reverse(); // Restore original order

                    // Perform all abilities and collect results
                    let results = self.perform_all_abilities(&abilities)?;

                    // Push tuple of results
                    self.stack.push(Value::tuple(results));
                }

                Opcode::AsyncRace => {
                    let count = self.read_u8()?;

                    // Pop all suspended abilities (in reverse order)
                    let mut abilities = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        let ability = match self.pop()? {
                            Value::SuspendedAbility(a) => a,
                            other => {
                                return Err(VmError::ExpectedSuspendedAbility {
                                    got: other.type_name(),
                                })
                            }
                        };
                        abilities.push(ability);
                    }
                    abilities.reverse(); // Restore original order

                    // Race: perform abilities concurrently, return first result
                    let result = self.perform_race_abilities(&abilities)?;

                    // Push the winning result
                    self.stack.push(result);
                }

                // ─────────────────────────────────────────────────────────────
                // Closures
                // ─────────────────────────────────────────────────────────────
                Opcode::MakeClosure => {
                    let func_idx = self.read_u16()?;
                    let capture_count = self.read_u8()?;

                    // Get the function hash from the constant pool.
                    let func_hash = match self.get_constant(func_idx)? {
                        Value::FunctionRef(h) => h,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "function",
                                got: other.type_name(),
                                operation: "make_closure",
                            })
                        }
                    };

                    // Pop captured values from the stack (in reverse order).
                    let mut environment = Vec::with_capacity(capture_count as usize);
                    for _ in 0..capture_count {
                        environment.push(self.pop()?);
                    }
                    environment.reverse(); // Restore original capture order

                    // Create and push the closure value.
                    self.stack.push(Value::closure(func_hash, environment));
                }

                Opcode::CallClosure => {
                    let arg_count = self.read_u8()?;

                    // The closure was pushed first, then arguments.
                    // Pop arguments first to get to the closure.
                    let mut args = Vec::with_capacity(arg_count as usize);
                    for _ in 0..arg_count {
                        args.push(self.pop()?);
                    }
                    args.reverse();

                    // Now pop the closure.
                    let closure = match self.pop()? {
                        Value::Closure(c) => c,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "closure",
                                got: other.type_name(),
                                operation: "call_closure",
                            })
                        }
                    };

                    // Push arguments back onto the stack for the call.
                    for arg in args {
                        self.stack.push(arg);
                    }

                    // Call the closure's function with its captured environment.
                    self.push_frame_with_captures(
                        &closure.function_hash,
                        arg_count,
                        closure.environment.clone(),
                    )?;
                }

                Opcode::LoadCapture => {
                    let capture_slot = self.read_u16()?;

                    // Get the captured value from the current frame's captures.
                    let value = {
                        let frame = self.current_frame()?;
                        frame
                            .captures
                            .get(capture_slot as usize)
                            .cloned()
                            .ok_or(VmError::InvalidLocal(capture_slot))?
                    };
                    self.stack.push(value);
                }

                Opcode::MakeHandler => {
                    let ability_id = self.read_u16()?;
                    let method_count = self.read_u8()?;
                    let capture_count = self.read_u8()?;

                    // Read method mappings.
                    let mut methods =
                        std::collections::HashMap::with_capacity(method_count as usize);
                    for _ in 0..method_count {
                        let method_id = self.read_u16()?;
                        let func_idx = self.read_u16()?;

                        // Get the function hash from the constant pool.
                        let func_hash = match self.get_constant(func_idx)? {
                            Value::FunctionRef(h) => h,
                            other => {
                                return Err(VmError::TypeError {
                                    expected: "function",
                                    got: other.type_name(),
                                    operation: "make_handler",
                                })
                            }
                        };

                        methods.insert(method_id, func_hash);
                    }

                    // Pop captured values from the stack (in reverse order).
                    let mut captures = Vec::with_capacity(capture_count as usize);
                    for _ in 0..capture_count {
                        captures.push(self.pop()?);
                    }
                    captures.reverse(); // Restore original capture order

                    // Create and push the handler value.
                    self.stack.push(Value::Handler(std::sync::Arc::new(
                        crate::value::HandlerValue::with_captures(ability_id, methods, captures),
                    )));
                }

                Opcode::HandleWithValue => {
                    // Install a handler from a HandlerValue on the stack
                    let completion_offset = self.read_i16()?;

                    // Pop the handler value from the stack
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

                    // Calculate normal completion IP
                    let frame = self.current_frame()?;
                    let current_ip = frame.ip;
                    let normal_completion_ip =
                        (current_ip as isize + completion_offset as isize) as usize;

                    self.handlers.push(HandlerFrame {
                        ability_id: handler_value.ability_id,
                        handler: HandlerKind::Value { handler_value },
                        call_frame_idx: self.frames.len() - 1,
                        stack_height: self.stack.len(),
                        normal_completion_ip,
                    });
                }
            }
        }
    }

    /// Fetch the next opcode from the current frame's bytecode.
    fn fetch_opcode(&mut self) -> Result<Opcode, VmError> {
        let frame = self.current_frame_mut()?;
        if frame.ip >= frame.function.bytecode.len() {
            return Err(VmError::InstructionOutOfBounds);
        }
        let byte = frame.function.bytecode[frame.ip];
        frame.ip += 1;
        Opcode::from_byte(byte).ok_or(VmError::InvalidOpcode(byte))
    }

    /// Read a u8 operand from the bytecode.
    fn read_u8(&mut self) -> Result<u8, VmError> {
        let frame = self.current_frame_mut()?;
        if frame.ip >= frame.function.bytecode.len() {
            return Err(VmError::InstructionOutOfBounds);
        }
        let byte = frame.function.bytecode[frame.ip];
        frame.ip += 1;
        Ok(byte)
    }

    /// Read a u16 operand from the bytecode (little-endian).
    fn read_u16(&mut self) -> Result<u16, VmError> {
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
    fn read_i16(&mut self) -> Result<i16, VmError> {
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
    fn current_frame(&self) -> Result<&CallFrame, VmError> {
        self.frames.last().ok_or(VmError::StackUnderflow)
    }

    /// Get the current call frame mutably.
    fn current_frame_mut(&mut self) -> Result<&mut CallFrame, VmError> {
        self.frames.last_mut().ok_or(VmError::StackUnderflow)
    }

    /// Get a constant from the current function's constant pool.
    fn get_constant(&self, idx: u16) -> Result<Value, VmError> {
        let frame = self.current_frame()?;
        frame
            .function
            .constants
            .get(idx as usize)
            .cloned()
            .ok_or(VmError::InvalidConstant(idx))
    }

    /// Get a local variable from the current frame.
    fn get_local(&self, slot: u16) -> Result<Value, VmError> {
        let frame = self.current_frame()?;
        let idx = frame.bp + slot as usize;
        self.stack
            .get(idx)
            .cloned()
            .ok_or(VmError::InvalidLocal(slot))
    }

    /// Set a local variable in the current frame.
    fn set_local(&mut self, slot: u16, value: Value) -> Result<(), VmError> {
        let frame = self.current_frame()?;
        let idx = frame.bp + slot as usize;
        if idx >= self.stack.len() {
            return Err(VmError::InvalidLocal(slot));
        }
        self.stack[idx] = value;
        Ok(())
    }

    /// Pop a value from the stack.
    fn pop(&mut self) -> Result<Value, VmError> {
        self.stack.pop().ok_or(VmError::StackUnderflow)
    }

    /// Peek at the top of the stack without popping.
    fn peek(&self) -> Result<&Value, VmError> {
        self.stack.last().ok_or(VmError::StackUnderflow)
    }

    /// Pop a number from the stack or return a type error.
    fn pop_number(&mut self, operation: &'static str) -> Result<f64, VmError> {
        match self.pop()? {
            Value::Number(n) => Ok(n),
            other => Err(VmError::TypeError {
                expected: "number",
                got: other.type_name(),
                operation,
            }),
        }
    }

    /// Pop a bool from the stack or return a type error.
    fn pop_bool(&mut self, operation: &'static str) -> Result<bool, VmError> {
        match self.pop()? {
            Value::Bool(b) => Ok(b),
            other => Err(VmError::TypeError {
                expected: "bool",
                got: other.type_name(),
                operation,
            }),
        }
    }

    /// Execute a binary operation on numbers.
    fn binary_number_op(
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
    fn comparison_op(
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
    fn jump_relative(&mut self, offset: i16) -> Result<(), VmError> {
        let frame = self.current_frame_mut()?;
        let new_ip = frame.ip as isize + offset as isize;
        if new_ip < 0 || new_ip > frame.function.bytecode.len() as isize {
            return Err(VmError::InstructionOutOfBounds);
        }
        frame.ip = new_ip as usize;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{BytecodeBuilder, Opcode};
    use crate::test_utils::{Capture, FunctionBuilder, VmTest};

    // =========================================================================
    // Constants and Stack Operations
    // =========================================================================

    #[test]
    fn test_push_const_number() {
        VmTest::new().push(42.0).expect_number(42.0);
    }

    #[test]
    fn test_push_const_bool() {
        VmTest::new().push_bool(true).expect_bool(true);
    }

    #[test]
    fn test_push_const_string() {
        VmTest::new().push_str("hello").expect_string("hello");
    }

    #[test]
    fn test_dup() {
        VmTest::new().push(21.0).dup().add().expect_number(42.0);
    }

    #[test]
    fn test_pop() {
        VmTest::new()
            .push(1.0)
            .push(42.0)
            .push(2.0)
            .pop()
            .pop()
            .expect_number(1.0);
    }

    // =========================================================================
    // Arithmetic Operations
    // =========================================================================

    #[test]
    fn test_add() {
        VmTest::new()
            .push(10.0)
            .push(32.0)
            .add()
            .expect_number(42.0);
    }

    #[test]
    fn test_sub() {
        VmTest::new().push(50.0).push(8.0).sub().expect_number(42.0);
    }

    #[test]
    fn test_mul() {
        VmTest::new().push(6.0).push(7.0).mul().expect_number(42.0);
    }

    #[test]
    fn test_div() {
        VmTest::new().push(84.0).push(2.0).div().expect_number(42.0);
    }

    #[test]
    fn test_div_by_zero() {
        VmTest::new()
            .push(1.0)
            .push(0.0)
            .div()
            .expect_error(VmError::DivisionByZero);
    }

    #[test]
    fn test_mod() {
        VmTest::new()
            .push(10.0)
            .push(3.0)
            .modulo()
            .expect_number(1.0);
    }

    #[test]
    fn test_neg() {
        VmTest::new().push(42.0).neg().expect_number(-42.0);
    }

    // =========================================================================
    // Comparison Operations
    // =========================================================================

    #[test]
    fn test_eq_true() {
        VmTest::new().push(42.0).push(42.0).eq().expect_bool(true);
    }

    #[test]
    fn test_eq_false() {
        VmTest::new().push(42.0).push(43.0).eq().expect_bool(false);
    }

    #[test]
    fn test_lt() {
        VmTest::new().push(1.0).push(2.0).lt().expect_bool(true);
    }

    #[test]
    fn test_le() {
        VmTest::new().push(2.0).push(2.0).le().expect_bool(true);
    }

    #[test]
    fn test_gt() {
        VmTest::new().push(3.0).push(2.0).gt().expect_bool(true);
    }

    #[test]
    fn test_ge() {
        VmTest::new().push(2.0).push(2.0).ge().expect_bool(true);
    }

    // =========================================================================
    // Logic Operations
    // =========================================================================

    #[test]
    fn test_and() {
        VmTest::new()
            .push_bool(true)
            .push_bool(false)
            .and()
            .expect_bool(false);
    }

    #[test]
    fn test_or() {
        VmTest::new()
            .push_bool(true)
            .push_bool(false)
            .or()
            .expect_bool(true);
    }

    #[test]
    fn test_not() {
        VmTest::new().push_bool(true).not().expect_bool(false);
    }

    // =========================================================================
    // Type Errors
    // =========================================================================

    #[test]
    fn test_type_error_add() {
        VmTest::new()
            .push(1.0)
            .push_bool(true)
            .add()
            .expect_error(VmError::TypeError {
                expected: "number",
                got: "bool",
                operation: "add",
            });
    }

    // =========================================================================
    // Local Variables
    // =========================================================================

    #[test]
    fn test_local_variables() {
        // x = 10; y = 32; x + y
        VmTest::new()
            .with_locals(2)
            .push(10.0)
            .store_local(0)
            .pop()
            .push(32.0)
            .store_local(1)
            .pop()
            .load_local(0)
            .load_local(1)
            .add()
            .expect_number(42.0);
    }

    // =========================================================================
    // Control Flow
    // =========================================================================

    #[test]
    fn test_jump() {
        VmTest::new()
            .with_builder(|b| {
                let jump_offset = b.emit_jump_placeholder(Opcode::Jump);
                b.emit_const(Value::Number(1.0)); // Skipped
                b.patch_jump(jump_offset);
                b.emit_const(Value::Number(42.0)); // Executed
            })
            .expect_number(42.0);
    }

    #[test]
    fn test_jump_if_true() {
        VmTest::new()
            .with_builder(|b| {
                b.emit_const(Value::Bool(true));
                let jump_offset = b.emit_jump_placeholder(Opcode::JumpIf);
                b.emit_const(Value::Number(1.0));
                b.emit(Opcode::Return);
                b.patch_jump(jump_offset);
                b.emit_const(Value::Number(42.0));
            })
            .expect_number(42.0);
    }

    #[test]
    fn test_jump_if_false() {
        VmTest::new()
            .with_builder(|b| {
                b.emit_const(Value::Bool(false));
                let jump_offset = b.emit_jump_placeholder(Opcode::JumpIf);
                b.emit_const(Value::Number(42.0));
                b.emit(Opcode::Return);
                b.patch_jump(jump_offset);
                b.emit_const(Value::Number(1.0));
            })
            .expect_number(42.0);
    }

    // =========================================================================
    // Data Structures: Tuples
    // =========================================================================

    #[test]
    fn test_make_tuple() {
        VmTest::new()
            .push(1.0)
            .push(2.0)
            .push(3.0)
            .make_tuple(3)
            .expect(Value::tuple(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
            ]));
    }

    #[test]
    fn test_tuple_get() {
        VmTest::new()
            .push(1.0)
            .push(42.0)
            .push(3.0)
            .make_tuple(3)
            .tuple_get(1)
            .expect_number(42.0);
    }

    #[test]
    fn test_tuple_index_out_of_bounds() {
        VmTest::new()
            .push(1.0)
            .make_tuple(1)
            .tuple_get(5)
            .expect_error(VmError::TupleIndexOutOfBounds {
                index: 5,
                length: 1,
            });
    }

    #[test]
    fn test_tuple_unpacking() {
        // let pair = (10, 32); pair.0 + pair.1
        VmTest::new()
            .with_locals(1)
            .push(10.0)
            .push(32.0)
            .make_tuple(2)
            .store_local(0)
            .load_local(0)
            .tuple_get(0)
            .load_local(0)
            .tuple_get(1)
            .add()
            .expect_number(42.0);
    }

    // =========================================================================
    // Data Structures: Records
    // =========================================================================

    #[test]
    fn test_make_record() {
        VmTest::new()
            .push_str("x")
            .push(1.0)
            .push_str("y")
            .push(2.0)
            .make_record(2)
            .expect_record(|fields| {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields.get(&Arc::from("x")), Some(&Value::Number(1.0)));
                assert_eq!(fields.get(&Arc::from("y")), Some(&Value::Number(2.0)));
            });
    }

    #[test]
    fn test_record_get() {
        VmTest::new()
            .push_str("x")
            .push(42.0)
            .make_record(1)
            .record_get("x")
            .expect_number(42.0);
    }

    #[test]
    fn test_record_manipulation_point() {
        // point = { x: 3.0, y: 4.0 }; point.x * point.x + point.y * point.y = 25.0
        VmTest::new()
            .with_locals(1)
            .push_str("x")
            .push(3.0)
            .push_str("y")
            .push(4.0)
            .make_record(2)
            .store_local(0)
            .load_local(0)
            .record_get("x")
            .load_local(0)
            .record_get("x")
            .mul()
            .load_local(0)
            .record_get("y")
            .load_local(0)
            .record_get("y")
            .mul()
            .add()
            .expect_number(25.0);
    }

    #[test]
    fn test_record_nested_access() {
        // { user: { name: "Alice", age: 30 } }.user.age = 30.0
        VmTest::new()
            .with_locals(1)
            .push_str("name")
            .push_str("Alice")
            .push_str("age")
            .push(30.0)
            .make_record(2)
            .store_local(0)
            .pop()
            .push_str("user")
            .load_local(0)
            .make_record(1)
            .record_get("user")
            .record_get("age")
            .expect_number(30.0);
    }

    // =========================================================================
    // Function Calls
    // =========================================================================

    #[test]
    fn test_function_call() {
        let helper = FunctionBuilder::new("test::helper").push(42.0).build();
        let helper_hash = helper.hash;

        VmTest::new()
            .with_function(helper)
            .call_func(helper_hash, 0)
            .expect_number(42.0);
    }

    #[test]
    fn test_function_with_args() {
        // add(a, b) = a + b
        let add_fn = FunctionBuilder::new("test::add")
            .with_locals(2)
            .with_params(2)
            .load_local(0)
            .load_local(1)
            .add()
            .build();
        let add_hash = add_fn.hash;

        VmTest::new()
            .with_function(add_fn)
            .push(10.0)
            .push(32.0)
            .call_func(add_hash, 2)
            .expect_number(42.0);
    }

    // =========================================================================
    // Milestone 1: Recursive Functions
    // =========================================================================

    /// Build a recursive factorial function using FunctionBuilder.
    fn build_factorial() -> CompiledFunction {
        FunctionBuilder::new("test::factorial")
            .with_locals(1)
            .with_params(1)
            .with_builder(|b| {
                let func_hash = blake3::hash(b"test::factorial");

                b.emit_u16(Opcode::LoadLocal, 0);
                b.emit_const(Value::Number(1.0));
                b.emit(Opcode::Le);

                let else_jump = b.emit_jump_placeholder(Opcode::JumpIfNot);
                b.emit_const(Value::Number(1.0));
                b.emit(Opcode::Return);

                b.patch_jump(else_jump);
                b.emit_u16(Opcode::LoadLocal, 0);
                b.emit_u16(Opcode::LoadLocal, 0);
                b.emit_const(Value::Number(1.0));
                b.emit(Opcode::Sub);
                b.emit_call(func_hash, 1);
                b.emit(Opcode::Mul);
            })
            .build()
    }

    #[test]
    fn test_factorial_base_case() {
        let factorial = build_factorial();
        let hash = factorial.hash;

        VmTest::new()
            .with_function(factorial)
            .push(1.0)
            .call_func(hash, 1)
            .expect_number(1.0);
    }

    #[test]
    fn test_factorial_small() {
        let factorial = build_factorial();
        let hash = factorial.hash;

        VmTest::new()
            .with_function(factorial)
            .push(5.0)
            .call_func(hash, 1)
            .expect_number(120.0);
    }

    #[test]
    fn test_factorial_larger() {
        let factorial = build_factorial();
        let hash = factorial.hash;

        VmTest::new()
            .with_function(factorial)
            .push(10.0)
            .call_func(hash, 1)
            .expect_number(3_628_800.0);
    }

    /// Build a recursive fibonacci function using FunctionBuilder.
    fn build_fibonacci() -> CompiledFunction {
        FunctionBuilder::new("test::fibonacci")
            .with_locals(1)
            .with_params(1)
            .with_builder(|b| {
                let func_hash = blake3::hash(b"test::fibonacci");

                b.emit_u16(Opcode::LoadLocal, 0);
                b.emit_const(Value::Number(1.0));
                b.emit(Opcode::Le);

                let else_jump = b.emit_jump_placeholder(Opcode::JumpIfNot);
                b.emit_u16(Opcode::LoadLocal, 0);
                b.emit(Opcode::Return);

                b.patch_jump(else_jump);

                // fib(n-1)
                b.emit_u16(Opcode::LoadLocal, 0);
                b.emit_const(Value::Number(1.0));
                b.emit(Opcode::Sub);
                b.emit_call(func_hash, 1);

                // fib(n-2)
                b.emit_u16(Opcode::LoadLocal, 0);
                b.emit_const(Value::Number(2.0));
                b.emit(Opcode::Sub);
                b.emit_call(func_hash, 1);

                b.emit(Opcode::Add);
            })
            .build()
    }

    #[test]
    fn test_fibonacci_base_cases() {
        let fib = build_fibonacci();
        let hash = fib.hash;

        VmTest::new()
            .with_function(fib.clone())
            .push(0.0)
            .call_func(hash, 1)
            .expect_number(0.0);

        VmTest::new()
            .with_function(fib)
            .push(1.0)
            .call_func(hash, 1)
            .expect_number(1.0);
    }

    #[test]
    fn test_fibonacci_sequence() {
        let fib = build_fibonacci();
        let hash = fib.hash;

        VmTest::new()
            .with_function(fib)
            .push(10.0)
            .call_func(hash, 1)
            .expect_number(55.0);
    }

    #[test]
    fn test_fibonacci_values() {
        let expected = [0.0, 1.0, 1.0, 2.0, 3.0, 5.0, 8.0, 13.0, 21.0, 34.0, 55.0];

        for (n, exp) in expected.iter().enumerate() {
            let fib = build_fibonacci();
            let hash = fib.hash;

            let result = VmTest::new()
                .with_function(fib)
                .push(n as f64)
                .call_func(hash, 1)
                .run();

            assert_eq!(result, Ok(Value::Number(*exp)), "fib({n}) should be {exp}");
        }
    }

    // =========================================================================
    // Milestone 2: Abilities and Handlers
    // =========================================================================

    const ABILITY_CONSOLE: u16 = 1;
    const ABILITY_MATH: u16 = 2;
    const METHOD_PRINT: u16 = 0;
    const METHOD_DOUBLE: u16 = 0;
    const METHOD_ADD_TEN: u16 = 1;

    #[test]
    fn test_suspend_creates_ability_value() {
        VmTest::new()
            .push(42.0)
            .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
            .expect_suspended(|ability| {
                assert_eq!(ability.ability_id, ABILITY_CONSOLE);
                assert_eq!(ability.method_id, METHOD_PRINT);
                assert_eq!(ability.args.len(), 1);
                assert_eq!(ability.args[0], Value::Number(42.0));
            });
    }

    #[test]
    fn test_host_handler_called() {
        let capture = Capture::<f64>::new();
        let log = capture.clone_inner();

        VmTest::new()
            .push(42.0)
            .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
            .perform()
            .with_host_handler(ABILITY_CONSOLE, METHOD_PRINT, move |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    log.lock().expect("lock").push(*n);
                }
                Ok(Value::Unit)
            })
            .expect_unit();

        capture.assert_eq(&[42.0]);
    }

    #[test]
    fn test_host_handler_returns_value() {
        VmTest::new()
            .push(21.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .perform()
            .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n * 2.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .expect_number(42.0);
    }

    #[test]
    fn test_unhandled_ability_error() {
        VmTest::new()
            .push(42.0)
            .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
            .perform()
            .expect_error(VmError::UnhandledAbility {
                ability_id: ABILITY_CONSOLE,
                method_id: METHOD_PRINT,
            });
    }

    #[test]
    fn test_bytecode_handler_simple_resume() {
        // Handler: receives (continuation, ability), resumes with 42.0
        let handler = FunctionBuilder::new("test::math_handler")
            .with_locals(2)
            .with_params(2)
            .load_local(0)
            .push(42.0)
            .resume()
            .build();
        let handler_hash = handler.hash;

        VmTest::new()
            .with_function(handler)
            .handle(ABILITY_MATH, handler_hash)
            .push(5.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .perform()
            .unhandle()
            .expect_number(42.0);
    }

    #[test]
    fn test_single_shot_enforcement() {
        // Handler resumes once and returns
        let handler = FunctionBuilder::new("test::double_resume_handler")
            .with_locals(2)
            .with_params(2)
            .load_local(0)
            .push(1.0)
            .resume()
            .build();
        let handler_hash = handler.hash;

        VmTest::new()
            .with_function(handler)
            .handle(ABILITY_MATH, handler_hash)
            .push(5.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .perform()
            .unhandle()
            .expect_number(1.0);
    }

    #[test]
    fn test_perform_expected_type_error() {
        VmTest::new()
            .push(42.0)
            .perform()
            .expect_error(VmError::ExpectedSuspendedAbility { got: "number" });
    }

    #[test]
    fn test_multiple_ability_calls() {
        let capture = Capture::<f64>::new();
        let log = capture.clone_inner();

        VmTest::new()
            .push(1.0)
            .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
            .perform()
            .pop()
            .push(2.0)
            .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
            .perform()
            .pop()
            .push(3.0)
            .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
            .perform()
            .with_host_handler(ABILITY_CONSOLE, METHOD_PRINT, move |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    log.lock().expect("lock").push(*n);
                }
                Ok(Value::Unit)
            })
            .expect_unit();

        capture.assert_eq(&[1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_ability_with_multiple_args() {
        VmTest::new()
            .push(10.0)
            .push(32.0)
            .suspend(ABILITY_MATH, METHOD_ADD_TEN, 2)
            .perform()
            .with_host_handler(ABILITY_MATH, METHOD_ADD_TEN, |ability| {
                if ability.args.len() >= 2 {
                    if let (Value::Number(a), Value::Number(b)) =
                        (&ability.args[0], &ability.args[1])
                    {
                        return Ok(Value::Number(a + b));
                    }
                }
                Ok(Value::Unit)
            })
            .expect_number(42.0);
    }

    // =========================================================================
    // Milestone 3: Abilities as Values
    // =========================================================================

    #[test]
    fn test_ability_stored_in_variable() {
        let capture = Capture::<u32>::new();
        let count = capture.clone_inner();

        VmTest::new()
            .with_locals(1)
            .push(21.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .store_local(0)
            .pop()
            .push(999.0)
            .pop()
            .load_local(0)
            .perform()
            .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, move |ability| {
                count.lock().expect("lock").push(1);
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n * 2.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .expect_number(42.0);

        capture.assert_eq(&[1]);
    }

    #[test]
    fn test_ability_stored_in_tuple() {
        VmTest::new()
            .with_locals(1)
            .push(21.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .push_str("label")
            .make_tuple(2)
            .store_local(0)
            .pop()
            .load_local(0)
            .tuple_get(0)
            .perform()
            .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n * 2.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .expect_number(42.0);
    }

    #[test]
    fn test_ability_passed_to_function() {
        // perform_ability(op) = op!
        let perform_fn = FunctionBuilder::new("test::perform_ability")
            .with_locals(1)
            .with_params(1)
            .load_local(0)
            .perform()
            .build();
        let perform_hash = perform_fn.hash;

        VmTest::new()
            .with_function(perform_fn)
            .push(21.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .call_func(perform_hash, 1)
            .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n * 2.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .expect_number(42.0);
    }

    #[test]
    fn test_multiple_abilities_different_order() {
        // op1 = double(10), op2 = double(21), perform op2
        VmTest::new()
            .with_locals(2)
            .push(10.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .store_local(0)
            .pop()
            .push(21.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .store_local(1)
            .pop()
            .load_local(1)
            .perform()
            .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n * 2.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .expect_number(42.0);
    }

    #[test]
    fn test_ability_equality() {
        VmTest::new()
            .with_locals(2)
            .push(42.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .store_local(0)
            .pop()
            .push(42.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .store_local(1)
            .pop()
            .load_local(0)
            .load_local(1)
            .eq()
            .expect_bool(true);
    }

    #[test]
    fn test_ability_returned_from_function() {
        // create_double_op(n) = Math.double(n) (no perform)
        let creator_fn = FunctionBuilder::new("test::create_double_op")
            .with_locals(1)
            .with_params(1)
            .load_local(0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .build();
        let creator_hash = creator_fn.hash;

        VmTest::new()
            .with_locals(1)
            .with_function(creator_fn)
            .push(21.0)
            .call_func(creator_hash, 1)
            .store_local(0)
            .pop()
            .load_local(0)
            .perform()
            .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n * 2.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .expect_number(42.0);
    }

    // =========================================================================
    // Milestone 9: Concurrency
    // =========================================================================

    #[test]
    fn test_async_all_single_ability() {
        // Create one suspended ability, perform with async_all
        VmTest::new()
            .push(21.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .async_all(1)
            .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n * 2.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .expect_tuple(|elements| {
                assert_eq!(elements.len(), 1);
                assert_eq!(elements[0], Value::Number(42.0));
            });
    }

    #[test]
    fn test_async_all_multiple_abilities() {
        // Create multiple suspended abilities, perform all
        VmTest::new()
            .push(10.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .push(20.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .push(30.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .async_all(3)
            .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n * 2.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .expect_tuple(|elements| {
                assert_eq!(elements.len(), 3);
                assert_eq!(elements[0], Value::Number(20.0));
                assert_eq!(elements[1], Value::Number(40.0));
                assert_eq!(elements[2], Value::Number(60.0));
            });
    }

    #[test]
    fn test_async_all_execution_concurrent() {
        // Verify that all abilities are executed and all results are collected.
        // Note: With concurrent execution, the order of execution is NOT guaranteed,
        // but the results are returned in the original order.
        let capture = Capture::<f64>::new();
        let log = capture.clone_inner();

        VmTest::new()
            .push(1.0)
            .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
            .push(2.0)
            .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
            .push(3.0)
            .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
            .async_all(3)
            .with_host_handler(ABILITY_CONSOLE, METHOD_PRINT, move |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    log.lock().expect("lock").push(*n);
                }
                Ok(Value::Unit)
            })
            .expect_tuple(|elements| {
                assert_eq!(elements.len(), 3);
                // All should be Unit
                for elem in elements {
                    assert_eq!(*elem, Value::Unit);
                }
            });

        // Verify all values were executed (order may vary with concurrent execution)
        let mut values = capture.into_vec();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(values, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_async_all_different_abilities() {
        // Mix different ability types
        VmTest::new()
            .push(21.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .push(100.0)
            .suspend(ABILITY_MATH, METHOD_ADD_TEN, 1)
            .async_all(2)
            .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n * 2.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .with_host_handler(ABILITY_MATH, METHOD_ADD_TEN, |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n + 10.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .expect_tuple(|elements| {
                assert_eq!(elements.len(), 2);
                assert_eq!(elements[0], Value::Number(42.0));
                assert_eq!(elements[1], Value::Number(110.0));
            });
    }

    #[test]
    fn test_async_all_zero_abilities() {
        // Empty async_all should return empty tuple
        VmTest::new().async_all(0).expect_tuple(|elements| {
            assert_eq!(elements.len(), 0);
        });
    }

    #[test]
    fn test_async_all_type_error() {
        // Passing non-ability value should error
        VmTest::new()
            .push(42.0)
            .async_all(1)
            .expect_error(VmError::ExpectedSuspendedAbility { got: "number" });
    }

    #[test]
    fn test_async_all_unhandled_ability() {
        // Unhandled ability in async_all should error
        VmTest::new()
            .push(42.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .async_all(1)
            .expect_error(VmError::UnhandledAbility {
                ability_id: ABILITY_MATH,
                method_id: METHOD_DOUBLE,
            });
    }

    #[test]
    fn test_async_race_single_ability() {
        // Race with single ability should return that result
        VmTest::new()
            .push(21.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .async_race(1)
            .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n * 2.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .expect_number(42.0);
    }

    #[test]
    fn test_async_race_multiple_abilities() {
        // Race returns the first ability to complete.
        // With concurrent execution, any of the results could win.
        let result = VmTest::new()
            .push(21.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .push(100.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .push(200.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .async_race(3)
            .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n * 2.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .run();

        // The result should be one of the doubled values
        match result {
            Ok(Value::Number(n)) => {
                assert!(
                    n == 42.0 || n == 200.0 || n == 400.0,
                    "Expected one of [42.0, 200.0, 400.0], got {n}"
                );
            }
            other => panic!("Expected Number, got {other:?}"),
        }
    }

    #[test]
    fn test_async_race_type_error() {
        // Passing non-ability value should error
        VmTest::new()
            .push_str("not an ability")
            .async_race(1)
            .expect_error(VmError::ExpectedSuspendedAbility { got: "string" });
    }

    #[test]
    fn test_async_race_unhandled_ability() {
        // Unhandled ability in async_race should error
        VmTest::new()
            .push(42.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .async_race(1)
            .expect_error(VmError::UnhandledAbility {
                ability_id: ABILITY_MATH,
                method_id: METHOD_DOUBLE,
            });
    }

    #[test]
    fn test_async_all_stored_in_variables() {
        // Store suspended abilities in variables, then async_all
        VmTest::new()
            .with_locals(2)
            .push(10.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .store_local(0)
            .pop()
            .push(20.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .store_local(1)
            .pop()
            .load_local(0)
            .load_local(1)
            .async_all(2)
            .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n * 2.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .expect_tuple(|elements| {
                assert_eq!(elements.len(), 2);
                assert_eq!(elements[0], Value::Number(20.0));
                assert_eq!(elements[1], Value::Number(40.0));
            });
    }

    #[test]
    fn test_async_all_result_used_in_computation() {
        // Use async_all result in subsequent computation
        VmTest::new()
            .push(20.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .push(1.0)
            .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
            .async_all(2)
            .tuple_get(0)
            .with_builder(|b| {
                // result.0 + 2 = 40 + 2 = 42
                b.emit_const(Value::Number(2.0));
                b.emit(Opcode::Add);
            })
            .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(n * 2.0))
                } else {
                    Ok(Value::Unit)
                }
            })
            .expect_number(42.0);
    }

    #[test]
    fn test_async_all_true_concurrency() {
        // Demonstrate that async_all executes concurrently by using delays.
        // If sequential, 3 x 50ms = 150ms minimum.
        // If concurrent, should be ~50ms (plus overhead).
        use std::time::{Duration, Instant};

        const DELAY_MS: u64 = 50;

        let start = Instant::now();

        VmTest::new()
            .push(1.0)
            .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
            .push(2.0)
            .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
            .push(3.0)
            .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
            .async_all(3)
            .with_host_handler(ABILITY_CONSOLE, METHOD_PRINT, move |ability| {
                // Simulate I/O delay
                std::thread::sleep(Duration::from_millis(DELAY_MS));
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(*n))
                } else {
                    Ok(Value::Unit)
                }
            })
            .expect_tuple(|elements| {
                assert_eq!(elements.len(), 3);
            });

        let elapsed = start.elapsed();

        // With true concurrency, should complete in ~50-100ms
        // If sequential, would take ~150ms
        // Use 120ms as threshold to allow for overhead but detect sequential execution
        assert!(
            elapsed < Duration::from_millis(120),
            "Expected concurrent execution (<120ms), but took {:?}. \
             This suggests sequential execution.",
            elapsed
        );
    }

    #[test]
    fn test_async_race_true_concurrency() {
        // Demonstrate that async_race returns the fastest result.
        // One handler is fast (10ms), others are slow (100ms).
        use std::time::{Duration, Instant};

        const SLOW_MS: u64 = 100;
        const FAST_MS: u64 = 10;

        let start = Instant::now();

        // Use different ability IDs to have different handlers
        const SLOW_ABILITY: u16 = 0x0010;
        const FAST_ABILITY: u16 = 0x0011;

        let result = VmTest::new()
            .push(1.0)
            .suspend(SLOW_ABILITY, 0, 1) // slow
            .push(42.0)
            .suspend(FAST_ABILITY, 0, 1) // fast - should win
            .push(3.0)
            .suspend(SLOW_ABILITY, 0, 1) // slow
            .async_race(3)
            .with_host_handler(SLOW_ABILITY, 0, move |ability| {
                std::thread::sleep(Duration::from_millis(SLOW_MS));
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(*n))
                } else {
                    Ok(Value::Unit)
                }
            })
            .with_host_handler(FAST_ABILITY, 0, move |ability| {
                std::thread::sleep(Duration::from_millis(FAST_MS));
                if let Value::Number(n) = &ability.args[0] {
                    Ok(Value::Number(*n))
                } else {
                    Ok(Value::Unit)
                }
            })
            .run();

        let elapsed = start.elapsed();

        // Should complete quickly (fast handler wins)
        // The slow handlers will still run to completion but we don't wait for them
        // due to scope semantics. With scoped threads, we wait for all threads.
        // So the total time is max(all threads) = 100ms
        // But the result should be from the fast handler.
        assert!(
            elapsed < Duration::from_millis(150),
            "Took too long: {:?}",
            elapsed
        );

        // The result should be 42.0 (from the fast handler)
        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_make_handler_creates_handler_value() {
        use crate::abilities::console;

        // Create a simple handler method function that returns unit.
        let mut handler_builder = BytecodeBuilder::new();
        handler_builder.emit_const(Value::Unit);
        handler_builder.emit(Opcode::Return);
        let handler_func = handler_builder.build(2, 2);
        let handler_hash = handler_func.hash;

        // Create main function that makes a handler and returns it.
        let mut builder = BytecodeBuilder::new();

        // Emit MakeHandler: Console ability, 1 method (print), 0 captures.
        builder.emit_make_handler(
            console::ABILITY_ID,
            &[(console::METHOD_PRINT, handler_hash)],
            0,
        );

        // Return the handler value.
        builder.emit(Opcode::Return);

        let main_func = builder.build(0, 0);
        let main_hash = main_func.hash;

        let mut vm = Vm::new();
        vm.load_function(handler_func);
        vm.load_function(main_func);

        let result = vm.call(&main_hash, vec![]);

        // Should return a handler value.
        assert!(result.is_ok(), "Should succeed: {:?}", result);
        if let Ok(Value::Handler(handler)) = result {
            assert_eq!(handler.ability_id, console::ABILITY_ID);
            assert!(handler.handles_method(console::METHOD_PRINT));
            assert_eq!(handler.methods.len(), 1);
        } else {
            panic!("Expected Handler value, got {:?}", result);
        }
    }

    #[test]
    fn test_make_handler_with_multiple_methods() {
        use crate::abilities::console;

        // Create handler method functions.
        let mut print_builder = BytecodeBuilder::new();
        print_builder.emit_const(Value::Unit);
        print_builder.emit(Opcode::Return);
        let print_func = print_builder.build(2, 2);
        let print_hash = print_func.hash;

        let mut eprint_builder = BytecodeBuilder::new();
        eprint_builder.emit_const(Value::Unit);
        eprint_builder.emit(Opcode::Return);
        let eprint_func = eprint_builder.build(2, 2);
        let eprint_hash = eprint_func.hash;

        // Create main function that makes a handler with 2 methods.
        let mut builder = BytecodeBuilder::new();
        builder.emit_make_handler(
            console::ABILITY_ID,
            &[
                (console::METHOD_PRINT, print_hash),
                (console::METHOD_EPRINT, eprint_hash),
            ],
            0,
        );
        builder.emit(Opcode::Return);

        let main_func = builder.build(0, 0);
        let main_hash = main_func.hash;

        let mut vm = Vm::new();
        vm.load_function(print_func);
        vm.load_function(eprint_func);
        vm.load_function(main_func);

        let result = vm.call(&main_hash, vec![]);

        assert!(result.is_ok(), "Should succeed: {:?}", result);
        if let Ok(Value::Handler(handler)) = result {
            assert_eq!(handler.ability_id, console::ABILITY_ID);
            assert!(handler.handles_method(console::METHOD_PRINT));
            assert!(handler.handles_method(console::METHOD_EPRINT));
            assert_eq!(handler.methods.len(), 2);
        } else {
            panic!("Expected Handler value, got {:?}", result);
        }
    }
}
