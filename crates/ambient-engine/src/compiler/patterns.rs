//! Pattern matching compilation.
//!
//! This module handles compilation of match expressions and pattern matching.

use crate::ast::{Expr, Literal, MatchArm, Pattern, PatternKind};
use crate::bytecode::Opcode;
use crate::value::Value;

use super::error::{CompileError, CompileErrorKind};
use super::{compile_expr, str_to_value, FunctionCompiler, ModuleContext};

/// Compile a match expression.
pub(super) fn compile_match(
    fc: &mut FunctionCompiler,
    scrutinee: &Expr,
    arms: &[MatchArm],
    span: (u32, u32),
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    if arms.is_empty() {
        return Err(CompileError::new(
            CompileErrorKind::Unsupported {
                feature: "empty match expression".to_string(),
            },
            span,
        ));
    }

    // Compile the scrutinee.
    compile_expr(fc, scrutinee, ctx)?;

    // For now, only support simple patterns (wildcards, bindings, literals).
    // TODO: Full pattern matching with nested patterns and guards.

    let mut end_jumps = Vec::new();

    for (i, arm) in arms.iter().enumerate() {
        let is_last = i == arms.len() - 1;

        // Duplicate scrutinee for pattern matching (except last arm).
        if !is_last {
            fc.builder.emit(Opcode::Dup);
        }

        // Compile pattern match.
        let fail_jump = compile_pattern_match(fc, ctx, &arm.pattern, is_last)?;

        // If guard exists, compile it.
        if let Some(guard) = &arm.guard {
            compile_expr(fc, guard, ctx)?;
            if let Some(fj) = fail_jump {
                // If pattern matched but guard fails, need to jump to next arm.
                let guard_fail = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
                // Pattern and guard both succeeded - compile body.
                compile_expr(fc, &arm.body, ctx)?;
                let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
                end_jumps.push(end_jump);
                fc.builder.patch_jump(guard_fail);
                fc.builder.patch_jump(fj);
            } else {
                // Last arm with guard.
                let guard_fail = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
                compile_expr(fc, &arm.body, ctx)?;
                let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
                end_jumps.push(end_jump);
                fc.builder.patch_jump(guard_fail);
                // If guard fails on last arm, push unit as result.
                fc.builder.emit_const(Value::Unit);
            }
        } else if let Some(fj) = fail_jump {
            // Pattern match can fail, compile body and jump to end.
            compile_expr(fc, &arm.body, ctx)?;
            let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
            end_jumps.push(end_jump);
            fc.builder.patch_jump(fj);
        } else {
            // Pattern always matches (wildcard or binding) and it's the last arm.
            compile_expr(fc, &arm.body, ctx)?;
        }
    }

    // Patch all end jumps to here.
    for jump in end_jumps {
        fc.builder.patch_jump(jump);
    }

    Ok(())
}

