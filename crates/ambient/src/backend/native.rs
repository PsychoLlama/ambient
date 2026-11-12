//! The native backend: Cranelift, and loading what it produced into this
//! process.

use ambient_runtime::layout::{
    CLOSURE_CAPTURES, CLOSURE_DEFINITION, DEFINITION_CODE, DEFINITION_SLOTS,
};
use ambient_runtime::{Context, Entry, FALSE, Fault, NULL, TRUE, Word};
use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::I64;
use cranelift_codegen::ir::{
    AbiParam, BlockArg, Function, InstBuilder, MemFlagsData, SigRef, Signature, UserFuncName,
};
use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use memmap2::{Mmap, MmapMut};

use super::MAX_ARITY;
use super::statics::{self, Field, Make};
use crate::core::Open;
use crate::engine::{Backend, Body, Parts};
use crate::lir::{self, Arg, Expr, Op};
use crate::lower::lower;
use crate::value::{Inspect, Value};

/// Compiles for the machine this process runs on.
pub struct Native {
    isa: OwnedTargetIsa,
}

impl Native {
    /// A backend for the host.
    ///
    /// # Errors
    ///
    /// Fails if Cranelift does not support the host.
    pub fn new() -> Result<Self, String> {
        let mut flags = settings::builder();
        flags
            .set("opt_level", "speed")
            .map_err(|error| error.to_string())?;
        let isa = cranelift_native::builder()
            .map_err(str::to_owned)?
            .finish(settings::Flags::new(flags))
            .map_err(|error| error.to_string())?;
        if isa.pointer_type() != I64 {
            return Err("only 64-bit hosts are supported".to_owned());
        }
        Ok(Self { isa })
    }
}

impl Backend for Native {
    fn name(&self) -> &'static str {
        "native"
    }

    fn compile(&self, definition: &Open) -> Result<Vec<u8>, String> {
        let function =
            lower(definition).map_err(|error| format!("{} are not compiled yet", error.0))?;
        if function.params > MAX_ARITY {
            return Err(format!("more than {MAX_ARITY} parameters"));
        }

        let mut signature = Signature::new(self.isa.default_call_conv());
        signature.params = vec![AbiParam::new(I64); function.params + 2];
        signature.returns.push(AbiParam::new(I64));
        let mut ir = Function::with_name_signature(UserFuncName::default(), signature);
        let mut context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ir, &mut context);

        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let params = builder.block_params(entry).to_vec();
        let locals: Vec<Variable> = (0..function.locals)
            .map(|_| builder.declare_var(I64))
            .collect();
        for (local, param) in locals.iter().zip(&params[2..]) {
            builder.def_var(*local, *param);
        }
        // Locals are read on unwinding paths that never assigned them.
        let zero = builder.ins().iconst(I64, 0);
        for local in &locals[function.params..] {
            builder.def_var(*local, zero);
        }

        let exit = builder.create_block();
        builder.append_block_param(exit, I64);
        let mut emitter = Emitter {
            call_conv: self.isa.default_call_conv(),
            context: params[0],
            closure: params[1],
            locals,
            slots: function.slots,
            unwind: vec![exit],
            builder,
        };
        let result = emitter.expr(&function.body);
        let mut builder = emitter.builder;
        builder.ins().jump(exit, &[BlockArg::Value(result)]);
        builder.switch_to_block(exit);
        let result = builder.block_params(exit)[0];
        builder.ins().return_(&[result]);
        builder.seal_all_blocks();
        builder.finalize(self.isa.frontend_config());

        let mut context = cranelift_codegen::Context::for_function(ir);
        let code = context
            .compile(&*self.isa, &mut ControlPlane::default())
            .map_err(|error| format!("{:?}", error.inner))?;
        if !code.buffer.relocs().is_empty() {
            return Err("compiled code is not position-independent".to_owned());
        }
        Ok(code.code_buffer().to_vec())
    }
}

