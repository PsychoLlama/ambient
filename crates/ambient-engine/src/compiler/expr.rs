//! Expression and statement compilation.
//!
//! This module handles compilation of AST expressions and statements into
//! bytecode. It is the core of the compilation pipeline, transforming
//! high-level language constructs into low-level VM instructions.
//!
//! # Expression Compilation
//!
//! Each expression type is compiled to bytecode that pushes its result onto
//! the value stack. For example:
//! - Literals: push constant value
//! - Variables: load from local slot or capture
//! - Operators: compile operands, emit operation opcode
//! - Control flow: emit jumps with placeholder patching
//!
//! # Statement Compilation
//!
//! Statements modify program state without producing a value:
//! - Let bindings: compile initializer, store in new local slot
//! - Expression statements: compile expression, pop result

use std::sync::Arc;

use crate::ast::{BinaryOp, ConstDef, Expr, ExprKind, LetBinding, Stmt, StmtKind, UnaryOp};
use crate::bytecode::Opcode;
use crate::fqn::NameKey;
use crate::value::Value;

use super::error::{CompileError, CompileErrorKind};
use super::lambdas::{
    compile_ability_call, compile_handle_expr, compile_handler_literal, compile_lambda,
};
use super::patterns::compile_match;
use super::{FunctionCompiler, ModuleContext, str_to_value};

