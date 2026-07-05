//! Lambda and closure compilation.
//!
//! This module handles compilation of lambda expressions and closures,
//! including variable capture analysis and closure creation.

use std::sync::Arc;

use crate::ast::{Expr, Lambda};
use crate::bytecode::{CompiledFunction, Opcode};
use crate::types::AbilityId;

use super::error::{CompileError, CompileErrorKind};
use super::{FunctionCompiler, ModuleContext, compile_expr};

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
    let compiled_func = CompiledFunction {
        hash: temp_hash,
        bytecode,
        constants,
        local_count,
        param_count,
        dependencies,
        debug_info: None,
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
    );

    // Install handler values from the `with` clause. The expressions are
    // compiled inside the thunk; free names capture through it.
    for handler_value in &handle_expr.handler_values {
        // The type checker guarantees this is a Handler type and records it
        // on the expression. A missing/mismatched type here means inference
        // results were lost — installing no handler would silently drop
        // performs, so fail loudly instead.
        match &handler_value.ty {
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
                    (handler_value.span.start, handler_value.span.end),
                ));
            }
        }

        compile_expr(&mut thunk_fc, handler_value, ctx)?;
        thunk_fc.builder.emit_handle_with_value();
    }

    // Compile each inline handler arm as a separate function and install
    // it. Arms are compiled with the *installer* as parent scope (that is
    // the environment they close over); their captured values are loaded
    // in the thunk (capturing through it on demand) and packaged with
    // MakeClosure so the Handle instruction can install the arm closure.
    for handler in &handle_expr.handlers {
        compile_handler_arm(fc, &mut thunk_fc, handler, ctx)?;
    }

    // Compile the body expression inside the thunk.
    compile_expr(&mut thunk_fc, &handle_expr.body, ctx)?;

    // Pop this handle expression's handlers (normal completion path; a
    // fired handler drains them into the captured continuation instead).
    for _ in 0..(handle_expr.handlers.len() + handle_expr.handler_values.len()) {
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

/// Compile one inline handler arm and emit its installation in the thunk.
///
/// The arm is compiled as a separate function with the *installer* as its
/// parent scope. Arm functions have implicit parameters:
/// - slot 0: continuation
/// - slot 1: suspended ability value
/// - slot 2+: ability method arguments
///
/// The arm's captured values are loaded in the thunk (capturing through it
/// on demand), packaged with `MakeClosure`, and installed with `Handle`.
fn compile_handler_arm(
    fc: &mut FunctionCompiler,
    thunk_fc: &mut FunctionCompiler,
    handler: &crate::ast::Handler,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // Get ability ID for this handler. Locals, foreign, and prelude
    // abilities resolve through the context; core builtins (Exception)
    // fall back to the resolver.
    let ability_name = &handler.ability.name;
    let ability_id = ctx
        .resolve_ability(&handler.ability)
        .map(|info| info.id)
        .or_else(|| get_ability_id(ability_name))
        .ok_or_else(|| {
            CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: format!("unknown ability: {ability_name}"),
                },
                (handler.span.start, handler.span.end),
            )
        })?;

    let mut arm_fc = FunctionCompiler::new_for_closure(
        fc.function_hashes.clone(),
        fc.locals.clone(),
        fc.local_names.clone(),
    );

    // Allocate implicit slots for continuation and suspended ability.
    let _continuation_slot =
        arm_fc.alloc_local_with_name(0, &Arc::from(super::HANDLER_PARAM_CONTINUATION))?;
    let _ability_slot =
        arm_fc.alloc_local_with_name(0, &Arc::from(super::HANDLER_PARAM_SUSPENDED_ABILITY))?;

    // At the start of the arm, extract ability arguments into param slots.
    for (i, param) in handler.params.iter().enumerate() {
        arm_fc.alloc_local_with_name(param.id, &param.name)?;

        // Load suspended ability from slot 1, extract argument i,
        // store to the param slot (slot 2+i).
        arm_fc.builder.emit_u16(Opcode::LoadLocal, 1);
        arm_fc.builder.emit_get_ability_arg(i as u8);
        arm_fc.builder.emit_u16(Opcode::StoreLocal, 2 + i as u16);
    }

    // Compile the arm body.
    compile_expr(&mut arm_fc, &handler.body, ctx)?;
    arm_fc.builder.emit(Opcode::Return);

    // Build the arm function (2 implicit params).
    let local_count = arm_fc.next_local;
    let capture_names = arm_fc.get_capture_names_in_order();
    let arm_func = arm_fc.builder.build(local_count, 2);
    let arm_hash = ctx.register_lambda(arm_func);

    // In the thunk: load the arm's captured values (capturing through
    // the thunk when they come from the installer), then create the
    // arm closure and install it.
    for (name, _slot) in &capture_names {
        if let Some(&slot) = thunk_fc.local_names.get(name) {
            thunk_fc.builder.emit_u16(Opcode::LoadLocal, slot);
        } else {
            let capture_slot = thunk_fc.get_or_create_capture_by_name(Arc::clone(name));
            thunk_fc.builder.emit_load_capture(capture_slot);
        }
    }
    thunk_fc
        .builder
        .emit_make_closure(arm_hash, capture_names.len() as u8);
    thunk_fc.builder.emit_handle(ability_id);

    Ok(())
}

