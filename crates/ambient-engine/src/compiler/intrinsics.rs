//! The intrinsic function table: the single source of truth for every
//! `core::*` builtin that compiles to dedicated bytecode.
//!
//! Each entry declares the intrinsic's qualified path, its full type
//! signature, and how to emit its bytecode. The type checker
//! (`infer::intrinsics`) and the compiler both consult this table, so a
//! builtin cannot type-check without compiling or vice versa — previously
//! two hand-maintained lists drifted (e.g. `core::collections::List::first` compiled
//! but never type-checked).
//!
//! Intrinsics must be called with their full qualified path
//! (`core::primitives::Number::sqrt`, `core::collections::List::head`, ...). Signatures may be generic:
//! the signature builder receives a variable supply, and `vars.var(0)`
//! names the same fresh inference variable at every use within one call
//! site.

use crate::ast::{Expr, QualifiedName};
use crate::bytecode::Opcode;
use crate::types::{Type, TypeVarGen};

use super::error::CompileError;
use super::{FunctionCompiler, ModuleContext, compile_expr};

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
    EnumTag,
    EnumPayload,
}

/// A supply of fresh-but-shared inference variables for one signature
/// instantiation: `var(0)` returns the same variable every time within one
/// `SigVars`, and a different one per call site.
pub struct SigVars<'a> {
    r#gen: &'a mut TypeVarGen,
    vars: Vec<Type>,
}

impl<'a> SigVars<'a> {
    pub(crate) fn new(r#gen: &'a mut TypeVarGen) -> Self {
        Self {
            r#gen,
            vars: Vec::new(),
        }
    }

    /// The `idx`-th signature variable (allocated on first use).
    pub fn var(&mut self, idx: usize) -> Type {
        while self.vars.len() <= idx {
            self.vars.push(self.r#gen.fresh());
        }
        self.vars[idx].clone()
    }
}

/// An intrinsic's instantiated type signature.
pub struct Signature {
    pub params: Vec<Type>,
    pub ret: Type,
}

/// Builds an intrinsic's signature against a fresh variable supply.
type SigFn = fn(&mut SigVars) -> Signature;

/// An intrinsic function descriptor.
pub(crate) struct Intrinsic {
    /// Module path segments (e.g., `["core", "primitives", "Number"]`).
    path: &'static [&'static str],
    /// Function name (e.g., `sqrt`).
    name: &'static str,
    /// Number of arguments (must equal the signature's parameter count —
    /// pinned by a test).
    arity: u8,
    /// How to emit bytecode.
    emit: EmitStrategy,
    /// The declared type signature.
    sig: SigFn,
}

impl Intrinsic {
    /// Instantiate this intrinsic's signature with fresh variables.
    pub(crate) fn signature(&self, r#gen: &mut TypeVarGen) -> Signature {
        (self.sig)(&mut SigVars::new(r#gen))
    }
}

/// Look up an intrinsic by qualified path and name, regardless of arity.
pub(crate) fn find(path: &[&str], name: &str) -> Option<&'static Intrinsic> {
    INTRINSICS.iter().find(|i| i.path == path && i.name == name)
}

// ─────────────────────────────────────────────────────────────────────────────
// Signature vocabulary
// ─────────────────────────────────────────────────────────────────────────────

fn sig(params: Vec<Type>, ret: Type) -> Signature {
    Signature { params, ret }
}

fn list(elem: Type) -> Type {
    Type::named("List", vec![elem])
}

fn set(elem: Type) -> Type {
    Type::named("Set", vec![elem])
}

fn map(key: Type, value: Type) -> Type {
    Type::named("Map", vec![key, value])
}

const fn intrinsic(
    path: &'static [&'static str],
    name: &'static str,
    arity: u8,
    emit: EmitStrategy,
    sig: SigFn,
) -> Intrinsic {
    Intrinsic {
        path,
        name,
        arity,
        emit,
        sig,
    }
}

// Shared signature shapes.
fn num1(_: &mut SigVars) -> Signature {
    sig(vec![Type::number()], Type::number())
}
fn num2(_: &mut SigVars) -> Signature {
    sig(vec![Type::number(), Type::number()], Type::number())
}
fn str_to_str(_: &mut SigVars) -> Signature {
    sig(vec![Type::string()], Type::string())
}
fn str2_to_bool(_: &mut SigVars) -> Signature {
    sig(vec![Type::string(), Type::string()], Type::bool())
}
fn list_to_list(v: &mut SigVars) -> Signature {
    sig(vec![list(v.var(0))], list(v.var(0)))
}
fn list_to_opt_elem(v: &mut SigVars) -> Signature {
    sig(vec![list(v.var(0))], Type::option(v.var(0)))
}
fn set2_to_set(v: &mut SigVars) -> Signature {
    sig(vec![set(v.var(0)), set(v.var(0))], set(v.var(0)))
}