/// Compile an expression, pushing its value onto the stack.
#[allow(clippy::too_many_lines)]
pub(super) fn compile_expr(
    fc: &mut FunctionCompiler,
    expr: &Expr,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // Record source location for debugging
    fc.record_span(expr.span);

    match &expr.kind {
        // ─────────────────────────────────────────────────────────────────────
        // Literals
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Unit => {
            fc.builder.emit_const(Value::Unit);
        }

        ExprKind::Bool(b) => {
            fc.builder.emit_const(Value::Bool(*b));
        }

        ExprKind::Number(n) => {
            fc.builder.emit_const(Value::Number(*n));
        }

        ExprKind::String(s) => {
            fc.builder.emit_const(str_to_value(s));
        }

        // ─────────────────────────────────────────────────────────────────────
        // Variables
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Local(id) => {
            // Check if this is a captured variable from the parent scope.
            if let Some(&capture_slot) = fc.captures.get(id) {
                fc.builder.emit_load_capture(capture_slot);
            } else if fc.is_parent_binding(*id) {
                // This is a free variable from the parent - add it as a capture.
                // Find the name for this binding from parent.
                let name = fc
                    .parent_locals
                    .as_ref()
                    .and_then(|_| {
                        // Look up name from parent_local_names by finding which name maps to the same slot
                        fc.parent_local_names.as_ref().and_then(|names| {
                            names.iter().find(|&(_, &slot)| {
                                fc.parent_locals
                                    .as_ref()
                                    .is_some_and(|pl| pl.get(id).copied() == Some(slot))
                            })
                        })
                    })
                    .map_or_else(|| format!("__binding_{id}").into(), |(n, _)| Arc::clone(n));
                let capture_slot = fc.get_or_create_capture(*id, name);
                fc.builder.emit_load_capture(capture_slot);
            } else {
                let slot = fc.get_local(*id, (expr.span.start, expr.span.end))?;
                fc.builder.emit_u16(Opcode::LoadLocal, slot);
            }
        }

        ExprKind::Name(name) => {
            // Locals go by their bare spelled name; everything else goes by
            // the resolution key — the canonical qualified form for
            // cross-module references, the bare name for module-local ones.
            let var_name = &name.name;
            let key = name.resolution_key();
            if name.path.is_empty()
                && name.resolved.is_none()
                && let Some(slot) = fc.get_local_by_name(var_name)
            {
                // Load the local variable.
                fc.builder.emit_u16(Opcode::LoadLocal, slot);
            } else if name.path.is_empty()
                && name.resolved.is_none()
                && let Some(&capture_slot) = fc.capture_names.get(var_name)
            {
                // Load from captured environment.
                fc.builder.emit_load_capture(capture_slot);
            } else if name.path.is_empty() && name.resolved.is_none() && fc.is_parent_name(var_name)
            {
                // This is a free variable from the parent - add it as a capture.
                let capture_slot = fc.get_or_create_capture_by_name(Arc::clone(var_name));
                fc.builder.emit_load_capture(capture_slot);
            } else if name.path.is_empty()
                && name.resolved.is_none()
                && let Some(&hash) = fc.block_consts.get(var_name)
            {
                // Block-scoped constant: like a module const, load its
                // content-addressed value object by hash (recorded as a
                // dependency). Checked before the module-level tables so a
                // block `const` shadows an outer const or function.
                fc.builder.emit_load_object(hash);
            } else if let Some(hash) = ctx.constant_hash(&key) {
                // Module-level constant: load its content-addressed value
                // object. The `const` is stored once and referenced by hash
                // (recorded as a dependency), deduplicated like a function —
                // never inlined at the reference site.
                fc.builder.emit_load_object(hash);
            } else if let Some(&hash) = fc.function_hashes.get(&key) {
                // A bare identifier resolving to a function hash is a function
                // reference - push it for later use.
                fc.builder.emit_const(Value::FunctionRef(hash));
            } else if let Some(variant) = name
                .resolved
                .as_ref()
                .and_then(|fqn| ctx.foreign_variants.get(fqn))
                .cloned()
            {
                // Fully-qualified unit variant as a value (`core::option::None`).
                // Keyed by `Fqn`, consulted before the bare `ctx.enums` table
                // so a same-named local variant can't steal its tag.
                if variant.has_payload {
                    return Err(CompileError::new(
                        CompileErrorKind::Unsupported {
                            feature: format!(
                                "constructor `{var_name}` used as a value; apply it: `{var_name}(...)`"
                            ),
                        },
                        (expr.span.start, expr.span.end),
                    ));
                }
                fc.builder
                    .emit_make_enum(&variant.enum_name, variant.tag, var_name, false);
            } else if let Some(variant) = ctx.enums.get(var_name).cloned() {
                // Unit enum variant as a value: `None`, `Nothing`.
                if variant.has_payload {
                    return Err(CompileError::new(
                        CompileErrorKind::Unsupported {
                            feature: format!(
                                "constructor `{var_name}` used as a value; apply it: `{var_name}(...)`"
                            ),
                        },
                        (expr.span.start, expr.span.end),
                    ));
                }
                fc.builder
                    .emit_make_enum(&variant.enum_name, variant.tag, var_name, false);
            } else if ctx.unit_structs.contains(&key) {
                // Unit struct constructed by its bare name: `Origin`. It is an
                // empty record value — a unit struct carries no runtime
                // nominal tag, exactly like every nominal record (identity is
                // compile-time only). Parallel to the nullary-variant branch.
                fc.builder.emit_u8(Opcode::MakeRecord, 0);
            } else {
                return Err(CompileError::new(
                    CompileErrorKind::UndefinedFunction {
                        name: Arc::clone(var_name),
                    },
                    (expr.span.start, expr.span.end),
                ));
            }
        }

        // ─────────────────────────────────────────────────────────────────────
        // Compound expressions
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Tuple(elements) => {
            for elem in elements {
                compile_expr(fc, elem, ctx)?;
            }
            fc.builder.emit_u8(Opcode::MakeTuple, elements.len() as u8);
        }

        ExprKind::TupleIndex(tuple, index) => {
            compile_expr(fc, tuple, ctx)?;
            fc.builder.emit_u8(Opcode::TupleGet, *index as u8);
        }

        ExprKind::Record(fields) => {
            // Push field names and values interleaved.
            for (name, value) in fields {
                fc.builder.emit_const(str_to_value(name));
                compile_expr(fc, value, ctx)?;
            }
            fc.builder.emit_u8(Opcode::MakeRecord, fields.len() as u8);
        }

        ExprKind::TypedRecord { fields, .. } => {
            // Typed records compile exactly like regular records.
            // The type information is a compile-time concept only.
            for (name, value) in fields {
                fc.builder.emit_const(str_to_value(name));
                compile_expr(fc, value, ctx)?;
            }
            fc.builder.emit_u8(Opcode::MakeRecord, fields.len() as u8);
        }

        ExprKind::RecordField(record, field) => {
            compile_expr(fc, record, ctx)?;
            let idx = fc.builder.add_constant(str_to_value(field));
            fc.builder.emit_u16(Opcode::RecordGet, idx);
        }

        ExprKind::List(elements) => {
            for elem in elements {
                compile_expr(fc, elem, ctx)?;
            }
            fc.builder.emit_make_list(elements.len() as u16);
        }

        // ─────────────────────────────────────────────────────────────────────
        // Operators
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Binary {
            op,
            left,
            right,
            resolved_op,
        } => {
            // Short-circuit evaluation for logical operators.
            match op {
                BinaryOp::And => {
                    compile_expr(fc, left, ctx)?;
                    fc.builder.emit(Opcode::Dup);
                    let jump = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
                    fc.builder.emit(Opcode::Pop);
                    compile_expr(fc, right, ctx)?;
                    fc.builder.patch_jump(jump);
                }
                BinaryOp::Or => {
                    compile_expr(fc, left, ctx)?;
                    fc.builder.emit(Opcode::Dup);
                    let jump = fc.builder.emit_jump_placeholder(Opcode::JumpIf);
                    fc.builder.emit(Opcode::Pop);
                    compile_expr(fc, right, ctx)?;
                    fc.builder.patch_jump(jump);
                }
                _ => {
                    compile_expr(fc, left, ctx)?;
                    compile_expr(fc, right, ctx)?;

                    // Check if we have a resolved trait method for operator overloading
                    if let Some(symbol) = resolved_op {
                        // Operator is overloaded - call the trait method.
                        let Some(&hash) =
                            fc.function_hashes.get(&NameKey::Bare(Arc::clone(symbol)))
                        else {
                            return Err(CompileError::new(
                                CompileErrorKind::UndefinedFunction {
                                    name: Arc::clone(symbol),
                                },
                                (expr.span.start, expr.span.end),
                            ));
                        };
                        fc.builder.emit_call(hash, 2);

                        // Adapt the trait method's result to the operator's
                        // semantics: `Eq.eq` provides `==` directly, `!=` is
                        // its negation; `Ord.cmp` returns -1/0/1 which each
                        // ordering operator compares against 0.
                        match op {
                            BinaryOp::Ne => {
                                fc.builder.emit(Opcode::Not);
                            }
                            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                                fc.builder.emit_const(Value::Number(0.0));
                                fc.builder.emit(match op {
                                    BinaryOp::Lt => Opcode::Lt,
                                    BinaryOp::Le => Opcode::Le,
                                    BinaryOp::Gt => Opcode::Gt,
                                    _ => Opcode::Ge,
                                });
                            }
                            _ => {}
                        }
                    } else {
                        // Built-in operator
                        let opcode = match op {
                            BinaryOp::Add => Opcode::Add,
                            BinaryOp::Sub => Opcode::Sub,
                            BinaryOp::Mul => Opcode::Mul,
                            BinaryOp::Div => Opcode::Div,
                            BinaryOp::Mod => Opcode::Mod,
                            BinaryOp::Eq => Opcode::Eq,
                            BinaryOp::Ne => Opcode::Ne,
                            BinaryOp::Lt => Opcode::Lt,
                            BinaryOp::Le => Opcode::Le,
                            BinaryOp::Gt => Opcode::Gt,
                            BinaryOp::Ge => Opcode::Ge,
                            BinaryOp::And | BinaryOp::Or => unreachable!(),
                        };
                        fc.builder.emit(opcode);
                    }
                }
            }
        }

        ExprKind::Unary(op, operand) => {
            compile_expr(fc, operand, ctx)?;
            let opcode = match op {
                UnaryOp::Neg => Opcode::Neg,
                UnaryOp::Not => Opcode::Not,
            };
            fc.builder.emit(opcode);
        }

        // ─────────────────────────────────────────────────────────────────────
        // Control flow
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::If(cond, then_branch, else_branch) => {
            compile_expr(fc, cond, ctx)?;

            let else_jump = fc.builder.emit_jump_placeholder(Opcode::JumpIfNot);
            compile_expr(fc, then_branch, ctx)?;

            if let Some(else_expr) = else_branch {
                let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
                fc.builder.patch_jump(else_jump);
                compile_expr(fc, else_expr, ctx)?;
                fc.builder.patch_jump(end_jump);
            } else {
                // No else branch - if condition is false, push unit.
                let end_jump = fc.builder.emit_jump_placeholder(Opcode::Jump);
                fc.builder.patch_jump(else_jump);
                fc.builder.emit_const(Value::Unit);
                fc.builder.patch_jump(end_jump);
            }
        }

        ExprKind::Match(scrutinee, arms) => {
            compile_match(fc, scrutinee, arms, (expr.span.start, expr.span.end), ctx)?;
        }

        ExprKind::Block(stmts, result) => {
            for stmt in stmts {
                compile_stmt(fc, stmt, ctx)?;
            }
            if let Some(result_expr) = result {
                compile_expr(fc, result_expr, ctx)?;
            } else {
                fc.builder.emit_const(Value::Unit);
            }
        }

        // ─────────────────────────────────────────────────────────────────────
        // Functions and calls
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Lambda(lambda) => {
            // Compile the lambda body as a separate function.
            // The closure will capture variables from the enclosing scope.
            compile_lambda(fc, lambda, ctx)?;
        }

        ExprKind::Call(callee, args) => {
            // Check if this is a direct call to a known function or an indirect call.
            if let ExprKind::Name(name) = &callee.kind {
                let key = name.resolution_key();
                let is_cross_module = name.resolved.is_some() || !name.path.is_empty();
                if fc.function_hashes.contains_key(&key) {
                    // Direct call to a known function: module-local by bare
                    // name, cross-module by canonical qualified key.
                    // Compile arguments first (left to right).
                    for arg in args {
                        compile_expr(fc, arg, ctx)?;
                    }
                    let hash = fc.function_hashes[&key];
                    fc.builder.emit_call(hash, args.len() as u8);
                } else if name.resolved.is_none()
                    && name.path.is_empty()
                    && (fc.get_local_by_name(&name.name).is_some()
                        || fc.capture_names.contains_key(&name.name)
                        || fc.is_parent_name(&name.name))
                {
                    // Indirect call through a closure stored in a variable.
                    // Only a *bare* unresolved name can be a local, so this is
                    // checked before the enum branch — a local shadowing a
                    // variant constructor wins. First compile the closure
                    // (callee), then arguments.
                    compile_expr(fc, callee, ctx)?;
                    for arg in args {
                        compile_expr(fc, arg, ctx)?;
                    }
                    fc.builder.emit_call_closure(args.len() as u8);
                } else if let Some(variant) = name
                    .resolved
                    .as_ref()
                    .and_then(|fqn| ctx.foreign_variants.get(fqn))
                    .cloned()
                {
                    // A fully-qualified variant constructor
                    // (`core::option::Some(x)`, `pkg::shapes::Shape::Circle(3)`).
                    // Keyed by `Fqn`, so it is consulted *before* the bare
                    // `ctx.enums` table and the cross-module bail — a
                    // same-named local variant can never steal its tag.
                    if !variant.has_payload || args.len() != 1 {
                        return Err(CompileError::new(
                            CompileErrorKind::Internal {
                                message: "enum constructor arity mismatch (checker bug)",
                            },
                            (callee.span.start, callee.span.end),
                        ));
                    }
                    compile_expr(fc, &args[0], ctx)?;
                    fc.builder
                        .emit_make_enum(&variant.enum_name, variant.tag, &name.name, true);
                } else if let Some(variant) = ctx.enums.get(&name.name).cloned() {
                    // Enum variant constructor: `Some(x)`, `Just(v)`. Checked
                    // before the cross-module bail: a same-module variant now
                    // resolves to its `Fqn` (so `is_cross_module` is true), but
                    // its runtime identity is still the bare-named tag in
                    // `ctx.enums`, not a linked function hash.
                    if !variant.has_payload || args.len() != 1 {
                        return Err(CompileError::new(
                            CompileErrorKind::Internal {
                                message: "enum constructor arity mismatch (checker bug)",
                            },
                            (callee.span.start, callee.span.end),
                        ));
                    }
                    compile_expr(fc, &args[0], ctx)?;
                    fc.builder
                        .emit_make_enum(&variant.enum_name, variant.tag, &name.name, true);
                } else if is_cross_module {
                    // A qualified reference that linked to nothing.
                    return Err(CompileError::new(
                        CompileErrorKind::UndefinedFunction {
                            name: name.joined(),
                        },
                        (callee.span.start, callee.span.end),
                    ));
                } else {
                    // Unknown function - will error at runtime
                    compile_expr(fc, callee, ctx)?;
                    for arg in args {
                        compile_expr(fc, arg, ctx)?;
                    }
                    fc.builder.emit_call_closure(args.len() as u8);
                }
            } else {
                // General indirect call (e.g., calling a lambda inline or result of expression).
                // Compile callee first, then arguments.
                compile_expr(fc, callee, ctx)?;
                for arg in args {
                    compile_expr(fc, arg, ctx)?;
                }
                fc.builder.emit_call_closure(args.len() as u8);
            }
        }

        // ─────────────────────────────────────────────────────────────────────
        // Abilities
        // ─────────────────────────────────────────────────────────────────────
        ExprKind::Perform(ability_call) => {
            compile_ability_call(fc, ability_call, ctx)?;
        }

        ExprKind::Handle(handle_expr) => {
            compile_handle_expr(fc, handle_expr, ctx)?;
        }

        ExprKind::Resume(value) => {
            // Resume transfers control back to a captured continuation.
            // In a handler function, the continuation is passed as the first argument (local slot 0).
            //
            // Compile to:
            // 1. Load continuation from local slot 0
            // 2. Push the resume value
            // 3. Emit Resume opcode

            // Load continuation from local slot 0 (implicit first parameter in handlers)
            fc.builder.emit_u16(Opcode::LoadLocal, 0);

            // Compile the value to resume with
            compile_expr(fc, value, ctx)?;

            // Emit Resume opcode
            fc.builder.emit(Opcode::Resume);
        }

        ExprKind::HandlerLiteral(handler_lit) => {
            compile_handler_literal(fc, expr, handler_lit, ctx)?;
        }

        ExprKind::Sandbox(sandbox_expr) => {
            // Sandbox compilation (Milestone 14)
            //
            // Sandboxing is a compile-time feature - the type checker has already
            // verified that the body only uses allowed abilities. At runtime,
            // we simply execute the body with no special handling.
            //
            // The type system ensures:
            // - Only allowed abilities are used within the sandbox body
            // - Unknown abilities in the `with` clause are rejected
            //
            // Future enhancements could add runtime enforcement via:
            // - Handler frame markers to prevent ability escalation
            // - Dynamic capability tracking
            compile_expr(fc, &sandbox_expr.body, ctx)?;
        }

        ExprKind::MethodCall {
            receiver,
            args,
            resolved_method,
            ..
        } => {
            // Method calls are compiled as regular function calls: type
            // checking resolved the call to a canonical impl-method symbol,
            // which we look up in the same name→hash table as ordinary calls.
            let Some(symbol) = resolved_method else {
                return Err(CompileError::new(
                    CompileErrorKind::Internal {
                        message: "method call missing resolved symbol",
                    },
                    (expr.span.start, expr.span.end),
                ));
            };
            let Some(&hash) = fc.function_hashes.get(&NameKey::Bare(Arc::clone(symbol))) else {
                return Err(CompileError::new(
                    CompileErrorKind::UndefinedFunction {
                        name: Arc::clone(symbol),
                    },
                    (expr.span.start, expr.span.end),
                ));
            };

            // Compile receiver (self) as first argument
            compile_expr(fc, receiver, ctx)?;

            // Compile other arguments
            for arg in args {
                compile_expr(fc, arg, ctx)?;
            }

            // Emit call with arity = self + args
            #[allow(clippy::cast_possible_truncation)]
            let arity = (1 + args.len()) as u8;
            fc.builder.emit_call(hash, arity);
        }
    }

    Ok(())
}