/// Compile an ability call (perform).
pub(super) fn compile_ability_call(
    fc: &mut FunctionCompiler,
    ability_call: &crate::ast::AbilityCall,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // Compile arguments.
    for arg in &ability_call.args {
        compile_expr(fc, arg, ctx)?;
    }

    // Get ability and method IDs. Locals, foreign, and prelude abilities
    // resolve through the context (identities come from the type
    // checker); core builtins (Exception) fall back to the resolver.
    let ability_name = &ability_call.ability.name;
    let method_name = &ability_call.method;

    let (ability_id, method_id) = ctx
        .resolve_ability(&ability_call.ability)
        .and_then(|info| info.method_id(method_name).map(|m| (info.id, m)))
        .or_else(|| get_ability_ids(ability_name, method_name))
        .ok_or_else(|| {
            CompileError::new(
                CompileErrorKind::UnknownAbilityMethod {
                    ability: Arc::clone(ability_name),
                    method: Arc::clone(method_name),
                },
                (ability_call.span.start, ability_call.span.end),
            )
        })?;

    // Emit suspend instruction (packages the args), then perform.
    fc.builder
        .emit_suspend(ability_id, method_id, ability_call.args.len() as u8);
    fc.builder.emit(Opcode::Perform);

    Ok(())
}

