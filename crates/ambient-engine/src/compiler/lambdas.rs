//! Lambda and closure compilation.
//!
//! This module handles compilation of lambda expressions and closures,
//! including variable capture analysis and closure creation.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{Expr, HandlerLiteralMethod, Lambda};
use crate::bytecode::{CompiledFunction, Opcode};
use crate::fqn::NameKey;
use crate::types::AbilityId;
use crate::value::AbilityMethodRef;

use super::context::CompiledAbilityInfo;
use super::error::{CompileError, CompileErrorKind};
use super::{FunctionCompiler, ModuleContext, compile_expr};

/// A handler's method table (method reference → arm-function hash) paired
/// with the ordered captures its installer must push before `MakeHandler`.
type HandlerMethods = (Vec<(AbilityMethodRef, blake3::Hash)>, Vec<(Arc<str>, u16)>);

/// Build the constant-pool reference for one ability method: the identity
/// inputs from the checker plus the default implementation's hash, looked
/// up under the method's dispatch symbol (`<uuid>::<method>`). Local
/// implementations resolve to their temporary hash (finalization rewrites
/// it); foreign ones arrive final through the linking table.
fn resolve_method_ref(
    function_hashes: &std::collections::HashMap<NameKey, blake3::Hash>,
    info: &CompiledAbilityInfo,
    method_name: &str,
    span: (u32, u32),
) -> Result<AbilityMethodRef, CompileError> {
    let method = info.method(method_name).ok_or_else(|| {
        CompileError::new(
            CompileErrorKind::Unsupported {
                feature: format!("unknown method `{method_name}` for ability id {}", info.id),
            },
            span,
        )
    })?;
    let impl_fn = if method.has_impl {
        let symbol = info.impl_symbol(method_name);
        let hash = function_hashes
            .get(&NameKey::Bare(Arc::clone(&symbol)))
            .copied()
            .ok_or_else(|| {
                CompileError::new(
                    CompileErrorKind::Internal {
                        message: "ability method implementation not in the linking table",
                    },
                    span,
                )
            })?;
        Some(hash)
    } else {
        None
    };
    Ok(AbilityMethodRef {
        ability_id: info.id,
        ability_uuid: info.uuid,
        signature: method.signature,
        impl_fn,
        never: method.never,
    })
}

/// Compile a lambda expression.
///
/// This compiles the lambda body as a separate function and emits code
/// to create a closure value with captured variables.
pub(super) fn compile_lambda(
    fc: &mut FunctionCompiler,
    lambda: &Lambda,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // Create a new FunctionCompiler for the lambda body.
    // Pass the current scope's locals as the parent scope.
    let mut lambda_fc = FunctionCompiler::new_for_closure(
        fc.function_hashes.clone(),
        fc.locals.clone(),
        fc.local_names.clone(),
        fc.block_consts.clone(),
    );

    // Allocate slots for lambda parameters.
    for param in &lambda.params {
        lambda_fc.alloc_local_with_name(param.id, &param.name)?;
    }

    // Compile the lambda body.
    // During this compilation, lambda_fc will track any captured variables.
    compile_expr(&mut lambda_fc, &lambda.body, ctx)?;

    // Emit return instruction.
    lambda_fc.builder.emit(Opcode::Return);

    // Build the compiled lambda function.
    let bytecode = lambda_fc.builder.bytecode().to_vec();
    let constants = lambda_fc.builder.constants().to_vec();
    let dependencies = lambda_fc.builder.dependencies().to_vec();
    let local_count = lambda_fc.next_local;
    let param_count = lambda.params.len() as u8;

    // Create the CompiledFunction with a temporary hash.
    let temp_hash = ctx.next_lambda_hash();
    let method_keys = CompiledFunction::index_method_keys(&constants);
    let compiled_func = CompiledFunction {
        hash: temp_hash,
        bytecode,
        constants,
        local_count,
        param_count,
        dependencies,
        debug_info: None,
        method_keys,
    };

    // Get the captures in order (by name, since that's how ExprKind::Name captures work).
    let capture_names = lambda_fc.get_capture_names_in_order();
    let capture_count = capture_names.len();

    // Register the lambda with the module context.
    let lambda_hash = ctx.register_lambda(compiled_func);

    // Now emit code in the enclosing function to create the closure.
    // Push captured values onto the stack in capture slot order.
    for (name, _slot) in &capture_names {
        // Load the captured value from the current function's scope.
        if let Some(&slot) = fc.local_names.get(name) {
            fc.builder.emit_u16(Opcode::LoadLocal, slot);
        } else if let Some(&capture_slot) = fc.capture_names.get(name) {
            // If the enclosing function is itself a closure, load from its captures.
            fc.builder.emit_load_capture(capture_slot);
        } else {
            // Should not happen if capture analysis is correct.
            return Err(CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: format!("unknown capture: {name}"),
                },
                (0, 0),
            ));
        }
    }

    // 2. Emit MakeClosure instruction.
    fc.builder
        .emit_make_closure(lambda_hash, capture_count as u8);

    Ok(())
}

