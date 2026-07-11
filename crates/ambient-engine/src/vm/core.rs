//! Core VM structures and infrastructure.

use std::collections::HashMap;
use std::sync::Arc;

use ambient_ability::{RuntimeError, StackTraceFrame, Value, VmError};
use ambient_core::AbilityId;

use crate::bytecode::{CompiledFunction, Opcode};

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
}

/// An installed ability handler that can intercept ability operations.
///
/// A handler delimits the computation of the frame it was installed in
/// (the handle expression's body thunk, or the entry frame for base
/// handlers). Everything at or above `boundary_frame_idx` is inside the
/// delimited region: performing the handled ability there captures
/// `frames[boundary..]` (and the stack from the boundary frame's base
/// pointer) into a continuation.
#[derive(Debug, Clone)]
pub(super) struct HandlerFrame {
    /// The ability ID this handler handles.
    pub ability_id: AbilityId,

    /// The installed handler value (per-method functions + shared captures).
    pub handler: std::sync::Arc<ambient_ability::HandlerValue>,

    /// Index of the first call frame inside the delimited region.
    pub boundary_frame_idx: usize,
}

/// The Ambient virtual machine.
pub struct Vm {
    /// The value stack.
    pub(super) stack: Vec<Value>,

    /// The call stack.
    pub(super) frames: Vec<CallFrame>,

    /// The handler stack for installed ability handlers.
    pub(super) handlers: Vec<HandlerFrame>,

    /// Handlers installed under every call (see `install_base_handler`).
    /// Re-installed after each `call`/`call_closure` state reset.
    pub(super) base_handlers: Vec<Arc<ambient_ability::HandlerValue>>,

    /// Content-addressed function store.
    pub(super) functions: HashMap<blake3::Hash, Arc<CompiledFunction>>,

    /// Content-addressed `const` value objects, keyed by object hash. The
    /// `LoadObject` opcode resolves against this map.
    pub(super) values: HashMap<blake3::Hash, Value>,

    /// Native (extern fn) objects loaded into this VM: content hash →
    /// `(uuid, param_count)`. Calls to one of these hashes dispatch to the
    /// implementation registered for the uuid instead of pushing a frame.
    pub(super) native_functions: HashMap<blake3::Hash, (uuid::Uuid, u8)>,

    /// Native implementations, keyed by their stable uuid. Calling a loaded
    /// native whose uuid has no entry here is a loud
    /// [`VmError::UnboundNative`] — the safety net for code compiled
    /// against a host binding this VM does not provide.
    pub(super) native_impls: HashMap<uuid::Uuid, crate::natives::NativeFn>,

    /// VM-invoking native implementations (see
    /// [`crate::natives::NativeVmFn`]). Checked before `native_impls`, so
    /// registering one overrides a pure implementation (or stub) under the
    /// same uuid.
    pub(super) native_vm_impls: HashMap<uuid::Uuid, crate::natives::NativeVmFn>,

    /// Handler-visibility barriers for reentrant [`Self::invoke`] calls,
    /// one entry per nesting level: performs inside the nested region only
    /// dispatch to handlers at or above the barrier index. A continuation
    /// cannot capture the invoking native's Rust frame, so handlers below
    /// it must be invisible — nested performs fall through to default
    /// implementations instead.
    pub(super) handler_barriers: Vec<usize>,

    /// Host hard-stop flag (see [`Self::set_interrupt_flag`]). When set,
    /// the execution loop aborts with [`VmError::HardStopped`] at its
    /// next check — the runtime's "next opportunity" for a computation
    /// that failed to reach an interruptible perform before a drain
    /// deadline. Host wiring, like natives: it survives the state reset
    /// at the start of each `call`.
    pub(super) interrupt: Option<Arc<std::sync::atomic::AtomicBool>>,

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
    ///
    /// Every VM starts with the engine's core native implementations
    /// installed (see [`crate::natives::core_natives`]): natives are pure
    /// value transformations, so even isolated Execute VMs get them
    /// unconditionally. Embedder natives are registered separately
    /// ([`Self::register_natives`]).
    #[must_use]
    pub fn new() -> Self {
        let mut vm = Self {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            handlers: Vec::with_capacity(16),
            base_handlers: Vec::new(),
            functions: HashMap::new(),
            values: HashMap::new(),
            native_functions: HashMap::new(),
            native_impls: HashMap::new(),
            native_vm_impls: HashMap::new(),
            handler_barriers: Vec::new(),
            interrupt: None,
            max_call_depth: 1000,
        };
        vm.register_natives(crate::natives::core_natives());
        vm
    }

