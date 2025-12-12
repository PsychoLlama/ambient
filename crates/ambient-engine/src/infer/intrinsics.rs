//! Intrinsic type inference for built-in functions.
//!
//! This module handles type inference for intrinsic functions like:
//! - `core.math.*` - Mathematical operations
//! - `core.list.*` - List operations
//! - `core.string.*` - String operations
//! - `core.map.*` - Map operations
//! - `core.set.*` - Set operations
//! - `core.option.*` - Option operations
//! - `core.result.*` - Result operations
//! - `core.convert.*` - Type conversions
//! - `core.enum.*` - Enum operations

use super::{Infer, InferResult, TypeEnv};
use crate::ast::Expr;
use crate::types::Type;

impl Infer {
    /// Try to infer the type of an intrinsic function call.
    ///
    /// Returns `Some(return_type)` if the name is a known intrinsic,
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
    ///
    /// # Errors
    ///
    /// Returns a `TypeError` if argument types don't match the intrinsic's signature.
    #[allow(clippy::too_many_lines)]
    pub fn try_infer_intrinsic(
        &mut self,
        env: &TypeEnv,
        qualified_name: &crate::ast::QualifiedName,
        args: &mut [Expr],
        span: (u32, u32),
    ) -> InferResult<Option<Type>> {
        // Helper to create List type
        let list_of = |elem: Type| Type::named("List", vec![elem]);

        // Convert path to slice for matching
        let path: Vec<&str> = qualified_name.path.iter().map(AsRef::as_ref).collect();
        let name = qualified_name.name.as_ref();

        match (path.as_slice(), name) {
            // ─────────────────────────────────────────────────────────────────
            // core.math - Math intrinsics
            // ─────────────────────────────────────────────────────────────────
            (
                ["core", "math"],
                "sqrt" | "abs" | "floor" | "ceil" | "round" | "trunc" | "sin" | "cos" | "tan"
                | "ln" | "exp" | "asin" | "acos" | "atan" | "log10" | "log2",
            ) if args.len() == 1 => {
                let arg_ty = self.infer_expr(env, &mut args[0])?;
                self.unify(&arg_ty, &Type::Number, span)?;
                Ok(Some(Type::Number))
            }
            (["core", "math"], "pow" | "min" | "max" | "atan2") if args.len() == 2 => {
                let a_ty = self.infer_expr(env, &mut args[0])?;
                let b_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&a_ty, &Type::Number, span)?;
                self.unify(&b_ty, &Type::Number, span)?;
                Ok(Some(Type::Number))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.list - List operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "list"], "length") if args.len() == 1 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty), span)?;
                Ok(Some(Type::Number))
            }
            (["core", "list"], "get") if args.len() == 2 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let index_ty = self.infer_expr(env, &mut args[1])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty.clone()), span)?;
                self.unify(&index_ty, &Type::Number, span)?;
                // Returns the element or Unit for out of bounds
                // For now, return the element type (assuming in bounds)
                Ok(Some(elem_ty))
            }
            (["core", "list"], "head") if args.len() == 1 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty.clone()), span)?;
                // Returns the element or Unit for empty list
                Ok(Some(elem_ty))
            }
            (["core", "list"], "tail" | "reverse" | "sort") if args.len() == 1 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty.clone()), span)?;
                Ok(Some(list_of(elem_ty)))
            }
            (["core", "list"], "concat") if args.len() == 2 => {
                let list1_ty = self.infer_expr(env, &mut args[0])?;
                let list2_ty = self.infer_expr(env, &mut args[1])?;
                let elem_ty = self.fresh();
                self.unify(&list1_ty, &list_of(elem_ty.clone()), span)?;
                self.unify(&list2_ty, &list_of(elem_ty.clone()), span)?;
                Ok(Some(list_of(elem_ty)))
            }
            (["core", "list"], "append") if args.len() == 2 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let elem_arg_ty = self.infer_expr(env, &mut args[1])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty.clone()), span)?;
                self.unify(&elem_arg_ty, &elem_ty, span)?;
                Ok(Some(list_of(elem_ty)))
            }
            (["core", "list"], "is_empty") if args.len() == 1 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty), span)?;
                Ok(Some(Type::Bool))
            }
            (["core", "list"], "slice") if args.len() == 3 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let start_ty = self.infer_expr(env, &mut args[1])?;
                let end_ty = self.infer_expr(env, &mut args[2])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty.clone()), span)?;
                self.unify(&start_ty, &Type::Number, span)?;
                self.unify(&end_ty, &Type::Number, span)?;
                Ok(Some(list_of(elem_ty)))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.string - String operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "string"], "length") if args.len() == 1 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                self.unify(&str_ty, &Type::String, span)?;
                Ok(Some(Type::Number))
            }
            (["core", "string"], "concat") if args.len() == 2 => {
                let str1_ty = self.infer_expr(env, &mut args[0])?;
                let str2_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&str1_ty, &Type::String, span)?;
                self.unify(&str2_ty, &Type::String, span)?;
                Ok(Some(Type::String))
            }
            (["core", "string"], "contains" | "starts_with" | "ends_with") if args.len() == 2 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                let substr_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&str_ty, &Type::String, span)?;
                self.unify(&substr_ty, &Type::String, span)?;
                Ok(Some(Type::Bool))
            }
            (["core", "string"], "split") if args.len() == 2 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                let delim_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&str_ty, &Type::String, span)?;
                self.unify(&delim_ty, &Type::String, span)?;
                Ok(Some(list_of(Type::String)))
            }
            (["core", "string"], "join") if args.len() == 2 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let delim_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&list_ty, &list_of(Type::String), span)?;
                self.unify(&delim_ty, &Type::String, span)?;
                Ok(Some(Type::String))
            }
            (["core", "string"], "trim" | "to_upper" | "to_lower" | "reverse")
                if args.len() == 1 =>
            {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                self.unify(&str_ty, &Type::String, span)?;
                Ok(Some(Type::String))
            }
            (["core", "string"], "slice") if args.len() == 3 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                let start_ty = self.infer_expr(env, &mut args[1])?;
                let end_ty = self.infer_expr(env, &mut args[2])?;
                self.unify(&str_ty, &Type::String, span)?;
                self.unify(&start_ty, &Type::Number, span)?;
                self.unify(&end_ty, &Type::Number, span)?;
                Ok(Some(Type::String))
            }
            (["core", "string"], "chars") if args.len() == 1 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                self.unify(&str_ty, &Type::String, span)?;
                Ok(Some(list_of(Type::String)))
            }
            (["core", "string"], "replace") if args.len() == 3 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                let pattern_ty = self.infer_expr(env, &mut args[1])?;
                let replacement_ty = self.infer_expr(env, &mut args[2])?;
                self.unify(&str_ty, &Type::String, span)?;
                self.unify(&pattern_ty, &Type::String, span)?;
                self.unify(&replacement_ty, &Type::String, span)?;
                Ok(Some(Type::String))
            }
            (["core", "string"], "index_of") if args.len() == 2 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                let substr_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&str_ty, &Type::String, span)?;
                self.unify(&substr_ty, &Type::String, span)?;
                Ok(Some(Type::Number))
            }
            (["core", "string"], "repeat") if args.len() == 2 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                let count_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&str_ty, &Type::String, span)?;
                self.unify(&count_ty, &Type::Number, span)?;
                Ok(Some(Type::String))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.convert - Type conversion
            // ─────────────────────────────────────────────────────────────────
            (["core", "convert"], "to_string") if args.len() == 1 => {
                // Accept any type
                let _arg_ty = self.infer_expr(env, &mut args[0])?;
                Ok(Some(Type::String))
            }
            (["core", "convert"], "parse_number") if args.len() == 1 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                self.unify(&str_ty, &Type::String, span)?;
                // Returns (bool, number) tuple
                Ok(Some(Type::tuple(vec![Type::Bool, Type::Number])))
            }
            (["core", "convert"], "parse_bool") if args.len() == 1 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                self.unify(&str_ty, &Type::String, span)?;
                // Returns (bool, bool) tuple
                Ok(Some(Type::tuple(vec![Type::Bool, Type::Bool])))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.map - Map operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "map"], "empty") if args.is_empty() => {
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                Ok(Some(Type::named("Map", vec![key_ty, val_ty])))
            }
            (["core", "map"], "get") if args.len() == 2 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_arg_ty = self.infer_expr(env, &mut args[1])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(
                    &map_ty,
                    &Type::named("Map", vec![key_ty.clone(), val_ty.clone()]),
                    span,
                )?;
                self.unify(&key_arg_ty, &key_ty, span)?;
                // Returns Unit if key not found (should return Option)
                Ok(Some(val_ty))
            }
            (["core", "map"], "insert") if args.len() == 3 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_arg_ty = self.infer_expr(env, &mut args[1])?;
                let val_arg_ty = self.infer_expr(env, &mut args[2])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(
                    &map_ty,
                    &Type::named("Map", vec![key_ty.clone(), val_ty.clone()]),
                    span,
                )?;
                self.unify(&key_arg_ty, &key_ty, span)?;
                self.unify(&val_arg_ty, &val_ty.clone(), span)?;
                Ok(Some(Type::named("Map", vec![key_ty, val_ty])))
            }
            (["core", "map"], "remove") if args.len() == 2 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_arg_ty = self.infer_expr(env, &mut args[1])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(
                    &map_ty,
                    &Type::named("Map", vec![key_ty.clone(), val_ty.clone()]),
                    span,
                )?;
                self.unify(&key_arg_ty, &key_ty, span)?;
                Ok(Some(Type::named("Map", vec![key_ty, val_ty])))
            }
            (["core", "map"], "contains") if args.len() == 2 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_arg_ty = self.infer_expr(env, &mut args[1])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(
                    &map_ty,
                    &Type::named("Map", vec![key_ty.clone(), val_ty]),
                    span,
                )?;
                self.unify(&key_arg_ty, &key_ty, span)?;
                Ok(Some(Type::Bool))
            }
            (["core", "map"], "length") if args.len() == 1 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(&map_ty, &Type::named("Map", vec![key_ty, val_ty]), span)?;
                Ok(Some(Type::Number))
            }
            (["core", "map"], "keys") if args.len() == 1 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(
                    &map_ty,
                    &Type::named("Map", vec![key_ty.clone(), val_ty]),
                    span,
                )?;
                Ok(Some(list_of(key_ty)))
            }
            (["core", "map"], "values") if args.len() == 1 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(
                    &map_ty,
                    &Type::named("Map", vec![key_ty, val_ty.clone()]),
                    span,
                )?;
                Ok(Some(list_of(val_ty)))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.set - Set operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "set"], "empty") if args.is_empty() => {
                let elem_ty = self.fresh();
                Ok(Some(Type::named("Set", vec![elem_ty])))
            }
            (["core", "set"], "insert" | "remove") if args.len() == 2 => {
                let set_ty = self.infer_expr(env, &mut args[0])?;
                let elem_arg_ty = self.infer_expr(env, &mut args[1])?;
                let elem_ty = self.fresh();
                self.unify(&set_ty, &Type::named("Set", vec![elem_ty.clone()]), span)?;
                self.unify(&elem_arg_ty, &elem_ty.clone(), span)?;
                Ok(Some(Type::named("Set", vec![elem_ty])))
            }
            (["core", "set"], "contains") if args.len() == 2 => {
                let set_ty = self.infer_expr(env, &mut args[0])?;
                let elem_arg_ty = self.infer_expr(env, &mut args[1])?;
                let elem_ty = self.fresh();
                self.unify(&set_ty, &Type::named("Set", vec![elem_ty.clone()]), span)?;
                self.unify(&elem_arg_ty, &elem_ty, span)?;
                Ok(Some(Type::Bool))
            }
            (["core", "set"], "length") if args.len() == 1 => {
                let set_ty = self.infer_expr(env, &mut args[0])?;
                let elem_ty = self.fresh();
                self.unify(&set_ty, &Type::named("Set", vec![elem_ty]), span)?;
                Ok(Some(Type::Number))
            }
            (["core", "set"], "union" | "intersection" | "difference") if args.len() == 2 => {
                let set1_ty = self.infer_expr(env, &mut args[0])?;
                let set2_ty = self.infer_expr(env, &mut args[1])?;
                let elem_ty = self.fresh();
                self.unify(&set1_ty, &Type::named("Set", vec![elem_ty.clone()]), span)?;
                self.unify(&set2_ty, &Type::named("Set", vec![elem_ty.clone()]), span)?;
                Ok(Some(Type::named("Set", vec![elem_ty])))
            }
            (["core", "set"], "to_list") if args.len() == 1 => {
                let set_ty = self.infer_expr(env, &mut args[0])?;
                let elem_ty = self.fresh();
                self.unify(&set_ty, &Type::named("Set", vec![elem_ty.clone()]), span)?;
                Ok(Some(list_of(elem_ty)))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.option - Option operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "option"], "unwrap_or") if args.len() == 2 => {
                let opt_ty = self.infer_expr(env, &mut args[0])?;
                let default_ty = self.infer_expr(env, &mut args[1])?;
                let inner_ty = self.fresh();
                self.unify(&opt_ty, &Type::option(inner_ty.clone()), span)?;
                self.unify(&default_ty, &inner_ty.clone(), span)?;
                Ok(Some(inner_ty))
            }
            (["core", "option"], "is_some" | "is_none") if args.len() == 1 => {
                let opt_ty = self.infer_expr(env, &mut args[0])?;
                let inner_ty = self.fresh();
                self.unify(&opt_ty, &Type::option(inner_ty), span)?;
                Ok(Some(Type::Bool))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.result - Result operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "result"], "is_ok" | "is_err") if args.len() == 1 => {
                let res_ty = self.infer_expr(env, &mut args[0])?;
                let ok_ty = self.fresh();
                let err_ty = self.fresh();
                self.unify(&res_ty, &Type::result(ok_ty, err_ty), span)?;
                Ok(Some(Type::Bool))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.enum - Enum operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "enum"], "tag") if args.len() == 1 => {
                // Accept any enum type
                let _enum_ty = self.infer_expr(env, &mut args[0])?;
                Ok(Some(Type::Number))
            }
            (["core", "enum"], "payload") if args.len() == 1 => {
                // Accept any enum type, return the payload type
                // This is tricky - we'd need to know which variant
                // For now, return a fresh type variable
                let _enum_ty = self.infer_expr(env, &mut args[0])?;
                let payload_ty = self.fresh();
                Ok(Some(payload_ty))
            }

            _ => Ok(None),
        }
    }
}