/// Compile a handle expression.
///
/// A handle expression installs ability handlers and executes a body expression.
/// Each handler is compiled as a separate function that receives:
/// - Local slot 0: the continuation (to resume with)
/// - Local slot 1: the suspended ability (containing method args)
/// - Local slots 2+: extracted ability arguments bound to handler params
pub(super) fn compile_handle_expr(
    fc: &mut FunctionCompiler,
    handle_expr: &crate::ast::HandleExpr,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // The handle expression compiles to a *body thunk*: a zero-parameter
    // closure whose frame delimits the handled computation.
    //
    //   installer:  <thunk capture loads> MakeClosure(thunk) CallClosure(0)
    //   thunk:      <install handlers> <body> <Unhandle...> [<else>] Return
    //
    // Delimitation falls out of ordinary call discipline: a handler arm
    // that returns without resuming delivers its value exactly where the
    // thunk's return value would have landed (the instruction after
    // CallClosure), and `resume` reinstates the thunk's frames above the
    // arm so the thunk's eventual return becomes the resume expression's
    // value. See vm/abilities.rs for the runtime half.
    let mut thunk_fc = FunctionCompiler::new_for_closure(
        fc.function_hashes.clone(),
        fc.locals.clone(),
        fc.local_names.clone(),
        fc.block_consts.clone(),
    );

    // Install each handler in the flat `with` list, in source order (so a
    // later handler shadows an earlier one for the same ability — "last
    // wins"). Every handler installs through the single `HandlerValue` path
    // (`MakeHandler` + `HandleWithValue`):
    //   - a handler-literal node is compiled in place, its arms grouped by
    //     ability into one `HandlerValue` per ability (a multi-ability inline
    //     brace thus becomes one install per ability);
    //   - any other expression already evaluates to a `Handler` value.
    // Everything is compiled inside the thunk; free names capture through it.
    let mut install_count: usize = 0;
    for handler in &handle_expr.handlers {
        if let crate::ast::ExprKind::HandlerLiteral(lit) = &handler.kind {
            // Group arms by ability, preserving first-seen order.
            let mut groups: Vec<(AbilityId, Vec<&HandlerLiteralMethod>)> = Vec::new();
            for method in &lit.methods {
                let ability_id = resolve_arm_ability(ctx, method)?;
                match groups.iter_mut().find(|(id, _)| *id == ability_id) {
                    Some((_, arms)) => arms.push(method),
                    None => groups.push((ability_id, vec![method])),
                }
            }

            for (ability_id, arms) in &groups {
                let span = (handler.span.start, handler.span.end);
                let (method_hashes, captures) =
                    compile_handler_methods(fc, *ability_id, arms, span, ctx)?;
                // Load the shared captures in the thunk, capturing them
                // through it when they come from the installer's scope.
                for (name, _slot) in &captures {
                    if let Some(&slot) = thunk_fc.local_names.get(name) {
                        thunk_fc.builder.emit_u16(Opcode::LoadLocal, slot);
                    } else {
                        let capture_slot = thunk_fc.get_or_create_capture_by_name(Arc::clone(name));
                        thunk_fc.builder.emit_load_capture(capture_slot);
                    }
                }
                thunk_fc.builder.emit_make_handler(
                    *ability_id,
                    &method_hashes,
                    captures.len() as u8,
                );
                thunk_fc.builder.emit_handle_with_value();
                install_count += 1;
            }
        } else {
            // The type checker guarantees this is a Handler value and records
            // it on the expression. A missing/mismatched type means inference
            // results were lost — installing no handler would silently drop
            // performs, so fail loudly instead.
            match &handler.ty {
                Some(crate::types::Type::Handler(_)) => {}
                other => {
                    return Err(CompileError::new(
                        CompileErrorKind::Internal {
                            message: if other.is_some() {
                                "handle-with expression has a non-handler type"
                            } else {
                                "handler value expression missing its inferred type"
                            },
                        },
                        (handler.span.start, handler.span.end),
                    ));
                }
            }

            compile_expr(&mut thunk_fc, handler, ctx)?;
            thunk_fc.builder.emit_handle_with_value();
            install_count += 1;
        }
    }

    // Compile the body expression inside the thunk.
    compile_expr(&mut thunk_fc, &handle_expr.body, ctx)?;

    // Pop this handle expression's handlers (normal completion path; a
    // fired handler drains them into the captured continuation instead).
    for _ in 0..install_count {
        thunk_fc.builder.emit(Opcode::Unhandle);
    }

    // Apply the else clause to the body's result on normal completion.
    // `else` is a transform `(result) => expr`; handler arms bypass it.
    if let Some(else_clause) = &handle_expr.else_clause {
        let tmp_slot = thunk_fc.alloc_local_with_name(0, &Arc::from("__handle_result"))?;
        thunk_fc.builder.emit_u16(Opcode::StoreLocal, tmp_slot);
        compile_expr(&mut thunk_fc, else_clause, ctx)?;
        thunk_fc.builder.emit_u16(Opcode::LoadLocal, tmp_slot);
        thunk_fc.builder.emit_call_closure(1);
    }

    thunk_fc.builder.emit(Opcode::Return);

    // Build the thunk (zero parameters) and emit the installer code:
    // load thunk captures, make the closure, call it.
    let local_count = thunk_fc.next_local;
    let capture_names = thunk_fc.get_capture_names_in_order();
    let thunk_func = thunk_fc.builder.build(local_count, 0);
    let thunk_hash = ctx.register_lambda(thunk_func);

    for (name, _slot) in &capture_names {
        if let Some(&slot) = fc.local_names.get(name) {
            fc.builder.emit_u16(Opcode::LoadLocal, slot);
        } else if let Some(&capture_slot) = fc.capture_names.get(name) {
            fc.builder.emit_load_capture(capture_slot);
        } else {
            return Err(CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: format!("unknown capture in handle expression: {name}"),
                },
                (0, 0),
            ));
        }
    }
    fc.builder
        .emit_make_closure(thunk_hash, capture_names.len() as u8);
    fc.builder.emit_call_closure(0);

    Ok(())
}