type Val = cranelift_codegen::ir::Value;

struct Emitter<'a> {
    builder: FunctionBuilder<'a>,
    call_conv: cranelift_codegen::isa::CallConv,
    context: Val,
    closure: Val,
    locals: Vec<Variable>,
    /// How many slots the definition has; its constants follow them.
    slots: usize,
    /// Where unwinding goes from here: the end of each enclosing `Let`,
    /// innermost last, each taking the result as its parameter.
    unwind: Vec<cranelift_codegen::ir::Block>,
}

impl Emitter<'_> {
    fn constant(&mut self, word: Word) -> Val {
        self.builder.ins().iconst(I64, word.cast_signed())
    }

    fn load(&mut self, base: Val, offset: i32) -> Val {
        // Every address compiled code reads is one the runtime laid out:
        // aligned, and never a trap.
        self.builder
            .ins()
            .load(I64, MemFlagsData::trusted(), base, offset)
    }

    fn signature(&mut self, params: usize, returns: bool) -> SigRef {
        let mut signature = Signature::new(self.call_conv);
        signature.params = vec![AbiParam::new(I64); params];
        if returns {
            signature.returns.push(AbiParam::new(I64));
        }
        self.builder.import_signature(signature)
    }

    fn value(&mut self, value: lir::Value) -> Val {
        match value {
            lir::Value::Local(local) => self.builder.use_var(self.locals[local]),
            lir::Value::Captured(position) => {
                let offset = CLOSURE_CAPTURES + i32::try_from(position * 8).unwrap_or(0);
                self.load(self.closure, offset)
            }
            lir::Value::Constant(position) => self.in_definition(self.slots + position),
            lir::Value::Null => self.constant(NULL),
            lir::Value::Bool(true) => self.constant(TRUE),
            lir::Value::Bool(false) => self.constant(FALSE),
            lir::Value::Int(value) => self.builder.ins().iconst(I64, (value << 1) | 1),
        }
    }

    /// The word at `position` after the fixed part of this definition: a
    /// slot, or past the slots a constant.
    fn in_definition(&mut self, position: usize) -> Val {
        let definition = self.load(self.closure, CLOSURE_DEFINITION);
        let offset = DEFINITION_SLOTS + i32::try_from(position * 8).unwrap_or(0);
        self.load(definition, offset)
    }

    /// Calls the function a closure refers to.
    fn call(&mut self, callee: Val, args: &[lir::Value]) -> Val {
        let definition = self.load(callee, CLOSURE_DEFINITION);
        let code = self.load(definition, DEFINITION_CODE);
        let mut all = vec![self.context, callee];
        all.extend(args.iter().map(|arg| self.value(*arg)));
        let signature = self.signature(all.len(), true);
        let call = self.builder.ins().call_indirect(signature, code, &all);
        self.builder.inst_results(call)[0]
    }

    /// Calls an entry point of the runtime, through the context's table.
    fn runtime(&mut self, entry: Entry, args: &[Val]) -> Option<Val> {
        let table = self.load(self.context, Context::ENTRIES);
        let offset = i32::try_from(entry as usize * 8).unwrap_or(0);
        let address = self.load(table, offset);
        let mut all = Vec::new();
        if entry.takes_context() {
            all.push(self.context);
        }
        all.extend_from_slice(args);
        let signature = self.signature(all.len(), entry.returns());
        let call = self.builder.ins().call_indirect(signature, address, &all);
        self.builder.inst_results(call).first().copied()
    }

    /// Leaves for the innermost unwinding target if `word` says a call is
    /// unwinding.
    fn check(&mut self, word: Val) {
        let Some(target) = self.unwind.last().copied() else {
            return;
        };
        let proceed = self.builder.create_block();
        let zero = self.builder.ins().iconst(I64, 0);
        self.builder
            .ins()
            .brif(word, proceed, &[], target, &[BlockArg::Value(zero)]);
        self.builder.switch_to_block(proceed);
    }

    fn expr(&mut self, expr: &Expr) -> Val {
        match expr {
            Expr::Op(op) => {
                let word = self.op(op);
                if op.unwinds() {
                    self.check(word);
                }
                word
            }
            Expr::Let { local, value, body } => {
                let word = self.expr(value);
                if !matches!(**value, Expr::Op(_)) {
                    self.check(word);
                }
                self.builder.def_var(self.locals[*local], word);

                let done = self.builder.create_block();
                self.builder.append_block_param(done, I64);
                self.unwind.push(done);
                let result = self.expr(body);
                self.unwind.pop();
                self.builder.ins().jump(done, &[BlockArg::Value(result)]);
                self.builder.switch_to_block(done);

                let owned = self.builder.use_var(self.locals[*local]);
                self.runtime(Entry::Release, &[owned]);
                self.builder.block_params(done)[0]
            }
            Expr::If {
                condition,
                then,
                otherwise,
            } => {
                let condition = self.value(*condition);
                let truth =
                    self.builder
                        .ins()
                        .icmp_imm_s(IntCC::Equal, condition, TRUE.cast_signed());
                let (yes, no, join) = (
                    self.builder.create_block(),
                    self.builder.create_block(),
                    self.builder.create_block(),
                );
                self.builder.append_block_param(join, I64);
                self.builder.ins().brif(truth, yes, &[], no, &[]);
                for (block, branch) in [(yes, then), (no, otherwise)] {
                    self.builder.switch_to_block(block);
                    let result = self.expr(branch);
                    self.builder.ins().jump(join, &[BlockArg::Value(result)]);
                }
                self.builder.switch_to_block(join);
                self.builder.block_params(join)[0]
            }
            Expr::Fault(fault) => {
                let code = self.constant(*fault as u64);
                let word = self.runtime(Entry::Fault, &[code]);
                let word = word.unwrap_or(code);
                self.check(word);
                word
            }
        }
    }

    fn op(&mut self, op: &Op) -> Val {
        match op {
            Op::Copy(value) => {
                let word = self.value(*value);
                if matches!(value, lir::Value::Local(_)) {
                    self.runtime(Entry::Retain, &[word]);
                }
                word
            }
            Op::Int(value) => {
                if (ambient_runtime::SMALL_MIN..=ambient_runtime::SMALL_MAX).contains(value) {
                    self.value(lir::Value::Int(*value))
                } else {
                    let raw = self.builder.ins().iconst(I64, *value);
                    self.entry(Entry::IntNew, &[raw])
                }
            }
            Op::Float(bits) => {
                let raw = self.constant(*bits);
                self.entry(Entry::FloatNew, &[raw])
            }
            Op::Slot(slot) => self.in_definition(*slot),
            Op::Call { slot, args } => {
                let callee = self.in_definition(*slot);
                self.call(callee, args)
            }
            Op::CallValue { callee, args } => {
                let callee = self.value(*callee);
                self.call(callee, args)
            }
            _ => {
                let mut object = self.context;
                for (position, step) in op.steps().unwrap_or_default().iter().enumerate() {
                    let args: Vec<Val> = step
                        .args
                        .iter()
                        .map(|arg| match arg {
                            Arg::Value(value) => self.value(*value),
                            Arg::Raw(raw) => self.constant(*raw),
                            Arg::Object => object,
                            Arg::Definition(slot) => {
                                let closure = self.in_definition(*slot);
                                self.load(closure, CLOSURE_DEFINITION)
                            }
                        })
                        .collect();
                    let result = self.runtime(step.entry, &args);
                    if position == 0 {
                        object = result.unwrap_or(object);
                    }
                }
                object
            }
        }
    }

    /// Calls an entry point that returns a word.
    fn entry(&mut self, entry: Entry, args: &[Val]) -> Val {
        let fallback = args.first().copied();
        self.runtime(entry, args)
            .or(fallback)
            .unwrap_or(self.context)
    }
}