/// Compile a statement.
pub(super) fn compile_stmt(
    fc: &mut FunctionCompiler,
    stmt: &Stmt,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    match &stmt.kind {
        StmtKind::Let(let_binding) => {
            compile_let(fc, let_binding, ctx)?;
        }
        StmtKind::Expr(expr) => {
            compile_expr(fc, expr, ctx)?;
            // Discard the result of expression statements.
            fc.builder.emit(Opcode::Pop);
        }
        StmtKind::Use(_) => {
            // Block-scoped imports were consumed by the resolve pass;
            // nothing executes.
        }
        StmtKind::Const(const_def) => {
            compile_block_const(fc, const_def, ctx)?;
        }
    }
    Ok(())
}

/// Compile a block-scoped `const`: content-address its value into a
/// standalone value object (deduplicated by hash like a module-level const)
/// and bind the name to that object's hash for the rest of the block. The
/// declaration emits no bytecode; a *reference* emits `LoadObject` (see the
/// `ExprKind::Name` arm), so the value's content hash flows into the
/// referencing function's identity — exactly the module-const behaviour.
fn compile_block_const(
    fc: &mut FunctionCompiler,
    const_def: &ConstDef,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // The checker already rejects non-literal consts. If a value still can't
    // be content-addressed, skip binding it — a reference then surfaces as an
    // undefined-name error, mirroring how module-level consts drop non-values.
    let Some(value) = crate::const_eval::literal_value(&const_def.value) else {
        return Ok(());
    };
    let object = crate::object::value_object(&value).map_err(|_| {
        CompileError::new(
            CompileErrorKind::Internal {
                message: "const value could not be content-addressed",
            },
            (const_def.value.span.start, const_def.value.span.end),
        )
    })?;
    let hash = object.hash();
    ctx.const_objects.insert(hash, object);
    fc.block_consts.insert(Arc::clone(&const_def.name), hash);
    Ok(())
}

/// Compile a let binding.
pub(super) fn compile_let(
    fc: &mut FunctionCompiler,
    binding: &LetBinding,
    ctx: &mut ModuleContext,
) -> Result<(), CompileError> {
    // Compile the initializer.
    compile_expr(fc, &binding.init, ctx)?;

    // Allocate a local slot and store the value.
    let slot = fc.alloc_local_with_name(binding.id, &binding.name)?;
    fc.builder.emit_u16(Opcode::StoreLocal, slot);

    Ok(())
}
