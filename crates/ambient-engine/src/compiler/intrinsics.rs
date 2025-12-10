//! Intrinsic function compilation.
//!
//! This module handles compilation of built-in intrinsic functions that
//! are compiled directly to VM opcodes rather than function calls.

use crate::ast::{Expr, QualifiedName};
use crate::bytecode::Opcode;

use super::error::CompileError;
use super::{compile_expr, FunctionCompiler, ModuleContext};

/// Check if a function name is an intrinsic and compile it if so.
///
/// Returns `Some(())` if the function was compiled as an intrinsic,
/// `None` if it should be handled as a regular function call.
///
/// Intrinsics must be called with their full qualified path:
/// - `core.math.sqrt`, `core.math.abs`, etc.
/// - `core.list.length`, `core.list.head`, etc.
/// - `core.string.length`, `core.string.split`, etc.
/// - `core.map.empty`, `core.map.get`, etc.
/// - `core.set.empty`, `core.set.insert`, etc.
/// - `core.option.unwrap_or`, `core.option.is_some`, etc.
/// - `core.result.is_ok`, `core.result.is_err`, etc.
/// - `core.convert.to_string`, `core.convert.parse_number`, etc.
#[allow(clippy::too_many_lines)]
pub(super) fn try_compile_intrinsic(
    fc: &mut FunctionCompiler,
    qualified_name: &QualifiedName,
    args: &[Expr],
    ctx: &mut ModuleContext,
) -> Result<Option<()>, CompileError> {
    // Convert path to slice for matching
    let path: Vec<&str> = qualified_name.path.iter().map(AsRef::as_ref).collect();
    let name = qualified_name.name.as_ref();

    match (path.as_slice(), name) {
        // ─────────────────────────────────────────────────────────────────────
        // core.math - Math intrinsics
        // ─────────────────────────────────────────────────────────────────────
        (["core", "math"], "sqrt") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Sqrt);
            return Ok(Some(()));
        }
        (["core", "math"], "abs") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Abs);
            return Ok(Some(()));
        }
        (["core", "math"], "floor") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Floor);
            return Ok(Some(()));
        }
        (["core", "math"], "ceil") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Ceil);
            return Ok(Some(()));
        }
        (["core", "math"], "round") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Round);
            return Ok(Some(()));
        }
        (["core", "math"], "trunc") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Trunc);
            return Ok(Some(()));
        }
        (["core", "math"], "sin") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Sin);
            return Ok(Some(()));
        }
        (["core", "math"], "cos") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Cos);
            return Ok(Some(()));
        }
        (["core", "math"], "tan") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Tan);
            return Ok(Some(()));
        }
        (["core", "math"], "ln") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Ln);
            return Ok(Some(()));
        }
        (["core", "math"], "exp") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Exp);
            return Ok(Some(()));
        }
        (["core", "math"], "pow") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit(Opcode::Pow);
            return Ok(Some(()));
        }
        (["core", "math"], "min") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit(Opcode::Min);
            return Ok(Some(()));
        }
        (["core", "math"], "max") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit(Opcode::Max);
            return Ok(Some(()));
        }
        (["core", "math"], "asin") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Asin);
            return Ok(Some(()));
        }
        (["core", "math"], "acos") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Acos);
            return Ok(Some(()));
        }
        (["core", "math"], "atan") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Atan);
            return Ok(Some(()));
        }
        (["core", "math"], "atan2") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit(Opcode::Atan2);
            return Ok(Some(()));
        }
        (["core", "math"], "log10") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Log10);
            return Ok(Some(()));
        }
        (["core", "math"], "log2") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::Log2);
            return Ok(Some(()));
        }

        // ─────────────────────────────────────────────────────────────────────
        // core.list - List operations
        // ─────────────────────────────────────────────────────────────────────
        (["core", "list"], "length") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::ListLength);
            return Ok(Some(()));
        }
        (["core", "list"], "get") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit_list_get();
            // list_get returns Unit for out of bounds - wrap in Option
            // For now, just return the raw value (Unit or element)
            return Ok(Some(()));
        }
        (["core", "list"], "head") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_list_head();
            return Ok(Some(()));
        }
        (["core", "list"], "tail") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_list_tail();
            return Ok(Some(()));
        }
        (["core", "list"], "concat") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit_list_concat();
            return Ok(Some(()));
        }
        (["core", "list"], "append") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit_list_append();
            return Ok(Some(()));
        }
        (["core", "list"], "is_empty") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::ListIsEmpty);
            return Ok(Some(()));
        }
        (["core", "list"], "reverse") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::ListReverse);
            return Ok(Some(()));
        }
        (["core", "list"], "sort") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::ListSort);
            return Ok(Some(()));
        }
        (["core", "list"], "slice") if args.len() == 3 => {
            compile_expr(fc, &args[0], ctx)?; // list
            compile_expr(fc, &args[1], ctx)?; // start
            compile_expr(fc, &args[2], ctx)?; // end
            fc.builder.emit(Opcode::ListSlice);
            return Ok(Some(()));
        }

        // ─────────────────────────────────────────────────────────────────────
        // core.string - String operations
        // ─────────────────────────────────────────────────────────────────────
        (["core", "string"], "length") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_string_length();
            return Ok(Some(()));
        }
        (["core", "string"], "concat") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit_string_concat();
            return Ok(Some(()));
        }
        (["core", "string"], "contains") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit_string_contains();
            return Ok(Some(()));
        }
        (["core", "string"], "split") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit_string_split();
            return Ok(Some(()));
        }
        (["core", "string"], "join") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?; // list
            compile_expr(fc, &args[1], ctx)?; // delimiter
            fc.builder.emit_string_join();
            return Ok(Some(()));
        }
        (["core", "string"], "trim") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_string_trim();
            return Ok(Some(()));
        }
        (["core", "string"], "slice") if args.len() == 3 => {
            compile_expr(fc, &args[0], ctx)?; // string
            compile_expr(fc, &args[1], ctx)?; // start
            compile_expr(fc, &args[2], ctx)?; // end
            fc.builder.emit(Opcode::StringSlice);
            return Ok(Some(()));
        }
        (["core", "string"], "chars") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::StringChars);
            return Ok(Some(()));
        }
        (["core", "string"], "replace") if args.len() == 3 => {
            compile_expr(fc, &args[0], ctx)?; // string
            compile_expr(fc, &args[1], ctx)?; // pattern
            compile_expr(fc, &args[2], ctx)?; // replacement
            fc.builder.emit(Opcode::StringReplace);
            return Ok(Some(()));
        }
        (["core", "string"], "starts_with") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit(Opcode::StringStartsWith);
            return Ok(Some(()));
        }
        (["core", "string"], "ends_with") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit(Opcode::StringEndsWith);
            return Ok(Some(()));
        }
        (["core", "string"], "to_upper") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::StringToUpper);
            return Ok(Some(()));
        }
        (["core", "string"], "to_lower") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::StringToLower);
            return Ok(Some(()));
        }
        (["core", "string"], "index_of") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit(Opcode::StringIndexOf);
            return Ok(Some(()));
        }
        (["core", "string"], "repeat") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit(Opcode::StringRepeat);
            return Ok(Some(()));
        }
        (["core", "string"], "reverse") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::StringReverse);
            return Ok(Some(()));
        }

        // ─────────────────────────────────────────────────────────────────────
        // core.convert - Type conversion
        // ─────────────────────────────────────────────────────────────────────
        (["core", "convert"], "to_string") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_to_string();
            return Ok(Some(()));
        }
        (["core", "convert"], "parse_number") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_parse_number();
            return Ok(Some(()));
        }
        (["core", "convert"], "parse_bool") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_parse_bool();
            return Ok(Some(()));
        }

        // ─────────────────────────────────────────────────────────────────────
        // core.map - Map operations
        // ─────────────────────────────────────────────────────────────────────
        (["core", "map"], "empty") if args.is_empty() => {
            fc.builder.emit(Opcode::MakeEmptyMap);
            return Ok(Some(()));
        }
        (["core", "map"], "get") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit(Opcode::MapGet);
            return Ok(Some(()));
        }
        (["core", "map"], "insert") if args.len() == 3 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            compile_expr(fc, &args[2], ctx)?;
            fc.builder.emit(Opcode::MapInsert);
            return Ok(Some(()));
        }
        (["core", "map"], "remove") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit(Opcode::MapRemove);
            return Ok(Some(()));
        }
        (["core", "map"], "contains") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit(Opcode::MapContains);
            return Ok(Some(()));
        }
        (["core", "map"], "length") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::MapLength);
            return Ok(Some(()));
        }
        (["core", "map"], "keys") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::MapKeys);
            return Ok(Some(()));
        }
        (["core", "map"], "values") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit(Opcode::MapValues);
            return Ok(Some(()));
        }

        // ─────────────────────────────────────────────────────────────────────
        // core.set - Set operations
        // ─────────────────────────────────────────────────────────────────────
        (["core", "set"], "empty") if args.is_empty() => {
            fc.builder.emit_make_empty_set();
            return Ok(Some(()));
        }
        (["core", "set"], "insert") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit_set_insert();
            return Ok(Some(()));
        }
        (["core", "set"], "remove") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit_set_remove();
            return Ok(Some(()));
        }
        (["core", "set"], "contains") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit_set_contains();
            return Ok(Some(()));
        }
        (["core", "set"], "length") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_set_length();
            return Ok(Some(()));
        }
        (["core", "set"], "union") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit_set_union();
            return Ok(Some(()));
        }
        (["core", "set"], "intersection") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit_set_intersection();
            return Ok(Some(()));
        }
        (["core", "set"], "difference") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit_set_difference();
            return Ok(Some(()));
        }
        (["core", "set"], "to_list") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_set_to_list();
            return Ok(Some(()));
        }

        // ─────────────────────────────────────────────────────────────────────
        // core.option - Option operations
        // ─────────────────────────────────────────────────────────────────────
        (["core", "option"], "unwrap_or") if args.len() == 2 => {
            compile_expr(fc, &args[0], ctx)?;
            compile_expr(fc, &args[1], ctx)?;
            fc.builder.emit_option_unwrap_or();
            return Ok(Some(()));
        }
        (["core", "option"], "is_some") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_enum_is(1); // Some has tag 1
            return Ok(Some(()));
        }
        (["core", "option"], "is_none") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_enum_is(0); // None has tag 0
            return Ok(Some(()));
        }

        // ─────────────────────────────────────────────────────────────────────
        // core.result - Result operations
        // ─────────────────────────────────────────────────────────────────────
        (["core", "result"], "is_ok") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_enum_is(0); // Ok has tag 0
            return Ok(Some(()));
        }
        (["core", "result"], "is_err") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_enum_is(1); // Err has tag 1
            return Ok(Some(()));
        }

        // ─────────────────────────────────────────────────────────────────────
        // core.enum - Enum operations (general)
        // ─────────────────────────────────────────────────────────────────────
        (["core", "enum"], "tag") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_enum_tag();
            return Ok(Some(()));
        }
        (["core", "enum"], "payload") if args.len() == 1 => {
            compile_expr(fc, &args[0], ctx)?;
            fc.builder.emit_enum_payload();
            return Ok(Some(()));
        }

        _ => {}
    }

    Ok(None)
}