/// A program loaded into this process, ready to call.
pub struct Program {
    /// The code. Held so that it stays mapped.
    _code: Mmap,
    /// Every definition and its closure. Held so that they stay put.
    _statics: Box<[u64]>,
    entries: Box<[usize]>,
    entry_closure: u64,
    entry_code: usize,
    entry_arity: usize,
}

impl Program {
    /// Loads the parts of a program compiled by [`Native`].
    ///
    /// # Errors
    ///
    /// Fails if executable memory cannot be mapped.
    pub fn load(parts: &Parts) -> Result<Self, String> {
        let io = |error: std::io::Error| error.to_string();
        let code_of = |index: usize| match &parts.parts[index].body {
            Body::Function { code, .. } => code.0.as_slice(),
            Body::Type(_) => &[],
        };
        let mut offsets = Vec::new();
        let mut size = 0usize;
        for index in 0..parts.parts.len() {
            offsets.push(size);
            size += code_of(index).len().next_multiple_of(16);
        }
        let mut code = MmapMut::map_anon(size.max(1)).map_err(io)?;
        for (index, offset) in offsets.iter().enumerate() {
            let bytes = code_of(index);
            code[*offset..*offset + bytes.len()].copy_from_slice(bytes);
        }
        let code = code.make_exec().map_err(io)?;
        let code_base = code.as_ptr() as usize;

        let plan = statics::plan(parts);
        let mut block = vec![0u64; plan.words.max(1)].into_boxed_slice();
        let base = block.as_ptr() as usize;
        for (word, field) in &plan.fields {
            block[*word] = match *field {
                Field::Word(value) => value,
                Field::Address(target) => (base + target * 8) as u64,
                Field::Code(part) => (code_base + offsets[part]) as u64,
                Field::Text {
                    make,
                    offset,
                    length,
                } => {
                    let text = &plan.text[offset..offset + length];
                    // SAFETY: the pointer and length are those of a live
                    // slice, which the runtime copies.
                    unsafe {
                        match make {
                            Make::String => {
                                ambient_runtime::amb_string_pinned(text.as_ptr(), length as u64)
                            }
                            Make::Field => {
                                ambient_runtime::amb_intern(text.as_ptr(), length as u64)
                            }
                        }
                    }
                }
            };
        }

        let arity = match &parts.parts[parts.entry].body {
            Body::Function { arity, .. } => *arity,
            Body::Type(_) => return Err("the entry is not a function".to_owned()),
        };
        Ok(Self {
            entry_closure: (base + plan.entry * 8) as u64,
            entry_code: code_base + offsets[parts.entry],
            entry_arity: arity,
            entries: Entry::ALL.iter().map(|entry| entry.address()).collect(),
            _statics: block,
            _code: code,
        })
    }