/// Resolve the ability an inline handler arm covers, so a multi-ability
/// brace can be grouped into one `HandlerValue` per ability. Every ability
/// — locals, foreign, and prelude (including `Exception`) — resolves
/// through the context by its checker-assigned identity.
fn resolve_arm_ability(
    ctx: &ModuleContext,
    method: &HandlerLiteralMethod,
) -> Result<AbilityId, CompileError> {
    let ability_name = &method.ability.name;
    ctx.resolve_ability(&method.ability)
        .map(|info| info.id)
        .ok_or_else(|| {
            CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: format!("unknown ability: {ability_name}"),
                },
                (method.span.start, method.span.end),
            )
        })
}

/// Compile an ability call (perform).
pub(super) fn compile_ability_call(
    fc: &mut FunctionCompiler,
    ability_call: &crate::ast::AbilityCall,
    dicts: Option<&crate::ast::Dicts>,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // Compile arguments.
    for arg in &ability_call.args {
        compile_expr(fc, arg, ctx)?;
    }

    // A bounded method's dictionaries ride as hidden trailing perform
    // arguments; the default implementation binds them as its own hidden
    // trailing parameters.
    let dict_count = super::expr::compile_dicts(fc, dicts, ability_call.span)?;

    // Resolve the method reference. Every ability — locals, foreign, and
    // prelude (including `Exception`) — resolves through the context; the
    // identities come from the type checker, the implementation hash from
    // the same name→hash table calls link through.
    let ability_name = &ability_call.ability.name;
    let method_name = &ability_call.method;
    let span = (ability_call.span.start, ability_call.span.end);

    let info = ctx.resolve_ability(&ability_call.ability).ok_or_else(|| {
        CompileError::new(
            CompileErrorKind::UnknownAbilityMethod {
                ability: Arc::clone(ability_name),
                method: Arc::clone(method_name),
            },
            span,
        )
    })?;
    let method_ref = resolve_method_ref(&fc.function_hashes, info, method_name, span)?;

    // Emit suspend instruction (packages the args, dictionaries included),
    // then perform.
    #[allow(clippy::cast_possible_truncation)]
    fc.builder
        .emit_suspend(method_ref, (ability_call.args.len() + dict_count) as u8);
    fc.builder.emit(Opcode::Perform);

    Ok(())
}