    /// Load a compiled function into the VM.
    pub fn load_function(&mut self, func: CompiledFunction) {
        let hash = func.hash;
        self.functions.insert(hash, Arc::new(func));
    }

    /// Load a content-addressed `const` value object into the VM.
    ///
    /// Additive and content-addressed like [`Self::load_function`]:
    /// re-loading a hash the VM already knows is a harmless overwrite with
    /// an identical value.
    pub fn load_value(&mut self, hash: blake3::Hash, value: Value) {
        self.values.insert(hash, value);
    }

    /// Load a native (extern fn) object into the VM: calls to `hash` will
    /// dispatch to the implementation registered for `uuid`. Additive and
    /// content-addressed like [`Self::load_function`]. Loading does not
    /// require the implementation to exist yet — binding is checked at the
    /// first call, so hosts can load code before wiring implementations.
    pub fn load_native(&mut self, hash: blake3::Hash, uuid: uuid::Uuid, param_count: u8) {
        self.native_functions.insert(hash, (uuid, param_count));
    }

    /// Register the implementation for a native uuid.
    pub fn register_native_impl(&mut self, uuid: uuid::Uuid, func: crate::natives::NativeFn) {
        self.native_impls.insert(uuid, func);
    }

    /// Register a VM-invoking implementation for a native uuid. Takes
    /// priority over a pure implementation (typically a stub) registered
    /// under the same uuid — the same override rule as uuid-keyed
    /// re-registration.
    pub fn register_native_vm_impl(&mut self, uuid: uuid::Uuid, func: crate::natives::NativeVmFn) {
        self.native_vm_impls.insert(uuid, func);
    }

    /// Wire the host's hard-stop flag: while it is set, the execution
    /// loop aborts with [`VmError::HardStopped`] at its next periodic
    /// check instead of fetching another opcode. The host sets the flag
    /// from another thread (a drain-deadline watchdog, say) to stop a
    /// computation that never reaches an interruptible perform. A native
    /// blocked in the host is not interrupted by this — the flag is
    /// observed only between opcodes.
    pub fn set_interrupt_flag(&mut self, flag: Arc<std::sync::atomic::AtomicBool>) {
        self.interrupt = Some(flag);
    }

    /// Register every implementation in a [`NativeRegistry`] — how a VM
    /// inherits the host's bindings at wiring time.
    ///
    /// [`NativeRegistry`]: crate::natives::NativeRegistry
    pub fn register_natives(&mut self, natives: &crate::natives::NativeRegistry) {
        for (uuid, func) in natives.impls() {
            self.native_impls.insert(uuid, func);
        }
    }

    /// Load an already-shared compiled function into the VM.
    ///
    /// Loading is additive and content-addressed: re-loading a hash the
    /// VM already knows is a no-op, so code generations can be layered
    /// onto a live VM (the deploy runtime does this on every deploy).
    pub fn load_function_shared(&mut self, func: Arc<CompiledFunction>) {
        self.functions.insert(func.hash, func);
    }

    /// Whether a function is loaded in this VM (any object kind a call
    /// can dispatch to: bytecode function or native object). How a host
    /// checks that a late-bound resolution's target is present before
    /// handing the ref to running code — a deploy may have bound names
    /// to objects this VM has not been topped up with yet.
    #[must_use]
    pub fn has_function(&self, hash: &blake3::Hash) -> bool {
        self.functions.contains_key(hash) || self.native_functions.contains_key(hash)
    }

    /// Install a first-class handler value at the base of the VM.
    ///
    /// Base handlers sit under every call frame, so performs anywhere in a
    /// subsequent `call` dispatch to them (taking priority over host
    /// handlers, like any bytecode handler). They survive the state reset
    /// at the start of each `call`. This is how isolated execution
    /// installs handlers that shipped with the code: the handler value's
    /// method functions must already be loaded into this VM.
    pub fn install_base_handler(&mut self, handler_value: Arc<ambient_ability::HandlerValue>) {
        self.base_handlers.push(handler_value);
    }

    /// Push handler frames for every installed base handler.
    fn install_base_frames(&mut self) {
        let base = std::mem::take(&mut self.base_handlers);
        for handler_value in &base {
            self.handlers.push(HandlerFrame {
                ability_id: handler_value.ability_id,
                handler: Arc::clone(handler_value),
                boundary_frame_idx: 0,
            });
        }
        self.base_handlers = base;
    }