    /// Calls the entry with `args`, which it borrows. The result is a word
    /// the caller owns, or the fault that ended the call.
    ///
    /// # Errors
    ///
    /// Returns the fault if the program faulted.
    ///
    /// # Panics
    ///
    /// Panics if `args` is not as many words as the entry takes, or more
    /// than four.
    pub fn call(&self, args: &[Word]) -> Result<Word, Fault> {
        type C = *mut Context;
        assert_eq!(args.len(), self.entry_arity, "wrong number of arguments");
        let (code, closure) = (self.entry_code, self.entry_closure);
        // SAFETY: the code was compiled by `Native` for a function of this
        // arity, with the calling convention these types spell out, and the
        // context and closure are laid out as the runtime defines.
        unsafe {
            let context = ambient_runtime::amb_context_new();
            (*context).entries = self.entries.as_ptr() as u64;
            let result = match *args {
                [] => std::mem::transmute::<usize, extern "C" fn(C, u64) -> u64>(code)(
                    context, closure,
                ),
                [a] => std::mem::transmute::<usize, extern "C" fn(C, u64, u64) -> u64>(code)(
                    context, closure, a,
                ),
                [a, b] => std::mem::transmute::<usize, extern "C" fn(C, u64, u64, u64) -> u64>(
                    code,
                )(context, closure, a, b),
                [a, b, c] => std::mem::transmute::<
                    usize,
                    extern "C" fn(C, u64, u64, u64, u64) -> u64,
                >(code)(context, closure, a, b, c),
                [a, b, c, d] => std::mem::transmute::<
                    usize,
                    extern "C" fn(C, u64, u64, u64, u64, u64) -> u64,
                >(code)(context, closure, a, b, c, d),
                _ => panic!("the host can pass at most four arguments"),
            };
            let fault = Fault::from_code((*context).fault);
            ambient_runtime::amb_context_free(context);
            if result == ambient_runtime::UNWIND {
                Err(fault)
            } else {
                Ok(result)
            }
        }
    }
}