/// Table of all intrinsic functions.
static INTRINSICS: &[Intrinsic] = &[
    // ─────────────────────────────────────────────────────────────────────
    // core::primitives::Number
    // ─────────────────────────────────────────────────────────────────────
    intrinsic(
        &["core", "primitives", "Number"],
        "sqrt",
        1,
        EmitStrategy::Opcode(Opcode::Sqrt),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "abs",
        1,
        EmitStrategy::Opcode(Opcode::Abs),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "floor",
        1,
        EmitStrategy::Opcode(Opcode::Floor),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "ceil",
        1,
        EmitStrategy::Opcode(Opcode::Ceil),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "round",
        1,
        EmitStrategy::Opcode(Opcode::Round),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "trunc",
        1,
        EmitStrategy::Opcode(Opcode::Trunc),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "sin",
        1,
        EmitStrategy::Opcode(Opcode::Sin),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "cos",
        1,
        EmitStrategy::Opcode(Opcode::Cos),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "tan",
        1,
        EmitStrategy::Opcode(Opcode::Tan),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "ln",
        1,
        EmitStrategy::Opcode(Opcode::Ln),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "exp",
        1,
        EmitStrategy::Opcode(Opcode::Exp),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "pow",
        2,
        EmitStrategy::Opcode(Opcode::Pow),
        num2,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "min",
        2,
        EmitStrategy::Opcode(Opcode::Min),
        num2,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "max",
        2,
        EmitStrategy::Opcode(Opcode::Max),
        num2,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "asin",
        1,
        EmitStrategy::Opcode(Opcode::Asin),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "acos",
        1,
        EmitStrategy::Opcode(Opcode::Acos),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "atan",
        1,
        EmitStrategy::Opcode(Opcode::Atan),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "atan2",
        2,
        EmitStrategy::Opcode(Opcode::Atan2),
        num2,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "log10",
        1,
        EmitStrategy::Opcode(Opcode::Log10),
        num1,
    ),
    intrinsic(
        &["core", "primitives", "Number"],
        "log2",
        1,
        EmitStrategy::Opcode(Opcode::Log2),
        num1,
    ),
    // ─────────────────────────────────────────────────────────────────────
    // core::collections::List
    // ─────────────────────────────────────────────────────────────────────
    intrinsic(
        &["core", "collections", "List"],
        "length",
        1,
        EmitStrategy::Opcode(Opcode::ListLength),
        |v| sig(vec![list(v.var(0))], Type::number()),
    ),
    intrinsic(
        &["core", "collections", "List"],
        "get",
        2,
        EmitStrategy::Helper(Helper::ListGet),
        |v| sig(vec![list(v.var(0)), Type::number()], Type::option(v.var(0))),
    ),
    intrinsic(
        &["core", "collections", "List"],
        "head",
        1,
        EmitStrategy::Helper(Helper::ListHead),
        list_to_opt_elem,
    ),
    intrinsic(
        &["core", "collections", "List"],
        "tail",
        1,
        EmitStrategy::Helper(Helper::ListTail),
        list_to_list,
    ),
    intrinsic(
        &["core", "collections", "List"],
        "concat",
        2,
        EmitStrategy::Helper(Helper::ListConcat),
        |v| sig(vec![list(v.var(0)), list(v.var(0))], list(v.var(0))),
    ),
    intrinsic(
        &["core", "collections", "List"],
        "append",
        2,
        EmitStrategy::Helper(Helper::ListAppend),
        |v| sig(vec![list(v.var(0)), v.var(0)], list(v.var(0))),
    ),
    intrinsic(
        &["core", "collections", "List"],
        "is_empty",
        1,
        EmitStrategy::Opcode(Opcode::ListIsEmpty),
        |v| sig(vec![list(v.var(0))], Type::bool()),
    ),
    intrinsic(
        &["core", "collections", "List"],
        "first",
        1,
        EmitStrategy::Helper(Helper::ListHead),
        list_to_opt_elem,
    ),
    intrinsic(
        &["core", "collections", "List"],
        "last",
        1,
        EmitStrategy::Opcode(Opcode::ListLast),
        list_to_opt_elem,
    ),
    intrinsic(
        &["core", "collections", "List"],
        "reverse",
        1,
        EmitStrategy::Opcode(Opcode::ListReverse),
        list_to_list,
    ),
    intrinsic(
        &["core", "collections", "List"],
        "sort",
        1,
        EmitStrategy::Opcode(Opcode::ListSort),
        list_to_list,
    ),
    intrinsic(
        &["core", "collections", "List"],
        "slice",
        3,
        EmitStrategy::Opcode(Opcode::ListSlice),
        |v| {
            sig(
                vec![list(v.var(0)), Type::number(), Type::number()],
                list(v.var(0)),
            )
        },
    ),
    // ─────────────────────────────────────────────────────────────────────
    // core::primitives::String
    // ─────────────────────────────────────────────────────────────────────
    intrinsic(
        &["core", "primitives", "String"],
        "length",
        1,
        EmitStrategy::Helper(Helper::StringLength),
        |_| sig(vec![Type::string()], Type::number()),
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "concat",
        2,
        EmitStrategy::Helper(Helper::StringConcat),
        |_| sig(vec![Type::string(), Type::string()], Type::string()),
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "contains",
        2,
        EmitStrategy::Helper(Helper::StringContains),
        str2_to_bool,
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "split",
        2,
        EmitStrategy::Helper(Helper::StringSplit),
        |_| sig(vec![Type::string(), Type::string()], list(Type::string())),
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "join",
        2,
        EmitStrategy::Helper(Helper::StringJoin),
        |_| sig(vec![list(Type::string()), Type::string()], Type::string()),
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "trim",
        1,
        EmitStrategy::Helper(Helper::StringTrim),
        str_to_str,
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "slice",
        3,
        EmitStrategy::Opcode(Opcode::StringSlice),
        |_| {
            sig(
                vec![Type::string(), Type::number(), Type::number()],
                Type::string(),
            )
        },
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "chars",
        1,
        EmitStrategy::Opcode(Opcode::StringChars),
        |_| sig(vec![Type::string()], list(Type::string())),
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "replace",
        3,
        EmitStrategy::Opcode(Opcode::StringReplace),
        |_| {
            sig(
                vec![Type::string(), Type::string(), Type::string()],
                Type::string(),
            )
        },
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "starts_with",
        2,
        EmitStrategy::Opcode(Opcode::StringStartsWith),
        str2_to_bool,
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "ends_with",
        2,
        EmitStrategy::Opcode(Opcode::StringEndsWith),
        str2_to_bool,
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "to_upper",
        1,
        EmitStrategy::Opcode(Opcode::StringToUpper),
        str_to_str,
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "to_lower",
        1,
        EmitStrategy::Opcode(Opcode::StringToLower),
        str_to_str,
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "index_of",
        2,
        EmitStrategy::Opcode(Opcode::StringIndexOf),
        |_| {
            sig(
                vec![Type::string(), Type::string()],
                Type::option(Type::number()),
            )
        },
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "repeat",
        2,
        EmitStrategy::Opcode(Opcode::StringRepeat),
        |_| sig(vec![Type::string(), Type::number()], Type::string()),
    ),
    intrinsic(
        &["core", "primitives", "String"],
        "reverse",
        1,
        EmitStrategy::Opcode(Opcode::StringReverse),
        str_to_str,
    ),
    // ─────────────────────────────────────────────────────────────────────
    // core::convert
    // ─────────────────────────────────────────────────────────────────────
    intrinsic(
        &["core", "convert"],
        "to_string",
        1,
        EmitStrategy::Helper(Helper::ToString),
        |v| sig(vec![v.var(0)], Type::string()),
    ),
    intrinsic(
        &["core", "convert"],
        "parse_number",
        1,
        EmitStrategy::Helper(Helper::ParseNumber),
        |_| sig(vec![Type::string()], Type::option(Type::number())),
    ),
    intrinsic(
        &["core", "convert"],
        "parse_bool",
        1,
        EmitStrategy::Helper(Helper::ParseBool),
        |_| sig(vec![Type::string()], Type::option(Type::bool())),
    ),
    // ─────────────────────────────────────────────────────────────────────
    // core::collections::map
    // ─────────────────────────────────────────────────────────────────────
    intrinsic(
        &["core", "collections", "map"],
        "empty",
        0,
        EmitStrategy::Opcode(Opcode::MakeEmptyMap),
        |v| sig(vec![], map(v.var(0), v.var(1))),
    ),
    intrinsic(
        &["core", "collections", "map"],
        "get",
        2,
        EmitStrategy::Opcode(Opcode::MapGet),
        |v| {
            sig(
                vec![map(v.var(0), v.var(1)), v.var(0)],
                Type::option(v.var(1)),
            )
        },
    ),
    intrinsic(
        &["core", "collections", "map"],
        "insert",
        3,
        EmitStrategy::Opcode(Opcode::MapInsert),
        |v| {
            sig(
                vec![map(v.var(0), v.var(1)), v.var(0), v.var(1)],
                map(v.var(0), v.var(1)),
            )
        },
    ),
    intrinsic(
        &["core", "collections", "map"],
        "remove",
        2,
        EmitStrategy::Opcode(Opcode::MapRemove),
        |v| {
            sig(
                vec![map(v.var(0), v.var(1)), v.var(0)],
                map(v.var(0), v.var(1)),
            )
        },
    ),
    intrinsic(
        &["core", "collections", "map"],
        "contains",
        2,
        EmitStrategy::Opcode(Opcode::MapContains),
        |v| sig(vec![map(v.var(0), v.var(1)), v.var(0)], Type::bool()),
    ),
    intrinsic(
        &["core", "collections", "map"],
        "length",
        1,
        EmitStrategy::Opcode(Opcode::MapLength),
        |v| sig(vec![map(v.var(0), v.var(1))], Type::number()),
    ),
    intrinsic(
        &["core", "collections", "map"],
        "keys",
        1,
        EmitStrategy::Opcode(Opcode::MapKeys),
        |v| sig(vec![map(v.var(0), v.var(1))], list(v.var(0))),
    ),
    intrinsic(
        &["core", "collections", "map"],
        "values",
        1,
        EmitStrategy::Opcode(Opcode::MapValues),
        |v| sig(vec![map(v.var(0), v.var(1))], list(v.var(1))),
    ),
    // ─────────────────────────────────────────────────────────────────────
    // core::collections::set
    // ─────────────────────────────────────────────────────────────────────
    intrinsic(
        &["core", "collections", "set"],
        "empty",
        0,
        EmitStrategy::Helper(Helper::MakeEmptySet),
        |v| sig(vec![], set(v.var(0))),
    ),
    intrinsic(
        &["core", "collections", "set"],
        "insert",
        2,
        EmitStrategy::Helper(Helper::SetInsert),
        |v| sig(vec![set(v.var(0)), v.var(0)], set(v.var(0))),
    ),
    intrinsic(
        &["core", "collections", "set"],
        "remove",
        2,
        EmitStrategy::Helper(Helper::SetRemove),
        |v| sig(vec![set(v.var(0)), v.var(0)], set(v.var(0))),
    ),
    intrinsic(
        &["core", "collections", "set"],
        "contains",
        2,
        EmitStrategy::Helper(Helper::SetContains),
        |v| sig(vec![set(v.var(0)), v.var(0)], Type::bool()),
    ),
    intrinsic(
        &["core", "collections", "set"],
        "length",
        1,
        EmitStrategy::Helper(Helper::SetLength),
        |v| sig(vec![set(v.var(0))], Type::number()),
    ),
    intrinsic(
        &["core", "collections", "set"],
        "union",
        2,
        EmitStrategy::Helper(Helper::SetUnion),
        set2_to_set,
    ),
    intrinsic(
        &["core", "collections", "set"],
        "intersection",
        2,
        EmitStrategy::Helper(Helper::SetIntersection),
        set2_to_set,
    ),
    intrinsic(
        &["core", "collections", "set"],
        "difference",
        2,
        EmitStrategy::Helper(Helper::SetDifference),
        set2_to_set,
    ),
    intrinsic(
        &["core", "collections", "set"],
        "to_list",
        1,
        EmitStrategy::Helper(Helper::SetToList),
        |v| sig(vec![set(v.var(0))], list(v.var(0))),
    ),
    // ─────────────────────────────────────────────────────────────────────
    // core::reflect - Enum reflection (general)
    // ─────────────────────────────────────────────────────────────────────
    intrinsic(
        &["core", "reflect"],
        "tag",
        1,
        EmitStrategy::Helper(Helper::EnumTag),
        |v| sig(vec![v.var(0)], Type::number()),
    ),
    // NOTE: `payload` materializes an arbitrary type from an enum value —
    // an unchecked escape hatch (the result unifies with anything).
    intrinsic(
        &["core", "reflect"],
        "payload",
        1,
        EmitStrategy::Helper(Helper::EnumPayload),
        |v| sig(vec![v.var(0)], v.var(1)),
    ),
    // ─────────────────────────────────────────────────────────────────────
    // core::protocol - Binary protocol operations
    // ─────────────────────────────────────────────────────────────────────
    intrinsic(
        &["core", "protocol"],
        "serialize_value",
        1,
        EmitStrategy::Opcode(Opcode::SerializeValue),
        |v| sig(vec![v.var(0)], Type::binary()),
    ),
    intrinsic(
        &["core", "protocol"],
        "deserialize_value",
        1,
        EmitStrategy::Opcode(Opcode::DeserializeValue),
        |v| sig(vec![Type::binary()], Type::option(v.var(0))),
    ),
    intrinsic(
        &["core", "protocol"],
        "closure_hash",
        1,
        EmitStrategy::Opcode(Opcode::ClosureHash),
        |v| sig(vec![v.var(0)], Type::string()),
    ),
    intrinsic(
        &["core", "protocol"],
        "closure_captures",
        1,
        EmitStrategy::Opcode(Opcode::ClosureCaptures),
        |v| sig(vec![v.var(0)], Type::binary()),
    ),
    intrinsic(
        &["core", "protocol"],
        "handler_methods",
        1,
        EmitStrategy::Opcode(Opcode::HandlerMethods),
        |v| sig(vec![v.var(0)], list(Type::string())),
    ),
    intrinsic(
        &["core", "protocol"],
        "hex_to_binary",
        1,
        EmitStrategy::Opcode(Opcode::HexToBinary),
        |_| sig(vec![Type::string()], Type::option(Type::binary())),
    ),
    intrinsic(
        &["core", "protocol"],
        "binary_to_hex",
        1,
        EmitStrategy::Opcode(Opcode::BinaryToHex),
        |_| sig(vec![Type::binary()], Type::string()),
    ),
    // ─────────────────────────────────────────────────────────────────────
    // core::primitives::Binary
    // ─────────────────────────────────────────────────────────────────────
    intrinsic(
        &["core", "primitives", "Binary"],
        "from",
        1,
        EmitStrategy::Opcode(Opcode::BinaryFrom),
        |_| sig(vec![list(Type::number())], Type::binary()),
    ),
    intrinsic(
        &["core", "primitives", "Binary"],
        "to_list",
        1,
        EmitStrategy::Opcode(Opcode::BinaryToList),
        |_| sig(vec![Type::binary()], list(Type::number())),
    ),
    intrinsic(
        &["core", "primitives", "Binary"],
        "length",
        1,
        EmitStrategy::Opcode(Opcode::BinaryLength),
        |_| sig(vec![Type::binary()], Type::number()),
    ),
    intrinsic(
        &["core", "primitives", "Binary"],
        "get",
        2,
        EmitStrategy::Opcode(Opcode::BinaryGet),
        |_| {
            sig(
                vec![Type::binary(), Type::number()],
                Type::option(Type::number()),
            )
        },
    ),
    intrinsic(
        &["core", "primitives", "Binary"],
        "slice",
        3,
        EmitStrategy::Opcode(Opcode::BinarySlice),
        |_| {
            sig(
                vec![Type::binary(), Type::number(), Type::number()],
                Type::binary(),
            )
        },
    ),
    intrinsic(
        &["core", "primitives", "Binary"],
        "concat",
        2,
        EmitStrategy::Opcode(Opcode::BinaryConcat),
        |_| sig(vec![Type::binary(), Type::binary()], Type::binary()),
    ),
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
    // Match on the canonical target, so aliased and imported spellings hit
    // the intrinsic exactly like the fully-qualified one (mirrors
    // `Infer::try_infer_intrinsic`).
    let path = qualified_name.resolved_module_segments();
    let name = qualified_name.resolved_name();

    // The type checker already verified arity against this same table, so
    // a mismatch here means the expression bypassed checking.
    let Some(intrinsic) = find(&path, name).filter(|i| i.arity as usize == args.len()) else {
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
#[must_use]
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
            Helper::EnumTag => fc.builder.emit_enum_tag(),
            Helper::EnumPayload => fc.builder.emit_enum_payload(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry's declared arity must equal its signature's parameter
    /// count — the checker trusts `arity` for error messages and the
    /// compiler for lookup.
    #[test]
    fn arity_matches_signature() {
        let mut r#gen = TypeVarGen::new();
        for intrinsic in INTRINSICS {
            let signature = intrinsic.signature(&mut r#gen);
            assert_eq!(
                intrinsic.arity as usize,
                signature.params.len(),
                "arity/signature mismatch for {}::{}",
                intrinsic.path.join("."),
                intrinsic.name,
            );
        }
    }

    /// No duplicate (path, name) entries — `find` returns the first match.
    #[test]
    fn no_duplicate_entries() {
        let mut seen = std::collections::HashSet::new();
        for intrinsic in INTRINSICS {
            assert!(
                seen.insert((intrinsic.path, intrinsic.name)),
                "duplicate intrinsic {}::{}",
                intrinsic.path.join("."),
                intrinsic.name,
            );
        }
    }
}
