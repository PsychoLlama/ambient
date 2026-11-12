//! Emitting machine code for one lowered function.

use ambient_runtime::layout::{
    CLOSURE_CAPTURES, CLOSURE_DEFINITION, DEFINITION_CODE, DEFINITION_SLOTS,
};
use ambient_runtime::{Context, Entry, FALSE, NULL, TRUE, Word};
use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::I64;
use cranelift_codegen::ir::{
    AbiParam, BlockArg, Function, InstBuilder, MemFlagsData, SigRef, Signature, UserFuncName,
};
use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};

use crate::backend::MAX_ARITY;
use crate::core::Open;
use crate::engine::Backend;
use crate::lir::{self, Arg, Expr, Op};
use crate::lower::lower;

mod resume;

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
        let target = builder.declare_var(I64);
        let mut emitter = Emitter {
            call_conv: self.isa.default_call_conv(),
            context: params[0],
            closure: params[1],
            locals,
            target,
            slots: function.slots,
            unwind: vec![exit],
            builder,
        };
        emitter.enter();
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
    /// The point a function that is resuming is going back to, and `-1`
    /// in one that is not.
    target: Variable,
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

    /// Whether `word` is exactly `expected`.
    fn is(&mut self, word: Val, expected: u64) -> Val {
        self.builder
            .ins()
            .icmp_imm_s(IntCC::Equal, word, expected.cast_signed())
    }

    /// `body`, or, if it unwinds, `recover`.
    fn attempt(&mut self, body: &Expr, recover: &Expr, recovering: (usize, usize)) -> Val {
        let landing = self.builder.create_block();
        self.builder.append_block_param(landing, I64);
        let join = self.builder.create_block();
        self.builder.append_block_param(join, I64);

        // A function going back to a point in `recover` had left
        // the body already.
        let recovered = self.resuming_to(recovering);
        let enter = self.builder.create_block();
        let zero = self.builder.ins().iconst(I64, 0);
        let passed = [BlockArg::Value(zero)];
        self.builder
            .ins()
            .brif(recovered, landing, &passed, enter, &[]);
        self.builder.switch_to_block(enter);

        self.unwind.push(landing);
        let result = self.expr(body);
        if !matches!(*body, Expr::Op(_)) {
            self.check(result);
        }
        self.unwind.pop();
        self.builder.ins().jump(join, &[BlockArg::Value(result)]);

        self.builder.switch_to_block(landing);
        let result = self.expr(recover);
        self.builder.ins().jump(join, &[BlockArg::Value(result)]);
        self.builder.switch_to_block(join);
        self.builder.block_params(join)[0]
    }

    fn expr(&mut self, expr: &Expr) -> Val {
        match expr {
            Expr::Point { point, op } => self.point(*point, op),
            Expr::Resumed {
                points,
                local,
                value,
            } => self.resumed(*points, *local, value),
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
            Expr::Try {
                body,
                recover,
                recovering,
            } => self.attempt(body, recover, *recovering),
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