/// A value as a word the caller owns.
#[must_use]
pub fn encode(value: &Value) -> Word {
    let count = |length: usize| length as u64;
    // SAFETY: each constructor is given what it documents: lengths that
    // match the pointers, and indices within the lengths just allocated.
    unsafe {
        match value {
            Value::Null | Value::Function => NULL,
            Value::Bool(true) => TRUE,
            Value::Bool(false) => FALSE,
            Value::Int(value) => ambient_runtime::amb_int_new(*value),
            Value::Float(value) => ambient_runtime::amb_float_new(value.to_bits()),
            Value::String(text) => {
                ambient_runtime::amb_string_new(text.as_ptr(), count(text.len()))
            }
            Value::Tuple(members) | Value::List(members) => {
                let list = u64::from(matches!(value, Value::List(_)));
                let word = ambient_runtime::amb_sequence_new(list, count(members.len()));
                for (index, member) in members.iter().enumerate() {
                    let member = encode(member);
                    ambient_runtime::amb_sequence_set(word, count(index), member);
                    ambient_runtime::amb_release(member);
                }
                word
            }
            Value::Record(fields) => {
                let word = ambient_runtime::amb_record_new(count(fields.len()));
                for (name, member) in fields {
                    let key = ambient_runtime::amb_intern(name.as_ptr(), count(name.len()));
                    let member = encode(member);
                    ambient_runtime::amb_record_set(word, key, member);
                    ambient_runtime::amb_release(member);
                }
                word
            }
        }
    }
}

/// Reads words through the runtime linked into this process.
pub struct Host;

impl Inspect for Host {
    fn kind(&mut self, word: Word) -> u64 {
        // SAFETY: the word is a value a program returned.
        unsafe { ambient_runtime::amb_kind(word) }
    }

    fn int(&mut self, word: Word) -> i64 {
        // SAFETY: `decode` asks only for what `kind` reported.
        unsafe { ambient_runtime::amb_int_value(word) }
    }

    fn float_bits(&mut self, word: Word) -> u64 {
        // SAFETY: `decode` asks only for what `kind` reported.
        unsafe { ambient_runtime::amb_float_bits(word) }
    }

    fn length(&mut self, word: Word) -> u64 {
        // SAFETY: `decode` asks only about objects.
        unsafe { ambient_runtime::amb_length(word) }
    }

    fn word_at(&mut self, word: Word, index: u64) -> Word {
        // SAFETY: `decode` stays within the length it was told.
        unsafe { ambient_runtime::amb_word_at(word, index) }
    }

    fn string(&mut self, word: Word) -> Vec<u8> {
        // SAFETY: the word is a string, and its bytes are as long as its
        // length says.
        unsafe {
            let length = usize::try_from(ambient_runtime::amb_length(word)).unwrap_or(0);
            std::slice::from_raw_parts(ambient_runtime::amb_string_bytes(word), length).to_vec()
        }
    }
}
