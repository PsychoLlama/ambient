//! Dictionary-passing compilation for bounded generics.
//!
//! A call site whose callee carries trait bounds must supply one dictionary
//! per bound (see [`crate::ast::DictSource`]). This module lowers those
//! annotations to bytecode: building an impl's method tuple, forwarding an
//! enclosing dictionary parameter, or synthesizing a conditional impl's
//! per-method slot closures. It also lowers a bounded generic used as a
//! first-class value and the dictionary-slot callee of a bound-method call.
//!
//! Every dictionary slot is *variable-arity*: beyond the value arguments and
//! any captured inner dictionaries, it forwards the method's own bound
//! dictionaries (`fn m<U: Eq>`) as extra trailing runtime arguments — the
//! `impl ++ method` order the compiled impl method allocates its hidden
//! trailing dictionary parameters in.

use std::sync::Arc;

use crate::bytecode::{BytecodeBuilder, CompiledFunction, Opcode};
use crate::fqn::NameKey;
use crate::value::Value;

use super::error::{CompileError, CompileErrorKind};
use super::{FunctionCompiler, ModuleContext};

/// Push the dictionary arguments a call site's annotation demands, in
/// order, returning how many were pushed. Each dictionary is either built
/// from a concrete impl (a tuple of function references, hash-linked like
/// any direct call) or forwarded from this function's own dictionary
/// parameters.
pub(super) fn compile_dicts(
    fc: &mut FunctionCompiler,
    dicts: Option<&crate::ast::Dicts>,
    span: crate::ast::Span,
    ctx: &mut ModuleContext,
) -> Result<usize, CompileError> {
    let sources = match dicts {
        None => return Ok(0),
        Some(crate::ast::Dicts::Resolved(sources)) => sources,
        Some(crate::ast::Dicts::Pending(_)) => {
            return Err(CompileError::new(
                CompileErrorKind::Internal {
                    message: "unsolved dictionary constraints reached compilation (checker bug)",
                },
                (span.start, span.end),
            ));
        }
    };
    for source in sources {
        compile_dict_source(fc, source, span, ctx)?;
    }
    Ok(sources.len())
}

/// Push one dictionary value.
fn compile_dict_source(
    fc: &mut FunctionCompiler,
    source: &crate::ast::DictSource,
    span: crate::ast::Span,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    match source {
        crate::ast::DictSource::Impl { symbols } => {
            for symbol in symbols {
                let hash = dict_method_hash(fc, symbol, span)?;
                fc.builder.emit_const(Value::FunctionRef(hash));
            }
            #[allow(clippy::cast_possible_truncation)]
            fc.builder.emit_u8(Opcode::MakeTuple, symbols.len() as u8);
            Ok(())
        }
        crate::ast::DictSource::Param { dict_index } => {
            // Forward the enclosing bounded item's dictionary. It is an
            // ordinary local there and a captured value inside a lambda; the
            // capture path handles both (and nested lambdas).
            fc.emit_load_dict(*dict_index, (span.start, span.end))
        }
        crate::ast::DictSource::Generic { methods, inner } => {
            // A conditional impl (`impl<T: Eq> Eq for Pair<T>`): each
            // dictionary slot is a closure over the impl's *inner*
            // dictionaries (its own bounds, already solved) that forwards its
            // value arguments plus those captured dictionaries to the impl
            // method. Build one closure per method and assemble the tuple.
            for method in methods {
                let method_hash = dict_method_hash(fc, &method.symbol, span)?;
                let closure = build_dict_slot_closure(
                    method_hash,
                    method.arity,
                    inner.len(),
                    method.method_dict_count,
                );
                let closure_hash = ctx.register_lambda(closure);
                // Push the inner dictionaries the closure captures, in order.
                for source in inner {
                    compile_dict_source(fc, source, span, ctx)?;
                }
                #[allow(clippy::cast_possible_truncation)]
                fc.builder
                    .emit_make_closure(closure_hash, inner.len() as u8);
            }
            #[allow(clippy::cast_possible_truncation)]
            fc.builder.emit_u8(Opcode::MakeTuple, methods.len() as u8);
            Ok(())
        }
    }
}

/// Resolve an impl-method dispatch symbol to its (temporary) content hash,
/// the same name→hash table a direct call links through.
fn dict_method_hash(
    fc: &FunctionCompiler,
    symbol: &Arc<str>,
    span: crate::ast::Span,
) -> Result<blake3::Hash, CompileError> {
    fc.function_hashes
        .get(&NameKey::Bare(Arc::clone(symbol)))
        .copied()
        .ok_or_else(|| {
            CompileError::new(
                CompileErrorKind::UndefinedFunction {
                    name: Arc::clone(symbol),
                },
                (span.start, span.end),
            )
        })
}

