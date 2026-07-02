//! Intrinsic function compilation.
//!
//! This module handles compilation of built-in intrinsic functions that
//! are compiled directly to VM opcodes rather than function calls.
//!
//! Intrinsics are defined declaratively in [`INTRINSICS`] and compiled
//! via table lookup rather than explicit match arms.

use crate::ast::{Expr, QualifiedName};
use crate::bytecode::Opcode;

use super::error::CompileError;
use super::{compile_expr, FunctionCompiler, ModuleContext};

/// How to emit bytecode for an intrinsic.
#[derive(Debug, Clone, Copy)]
enum EmitStrategy {
    /// Emit a single opcode.
    Opcode(Opcode),
    /// Call a builder helper method (identified by name).
    Helper(Helper),
}

/// Named helper methods on `BytecodeBuilder`.
#[derive(Debug, Clone, Copy)]
enum Helper {
    ListGet,
    ListHead,
    ListTail,
    ListConcat,
    ListAppend,
    StringLength,
    StringConcat,
    StringContains,
    StringSplit,
    StringJoin,
    StringTrim,
    ToString,
    ParseNumber,
    ParseBool,
    MakeEmptySet,
    SetInsert,
    SetRemove,
    SetContains,
    SetLength,
    SetUnion,
    SetIntersection,
    SetDifference,
    SetToList,
    OptionUnwrapOr,
    EnumIs(u16),
    EnumTag,
    EnumPayload,
}

/// An intrinsic function descriptor.
struct Intrinsic {
    /// Module path segments (e.g., `["core", "math"]`).
    path: &'static [&'static str],
    /// Function name (e.g., `sqrt`).
    name: &'static str,
    /// Number of arguments.
    arity: u8,
    /// How to emit bytecode.
    emit: EmitStrategy,
}

