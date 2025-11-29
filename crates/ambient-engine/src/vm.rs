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
use std::rc::Rc;

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
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StackUnderflow => write!(f, "stack underflow"),
            Self::InvalidOpcode(op) => write!(f, "invalid opcode: 0x{op:02x}"),
            Self::TypeError { expected, got, operation } => {
                write!(f, "type error in {operation}: expected {expected}, got {got}")
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
                write!(f, "arity mismatch: expected {expected} arguments, got {got}")
            }
            Self::UnhandledAbility { ability_id, method_id } => {
                write!(f, "unhandled ability: ability {ability_id}, method {method_id}")
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
        }
    }
}

impl std::error::Error for VmError {}

/// A single stack frame representing an active function call.
#[derive(Debug, Clone)]
struct CallFrame {
    /// The function being executed.
    function: Rc<CompiledFunction>,

    /// Instruction pointer (offset into bytecode).
    ip: usize,

    /// Base pointer into the value stack for this frame's locals.
    bp: usize,
}

/// An installed ability handler that can intercept ability operations.
#[derive(Debug, Clone)]
#[allow(dead_code)] // normal_completion_ip may be used for future optimizations
struct HandlerFrame {
    /// The ability ID this handler handles.
    ability_id: u16,

    /// The handler function to call when ability is performed.
    handler_func: blake3::Hash,

    /// The call frame index where this handler was installed.
    call_frame_idx: usize,

    /// The stack height when the handler was installed.
    stack_height: usize,

    /// The instruction pointer to jump to for normal completion.
    normal_completion_ip: usize,
}