/// Synthesize one dictionary slot: a closure taking `value_arity` value
/// arguments plus `method_dict_count` trailing method dictionaries as runtime
/// arguments, capturing `capture_count` inner dictionaries, whose body applies
/// the impl method to the value arguments, then the captured inner
/// dictionaries (the impl block's hidden trailing dictionary parameters), then
/// the method's own trailing dictionaries — the `impl ++ method` layout the
/// compiled impl method allocates.
///
/// `[LoadLocal 0..value_arity] [LoadCapture 0..capture_count]
///  [LoadLocal value_arity..value_arity+method_dict_count] Call(method) Return`.
/// The method's own dictionaries arrive as extra runtime arguments (the
/// bound-method call site pushes them after the value arguments), so they
/// occupy locals immediately after the value arguments. The method hash is
/// recorded as a dependency by `emit_call`, so this closure is
/// content-addressed like every other function.
fn build_dict_slot_closure(
    method_hash: blake3::Hash,
    value_arity: usize,
    capture_count: usize,
    method_dict_count: usize,
) -> CompiledFunction {
    let mut builder = BytecodeBuilder::new();
    for i in 0..value_arity {
        #[allow(clippy::cast_possible_truncation)]
        builder.emit_u16(Opcode::LoadLocal, i as u16);
    }
    for j in 0..capture_count {
        #[allow(clippy::cast_possible_truncation)]
        builder.emit_load_capture(j as u16);
    }
    // The method's own dictionaries follow the impl's captured ones, mirroring
    // `alloc_dict_locals(impl ++ method)`. They are runtime arguments in locals
    // `value_arity..value_arity+method_dict_count`.
    for k in 0..method_dict_count {
        #[allow(clippy::cast_possible_truncation)]
        builder.emit_u16(Opcode::LoadLocal, (value_arity + k) as u16);
    }
    #[allow(clippy::cast_possible_truncation)]
    builder.emit_call(
        method_hash,
        (value_arity + capture_count + method_dict_count) as u8,
    );
    builder.emit(Opcode::Return);
    // The value arguments and the method's own dictionaries are runtime
    // arguments (locals `0..value_arity+method_dict_count`); captures live
    // outside the local frame.
    let local_count = value_arity + method_dict_count;
    #[allow(clippy::cast_possible_truncation)]
    builder.build(local_count as u16, local_count as u8)
}

/// Lower a bounded generic function used as a *value* (not a direct callee)
/// to a closure that captures its resolved dictionaries and forwards to the
/// function. The closure takes the function's own value arguments (its
/// inferred function type's parameter count) and appends the captured
/// dictionaries as the function's hidden trailing dictionary parameters —
/// exactly the shape a conditional-impl dictionary slot uses
/// ([`build_dict_slot_closure`]), and hash-linked to the function like a
/// direct call. Works for every [`DictSource`](crate::ast::DictSource): the
/// captured value is a concrete impl's method tuple, a forwarded enclosing
/// dictionary (an ordinary capture inside a lambda), or a conditional impl's
/// closure tuple — [`compile_dict_source`] builds each identically here.
pub(super) fn compile_bounded_value_ref(
    fc: &mut FunctionCompiler,
    fn_hash: blake3::Hash,
    ty: Option<&crate::types::Type>,
    sources: &[crate::ast::DictSource],
    span: crate::ast::Span,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // The value arity is the reference's inferred function type's parameter
    // count; the dictionaries the closure supplies are never part of the
    // surface type. The checker only annotates dictionary sources on a
    // function-typed reference, so a missing/non-function type is a bug.
    let Some(crate::types::Type::Function(ft)) = ty else {
        return Err(CompileError::new(
            CompileErrorKind::Internal {
                message: "bounded generic used as a value lacks a function type (checker bug)",
            },
            (span.start, span.end),
        ));
    };
    // A bounded generic used as a value has no method-level dictionaries to
    // forward — its own dictionaries are all captured, so `method_dict_count`
    // is zero.
    let closure = build_dict_slot_closure(fn_hash, ft.params.len(), sources.len(), 0);
    let closure_hash = ctx.register_lambda(closure);
    // Push the dictionaries the closure captures, in order — the same
    // per-source lowering direct call sites use.
    for source in sources {
        compile_dict_source(fc, source, span, ctx)?;
    }
    #[allow(clippy::cast_possible_truncation)]
    fc.builder
        .emit_make_closure(closure_hash, sources.len() as u8);
    Ok(())
}

/// Push a bound method (dictionary slot) as the callee for a
/// `CallClosure`: load this function's `dict_index`-th dictionary — an
/// ordinary local, or a captured value inside a lambda — and take tuple
/// slot `slot`.
pub(super) fn emit_dict_method(
    fc: &mut FunctionCompiler,
    dict_index: usize,
    slot: usize,
    span: crate::ast::Span,
) -> Result<(), CompileError> {
    fc.emit_load_dict(dict_index, (span.start, span.end))?;
    #[allow(clippy::cast_possible_truncation)]
    fc.builder.emit_u8(Opcode::TupleGet, slot as u8);
    Ok(())
}