/// Table of all intrinsic functions.
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
static INTRINSICS: &[Intrinsic] = &[
    // ─────────────────────────────────────────────────────────────────────
    // core.math - Math intrinsics
    // ─────────────────────────────────────────────────────────────────────
    Intrinsic {
        path: &["core", "math"],
        name: "sqrt",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Sqrt),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "abs",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Abs),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "floor",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Floor),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "ceil",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Ceil),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "round",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Round),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "trunc",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Trunc),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "sin",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Sin),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "cos",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Cos),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "tan",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Tan),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "ln",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Ln),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "exp",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Exp),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "pow",
        arity: 2,
        emit: EmitStrategy::Opcode(Opcode::Pow),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "min",
        arity: 2,
        emit: EmitStrategy::Opcode(Opcode::Min),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "max",
        arity: 2,
        emit: EmitStrategy::Opcode(Opcode::Max),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "asin",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Asin),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "acos",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Acos),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "atan",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Atan),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "atan2",
        arity: 2,
        emit: EmitStrategy::Opcode(Opcode::Atan2),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "log10",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Log10),
    },
    Intrinsic {
        path: &["core", "math"],
        name: "log2",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::Log2),
    },
    // ─────────────────────────────────────────────────────────────────────
    // core.list - List operations
    // ─────────────────────────────────────────────────────────────────────
    Intrinsic {
        path: &["core", "list"],
        name: "length",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::ListLength),
    },
    Intrinsic {
        path: &["core", "list"],
        name: "get",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::ListGet),
    },
    Intrinsic {
        path: &["core", "list"],
        name: "head",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::ListHead),
    },
    Intrinsic {
        path: &["core", "list"],
        name: "tail",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::ListTail),
    },
    Intrinsic {
        path: &["core", "list"],
        name: "concat",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::ListConcat),
    },
    Intrinsic {
        path: &["core", "list"],
        name: "append",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::ListAppend),
    },
    Intrinsic {
        path: &["core", "list"],
        name: "is_empty",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::ListIsEmpty),
    },
    Intrinsic {
        path: &["core", "list"],
        name: "first",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::ListHead),
    },
    Intrinsic {
        path: &["core", "list"],
        name: "last",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::ListLast),
    },
    Intrinsic {
        path: &["core", "list"],
        name: "reverse",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::ListReverse),
    },
    Intrinsic {
        path: &["core", "list"],
        name: "sort",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::ListSort),
    },
    Intrinsic {
        path: &["core", "list"],
        name: "slice",
        arity: 3,
        emit: EmitStrategy::Opcode(Opcode::ListSlice),
    },
    // ─────────────────────────────────────────────────────────────────────
    // core.string - String operations
    // ─────────────────────────────────────────────────────────────────────
    Intrinsic {
        path: &["core", "string"],
        name: "length",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::StringLength),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "concat",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::StringConcat),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "contains",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::StringContains),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "split",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::StringSplit),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "join",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::StringJoin),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "trim",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::StringTrim),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "slice",
        arity: 3,
        emit: EmitStrategy::Opcode(Opcode::StringSlice),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "chars",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::StringChars),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "replace",
        arity: 3,
        emit: EmitStrategy::Opcode(Opcode::StringReplace),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "starts_with",
        arity: 2,
        emit: EmitStrategy::Opcode(Opcode::StringStartsWith),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "ends_with",
        arity: 2,
        emit: EmitStrategy::Opcode(Opcode::StringEndsWith),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "to_upper",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::StringToUpper),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "to_lower",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::StringToLower),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "index_of",
        arity: 2,
        emit: EmitStrategy::Opcode(Opcode::StringIndexOf),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "repeat",
        arity: 2,
        emit: EmitStrategy::Opcode(Opcode::StringRepeat),
    },
    Intrinsic {
        path: &["core", "string"],
        name: "reverse",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::StringReverse),
    },
    // ─────────────────────────────────────────────────────────────────────
    // core.convert - Type conversion
    // ─────────────────────────────────────────────────────────────────────
    Intrinsic {
        path: &["core", "convert"],
        name: "to_string",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::ToString),
    },
    Intrinsic {
        path: &["core", "convert"],
        name: "parse_number",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::ParseNumber),
    },
    Intrinsic {
        path: &["core", "convert"],
        name: "parse_bool",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::ParseBool),
    },
    // ─────────────────────────────────────────────────────────────────────
    // core.map - Map operations
    // ─────────────────────────────────────────────────────────────────────
    Intrinsic {
        path: &["core", "map"],
        name: "empty",
        arity: 0,
        emit: EmitStrategy::Opcode(Opcode::MakeEmptyMap),
    },
    Intrinsic {
        path: &["core", "map"],
        name: "get",
        arity: 2,
        emit: EmitStrategy::Opcode(Opcode::MapGet),
    },
    Intrinsic {
        path: &["core", "map"],
        name: "insert",
        arity: 3,
        emit: EmitStrategy::Opcode(Opcode::MapInsert),
    },
    Intrinsic {
        path: &["core", "map"],
        name: "remove",
        arity: 2,
        emit: EmitStrategy::Opcode(Opcode::MapRemove),
    },
    Intrinsic {
        path: &["core", "map"],
        name: "contains",
        arity: 2,
        emit: EmitStrategy::Opcode(Opcode::MapContains),
    },
    Intrinsic {
        path: &["core", "map"],
        name: "length",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::MapLength),
    },
    Intrinsic {
        path: &["core", "map"],
        name: "keys",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::MapKeys),
    },
    Intrinsic {
        path: &["core", "map"],
        name: "values",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::MapValues),
    },
    // ─────────────────────────────────────────────────────────────────────
    // core.set - Set operations
    // ─────────────────────────────────────────────────────────────────────
    Intrinsic {
        path: &["core", "set"],
        name: "empty",
        arity: 0,
        emit: EmitStrategy::Helper(Helper::MakeEmptySet),
    },
    Intrinsic {
        path: &["core", "set"],
        name: "insert",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::SetInsert),
    },
    Intrinsic {
        path: &["core", "set"],
        name: "remove",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::SetRemove),
    },
    Intrinsic {
        path: &["core", "set"],
        name: "contains",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::SetContains),
    },
    Intrinsic {
        path: &["core", "set"],
        name: "length",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::SetLength),
    },
    Intrinsic {
        path: &["core", "set"],
        name: "union",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::SetUnion),
    },
    Intrinsic {
        path: &["core", "set"],
        name: "intersection",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::SetIntersection),
    },
    Intrinsic {
        path: &["core", "set"],
        name: "difference",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::SetDifference),
    },
    Intrinsic {
        path: &["core", "set"],
        name: "to_list",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::SetToList),
    },
    // ─────────────────────────────────────────────────────────────────────
    // core.option - Option operations
    // ─────────────────────────────────────────────────────────────────────
    Intrinsic {
        path: &["core", "option"],
        name: "unwrap_or",
        arity: 2,
        emit: EmitStrategy::Helper(Helper::OptionUnwrapOr),
    },
    Intrinsic {
        path: &["core", "option"],
        name: "is_some",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::EnumIs(1)),
    }, // Some has tag 1
    Intrinsic {
        path: &["core", "option"],
        name: "is_none",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::EnumIs(0)),
    }, // None has tag 0
    // ─────────────────────────────────────────────────────────────────────
    // core.result - Result operations
    // ─────────────────────────────────────────────────────────────────────
    Intrinsic {
        path: &["core", "result"],
        name: "is_ok",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::EnumIs(0)),
    }, // Ok has tag 0
    Intrinsic {
        path: &["core", "result"],
        name: "is_err",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::EnumIs(1)),
    }, // Err has tag 1
    // ─────────────────────────────────────────────────────────────────────
    // core.enum - Enum operations (general)
    // ─────────────────────────────────────────────────────────────────────
    Intrinsic {
        path: &["core", "enum"],
        name: "tag",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::EnumTag),
    },
    Intrinsic {
        path: &["core", "enum"],
        name: "payload",
        arity: 1,
        emit: EmitStrategy::Helper(Helper::EnumPayload),
    },
    // ─────────────────────────────────────────────────────────────────────
    // core.protocol - Binary protocol operations
    // ─────────────────────────────────────────────────────────────────────
    Intrinsic {
        path: &["core", "protocol"],
        name: "serialize_value",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::SerializeValue),
    },
    Intrinsic {
        path: &["core", "protocol"],
        name: "deserialize_value",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::DeserializeValue),
    },
    Intrinsic {
        path: &["core", "protocol"],
        name: "closure_hash",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::ClosureHash),
    },
    Intrinsic {
        path: &["core", "protocol"],
        name: "closure_captures",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::ClosureCaptures),
    },
    Intrinsic {
        path: &["core", "protocol"],
        name: "handler_methods",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::HandlerMethods),
    },
    Intrinsic {
        path: &["core", "protocol"],
        name: "hex_to_bytes",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::HexToBytes),
    },
    Intrinsic {
        path: &["core", "protocol"],
        name: "bytes_to_hex",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::BytesToHex),
    },
    // ─────────────────────────────────────────────────────────────────────
    // core.bytes - Bytes operations
    // ─────────────────────────────────────────────────────────────────────
    Intrinsic {
        path: &["core", "bytes"],
        name: "from",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::BytesFrom),
    },
    Intrinsic {
        path: &["core", "bytes"],
        name: "to_list",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::BytesToList),
    },
    Intrinsic {
        path: &["core", "bytes"],
        name: "length",
        arity: 1,
        emit: EmitStrategy::Opcode(Opcode::BytesLength),
    },
    Intrinsic {
        path: &["core", "bytes"],
        name: "get",
        arity: 2,
        emit: EmitStrategy::Opcode(Opcode::BytesGet),
    },
    Intrinsic {
        path: &["core", "bytes"],
        name: "slice",
        arity: 3,
        emit: EmitStrategy::Opcode(Opcode::BytesSlice),
    },
    Intrinsic {
        path: &["core", "bytes"],
        name: "concat",
        arity: 2,
        emit: EmitStrategy::Opcode(Opcode::BytesConcat),
    },
];