/// Compile a handler literal expression (Milestone 13).
///
/// Handler literals create a `HandlerValue` at runtime. Each method
/// is compiled as a separate function that receives implicit parameters:
/// - slot 0: continuation
/// - slot 1: suspended ability value
/// - slot 2+: extracted ability method arguments
pub(super) fn compile_handler_literal(
    fc: &mut FunctionCompiler,
    expr: &Expr,
    handler_lit: &crate::ast::HandlerLiteralExpr,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // Get the ability ID from the type (set by type checker).
    // The type should be Handler<ability_id>.
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

    let dynamic = ctx
        .ability_by_id(ability_id)
        .map(|(name, info)| (Arc::clone(name), info.clone()));
    let ability_name = dynamic
        .as_ref()
        .map(|(name, _)| name.to_string())
        .or_else(|| get_ability_name(ability_id))
        .ok_or_else(|| {
            CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: format!("unknown ability ID: {ability_id}"),
                },
                (expr.span.start, expr.span.end),
            )
        })?;

    // Compile each handler method as a separate function.
    let mut method_hashes: Vec<(u16, blake3::Hash)> = Vec::new();

    for method in &handler_lit.methods {
        let method_id = dynamic
            .as_ref()
            .and_then(|(_, info)| info.method_id(&method.method))
            .or_else(|| get_method_id_for_ability(ability_id, &method.method))
            .ok_or_else(|| {
                CompileError::new(
                    CompileErrorKind::Unsupported {
                        feature: format!(
                            "unknown method `{}` for ability `{}`",
                            method.method, ability_name
                        ),
                    },
                    (method.span.start, method.span.end),
                )
            })?;

        // Create a new FunctionCompiler for the handler method.
        let mut method_fc = FunctionCompiler::new_for_closure(
            fc.function_hashes.clone(),
            fc.locals.clone(),
            fc.local_names.clone(),
        );

        // Allocate implicit slots for continuation and suspended ability.
        let _continuation_slot =
            method_fc.alloc_local_with_name(0, &Arc::from(super::HANDLER_PARAM_CONTINUATION))?;
        let _ability_slot = method_fc
            .alloc_local_with_name(0, &Arc::from(super::HANDLER_PARAM_SUSPENDED_ABILITY))?;

        // Extract ability arguments and store to param slots.
        for (i, param) in method.params.iter().enumerate() {
            method_fc.alloc_local_with_name(param.id, &param.name)?;

            // Load suspended ability from slot 1.
            method_fc.builder.emit_u16(Opcode::LoadLocal, 1);

            // Extract argument at index i.
            method_fc.builder.emit_get_ability_arg(i as u8);

            // Store to the param slot (slot 2+i).
            method_fc.builder.emit_u16(Opcode::StoreLocal, 2 + i as u16);
        }

        // Compile the handler method body.
        compile_expr(&mut method_fc, &method.body, ctx)?;

        // Emit return instruction.
        method_fc.builder.emit(Opcode::Return);

        // Build the handler method function.
        let bytecode = method_fc.builder.bytecode().to_vec();
        let constants = method_fc.builder.constants().to_vec();
        let dependencies = method_fc.builder.dependencies().to_vec();
        let local_count = method_fc.next_local;
        // Handler receives 2 implicit params: continuation and suspended ability.
        let param_count = 2;

        let method_func = CompiledFunction {
            hash: ctx.next_lambda_hash(),
            bytecode,
            constants,
            local_count,
            param_count,
            dependencies,
            debug_info: None,
        };

        // Get capture info for the handler method.
        let capture_names = method_fc.get_capture_names_in_order();

        // Register the handler method function.
        let final_hash = ctx.register_lambda(method_func);

        // If handler method captures variables, emit code to load them.
        // This is similar to closure compilation.
        for (name, _slot) in &capture_names {
            if let Some(&slot) = fc.local_names.get(name) {
                fc.builder.emit_u16(Opcode::LoadLocal, slot);
            } else if let Some(&capture_slot) = fc.capture_names.get(name) {
                fc.builder.emit_load_capture(capture_slot);
            }
            // If not found, the capture tracking is wrong, but we continue.
        }

        method_hashes.push((method_id, final_hash));
    }

    // Calculate total capture count (if any methods capture variables).
    // For simplicity, we don't support handler method captures yet.
    let capture_count = 0u8;

    // Emit MakeHandler instruction.
    fc.builder
        .emit_make_handler(ability_id, &method_hashes, capture_count);

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Ability ID Helpers
// ─────────────────────────────────────────────────────────────────────────────

// These consult the engine's builtin ability set, which is core-only
// (Exception). Platform abilities resolve through the module context's
// registered abilities (prelude + local declarations) before reaching
// these fallbacks.

/// Get ability and method IDs for a core ability.
pub(super) fn get_ability_ids(ability: &str, method: &str) -> Option<(AbilityId, u16)> {
    let resolver = crate::ability_resolver::core_abilities();
    resolver.get_method(ability, method)
}

/// Get a core ability's ID.
pub(super) fn get_ability_id(ability: &str) -> Option<AbilityId> {
    let resolver = crate::ability_resolver::core_abilities();
    resolver.name_to_id(ability)
}

/// Get a method ID from a core ability's ID and a method name.
pub(super) fn get_method_id_for_ability(ability_id: AbilityId, method_name: &str) -> Option<u16> {
    let resolver = crate::ability_resolver::core_abilities();
    resolver.get_method_by_ability_id(ability_id, method_name)
}

/// Get a core ability's name from its ID.
pub(super) fn get_ability_name(ability_id: AbilityId) -> Option<String> {
    let resolver = crate::ability_resolver::core_abilities();
    resolver.id_to_name(ability_id).map(String::from)
}