/// Compile a handler literal expression to a `HandlerValue`.
///
/// A handler literal in value position (`let h = { A::m(p) => … }`) covers
/// a single ability (the checker enforces this), so its ability id comes
/// from the expression's `Handler<A, R>` type. The methods are compiled
/// into one `HandlerValue`, sharing one capture environment, and the
/// installer loads those captures before `MakeHandler`.
pub(super) fn compile_handler_literal(
    fc: &mut FunctionCompiler,
    expr: &Expr,
    handler_lit: &crate::ast::HandlerLiteralExpr,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // Get the ability ID from the type (set by type checker).
    // The type should be Handler<ability_id, R>.
    let ability_id = match &expr.ty {
        Some(crate::types::Type::Handler(h)) => h.ability,
        _ => {
            return Err(CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: "handler literal without type annotation".to_string(),
                },
                (expr.span.start, expr.span.end),
            ));
        }
    };

    let methods: Vec<&HandlerLiteralMethod> = handler_lit.methods.iter().collect();
    let (method_hashes, captures) = compile_handler_methods(
        fc,
        ability_id,
        &methods,
        (expr.span.start, expr.span.end),
        ctx,
    )?;

    // Load the shared captures onto the stack (from the installer's own
    // locals, or through its captures if it is itself a closure), then
    // build the handler value.
    for (name, _slot) in &captures {
        if let Some(&slot) = fc.local_names.get(name) {
            fc.builder.emit_u16(Opcode::LoadLocal, slot);
        } else if let Some(&capture_slot) = fc.capture_names.get(name) {
            fc.builder.emit_load_capture(capture_slot);
        } else {
            return Err(CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: format!("unknown capture in handler literal: {name}"),
                },
                (expr.span.start, expr.span.end),
            ));
        }
    }

    fc.builder
        .emit_make_handler(ability_id, &method_hashes, captures.len() as u8);

    Ok(())
}