/// Check if a function name is an intrinsic and compile it if so.
///
/// Returns `Some(())` if the function was compiled as an intrinsic,
/// `None` if it should be handled as a regular function call.
pub(super) fn try_compile_intrinsic(
    fc: &mut FunctionCompiler,
    qualified_name: &QualifiedName,
    args: &[Expr],
    ctx: &mut ModuleContext,
) -> Result<Option<()>, CompileError> {
    // Convert path to slice for matching
    let path: Vec<&str> = qualified_name.path.iter().map(AsRef::as_ref).collect();
    let name = qualified_name.name.as_ref();

    // Look up intrinsic in the table
    let Some(intrinsic) = INTRINSICS
        .iter()
        .find(|i| i.path == path.as_slice() && i.name == name && i.arity == args.len() as u8)
    else {
        return Ok(None);
    };

    // Compile arguments
    for arg in args {
        compile_expr(fc, arg, ctx)?;
    }

    // Emit bytecode
    emit_intrinsic(fc, intrinsic.emit);

    Ok(Some(()))
}

/// Get intrinsic function names for a given module path.
///
/// Returns a list of (name, arity) pairs for all intrinsics in the module.
/// Used for REPL module introspection.
pub fn get_intrinsics_for_module(module_path: &[&str]) -> Vec<(&'static str, u8)> {
    INTRINSICS
        .iter()
        .filter(|i| i.path == module_path)
        .map(|i| (i.name, i.arity))
        .collect()
}

