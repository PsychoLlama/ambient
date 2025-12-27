//! Lambda and closure compilation.
//!
//! This module handles compilation of lambda expressions and closures,
//! including variable capture analysis and closure creation.

use std::sync::Arc;

use crate::ast::{Expr, Lambda};
use crate::bytecode::{CompiledFunction, Opcode};

use super::error::{CompileError, CompileErrorKind};
use super::{compile_expr, FunctionCompiler, ModuleContext};

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
    let mut handler_hashes = Vec::new();
    let mut handler_ability_ids = Vec::new();

    // Track jump offsets for handler values (from `with` clause)
    let mut handler_value_jump_offsets = Vec::new();

    // Compile handler values from `with` clause
    // Each handler value expression evaluates to a HandlerValue on the stack,
    // then HandleWithValue pops it and installs it as a handler.
    for handler_value in &handle_expr.handler_values {
        // Verify this is a Handler type (type checking should have caught errors)
        match &handler_value.ty {
            Some(crate::types::Type::Handler(_)) => {}
            _ => {
                // Skip if not a handler type - type checking should have caught this
                continue;
            }
        }

        // Compile the handler value expression - this leaves a HandlerValue on the stack.
        compile_expr(fc, handler_value, ctx)?;

        // Emit HandleWithValue to pop the handler and install it
        let jump_offset = fc.builder.emit_handle_with_value();
        handler_value_jump_offsets.push(jump_offset);
    }

    // First, compile each inline handler as a separate function.
    for handler in &handle_expr.handlers {
        // Get ability ID for this handler.
        let ability_name = &handler.ability.name;
        let ability_id = get_ability_id(ability_name).ok_or_else(|| {
            CompileError::new(
                CompileErrorKind::Unsupported {
                    feature: format!("unknown ability: {ability_name}"),
                },
                (handler.span.start, handler.span.end),
            )
        })?;

        // Create a new FunctionCompiler for the handler.
        // Handler functions have implicit parameters:
        // - slot 0: continuation
        // - slot 1: suspended ability value
        // - slot 2+: ability method arguments
        let mut handler_fc = FunctionCompiler::new(fc.function_hashes.clone());

        // Allocate implicit slots for continuation and suspended ability.
        let _continuation_slot =
            handler_fc.alloc_local_with_name(0, &Arc::from(super::HANDLER_PARAM_CONTINUATION))?;
        let _ability_slot = handler_fc
            .alloc_local_with_name(0, &Arc::from(super::HANDLER_PARAM_SUSPENDED_ABILITY))?;

        // At the start of the handler, extract ability arguments and store to param slots.
        // For each param, we need to:
        // 1. Load the suspended ability from slot 1
        // 2. Extract the argument at the corresponding index
        // 3. Store to the param's slot
        for (i, param) in handler.params.iter().enumerate() {
            // Allocate slot for this param.
            handler_fc.alloc_local_with_name(param.id, &param.name)?;

            // Load suspended ability from slot 1.
            handler_fc.builder.emit_u16(Opcode::LoadLocal, 1);

            // Extract argument at index i.
            handler_fc.builder.emit_get_ability_arg(i as u8);

            // Store to the param slot (slot 2+i).
            handler_fc
                .builder
                .emit_u16(Opcode::StoreLocal, 2 + i as u16);
        }

        // Compile the handler body.
        compile_expr(&mut handler_fc, &handler.body, ctx)?;

        // Emit return instruction.
        handler_fc.builder.emit(Opcode::Return);

        // Build the handler function.
        let local_count = handler_fc.next_local;
        // Handler receives 2 implicit params: continuation and suspended ability.
        let param_count = 2;
        let handler_func = handler_fc.builder.build(local_count, param_count);

        // Register the handler function (associates it with current parent function).
        let handler_hash = ctx.register_lambda(handler_func);

        handler_hashes.push(handler_hash);
        handler_ability_ids.push(ability_id);
    }

    // Now emit the handle expression code.
    // Install handlers and compile the body.

    // Store the handle instruction jump offsets for patching.
    let mut handle_jump_offsets = Vec::new();

    // Emit Handle instructions for each handler.
    for (i, (ability_id, handler_hash)) in handler_ability_ids
        .iter()
        .zip(handler_hashes.iter())
        .enumerate()
    {
        let _ = i; // Silence unused warning.
        let jump_offset = fc.builder.emit_handle(*ability_id, *handler_hash);
        handle_jump_offsets.push(jump_offset);
    }

    // Compile the body expression.
    compile_expr(fc, &handle_expr.body, ctx)?;

    // Emit Unhandle for each handler (in reverse order).
    // First unhandle inline handlers (most recently installed)
    for _ in &handle_expr.handlers {
        fc.builder.emit(Opcode::Unhandle);
    }
    // Then unhandle handler values (installed first)
    for _ in &handle_expr.handler_values {
        fc.builder.emit(Opcode::Unhandle);
    }

    // Patch all the handle instruction jump offsets to point here.
    // Patch inline handler offsets
    for offset in handle_jump_offsets {
        fc.builder.patch_handle(offset);
    }
    // Patch handler value offsets
    for offset in handler_value_jump_offsets {
        fc.builder.patch_handle_with_value(offset);
    }

    // Handle else clause if present.
    if let Some(else_clause) = &handle_expr.else_clause {
        // The else clause wraps the result of normal completion.
        // For now, just compile it and drop the body result.
        fc.builder.emit(Opcode::Pop);
        compile_expr(fc, else_clause, ctx)?;
    }

    Ok(())
}