    /// Call a function by its hash with the given arguments.
    pub fn call(&mut self, hash: &blake3::Hash, args: Vec<Value>) -> Result<Value, VmError> {
        // Reset state
        self.stack.clear();
        self.frames.clear();
        self.handlers.clear();
        self.handler_barriers.clear();
        self.install_base_frames();

        let arg_count = args.len() as u8;

        // Push arguments onto stack
        for arg in args {
            self.stack.push(arg);
        }

        // Set up initial call frame. A native entry point executes inline
        // (no frame), so its result is already on the stack.
        if !self.push_frame(hash, arg_count)? {
            return self.pop();
        }

        // Run the execution loop
        self.run_until(0)
    }

    /// Call a function and return a `RuntimeError` with stack trace on failure.
    ///
    /// This is the preferred method for calling functions when you want
    /// rich error messages with source locations.
    pub fn call_with_trace(
        &mut self,
        hash: &blake3::Hash,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        self.call(hash, args).map_err(|e| self.runtime_error(e))
    }

    /// Call a closure (function + captured environment).
    ///
    /// This is used for executing closures remotely where the captured
    /// values need to be passed along with the function call.
    pub fn call_closure(
        &mut self,
        hash: &blake3::Hash,
        args: Vec<Value>,
        captures: Vec<Value>,
    ) -> Result<Value, VmError> {
        // Reset state
        self.stack.clear();
        self.frames.clear();
        self.handlers.clear();
        self.handler_barriers.clear();
        self.install_base_frames();

        let arg_count = args.len() as u8;

        // Push arguments onto stack
        for arg in args {
            self.stack.push(arg);
        }

        // Set up initial call frame with captures. A native entry point
        // executes inline (no frame), so its result is already on the stack.
        if !self.push_frame_with_captures(hash, arg_count, captures)? {
            return self.pop();
        }

        // Run the execution loop
        self.run_until(0)
    }

    /// Invoke a function value (function ref or closure) *reentrantly*, in
    /// the middle of whatever this VM is executing — the channel through
    /// which a VM-invoking native ([`crate::natives::NativeVmFn`]) runs a
    /// function it was handed (e.g. `State::update`'s `f`).
    ///
    /// The callee runs on this VM's stacks in a nested execution loop that
    /// returns when the callee's entry frame returns. Ability dispatch
    /// inside the nested region is **delimited at the invoke boundary**:
    /// performs see handlers installed within the callee, but never the
    /// caller's handler stack — a captured continuation cannot span the
    /// native's Rust frame, so anything below the boundary falls through
    /// to default implementations, and an uncaught nested exception
    /// propagates out as `Err(VmError::Exception)` (which, returned from
    /// the native, re-raises at the caller's own call site).
    ///
    /// On error, every VM height (stack, frames, handlers) is restored to
    /// its pre-invoke value, so the caller's execution state is intact.
    pub fn invoke(&mut self, callee: &Value, args: Vec<Value>) -> Result<Value, VmError> {
        let (hash, captures) = match callee {
            Value::Closure(c) => (c.function_hash, c.environment.clone()),
            Value::FunctionRef(hash) => (*hash, Vec::new()),
            other => {
                return Err(VmError::TypeError {
                    expected: "function",
                    got: other.type_name(),
                    operation: "invoke",
                });
            }
        };

        let base_frames = self.frames.len();
        let base_stack = self.stack.len();
        let base_handlers = self.handlers.len();

        let arg_count = args.len() as u8;
        for arg in args {
            self.stack.push(arg);
        }

        self.handler_barriers.push(base_handlers);
        let result = (|| {
            // A native callee executes inline: its result is on the stack.
            if !self.push_frame_with_captures(&hash, arg_count, captures)? {
                return self.pop();
            }
            self.run_until(base_frames)
        })();
        self.handler_barriers.pop();

        match result {
            Ok(value) => {
                // A clean return already restored every height; truncation
                // is a no-op then, and re-levels the VM if the callee ended
                // through a `Halt` mid-frame.
                self.frames.truncate(base_frames);
                self.stack.truncate(base_stack);
                self.handlers.truncate(base_handlers);
                Ok(value)
            }
            Err(error) => {
                self.frames.truncate(base_frames);
                self.stack.truncate(base_stack);
                self.handlers.truncate(base_handlers);
                Err(error)
            }
        }
    }