/// Emit bytecode for an intrinsic based on its emit strategy.
fn emit_intrinsic(fc: &mut FunctionCompiler, strategy: EmitStrategy) {
    match strategy {
        EmitStrategy::Opcode(opcode) => {
            fc.builder.emit(opcode);
        }
        EmitStrategy::Helper(helper) => match helper {
            Helper::ListGet => fc.builder.emit_list_get(),
            Helper::ListHead => fc.builder.emit_list_head(),
            Helper::ListTail => fc.builder.emit_list_tail(),
            Helper::ListConcat => fc.builder.emit_list_concat(),
            Helper::ListAppend => fc.builder.emit_list_append(),
            Helper::StringLength => fc.builder.emit_string_length(),
            Helper::StringConcat => fc.builder.emit_string_concat(),
            Helper::StringContains => fc.builder.emit_string_contains(),
            Helper::StringSplit => fc.builder.emit_string_split(),
            Helper::StringJoin => fc.builder.emit_string_join(),
            Helper::StringTrim => fc.builder.emit_string_trim(),
            Helper::ToString => fc.builder.emit_to_string(),
            Helper::ParseNumber => fc.builder.emit_parse_number(),
            Helper::ParseBool => fc.builder.emit_parse_bool(),
            Helper::MakeEmptySet => fc.builder.emit_make_empty_set(),
            Helper::SetInsert => fc.builder.emit_set_insert(),
            Helper::SetRemove => fc.builder.emit_set_remove(),
            Helper::SetContains => fc.builder.emit_set_contains(),
            Helper::SetLength => fc.builder.emit_set_length(),
            Helper::SetUnion => fc.builder.emit_set_union(),
            Helper::SetIntersection => fc.builder.emit_set_intersection(),
            Helper::SetDifference => fc.builder.emit_set_difference(),
            Helper::SetToList => fc.builder.emit_set_to_list(),
            Helper::OptionUnwrapOr => fc.builder.emit_option_unwrap_or(),
            Helper::EnumIs(tag) => fc.builder.emit_enum_is(tag),
            Helper::EnumTag => fc.builder.emit_enum_tag(),
            Helper::EnumPayload => fc.builder.emit_enum_payload(),
        },
    }
}