/// Compile a pattern match. Returns jump offset if pattern can fail.
fn compile_pattern_match(
    fc: &mut FunctionCompiler,
    ctx: &ModuleContext,
    pattern: &Pattern,
    is_last: bool,
) -> Result<Option<usize>, CompileError> {
    match &pattern.kind {
        PatternKind::Wildcard => {
            // Wildcard always matches, consume the scrutinee.
            if !is_last {
                fc.builder.emit(Opcode::Pop);
            }
            Ok(None)
        }

        PatternKind::Binding(id, name) => {
            // Binding always matches, store in local.
            let slot = fc.alloc_local_with_name(*id, name)?;
            fc.builder.emit_u16(Opcode::StoreLocal, slot);
            if !is_last {
                // Consume the duplicated scrutinee.
                fc.builder.emit(Opcode::Pop);
            }
            Ok(None)
        }

        PatternKind::Literal(lit) => {
            // Compare with literal.
            let value = match lit {
                Literal::Unit => Value::Unit,
                Literal::Bool(b) => Value::Bool(*b),
                Literal::Number(n) => Value::Number(*n),
                Literal::String(s) => str_to_value(s),
            };
            fc.builder.emit_const(value);
            fc.builder.emit(Opcode::Eq);
            let fail_jump = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
            if !is_last {
                fc.builder.emit(Opcode::Pop);
            }
            Ok(Some(fail_jump))
        }

        PatternKind::Variant(variant_name, inner_pattern) => {
            // Enum variant pattern matching: `Some(x)`, `None`, `Ok(v)`, `Err(e)`
            //
            // Stack behavior (from VM dispatch.rs):
            // - EnumIs: uses peek(), pushes bool -> [enum] becomes [enum, bool]
            // - EnumPayload: pops enum, pushes payload -> [enum] becomes [payload]
            //
            // For non-last arms, compile_match does Dup first, so we have [orig, dup].
            // For last arm, we just have [orig].
            //
            // The challenge: EnumIs doesn't consume the enum, so on the fail path
            // we have [orig, dup, bool] -> JumpIfNot -> [orig, dup].
            // But the next arm expects [orig] to Dup from.
            //
            // Solution: On fail path, pop the dup before continuing to next arm.
            // We emit:
            //   EnumIs           -> [orig, dup, bool] or [orig, bool]
            //   JumpIfNot cleanup_and_fail
            //   <success code>
            //   Jump past_cleanup
            //   cleanup_and_fail: Pop  (remove dup on fail path)
            //   past_cleanup: <returned as fail_jump for compile_match to patch>

            let tag = ctx
                .enums
                .get(&variant_name.name)
                .map(|v| v.tag)
                .ok_or_else(|| {
                    CompileError::new(
                        CompileErrorKind::Unsupported {
                            feature: format!("unknown variant: {}", variant_name.name),
                        },
                        (pattern.span.start, pattern.span.end),
                    )
                })?;

            // Check if the enum matches this variant tag.
            // EnumIs: [enum] -> [enum, bool]
            fc.builder.emit_enum_is(tag);

            // Jump to cleanup on fail
            let cleanup_jump = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);

            // === SUCCESS PATH ===
            // Stack after JumpIfNot (not taken): [orig, dup] or [orig]
            // (JumpIfNot consumed the bool)

            if let Some(inner) = inner_pattern {
                // Extract payload: [orig, dup] -> [orig, payload]
                fc.builder.emit_enum_payload();

                // Recursively match the inner pattern against the payload.
                // is_last=true because we want inner match to consume the payload.
                compile_pattern_match(fc, ctx, inner, true)?;
                // Stack: [orig] (non-last) or [] (last)
            } else {
                // Unit variant (like None) - pop the matched enum
                fc.builder.emit(Opcode::Pop);
                // Stack: [orig] (non-last) or [] (last)
            }

            // For non-last arms, pop the original scrutinee (compile_match expects [])
            if !is_last {
                fc.builder.emit(Opcode::Pop);
            }
            // Stack: []

            // Success: jump over the fail-path cleanup AND the fail jump,
            // landing where compile_match emits the arm body.
            let to_body_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);

            // === FAIL PATH (cleanup) ===
            fc.builder.patch_jump(cleanup_jump);
            // JumpIfNot consumed the bool; stack: [orig, dup] (non-last)
            // or [orig] (last). Pop the enum EnumIs left behind.
            fc.builder.emit(Opcode::Pop);

            // Then skip the arm body: compile_match patches this to the
            // next arm (or the end of the match for the last arm).
            let fail_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);

            // The arm body starts here.
            fc.builder.patch_jump(to_body_jump);

            Ok(Some(fail_jump))
        }

        PatternKind::Tuple(_) | PatternKind::Record(_) => Err(CompileError::new(
            CompileErrorKind::Unsupported {
                feature: "complex patterns (tuple/record)".to_string(),
            },
            (pattern.span.start, pattern.span.end),
        )),
    }
}