/// Compile an ability call (either perform or suspend).
pub(super) fn compile_ability_call(
    fc: &mut FunctionCompiler,
    ability_call: &crate::ast::AbilityCall,
    perform: bool,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // Compile arguments.
    for arg in &ability_call.args {
        compile_expr(fc, arg, ctx)?;
    }

    // Get ability and method IDs.
    // For now, use a simple mapping based on well-known abilities.
    let ability_name = &ability_call.ability.name;
    let method_name = &ability_call.method;

    let (ability_id, method_id) = get_ability_ids(ability_name, method_name).ok_or_else(|| {
        CompileError::new(
            CompileErrorKind::UnknownAbilityMethod {
                ability: Arc::clone(ability_name),
                method: Arc::clone(method_name),
            },
            (ability_call.span.start, ability_call.span.end),
        )
    })?;

    // Emit suspend instruction.
    fc.builder
        .emit_suspend(ability_id, method_id, ability_call.args.len() as u8);

    // If performing, emit perform instruction.
    if perform {
        fc.builder.emit(Opcode::Perform);
    }

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

    let ability_name = get_ability_name(ability_id).ok_or_else(|| {
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
        let method_id = get_method_id_for_ability(ability_id, &method.method).ok_or_else(|| {
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

/// Get ability and method IDs using the ability resolver.
pub(super) fn get_ability_ids(ability: &str, method: &str) -> Option<(u16, u16)> {
    let resolver = crate::ability_resolver::standard_abilities();
    resolver.get_method(ability, method)
}

/// Get ability ID using the ability resolver.
pub(super) fn get_ability_id(ability: &str) -> Option<u16> {
    let resolver = crate::ability_resolver::standard_abilities();
    resolver.name_to_id(ability)
}

/// Get method ID from ability ID and method name using the ability resolver.
pub(super) fn get_method_id_for_ability(ability_id: u16, method_name: &str) -> Option<u16> {
    let resolver = crate::ability_resolver::standard_abilities();
    resolver.get_method_by_ability_id(ability_id, method_name)
}

/// Get ability name from ability ID using the ability resolver.
pub(super) fn get_ability_name(ability_id: u16) -> Option<&'static str> {
    // Use static names from the ability crates for known abilities.
    // The resolver returns &str from dynamic data, but these are compile-time constants.
    use ambient_core::exception::ExceptionAbility;
    use ambient_runtime::{
        async_ability::AsyncAbility, console::ConsoleAbility, log::LogAbility,
        random::RandomAbility, time::TimeAbility,
    };

    match ability_id {
        id if id == ConsoleAbility::ABILITY_ID => Some(ConsoleAbility::NAME),
        id if id == ExceptionAbility::ABILITY_ID => Some(ExceptionAbility::NAME),
        id if id == TimeAbility::ABILITY_ID => Some(TimeAbility::NAME),
        id if id == RandomAbility::ABILITY_ID => Some(RandomAbility::NAME),
        id if id == AsyncAbility::ABILITY_ID => Some(AsyncAbility::NAME),
        id if id == LogAbility::ABILITY_ID => Some(LogAbility::NAME),
        _ => None,
    }
}