    /// The handler-stack index below which the current execution context
    /// must not dispatch: 0 normally, the innermost invoke boundary during
    /// a reentrant [`Self::invoke`].
    pub(super) fn handler_barrier(&self) -> usize {
        self.handler_barriers.last().copied().unwrap_or(0)
    }

    /// Push a new call frame for the given function. Returns `false` if the
    /// target was a native executed inline (result already on the stack).
    pub(super) fn push_frame(
        &mut self,
        hash: &blake3::Hash,
        arg_count: u8,
    ) -> Result<bool, VmError> {
        self.push_frame_with_captures(hash, arg_count, Vec::new())
    }

    /// Execute a call to a native (extern fn): pop the arguments the caller
    /// pushed, run the bound implementation, push the result. No frame is
    /// pushed — a native is a pure value transformation, invisible to
    /// continuations and stack traces.
    fn call_native(
        &mut self,
        uuid: uuid::Uuid,
        param_count: u8,
        arg_count: u8,
    ) -> Result<(), VmError> {
        if arg_count != param_count {
            return Err(VmError::ArityMismatch {
                expected: param_count,
                got: arg_count,
            });
        }
        // A VM-invoking implementation takes priority over a pure one (the
        // stub it overrides); both use the same result contract below.
        if let Some(func) = self.native_vm_impls.get(&uuid).cloned() {
            let split = self.stack.len() - arg_count as usize;
            let args = self.stack.split_off(split);
            let result = func(self, args);
            return self.finish_native_call(result);
        }
        let func = self
            .native_impls
            .get(&uuid)
            .ok_or_else(|| VmError::UnboundNative {
                uuid: uuid.to_string(),
            })?
            .clone();
        let split = self.stack.len() - arg_count as usize;
        let args = self.stack.split_off(split);
        let result = func(args);
        self.finish_native_call(result)
    }

    /// Apply a native's result contract at its call site.
    ///
    /// - A native raising an exception behaves exactly like
    ///   `Exception.throw!` at the call site: the caller's frames are
    ///   intact, so the nearest in-language Exception handler catches
    ///   it. This is *not* how fallible operations report failure —
    ///   those (missing file, refused connection) return an in-language
    ///   `Result::Err` value instead. This channel is reserved for hard
    ///   faults a native can still detect at runtime: an unwired
    ///   capability (`... is not wired`) or a control error like
    ///   ensuring a task with a malformed body. Exception arms are catch-only
    ///   now, so such a throw cannot be resumed with a substitute — it
    ///   is caught-and-continued or surfaces uncaught.
    /// - A native interrupted by the host (a drain request unblocking a
    ///   blocking operation) returns [`VmError::Interrupted`]: the VM
    ///   performs the identified never-returning method here, at the
    ///   interrupted perform site (see [`Vm::deliver_interrupt`]).
    fn finish_native_call(&mut self, result: Result<Value, VmError>) -> Result<(), VmError> {
        match result {
            Ok(result) => {
                self.stack.push(result);
                Ok(())
            }
            Err(VmError::Exception(error)) => self.raise_exception(error),
            Err(VmError::Interrupted { ability_id, method }) => {
                self.deliver_interrupt(ability_id, method)
            }
            Err(other) => Err(other),
        }
    }

    /// Push a new call frame for a closure call with captured environment.
    /// Returns `false` if the target was a native executed inline (result
    /// already on the stack, no frame pushed).
    pub(super) fn push_frame_with_captures(
        &mut self,
        hash: &blake3::Hash,
        arg_count: u8,
        captures: Vec<Value>,
    ) -> Result<bool, VmError> {
        if self.frames.len() >= self.max_call_depth {
            return Err(VmError::StackOverflow);
        }

        if let Some(&(uuid, param_count)) = self.native_functions.get(hash) {
            self.call_native(uuid, param_count, arg_count)?;
            return Ok(false);
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

        Ok(true)
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
        self.pop_typed("Number", |v| v.as_number(), operation)
    }

    /// Pop a bool from the stack or return a type error.
    pub(super) fn pop_bool(&mut self, operation: &'static str) -> Result<bool, VmError> {
        self.pop_typed("Bool", |v| v.as_bool(), operation)
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
    pub fn capture_stack_trace(&self) -> Vec<StackTraceFrame> {
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

                StackTraceFrame {
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
    pub fn runtime_error(&self, error: VmError) -> RuntimeError {
        RuntimeError::with_stack_trace(error, self.capture_stack_trace())
    }
}