/// Compile the arms of a handler covering `ability_id` into method
/// functions that share one capture environment, returning the
/// `(method_id, hash)` map and the ordered captures the installer must
/// push onto the stack (by name and slot) before `MakeHandler`.
///
/// All methods of a `HandlerValue` read from one shared capture array at
/// runtime, so their capture slots must agree: a name used in two arms
/// must resolve to the same slot. We thread one by-name accumulator across
/// every arm — each arm compiler is seeded with the captures discovered so
/// far, so a reused name keeps its slot and any new free variable appends
/// at a stable index. (Locals from real source resolve by name; the by-id
/// capture path only fires for hand-built ASTs, which never share.)
fn compile_handler_methods(
    fc: &FunctionCompiler,
    ability_id: AbilityId,
    methods: &[&HandlerLiteralMethod],
    span: (u32, u32),
    ctx: &mut ModuleContext,
) -> Result<HandlerMethods, CompileError> {
    let info = ctx
        .ability_by_id(ability_id)
        .map(|(name, info)| (Arc::clone(name), info.clone()))
        .ok_or_else(|| {
            CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: format!("unknown ability ID: {ability_id}"),
                },
                span,
            )
        })?;
    let (ability_name, info) = info;

    let mut method_hashes: Vec<(AbilityMethodRef, blake3::Hash)> = Vec::new();
    let mut shared_captures: HashMap<Arc<str>, u16> = HashMap::new();

    for method in methods {
        let arm_span = (method.span.start, method.span.end);
        let method_ref = resolve_method_ref(&fc.function_hashes, &info, &method.method, arm_span)
            .map_err(|e| match e.kind {
            CompileErrorKind::Unsupported { .. } => CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: format!(
                        "unknown method `{}` for ability `{}`",
                        method.method, ability_name
                    ),
                },
                arm_span,
            ),
            _ => e,
        })?;

        let (func, captures) = compile_handler_method(fc, method, &shared_captures, ctx)?;
        let final_hash = ctx.register_lambda(func);
        method_hashes.push((method_ref, final_hash));
        // The arm was seeded with `shared_captures` and only appends, so
        // its map is the grown superset — adopt it for the next arm.
        shared_captures = captures;
    }

    let mut ordered: Vec<(Arc<str>, u16)> = shared_captures.into_iter().collect();
    ordered.sort_by_key(|(_, slot)| *slot);
    Ok((method_hashes, ordered))
}

/// Compile one handler arm into its own function, seeded with the handler's
/// shared capture map so capture slots stay consistent across arms. Returns
/// the compiled function and the arm's (grown) capture map for the caller
/// to carry into the next arm.
///
/// Arm functions take two implicit params — slot 0 the continuation, slot 1
/// the suspended ability — and bind the method's declared parameters into
/// slots 2.. by extracting the suspended ability's arguments.
fn compile_handler_method(
    fc: &FunctionCompiler,
    method: &HandlerLiteralMethod,
    seed_captures: &HashMap<Arc<str>, u16>,
    ctx: &mut ModuleContext,
) -> Result<(CompiledFunction, HashMap<Arc<str>, u16>), CompileError> {
    let mut method_fc = FunctionCompiler::new_for_closure(
        fc.function_hashes.clone(),
        fc.locals.clone(),
        fc.local_names.clone(),
        fc.block_consts.clone(),
    );
    // Seed the shared captures so a name reused across arms keeps its slot.
    method_fc.capture_names.clone_from(seed_captures);

    // Allocate implicit slots for continuation and suspended ability.
    method_fc.alloc_local_with_name(0, &Arc::from(super::HANDLER_PARAM_CONTINUATION))?;
    method_fc.alloc_local_with_name(0, &Arc::from(super::HANDLER_PARAM_SUSPENDED_ABILITY))?;

    // Bind declared params by extracting the suspended ability's arguments.
    for (i, param) in method.params.iter().enumerate() {
        method_fc.alloc_local_with_name(param.id, &param.name)?;
        method_fc.builder.emit_u16(Opcode::LoadLocal, 1);
        method_fc.builder.emit_get_ability_arg(i as u8);
        method_fc.builder.emit_u16(Opcode::StoreLocal, 2 + i as u16);
    }

    compile_expr(&mut method_fc, &method.body, ctx)?;
    method_fc.builder.emit(Opcode::Return);

    let local_count = method_fc.next_local;
    // Handler receives 2 implicit params: continuation and suspended ability.
    let func = method_fc.builder.build(local_count, 2);
    Ok((func, method_fc.capture_names))
}