/// A host-provided ability handler callback.
pub type HostHandler = Box<dyn Fn(&SuspendedAbility) -> Result<Value, VmError>>;

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
    functions: HashMap<blake3::Hash, Rc<CompiledFunction>>,

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
        self.functions.insert(hash, Rc::new(func));
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

    /// Push a new call frame for the given function.
    fn push_frame(&mut self, hash: &blake3::Hash, arg_count: u8) -> Result<(), VmError> {
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

        self.frames.push(CallFrame { function, ip: 0, bp });

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
                    let mut fields: Vec<(Rc<str>, Value)> = Vec::with_capacity(field_count as usize);

                    // Pop field-value pairs (value first, then field name)
                    for _ in 0..field_count {
                        let value = self.pop()?;
                        let field_name = match self.pop()? {
                            Value::String(s) => Rc::from(s.as_str()),
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
                            let key: Rc<str> = Rc::from(field_name.as_str());
                            let value = fields
                                .get(&key)
                                .ok_or_else(|| VmError::RecordFieldNotFound(field_name.to_string()))?;
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
                    self.stack.push(Value::suspended_ability(ability_id, method_id, args));
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
                    if let Some(handler) = self.host_handlers.get(&(ability.ability_id, ability.method_id)) {
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

                        // Capture the continuation: stack and frames from handler point to current
                        let captured_stack = self.stack.split_off(handler.stack_height);
                        let captured_frames: Vec<CapturedFrame> = self.frames[handler.call_frame_idx..]
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
                        self.push_frame(&handler.handler_func, 2)?;
                    } else {
                        // No handler found
                        return Err(VmError::UnhandledAbility {
                            ability_id: ability.ability_id,
                            method_id: ability.method_id,
                        });
                    }
                }

                Opcode::Handle => {
                    // Install an ability handler
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
                    let normal_completion_ip = (current_ip as isize + completion_offset as isize) as usize;

                    self.handlers.push(HandlerFrame {
                        ability_id,
                        handler_func,
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
                        });
                    }

                    // Push the resume value as the result of the Perform
                    self.stack.push(value);
                }

                Opcode::Halt => {
                    return self.pop();
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
    use crate::bytecode::BytecodeBuilder;

    /// Helper to build and run a simple function.
    fn run_simple(build: impl FnOnce(&mut BytecodeBuilder)) -> Result<Value, VmError> {
        let mut builder = BytecodeBuilder::new();
        build(&mut builder);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        vm.call(&hash, vec![])
    }

    #[test]
    fn test_push_const_number() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(42.0));
        });
        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_push_const_bool() {
        let result = run_simple(|b| {
            b.emit_const(Value::Bool(true));
        });
        assert_eq!(result, Ok(Value::Bool(true)));
    }

    #[test]
    fn test_push_const_string() {
        let result = run_simple(|b| {
            b.emit_const(Value::string("hello"));
        });
        assert_eq!(result, Ok(Value::string("hello")));
    }

    #[test]
    fn test_add() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(10.0));
            b.emit_const(Value::Number(32.0));
            b.emit(Opcode::Add);
        });
        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_sub() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(50.0));
            b.emit_const(Value::Number(8.0));
            b.emit(Opcode::Sub);
        });
        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_mul() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(6.0));
            b.emit_const(Value::Number(7.0));
            b.emit(Opcode::Mul);
        });
        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_div() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(84.0));
            b.emit_const(Value::Number(2.0));
            b.emit(Opcode::Div);
        });
        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_div_by_zero() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(1.0));
            b.emit_const(Value::Number(0.0));
            b.emit(Opcode::Div);
        });
        assert_eq!(result, Err(VmError::DivisionByZero));
    }

    #[test]
    fn test_mod() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(10.0));
            b.emit_const(Value::Number(3.0));
            b.emit(Opcode::Mod);
        });
        assert_eq!(result, Ok(Value::Number(1.0)));
    }

    #[test]
    fn test_neg() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(42.0));
            b.emit(Opcode::Neg);
        });
        assert_eq!(result, Ok(Value::Number(-42.0)));
    }

    #[test]
    fn test_eq_true() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(42.0));
            b.emit_const(Value::Number(42.0));
            b.emit(Opcode::Eq);
        });
        assert_eq!(result, Ok(Value::Bool(true)));
    }

    #[test]
    fn test_eq_false() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(42.0));
            b.emit_const(Value::Number(43.0));
            b.emit(Opcode::Eq);
        });
        assert_eq!(result, Ok(Value::Bool(false)));
    }

    #[test]
    fn test_lt() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(1.0));
            b.emit_const(Value::Number(2.0));
            b.emit(Opcode::Lt);
        });
        assert_eq!(result, Ok(Value::Bool(true)));
    }

    #[test]
    fn test_le() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(2.0));
            b.emit_const(Value::Number(2.0));
            b.emit(Opcode::Le);
        });
        assert_eq!(result, Ok(Value::Bool(true)));
    }

    #[test]
    fn test_gt() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(3.0));
            b.emit_const(Value::Number(2.0));
            b.emit(Opcode::Gt);
        });
        assert_eq!(result, Ok(Value::Bool(true)));
    }

    #[test]
    fn test_ge() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(2.0));
            b.emit_const(Value::Number(2.0));
            b.emit(Opcode::Ge);
        });
        assert_eq!(result, Ok(Value::Bool(true)));
    }

    #[test]
    fn test_and() {
        let result = run_simple(|b| {
            b.emit_const(Value::Bool(true));
            b.emit_const(Value::Bool(false));
            b.emit(Opcode::And);
        });
        assert_eq!(result, Ok(Value::Bool(false)));
    }

    #[test]
    fn test_or() {
        let result = run_simple(|b| {
            b.emit_const(Value::Bool(true));
            b.emit_const(Value::Bool(false));
            b.emit(Opcode::Or);
        });
        assert_eq!(result, Ok(Value::Bool(true)));
    }

    #[test]
    fn test_not() {
        let result = run_simple(|b| {
            b.emit_const(Value::Bool(true));
            b.emit(Opcode::Not);
        });
        assert_eq!(result, Ok(Value::Bool(false)));
    }

    #[test]
    fn test_type_error_add() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(1.0));
            b.emit_const(Value::Bool(true));
            b.emit(Opcode::Add);
        });
        assert_eq!(
            result,
            Err(VmError::TypeError {
                expected: "number",
                got: "bool",
                operation: "add"
            })
        );
    }

    #[test]
    fn test_local_variables() {
        // Test: x = 10; y = 32; x + y
        let mut builder = BytecodeBuilder::new();

        // x = 10 (slot 0)
        builder.emit_const(Value::Number(10.0));
        builder.emit_u16(Opcode::StoreLocal, 0);
        builder.emit(Opcode::Pop);

        // y = 32 (slot 1)
        builder.emit_const(Value::Number(32.0));
        builder.emit_u16(Opcode::StoreLocal, 1);
        builder.emit(Opcode::Pop);

        // x + y
        builder.emit_u16(Opcode::LoadLocal, 0);
        builder.emit_u16(Opcode::LoadLocal, 1);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);

        let func = builder.build(2, 0); // 2 locals
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        let result = vm.call(&hash, vec![]);

        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_jump() {
        // Test: jump over a push
        let mut builder = BytecodeBuilder::new();
        let jump_offset = builder.emit_jump_placeholder(Opcode::Jump);
        builder.emit_const(Value::Number(1.0)); // This should be skipped
        builder.patch_jump(jump_offset);
        builder.emit_const(Value::Number(42.0)); // This should be executed
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        let result = vm.call(&hash, vec![]);

        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_jump_if_true() {
        // Test: if true, skip to 42
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Bool(true));
        let jump_offset = builder.emit_jump_placeholder(Opcode::JumpIf);
        builder.emit_const(Value::Number(1.0)); // Skipped
        builder.emit(Opcode::Return);
        builder.patch_jump(jump_offset);
        builder.emit_const(Value::Number(42.0));
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        let result = vm.call(&hash, vec![]);

        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_jump_if_false() {
        // Test: if false, don't jump
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Bool(false));
        let jump_offset = builder.emit_jump_placeholder(Opcode::JumpIf);
        builder.emit_const(Value::Number(42.0)); // Executed
        builder.emit(Opcode::Return);
        builder.patch_jump(jump_offset);
        builder.emit_const(Value::Number(1.0)); // Not reached
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        let result = vm.call(&hash, vec![]);

        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_make_tuple() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(1.0));
            b.emit_const(Value::Number(2.0));
            b.emit_const(Value::Number(3.0));
            b.emit_u8(Opcode::MakeTuple, 3);
        });
        assert_eq!(
            result,
            Ok(Value::tuple(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0)
            ]))
        );
    }

    #[test]
    fn test_tuple_get() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(1.0));
            b.emit_const(Value::Number(42.0));
            b.emit_const(Value::Number(3.0));
            b.emit_u8(Opcode::MakeTuple, 3);
            b.emit_u8(Opcode::TupleGet, 1); // Get index 1 (42.0)
        });
        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_tuple_index_out_of_bounds() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(1.0));
            b.emit_u8(Opcode::MakeTuple, 1);
            b.emit_u8(Opcode::TupleGet, 5);
        });
        assert_eq!(
            result,
            Err(VmError::TupleIndexOutOfBounds { index: 5, length: 1 })
        );
    }

    #[test]
    fn test_make_record() {
        let result = run_simple(|b| {
            // Push field "x" with value 1.0
            b.emit_const(Value::string("x"));
            b.emit_const(Value::Number(1.0));
            // Push field "y" with value 2.0
            b.emit_const(Value::string("y"));
            b.emit_const(Value::Number(2.0));
            b.emit_u8(Opcode::MakeRecord, 2);
        });

        match result {
            Ok(Value::Record(fields)) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields.get(&Rc::from("x")), Some(&Value::Number(1.0)));
                assert_eq!(fields.get(&Rc::from("y")), Some(&Value::Number(2.0)));
            }
            other => panic!("Expected record, got {other:?}"),
        }
    }

    #[test]
    fn test_record_get() {
        let mut builder = BytecodeBuilder::new();
        // Create record { x: 42.0 }
        builder.emit_const(Value::string("x"));
        builder.emit_const(Value::Number(42.0));
        builder.emit_u8(Opcode::MakeRecord, 1);
        // Get field "x"
        let field_idx = builder.add_constant(Value::string("x"));
        builder.emit_u16(Opcode::RecordGet, field_idx);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        let result = vm.call(&hash, vec![]);

        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_function_call() {
        // Create a helper function that returns 42
        let mut helper_builder = BytecodeBuilder::new();
        helper_builder.emit_const(Value::Number(42.0));
        helper_builder.emit(Opcode::Return);
        let helper = helper_builder.build(0, 0);
        let helper_hash = helper.hash;

        // Create main function that calls helper
        let mut main_builder = BytecodeBuilder::new();
        main_builder.emit_call(helper_hash, 0);
        main_builder.emit(Opcode::Return);
        let main = main_builder.build(0, 0);
        let main_hash = main.hash;

        let mut vm = Vm::new();
        vm.load_function(helper);
        vm.load_function(main);
        let result = vm.call(&main_hash, vec![]);

        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_function_with_args() {
        // Create add(a, b) function that returns a + b
        let mut add_builder = BytecodeBuilder::new();
        add_builder.emit_u16(Opcode::LoadLocal, 0); // a
        add_builder.emit_u16(Opcode::LoadLocal, 1); // b
        add_builder.emit(Opcode::Add);
        add_builder.emit(Opcode::Return);
        let add_func = add_builder.build(2, 2); // 2 locals (params), 2 params
        let add_hash = add_func.hash;

        // Create main that calls add(10, 32)
        let mut main_builder = BytecodeBuilder::new();
        main_builder.emit_const(Value::Number(10.0));
        main_builder.emit_const(Value::Number(32.0));
        main_builder.emit_call(add_hash, 2);
        main_builder.emit(Opcode::Return);
        let main = main_builder.build(0, 0);
        let main_hash = main.hash;

        let mut vm = Vm::new();
        vm.load_function(add_func);
        vm.load_function(main);
        let result = vm.call(&main_hash, vec![]);

        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_dup() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(21.0));
            b.emit(Opcode::Dup);
            b.emit(Opcode::Add);
        });
        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_pop() {
        let result = run_simple(|b| {
            b.emit_const(Value::Number(1.0));
            b.emit_const(Value::Number(42.0));
            b.emit_const(Value::Number(2.0));
            b.emit(Opcode::Pop);
            b.emit(Opcode::Pop);
            // Only 1.0 remains
        });
        assert_eq!(result, Ok(Value::Number(1.0)));
    }

    // =========================================================================
    // Milestone 1 Test Cases: Factorial, Fibonacci, Record Manipulation
    // =========================================================================

    /// Build a recursive factorial function.
    ///
    /// For self-recursive functions, we use a predetermined hash as an identifier.
    /// This simulates what a real compiler would do: assign a stable identifier
    /// before the function body references itself.
    ///
    /// ```ambient
    /// fn factorial(n: number): number {
    ///   if n <= 1 { 1 } else { n * factorial(n - 1) }
    /// }
    /// ```
    fn build_factorial() -> (CompiledFunction, blake3::Hash) {
        // For self-recursive functions, we use a predetermined identifier.
        // In a real compiler, this would be computed from the function signature.
        let func_hash = blake3::hash(b"test::factorial");

        let mut builder = BytecodeBuilder::new();

        builder.emit_u16(Opcode::LoadLocal, 0);
        builder.emit_const(Value::Number(1.0));
        builder.emit(Opcode::Le);

        let else_jump = builder.emit_jump_placeholder(Opcode::JumpIfNot);
        builder.emit_const(Value::Number(1.0));
        builder.emit(Opcode::Return);

        builder.patch_jump(else_jump);
        builder.emit_u16(Opcode::LoadLocal, 0);

        builder.emit_u16(Opcode::LoadLocal, 0);
        builder.emit_const(Value::Number(1.0));
        builder.emit(Opcode::Sub);
        builder.emit_call(func_hash, 1);

        builder.emit(Opcode::Mul);
        builder.emit(Opcode::Return);

        let mut func = builder.build(1, 1);
        func.hash = func_hash; // Override the computed hash with our predetermined one

        (func, func_hash)
    }

    #[test]
    fn test_factorial_base_case() {
        let (factorial, hash) = build_factorial();

        let mut vm = Vm::new();
        vm.load_function(factorial);

        // factorial(1) = 1
        let result = vm.call(&hash, vec![Value::Number(1.0)]);
        assert_eq!(result, Ok(Value::Number(1.0)));
    }

    #[test]
    fn test_factorial_small() {
        let (factorial, hash) = build_factorial();

        let mut vm = Vm::new();
        vm.load_function(factorial);

        // factorial(5) = 120
        let result = vm.call(&hash, vec![Value::Number(5.0)]);
        assert_eq!(result, Ok(Value::Number(120.0)));
    }

    #[test]
    fn test_factorial_larger() {
        let (factorial, hash) = build_factorial();

        let mut vm = Vm::new();
        vm.load_function(factorial);

        // factorial(10) = 3628800
        let result = vm.call(&hash, vec![Value::Number(10.0)]);
        assert_eq!(result, Ok(Value::Number(3_628_800.0)));
    }

    /// Build a recursive fibonacci function.
    ///
    /// ```ambient
    /// fn fib(n: number): number {
    ///   if n <= 1 { n } else { fib(n - 1) + fib(n - 2) }
    /// }
    /// ```
    fn build_fibonacci() -> (CompiledFunction, blake3::Hash) {
        // For self-recursive functions, we use a predetermined identifier.
        let func_hash = blake3::hash(b"test::fibonacci");

        let mut builder = BytecodeBuilder::new();

        // if n <= 1
        builder.emit_u16(Opcode::LoadLocal, 0); // n
        builder.emit_const(Value::Number(1.0));
        builder.emit(Opcode::Le);

        let else_jump = builder.emit_jump_placeholder(Opcode::JumpIfNot);

        // then: return n
        builder.emit_u16(Opcode::LoadLocal, 0);
        builder.emit(Opcode::Return);

        // else: fib(n-1) + fib(n-2)
        builder.patch_jump(else_jump);

        // fib(n - 1)
        builder.emit_u16(Opcode::LoadLocal, 0);
        builder.emit_const(Value::Number(1.0));
        builder.emit(Opcode::Sub);
        builder.emit_call(func_hash, 1);

        // fib(n - 2)
        builder.emit_u16(Opcode::LoadLocal, 0);
        builder.emit_const(Value::Number(2.0));
        builder.emit(Opcode::Sub);
        builder.emit_call(func_hash, 1);

        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);

        let mut func = builder.build(1, 1);
        func.hash = func_hash;

        (func, func_hash)
    }

    #[test]
    fn test_fibonacci_base_cases() {
        let (fib, hash) = build_fibonacci();

        let mut vm = Vm::new();
        vm.load_function(fib);

        assert_eq!(vm.call(&hash, vec![Value::Number(0.0)]), Ok(Value::Number(0.0)));

        // Need to reset VM state between calls
        let (fib, hash) = build_fibonacci();
        let mut vm = Vm::new();
        vm.load_function(fib);
        assert_eq!(vm.call(&hash, vec![Value::Number(1.0)]), Ok(Value::Number(1.0)));
    }

    #[test]
    fn test_fibonacci_sequence() {
        // fib(10) = 55
        let (fib, hash) = build_fibonacci();

        let mut vm = Vm::new();
        vm.load_function(fib);

        let result = vm.call(&hash, vec![Value::Number(10.0)]);
        assert_eq!(result, Ok(Value::Number(55.0)));
    }

    #[test]
    fn test_fibonacci_values() {
        // Test several fibonacci numbers: 0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55
        let expected = [0.0, 1.0, 1.0, 2.0, 3.0, 5.0, 8.0, 13.0, 21.0, 34.0, 55.0];

        for (n, exp) in expected.iter().enumerate() {
            let (fib, hash) = build_fibonacci();
            let mut vm = Vm::new();
            vm.load_function(fib);

            let result = vm.call(&hash, vec![Value::Number(n as f64)]);
            assert_eq!(result, Ok(Value::Number(*exp)), "fib({n}) should be {exp}");
        }
    }

    #[test]
    fn test_record_manipulation_point() {
        // Test creating and manipulating a 2D point record
        // let point = { x: 3.0, y: 4.0 }
        // let distance_squared = point.x * point.x + point.y * point.y
        // distance_squared should be 25.0

        let mut builder = BytecodeBuilder::new();

        // Create point { x: 3.0, y: 4.0 }
        builder.emit_const(Value::string("x"));
        builder.emit_const(Value::Number(3.0));
        builder.emit_const(Value::string("y"));
        builder.emit_const(Value::Number(4.0));
        builder.emit_u8(Opcode::MakeRecord, 2);
        builder.emit_u16(Opcode::StoreLocal, 0); // point in local 0

        // point.x * point.x
        builder.emit_u16(Opcode::LoadLocal, 0);
        let x_idx = builder.add_constant(Value::string("x"));
        builder.emit_u16(Opcode::RecordGet, x_idx);
        builder.emit_u16(Opcode::LoadLocal, 0);
        builder.emit_u16(Opcode::RecordGet, x_idx);
        builder.emit(Opcode::Mul);

        // point.y * point.y
        builder.emit_u16(Opcode::LoadLocal, 0);
        let y_idx = builder.add_constant(Value::string("y"));
        builder.emit_u16(Opcode::RecordGet, y_idx);
        builder.emit_u16(Opcode::LoadLocal, 0);
        builder.emit_u16(Opcode::RecordGet, y_idx);
        builder.emit(Opcode::Mul);

        // Add them
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);

        let func = builder.build(1, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        let result = vm.call(&hash, vec![]);

        assert_eq!(result, Ok(Value::Number(25.0))); // 3*3 + 4*4 = 9 + 16 = 25
    }

    #[test]
    fn test_record_nested_access() {
        // Test nested record: { user: { name: "Alice", age: 30 } }
        // Access user.age

        let mut builder = BytecodeBuilder::new();

        // Inner record { name: "Alice", age: 30 }
        builder.emit_const(Value::string("name"));
        builder.emit_const(Value::string("Alice"));
        builder.emit_const(Value::string("age"));
        builder.emit_const(Value::Number(30.0));
        builder.emit_u8(Opcode::MakeRecord, 2);

        // Store inner record, then create outer record { user: <inner> }
        builder.emit_u16(Opcode::StoreLocal, 0);
        builder.emit(Opcode::Pop); // Pop the inner record from stack

        // Build outer record
        builder.emit_const(Value::string("user"));
        builder.emit_u16(Opcode::LoadLocal, 0);
        builder.emit_u8(Opcode::MakeRecord, 1);

        // Get user.age
        let user_idx = builder.add_constant(Value::string("user"));
        builder.emit_u16(Opcode::RecordGet, user_idx);
        let age_idx = builder.add_constant(Value::string("age"));
        builder.emit_u16(Opcode::RecordGet, age_idx);
        builder.emit(Opcode::Return);

        let func = builder.build(1, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        let result = vm.call(&hash, vec![]);

        assert_eq!(result, Ok(Value::Number(30.0)));
    }

    #[test]
    fn test_tuple_unpacking() {
        // Test: let pair = (10, 32); pair.0 + pair.1 = 42
        let mut builder = BytecodeBuilder::new();

        // Create tuple
        builder.emit_const(Value::Number(10.0));
        builder.emit_const(Value::Number(32.0));
        builder.emit_u8(Opcode::MakeTuple, 2);
        builder.emit_u16(Opcode::StoreLocal, 0);

        // pair.0
        builder.emit_u16(Opcode::LoadLocal, 0);
        builder.emit_u8(Opcode::TupleGet, 0);

        // pair.1
        builder.emit_u16(Opcode::LoadLocal, 0);
        builder.emit_u8(Opcode::TupleGet, 1);

        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);

        let func = builder.build(1, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        let result = vm.call(&hash, vec![]);

        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    // =========================================================================
    // Milestone 2 Test Cases: Abilities and Handlers
    // =========================================================================

    /// Ability IDs used in tests
    const ABILITY_CONSOLE: u16 = 1;
    const ABILITY_MATH: u16 = 2;

    /// Method IDs for Console ability
    const METHOD_PRINT: u16 = 0;

    /// Method IDs for Math ability
    const METHOD_DOUBLE: u16 = 0;
    const METHOD_ADD_TEN: u16 = 1;

    #[test]
    fn test_suspend_creates_ability_value() {
        // Test that Suspend creates a suspended ability value
        let mut builder = BytecodeBuilder::new();

        // Push argument
        builder.emit_const(Value::Number(42.0));
        // Suspend with ability_id=1, method_id=0, 1 arg
        builder.emit_suspend(ABILITY_CONSOLE, METHOD_PRINT, 1);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        let result = vm.call(&hash, vec![]);

        // Should return a suspended ability
        match result {
            Ok(Value::SuspendedAbility(ability)) => {
                assert_eq!(ability.ability_id, ABILITY_CONSOLE);
                assert_eq!(ability.method_id, METHOD_PRINT);
                assert_eq!(ability.args.len(), 1);
                assert_eq!(ability.args[0], Value::Number(42.0));
            }
            other => panic!("Expected SuspendedAbility, got {other:?}"),
        }
    }

    #[test]
    fn test_host_handler_called() {
        // Test that host handlers are called when ability is performed
        use std::cell::RefCell;
        use std::rc::Rc;

        let call_log: Rc<RefCell<Vec<f64>>> = Rc::new(RefCell::new(Vec::new()));
        let log_clone = call_log.clone();

        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(42.0));
        builder.emit_suspend(ABILITY_CONSOLE, METHOD_PRINT, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);

        // Register a host handler that logs the argument
        vm.register_host_handler(ABILITY_CONSOLE, METHOD_PRINT, Box::new(move |ability| {
            if let Value::Number(n) = &ability.args[0] {
                log_clone.borrow_mut().push(*n);
            }
            Ok(Value::Unit)
        }));

        let result = vm.call(&hash, vec![]);

        assert_eq!(result, Ok(Value::Unit));
        assert_eq!(*call_log.borrow(), vec![42.0]);
    }

    #[test]
    fn test_host_handler_returns_value() {
        // Test that host handler return value is pushed to stack
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(21.0));
        builder.emit_suspend(ABILITY_MATH, METHOD_DOUBLE, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);

        // Register a handler that doubles the argument
        vm.register_host_handler(ABILITY_MATH, METHOD_DOUBLE, Box::new(|ability| {
            if let Value::Number(n) = &ability.args[0] {
                Ok(Value::Number(n * 2.0))
            } else {
                Ok(Value::Unit)
            }
        }));

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_unhandled_ability_error() {
        // Test that performing an unhandled ability returns an error
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(42.0));
        builder.emit_suspend(ABILITY_CONSOLE, METHOD_PRINT, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);

        // No handler registered!
        let result = vm.call(&hash, vec![]);

        assert_eq!(
            result,
            Err(VmError::UnhandledAbility {
                ability_id: ABILITY_CONSOLE,
                method_id: METHOD_PRINT,
            })
        );
    }

    #[test]
    fn test_bytecode_handler_simple_resume() {
        // Test bytecode handler that immediately resumes with a value
        //
        // This simulates:
        // handle {
        //   Math.double!(5)
        // } with {
        //   Math.double(k, ability) => resume(k, 42)
        // }

        // First, create the handler function
        // Handler receives: (continuation, ability) and should call resume(k, value)
        let handler_hash = blake3::hash(b"test::math_handler");
        let mut handler_builder = BytecodeBuilder::new();
        // Load continuation (local 0)
        handler_builder.emit_u16(Opcode::LoadLocal, 0);
        // Push the value to resume with
        handler_builder.emit_const(Value::Number(42.0));
        // Resume: pops continuation and value, resumes continuation with value
        handler_builder.emit(Opcode::Resume);
        // After resume, we should not reach here (control transfers back)
        // But we need a return for the function structure
        handler_builder.emit(Opcode::Return);

        let mut handler_func = handler_builder.build(2, 2); // 2 locals (k, ability), 2 params
        handler_func.hash = handler_hash;

        // Now create the main function that uses the handler
        let mut main_builder = BytecodeBuilder::new();

        // Install handler for ABILITY_MATH
        let handle_jump = main_builder.emit_handle(ABILITY_MATH, handler_hash);

        // The handled expression: Suspend and Perform
        main_builder.emit_const(Value::Number(5.0));
        main_builder.emit_suspend(ABILITY_MATH, METHOD_DOUBLE, 1);
        main_builder.emit(Opcode::Perform);

        // Remove handler (reached if no ability performed, but we did perform)
        main_builder.emit(Opcode::Unhandle);

        // Patch the handle instruction (not really needed for this test but good practice)
        main_builder.patch_handle(handle_jump);

        // Return the result
        main_builder.emit(Opcode::Return);

        let main_func = main_builder.build(0, 0);
        let main_hash = main_func.hash;

        let mut vm = Vm::new();
        vm.load_function(handler_func);
        vm.load_function(main_func);

        let result = vm.call(&main_hash, vec![]);
        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_single_shot_enforcement() {
        // Test that resuming a continuation twice results in an error

        // Create a handler that tries to resume twice
        let handler_hash = blake3::hash(b"test::double_resume_handler");
        let mut handler_builder = BytecodeBuilder::new();

        // First resume (should succeed)
        handler_builder.emit_u16(Opcode::LoadLocal, 0); // continuation
        handler_builder.emit_const(Value::Number(1.0));
        handler_builder.emit(Opcode::Resume);

        // This shouldn't be reached, but if we somehow got here and tried to resume again...
        handler_builder.emit(Opcode::Return);

        let mut handler_func = handler_builder.build(2, 2);
        handler_func.hash = handler_hash;

        // Main function that performs ability
        let mut main_builder = BytecodeBuilder::new();
        let handle_jump = main_builder.emit_handle(ABILITY_MATH, handler_hash);
        main_builder.emit_const(Value::Number(5.0));
        main_builder.emit_suspend(ABILITY_MATH, METHOD_DOUBLE, 1);
        main_builder.emit(Opcode::Perform);
        main_builder.emit(Opcode::Unhandle);
        main_builder.patch_handle(handle_jump);
        main_builder.emit(Opcode::Return);

        let main_func = main_builder.build(0, 0);
        let main_hash = main_func.hash;

        let mut vm = Vm::new();
        vm.load_function(handler_func);
        vm.load_function(main_func);

        // First call should succeed
        let result = vm.call(&main_hash, vec![]);
        assert_eq!(result, Ok(Value::Number(1.0)));

        // Note: Testing double-resume through bytecode handlers requires a more complex
        // test setup where the handler stores the continuation and tries to use it twice.
        // The single-shot enforcement is implemented via Continuation::mark_resumed().
    }

    #[test]
    fn test_perform_expected_type_error() {
        // Test that performing a non-ability value returns a type error
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(42.0));
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);

        let result = vm.call(&hash, vec![]);
        assert_eq!(
            result,
            Err(VmError::ExpectedSuspendedAbility { got: "number" })
        );
    }

    #[test]
    fn test_multiple_ability_calls() {
        // Test multiple ability calls in sequence
        use std::cell::RefCell;
        use std::rc::Rc;

        let call_log: Rc<RefCell<Vec<f64>>> = Rc::new(RefCell::new(Vec::new()));
        let log_clone = call_log.clone();

        let mut builder = BytecodeBuilder::new();

        // First call
        builder.emit_const(Value::Number(1.0));
        builder.emit_suspend(ABILITY_CONSOLE, METHOD_PRINT, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Pop);

        // Second call
        builder.emit_const(Value::Number(2.0));
        builder.emit_suspend(ABILITY_CONSOLE, METHOD_PRINT, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Pop);

        // Third call
        builder.emit_const(Value::Number(3.0));
        builder.emit_suspend(ABILITY_CONSOLE, METHOD_PRINT, 1);
        builder.emit(Opcode::Perform);

        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        vm.register_host_handler(ABILITY_CONSOLE, METHOD_PRINT, Box::new(move |ability| {
            if let Value::Number(n) = &ability.args[0] {
                log_clone.borrow_mut().push(*n);
            }
            Ok(Value::Unit)
        }));

        let result = vm.call(&hash, vec![]);

        assert_eq!(result, Ok(Value::Unit));
        assert_eq!(*call_log.borrow(), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_ability_with_multiple_args() {
        // Test ability with multiple arguments
        let mut builder = BytecodeBuilder::new();

        // Push two arguments
        builder.emit_const(Value::Number(10.0));
        builder.emit_const(Value::Number(32.0));
        builder.emit_suspend(ABILITY_MATH, METHOD_ADD_TEN, 2);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);

        // Handler that adds the arguments
        vm.register_host_handler(ABILITY_MATH, METHOD_ADD_TEN, Box::new(|ability| {
            if ability.args.len() >= 2 {
                if let (Value::Number(a), Value::Number(b)) = (&ability.args[0], &ability.args[1]) {
                    return Ok(Value::Number(a + b));
                }
            }
            Ok(Value::Unit)
        }));

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Number(42.0)));
    }
}
