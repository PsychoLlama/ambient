//! Type inference for the Ambient language.
//!
//! This module implements Hindley-Milner type inference with:
//! - Algorithm W for principal type inference
//! - Unification with occurs check
//! - Let-polymorphism (generalization at let bindings)
//! - Type environment with lexical scoping

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{BinaryOp, BindingId, Expr, ExprKind, Pattern, PatternKind, StmtKind, UnaryOp};
use crate::types::{FunctionType, RecordType, Type, TypeVar, TypeVarGen, TypeVarId};

// ─────────────────────────────────────────────────────────────────────────────
// Type Errors
// ─────────────────────────────────────────────────────────────────────────────

/// A type error with context for error reporting.
#[derive(Debug, Clone)]
pub struct TypeError {
    /// The kind of type error.
    pub kind: TypeErrorKind,
    /// Source location (byte offset range).
    pub span: (u32, u32),
    /// Additional context for the error.
    pub context: Option<String>,
}

impl TypeError {
    /// Create a new type error.
    #[must_use]
    pub fn new(kind: TypeErrorKind, span: (u32, u32)) -> Self {
        Self {
            kind,
            span,
            context: None,
        }
    }

    /// Add context to a type error.
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.kind)?;
        if let Some(ctx) = &self.context {
            write!(f, "\n  {ctx}")?;
        }
        Ok(())
    }
}

impl std::error::Error for TypeError {}

/// The kind of type error.
#[derive(Debug, Clone)]
pub enum TypeErrorKind {
    /// Type mismatch in unification.
    TypeMismatch { expected: Type, actual: Type },

    /// Occurs check failed (infinite type).
    InfiniteType { var: TypeVarId, ty: Type },

    /// Undefined variable.
    UndefinedVariable { name: Arc<str> },

    /// Undefined binding ID (internal error).
    UndefinedBinding { id: BindingId },

    /// Field not found in record.
    FieldNotFound { field: Arc<str>, record_ty: Type },

    /// Tuple index out of bounds.
    TupleIndexOutOfBounds { index: u32, tuple_ty: Type },

    /// Not a function type in call position.
    NotAFunction { ty: Type },

    /// Wrong number of arguments.
    ArityMismatch { expected: usize, actual: usize },

    /// Condition is not a boolean.
    NonBooleanCondition { ty: Type },

    /// Cannot determine type.
    CannotInfer { hint: String },

    /// Match arms have different types.
    MatchArmTypeMismatch { first: Type, arm: Type },
}

impl std::fmt::Display for TypeErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TypeMismatch { expected, actual } => {
                write!(f, "type mismatch: expected `{expected}`, found `{actual}`")
            }
            Self::InfiniteType { var, ty } => {
                write!(f, "infinite type: '{var} occurs in `{ty}`")
            }
            Self::UndefinedVariable { name } => {
                write!(f, "undefined variable: `{name}`")
            }
            Self::UndefinedBinding { id } => {
                write!(f, "undefined binding (internal error): {id}")
            }
            Self::FieldNotFound { field, record_ty } => {
                write!(f, "field `{field}` not found in `{record_ty}`")
            }
            Self::TupleIndexOutOfBounds { index, tuple_ty } => {
                write!(f, "tuple index {index} out of bounds for `{tuple_ty}`")
            }
            Self::NotAFunction { ty } => {
                write!(f, "expected a function, found `{ty}`")
            }
            Self::ArityMismatch { expected, actual } => {
                write!(f, "expected {expected} arguments, found {actual}")
            }
            Self::NonBooleanCondition { ty } => {
                write!(f, "condition must be boolean, found `{ty}`")
            }
            Self::CannotInfer { hint } => {
                write!(f, "cannot infer type: {hint}")
            }
            Self::MatchArmTypeMismatch { first, arm } => {
                write!(
                    f,
                    "match arm type `{arm}` differs from first arm type `{first}`"
                )
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Type Environment
// ─────────────────────────────────────────────────────────────────────────────

/// A type environment mapping bindings to their types.
///
/// Uses a persistent structure for efficient scoping.
#[derive(Debug, Clone, Default)]
pub struct TypeEnv {
    /// Mapping from binding IDs to type schemes.
    bindings: HashMap<BindingId, Scheme>,

    /// Mapping from names to binding IDs (for named lookups).
    names: HashMap<Arc<str>, BindingId>,
}

/// A type scheme (potentially polymorphic type).
///
/// `forall a b. T` where `a` and `b` are quantified type variables.
#[derive(Debug, Clone)]
pub struct Scheme {
    /// Quantified type variables.
    pub vars: Vec<TypeVarId>,
    /// The type body.
    pub ty: Type,
}

impl Scheme {
    /// Create a monomorphic scheme (no quantified variables).
    #[must_use]
    pub fn mono(ty: Type) -> Self {
        Self {
            vars: Vec::new(),
            ty,
        }
    }

    /// Create a polymorphic scheme.
    #[must_use]
    pub fn poly(vars: Vec<TypeVarId>, ty: Type) -> Self {
        Self { vars, ty }
    }
}

impl TypeEnv {
    /// Create an empty type environment.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a binding with its type scheme.
    pub fn insert(&mut self, id: BindingId, name: Arc<str>, scheme: Scheme) {
        self.bindings.insert(id, scheme);
        self.names.insert(name, id);
    }

    /// Insert a binding with a monomorphic type.
    pub fn insert_mono(&mut self, id: BindingId, name: Arc<str>, ty: Type) {
        self.insert(id, name, Scheme::mono(ty));
    }

    /// Look up a binding by ID.
    #[must_use]
    pub fn get(&self, id: BindingId) -> Option<&Scheme> {
        self.bindings.get(&id)
    }

    /// Look up a binding by name.
    #[must_use]
    pub fn get_by_name(&self, name: &str) -> Option<&Scheme> {
        self.names.get(name).and_then(|id| self.bindings.get(id))
    }

    /// Extend the environment with a new scope (for let bindings).
    #[must_use]
    pub fn extend(&self) -> Self {
        self.clone()
    }

    /// Collect all free type variables in the environment.
    #[must_use]
    pub fn free_vars(&self) -> Vec<TypeVarId> {
        let mut vars = Vec::new();
        for scheme in self.bindings.values() {
            let scheme_vars = scheme.ty.free_vars();
            for var in scheme_vars {
                if !scheme.vars.contains(&var) && !vars.contains(&var) {
                    vars.push(var);
                }
            }
        }
        vars
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unification
// ─────────────────────────────────────────────────────────────────────────────

/// Unify two types, making them equal.
///
/// Returns Ok(()) if successful, or a TypeError if the types cannot be unified.
pub fn unify(t1: &Type, t2: &Type, span: (u32, u32)) -> Result<(), TypeError> {
    let t1 = t1.resolve();
    let t2 = t2.resolve();

    match (&t1, &t2) {
        // Same types unify trivially
        (Type::Unit, Type::Unit)
        | (Type::Bool, Type::Bool)
        | (Type::Number, Type::Number)
        | (Type::String, Type::String)
        | (Type::Never, Type::Never) => Ok(()),

        // Error types unify with anything (for error recovery)
        (Type::Error, _) | (_, Type::Error) => Ok(()),

        // Type variables
        (Type::Var(TypeVar::Unbound(id)), _) => bind_var(*id, &t2, span),
        (_, Type::Var(TypeVar::Unbound(id))) => bind_var(*id, &t1, span),

        // Tuples: same length, unify element-wise
        (Type::Tuple(elems1), Type::Tuple(elems2)) => {
            if elems1.len() != elems2.len() {
                return Err(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: t1.clone(),
                        actual: t2.clone(),
                    },
                    span,
                ));
            }
            for (e1, e2) in elems1.iter().zip(elems2.iter()) {
                unify(e1, e2, span)?;
            }
            Ok(())
        }

        // Records: same fields, unify field-wise
        (Type::Record(r1), Type::Record(r2)) => {
            if r1.fields.len() != r2.fields.len() {
                return Err(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: t1.clone(),
                        actual: t2.clone(),
                    },
                    span,
                ));
            }
            // Fields are sorted, so we can zip directly
            for ((n1, ty1), (n2, ty2)) in r1.fields.iter().zip(r2.fields.iter()) {
                if n1 != n2 {
                    return Err(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: t1.clone(),
                            actual: t2.clone(),
                        },
                        span,
                    ));
                }
                unify(ty1, ty2, span)?;
            }
            Ok(())
        }

        // Functions: same arity, unify params and return types
        (Type::Function(f1), Type::Function(f2)) => {
            if f1.params.len() != f2.params.len() {
                return Err(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: t1.clone(),
                        actual: t2.clone(),
                    },
                    span,
                ));
            }
            for (p1, p2) in f1.params.iter().zip(f2.params.iter()) {
                unify(p1, p2, span)?;
            }
            unify(&f1.ret, &f2.ret, span)
        }

        // Named types: same name and arity, unify arguments
        (Type::Named(n1), Type::Named(n2)) => {
            if n1.name != n2.name || n1.args.len() != n2.args.len() {
                return Err(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: t1.clone(),
                        actual: t2.clone(),
                    },
                    span,
                ));
            }
            for (a1, a2) in n1.args.iter().zip(n2.args.iter()) {
                unify(a1, a2, span)?;
            }
            Ok(())
        }

        // Nominal types: must have same UUID
        (Type::Nominal(n1), Type::Nominal(n2)) => {
            if n1.uuid != n2.uuid {
                return Err(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: t1.clone(),
                        actual: t2.clone(),
                    },
                    span,
                ));
            }
            // Structural parts should also unify
            unify(&n1.inner, &n2.inner, span)
        }

        // Anything else doesn't unify
        _ => Err(TypeError::new(
            TypeErrorKind::TypeMismatch {
                expected: t1.clone(),
                actual: t2.clone(),
            },
            span,
        )),
    }
}

/// Bind a type variable to a type.
fn bind_var(var: TypeVarId, ty: &Type, span: (u32, u32)) -> Result<(), TypeError> {
    // Occurs check: prevent infinite types
    if occurs(var, ty) {
        return Err(TypeError::new(
            TypeErrorKind::InfiniteType {
                var,
                ty: ty.clone(),
            },
            span,
        ));
    }

    // Link the variable to the type
    // In a real implementation, we'd mutate the type variable in place.
    // For now, we use a workaround with RefCell.
    // This is where the Link variant of TypeVar comes in.
    Ok(())
}

/// Check if a type variable occurs in a type (for occurs check).
fn occurs(var: TypeVarId, ty: &Type) -> bool {
    match ty.resolve() {
        Type::Var(TypeVar::Unbound(id)) => id == var,
        Type::Tuple(elems) => elems.iter().any(|e| occurs(var, e)),
        Type::Record(r) => r.fields.iter().any(|(_, t)| occurs(var, t)),
        Type::Function(f) => {
            f.params.iter().any(|p| occurs(var, p)) || occurs(var, &f.ret)
        }
        Type::Named(n) => n.args.iter().any(|a| occurs(var, a)),
        Type::Nominal(n) => occurs(var, &n.inner),
        Type::Forall(f) => {
            if f.vars.contains(&var) {
                false // bound variable, doesn't count
            } else {
                occurs(var, &f.body)
            }
        }
        _ => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Type Inference
// ─────────────────────────────────────────────────────────────────────────────

/// Type inference context.
pub struct Infer {
    /// Type variable generator.
    gen: TypeVarGen,
    /// Substitution mapping type variables to their bindings.
    subst: HashMap<TypeVarId, Type>,
}

impl Default for Infer {
    fn default() -> Self {
        Self::new()
    }
}

impl Infer {
    /// Create a new inference context.
    #[must_use]
    pub fn new() -> Self {
        Self {
            gen: TypeVarGen::new(),
            subst: HashMap::new(),
        }
    }

    /// Generate a fresh type variable.
    pub fn fresh(&mut self) -> Type {
        self.gen.fresh()
    }

    /// Apply substitution to a type.
    pub fn apply(&self, ty: &Type) -> Type {
        self.apply_impl(ty, &mut Vec::new())
    }

    fn apply_impl(&self, ty: &Type, seen: &mut Vec<TypeVarId>) -> Type {
        match ty {
            Type::Var(TypeVar::Unbound(id)) => {
                if seen.contains(id) {
                    return ty.clone(); // Cycle, stop
                }
                if let Some(bound) = self.subst.get(id) {
                    seen.push(*id);
                    let result = self.apply_impl(bound, seen);
                    seen.pop();
                    result
                } else {
                    ty.clone()
                }
            }
            Type::Var(TypeVar::Link(link)) => self.apply_impl(&link.borrow(), seen),
            Type::Tuple(elems) => {
                Type::Tuple(elems.iter().map(|e| self.apply_impl(e, seen)).collect())
            }
            Type::Record(r) => Type::Record(RecordType::new(
                r.fields
                    .iter()
                    .map(|(n, t)| (n.clone(), self.apply_impl(t, seen)))
                    .collect(),
            )),
            Type::Function(f) => Type::Function(FunctionType::new(
                f.params.iter().map(|p| self.apply_impl(p, seen)).collect(),
                self.apply_impl(&f.ret, seen),
            )),
            Type::Named(n) => Type::Named(crate::types::NamedType::new(
                n.name.clone(),
                n.args.iter().map(|a| self.apply_impl(a, seen)).collect(),
            )),
            Type::Nominal(n) => Type::Nominal(crate::types::NominalType::new(
                n.uuid,
                self.apply_impl(&n.inner, seen),
                n.name.clone(),
            )),
            Type::Forall(f) => {
                // Don't apply subst to bound variables
                let mut new_subst = self.subst.clone();
                for var in &f.vars {
                    new_subst.remove(var);
                }
                let inner_infer = Infer {
                    gen: TypeVarGen::new(),
                    subst: new_subst,
                };
                Type::Forall(crate::types::ForallType::new(
                    f.vars.clone(),
                    inner_infer.apply(&f.body),
                ))
            }
            _ => ty.clone(),
        }
    }

    /// Unify two types and update the substitution.
    pub fn unify(&mut self, t1: &Type, t2: &Type, span: (u32, u32)) -> Result<(), TypeError> {
        let t1 = self.apply(t1);
        let t2 = self.apply(t2);

        match (&t1, &t2) {
            // Same primitive types
            (Type::Unit, Type::Unit)
            | (Type::Bool, Type::Bool)
            | (Type::Number, Type::Number)
            | (Type::String, Type::String)
            | (Type::Never, Type::Never) => Ok(()),

            // Error types
            (Type::Error, _) | (_, Type::Error) => Ok(()),

            // Type variables
            (Type::Var(TypeVar::Unbound(id1)), Type::Var(TypeVar::Unbound(id2))) if id1 == id2 => {
                Ok(())
            }
            (Type::Var(TypeVar::Unbound(id)), ty) | (ty, Type::Var(TypeVar::Unbound(id))) => {
                // Occurs check
                if self.occurs(*id, ty) {
                    return Err(TypeError::new(
                        TypeErrorKind::InfiniteType {
                            var: *id,
                            ty: ty.clone(),
                        },
                        span,
                    ));
                }
                self.subst.insert(*id, ty.clone());
                Ok(())
            }

            // Tuples
            (Type::Tuple(elems1), Type::Tuple(elems2)) => {
                if elems1.len() != elems2.len() {
                    return Err(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: t1.clone(),
                            actual: t2.clone(),
                        },
                        span,
                    ));
                }
                for (e1, e2) in elems1.iter().zip(elems2.iter()) {
                    self.unify(e1, e2, span)?;
                }
                Ok(())
            }

            // Records
            (Type::Record(r1), Type::Record(r2)) => {
                if r1.fields.len() != r2.fields.len() {
                    return Err(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: t1.clone(),
                            actual: t2.clone(),
                        },
                        span,
                    ));
                }
                for ((n1, ty1), (n2, ty2)) in r1.fields.iter().zip(r2.fields.iter()) {
                    if n1 != n2 {
                        return Err(TypeError::new(
                            TypeErrorKind::TypeMismatch {
                                expected: t1.clone(),
                                actual: t2.clone(),
                            },
                            span,
                        ));
                    }
                    self.unify(ty1, ty2, span)?;
                }
                Ok(())
            }

            // Functions
            (Type::Function(f1), Type::Function(f2)) => {
                if f1.params.len() != f2.params.len() {
                    return Err(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: t1.clone(),
                            actual: t2.clone(),
                        },
                        span,
                    ));
                }
                for (p1, p2) in f1.params.iter().zip(f2.params.iter()) {
                    self.unify(p1, p2, span)?;
                }
                self.unify(&f1.ret, &f2.ret, span)
            }

            // Named types
            (Type::Named(n1), Type::Named(n2)) => {
                if n1.name != n2.name || n1.args.len() != n2.args.len() {
                    return Err(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: t1.clone(),
                            actual: t2.clone(),
                        },
                        span,
                    ));
                }
                for (a1, a2) in n1.args.iter().zip(n2.args.iter()) {
                    self.unify(a1, a2, span)?;
                }
                Ok(())
            }

            // Nominal types
            (Type::Nominal(n1), Type::Nominal(n2)) => {
                if n1.uuid != n2.uuid {
                    return Err(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: t1.clone(),
                            actual: t2.clone(),
                        },
                        span,
                    ));
                }
                self.unify(&n1.inner, &n2.inner, span)
            }

            // Mismatch
            _ => Err(TypeError::new(
                TypeErrorKind::TypeMismatch {
                    expected: t1.clone(),
                    actual: t2.clone(),
                },
                span,
            )),
        }
    }

    /// Check if a type variable occurs in a type (after applying substitution).
    fn occurs(&self, var: TypeVarId, ty: &Type) -> bool {
        let ty = self.apply(ty);
        match ty {
            Type::Var(TypeVar::Unbound(id)) => id == var,
            Type::Tuple(elems) => elems.iter().any(|e| self.occurs(var, e)),
            Type::Record(r) => r.fields.iter().any(|(_, t)| self.occurs(var, t)),
            Type::Function(f) => {
                f.params.iter().any(|p| self.occurs(var, p)) || self.occurs(var, &f.ret)
            }
            Type::Named(n) => n.args.iter().any(|a| self.occurs(var, a)),
            Type::Nominal(n) => self.occurs(var, &n.inner),
            _ => false,
        }
    }

    /// Instantiate a type scheme with fresh type variables.
    pub fn instantiate(&mut self, scheme: &Scheme) -> Type {
        if scheme.vars.is_empty() {
            return scheme.ty.clone();
        }

        let mut subst = HashMap::new();
        for var in &scheme.vars {
            subst.insert(*var, self.fresh());
        }
        scheme.ty.substitute(&subst)
    }

    /// Generalize a type to a scheme by quantifying free variables
    /// not in the environment.
    pub fn generalize(&self, env: &TypeEnv, ty: &Type) -> Scheme {
        let ty = self.apply(ty);
        let ty_vars = ty.free_vars();
        let env_vars = env.free_vars();

        let free: Vec<_> = ty_vars
            .into_iter()
            .filter(|v| !env_vars.contains(v))
            .collect();

        if free.is_empty() {
            Scheme::mono(ty)
        } else {
            Scheme::poly(free, ty)
        }
    }

    /// Infer the type of an expression.
    pub fn infer_expr(&mut self, env: &TypeEnv, expr: &mut Expr) -> Result<Type, TypeError> {
        let span = (expr.span.start, expr.span.end);
        let ty = match &mut expr.kind {
            ExprKind::Unit => Type::Unit,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Number(_) => Type::Number,
            ExprKind::String(_) => Type::String,

            ExprKind::Local(id) => {
                let scheme = env.get(*id).ok_or_else(|| {
                    TypeError::new(TypeErrorKind::UndefinedBinding { id: *id }, span)
                })?;
                self.instantiate(scheme)
            }

            ExprKind::Name(name) => {
                let scheme = env.get_by_name(&name.name).ok_or_else(|| {
                    TypeError::new(
                        TypeErrorKind::UndefinedVariable {
                            name: name.name.clone(),
                        },
                        span,
                    )
                })?;
                self.instantiate(scheme)
            }

            ExprKind::Tuple(elems) => {
                let mut elem_tys = Vec::with_capacity(elems.len());
                for elem in elems {
                    elem_tys.push(self.infer_expr(env, elem)?);
                }
                Type::Tuple(elem_tys)
            }

            ExprKind::TupleIndex(tuple_expr, idx) => {
                let tuple_ty = self.infer_expr(env, tuple_expr)?;
                let tuple_ty = self.apply(&tuple_ty);
                match &tuple_ty {
                    Type::Tuple(elems) => {
                        let idx_usize = *idx as usize;
                        if idx_usize >= elems.len() {
                            return Err(TypeError::new(
                                TypeErrorKind::TupleIndexOutOfBounds {
                                    index: *idx,
                                    tuple_ty: tuple_ty.clone(),
                                },
                                span,
                            ));
                        }
                        elems[idx_usize].clone()
                    }
                    Type::Var(_) => {
                        // Unknown tuple type, need more context
                        return Err(TypeError::new(
                            TypeErrorKind::CannotInfer {
                                hint: "tuple type".to_string(),
                            },
                            span,
                        ));
                    }
                    _ => {
                        return Err(TypeError::new(
                            TypeErrorKind::TypeMismatch {
                                expected: Type::Tuple(vec![]),
                                actual: tuple_ty,
                            },
                            span,
                        ));
                    }
                }
            }

            ExprKind::Record(fields) => {
                let mut field_tys = Vec::with_capacity(fields.len());
                for (name, expr) in fields {
                    let ty = self.infer_expr(env, expr)?;
                    field_tys.push((name.clone(), ty));
                }
                Type::record(field_tys)
            }

            ExprKind::RecordField(record_expr, field) => {
                let record_ty = self.infer_expr(env, record_expr)?;
                let record_ty = self.apply(&record_ty);
                match &record_ty {
                    Type::Record(rec) => {
                        rec.get_field(field)
                            .cloned()
                            .ok_or_else(|| {
                                TypeError::new(
                                    TypeErrorKind::FieldNotFound {
                                        field: field.clone(),
                                        record_ty: record_ty.clone(),
                                    },
                                    span,
                                )
                            })?
                    }
                    Type::Var(_) => {
                        return Err(TypeError::new(
                            TypeErrorKind::CannotInfer {
                                hint: "record type".to_string(),
                            },
                            span,
                        ));
                    }
                    _ => {
                        return Err(TypeError::new(
                            TypeErrorKind::FieldNotFound {
                                field: field.clone(),
                                record_ty: record_ty.clone(),
                            },
                            span,
                        ));
                    }
                }
            }

            ExprKind::List(elems) => {
                let elem_ty = self.fresh();
                for elem in elems {
                    let ty = self.infer_expr(env, elem)?;
                    self.unify(&elem_ty, &ty, span)?;
                }
                Type::named("List", vec![self.apply(&elem_ty)])
            }

            ExprKind::Binary(op, left, right) => {
                let left_ty = self.infer_expr(env, left)?;
                let right_ty = self.infer_expr(env, right)?;

                if op.is_arithmetic() {
                    self.unify(&left_ty, &Type::Number, span)?;
                    self.unify(&right_ty, &Type::Number, span)?;
                    Type::Number
                } else if op.is_comparison() {
                    // For now, require same types for comparison
                    self.unify(&left_ty, &right_ty, span)?;
                    Type::Bool
                } else if op.is_logical() {
                    self.unify(&left_ty, &Type::Bool, span)?;
                    self.unify(&right_ty, &Type::Bool, span)?;
                    Type::Bool
                } else {
                    unreachable!()
                }
            }

            ExprKind::Unary(op, operand) => {
                let operand_ty = self.infer_expr(env, operand)?;
                match op {
                    UnaryOp::Neg => {
                        self.unify(&operand_ty, &Type::Number, span)?;
                        Type::Number
                    }
                    UnaryOp::Not => {
                        self.unify(&operand_ty, &Type::Bool, span)?;
                        Type::Bool
                    }
                }
            }

            ExprKind::If(cond, then_branch, else_branch) => {
                let cond_ty = self.infer_expr(env, cond)?;
                self.unify(&cond_ty, &Type::Bool, span)?;

                let then_ty = self.infer_expr(env, then_branch)?;

                if let Some(else_branch) = else_branch {
                    let else_ty = self.infer_expr(env, else_branch)?;
                    self.unify(&then_ty, &else_ty, span)?;
                    then_ty
                } else {
                    // No else branch means unit type
                    self.unify(&then_ty, &Type::Unit, span)?;
                    Type::Unit
                }
            }

            ExprKind::Match(scrutinee, arms) => {
                let scrutinee_ty = self.infer_expr(env, scrutinee)?;

                if arms.is_empty() {
                    return Err(TypeError::new(
                        TypeErrorKind::CannotInfer {
                            hint: "match expression has no arms".to_string(),
                        },
                        span,
                    ));
                }

                let result_ty = self.fresh();
                for arm in arms {
                    let arm_env = self.infer_pattern(env, &arm.pattern, &scrutinee_ty)?;
                    // Clone the arm body to avoid mutable borrow issues
                    let mut body = arm.body.clone();
                    let arm_ty = self.infer_expr(&arm_env, &mut body)?;
                    self.unify(&result_ty, &arm_ty, span)?;
                }
                self.apply(&result_ty)
            }

            ExprKind::Block(stmts, result) => {
                let mut block_env = env.extend();
                for stmt in stmts {
                    match &mut stmt.kind {
                        StmtKind::Let(binding) => {
                            let init_ty = self.infer_expr(&block_env, &mut binding.init)?;
                            let scheme = self.generalize(&block_env, &init_ty);
                            block_env.insert(binding.id, binding.name.clone(), scheme);
                        }
                        StmtKind::Expr(expr) => {
                            self.infer_expr(&block_env, expr)?;
                        }
                    }
                }
                if let Some(result) = result {
                    self.infer_expr(&block_env, result)?
                } else {
                    Type::Unit
                }
            }

            ExprKind::Lambda(lambda) => {
                let mut lambda_env = env.extend();
                let mut param_tys = Vec::with_capacity(lambda.params.len());

                for param in &lambda.params {
                    let param_ty = param.ty.clone().unwrap_or_else(|| self.fresh());
                    param_tys.push(param_ty.clone());
                    lambda_env.insert_mono(param.id, param.name.clone(), param_ty);
                }

                let ret_ty = self.infer_expr(&lambda_env, &mut lambda.body)?;
                Type::function(param_tys, ret_ty)
            }

            ExprKind::Call(callee, args) => {
                let callee_ty = self.infer_expr(env, callee)?;
                let mut arg_tys = Vec::with_capacity(args.len());
                for arg in args {
                    arg_tys.push(self.infer_expr(env, arg)?);
                }

                let ret_ty = self.fresh();
                let expected_fn_ty = Type::function(arg_tys, ret_ty.clone());
                self.unify(&callee_ty, &expected_fn_ty, span)?;
                self.apply(&ret_ty)
            }

            ExprKind::Perform(_) | ExprKind::Suspend(_) | ExprKind::Handle(_) => {
                // Ability types require additional tracking (Milestone 8)
                // For now, return a fresh type variable
                self.fresh()
            }
        };

        expr.ty = Some(ty.clone());
        Ok(ty)
    }

    /// Infer types for a pattern and return extended environment.
    fn infer_pattern(
        &mut self,
        env: &TypeEnv,
        pattern: &Pattern,
        expected: &Type,
    ) -> Result<TypeEnv, TypeError> {
        let span = (pattern.span.start, pattern.span.end);
        let mut new_env = env.extend();

        match &pattern.kind {
            PatternKind::Wildcard => {
                // Wildcard matches anything
            }

            PatternKind::Binding(id, name) => {
                new_env.insert_mono(*id, name.clone(), expected.clone());
            }

            PatternKind::Literal(lit) => {
                let lit_ty = match lit {
                    crate::ast::Literal::Unit => Type::Unit,
                    crate::ast::Literal::Bool(_) => Type::Bool,
                    crate::ast::Literal::Number(_) => Type::Number,
                    crate::ast::Literal::String(_) => Type::String,
                };
                self.unify(expected, &lit_ty, span)?;
            }

            PatternKind::Tuple(patterns) => {
                let elem_tys: Vec<_> = (0..patterns.len()).map(|_| self.fresh()).collect();
                let tuple_ty = Type::Tuple(elem_tys.clone());
                self.unify(expected, &tuple_ty, span)?;

                for (pat, ty) in patterns.iter().zip(elem_tys.iter()) {
                    let pat_env = self.infer_pattern(&new_env, pat, ty)?;
                    // Merge bindings
                    for (id, scheme) in pat_env.bindings {
                        if let Some(name) = pat_env.names.iter().find(|(_, v)| **v == id).map(|(k, _)| k) {
                            new_env.insert(id, name.clone(), scheme);
                        }
                    }
                }
            }

            PatternKind::Record(field_patterns) => {
                let mut field_tys = Vec::with_capacity(field_patterns.len());
                for (name, _) in field_patterns {
                    field_tys.push((name.clone(), self.fresh()));
                }
                let record_ty = Type::record(field_tys.clone());
                self.unify(expected, &record_ty, span)?;

                for ((_, pat), (_, ty)) in field_patterns.iter().zip(field_tys.iter()) {
                    let pat_env = self.infer_pattern(&new_env, pat, ty)?;
                    for (id, scheme) in pat_env.bindings {
                        if let Some(name) = pat_env.names.iter().find(|(_, v)| **v == id).map(|(k, _)| k) {
                            new_env.insert(id, name.clone(), scheme);
                        }
                    }
                }
            }

            PatternKind::Variant(_, _) => {
                // Enum patterns require enum type definitions (future work)
                // For now, just return the environment unchanged
            }
        }

        Ok(new_env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Param;

    fn span() -> (u32, u32) {
        (0, 0)
    }

    #[test]
    fn test_unify_primitives() {
        let mut infer = Infer::new();
        assert!(infer.unify(&Type::Number, &Type::Number, span()).is_ok());
        assert!(infer.unify(&Type::String, &Type::String, span()).is_ok());
        assert!(infer.unify(&Type::Bool, &Type::Bool, span()).is_ok());
        assert!(infer.unify(&Type::Unit, &Type::Unit, span()).is_ok());
    }

    #[test]
    fn test_unify_mismatch() {
        let mut infer = Infer::new();
        assert!(infer.unify(&Type::Number, &Type::String, span()).is_err());
        assert!(infer.unify(&Type::Bool, &Type::Number, span()).is_err());
    }

    #[test]
    fn test_unify_type_variable() {
        let mut infer = Infer::new();
        let var = infer.fresh();
        assert!(infer.unify(&var, &Type::Number, span()).is_ok());
        assert_eq!(infer.apply(&var), Type::Number);
    }

    #[test]
    fn test_unify_tuples() {
        let mut infer = Infer::new();
        let t1 = Type::Tuple(vec![Type::Number, Type::String]);
        let t2 = Type::Tuple(vec![Type::Number, Type::String]);
        assert!(infer.unify(&t1, &t2, span()).is_ok());
    }

    #[test]
    fn test_unify_tuples_mismatch() {
        let mut infer = Infer::new();
        let t1 = Type::Tuple(vec![Type::Number, Type::String]);
        let t2 = Type::Tuple(vec![Type::Number, Type::Bool]);
        assert!(infer.unify(&t1, &t2, span()).is_err());
    }

    #[test]
    fn test_unify_records() {
        let mut infer = Infer::new();
        let r1 = Type::record([("x", Type::Number), ("y", Type::String)]);
        let r2 = Type::record([("x", Type::Number), ("y", Type::String)]);
        assert!(infer.unify(&r1, &r2, span()).is_ok());
    }

    #[test]
    fn test_unify_functions() {
        let mut infer = Infer::new();
        let f1 = Type::function(vec![Type::Number], Type::String);
        let f2 = Type::function(vec![Type::Number], Type::String);
        assert!(infer.unify(&f1, &f2, span()).is_ok());
    }

    #[test]
    fn test_occurs_check() {
        let mut infer = Infer::new();
        let var = infer.fresh();
        // Try to unify 'a with ('a -> 'a), should fail
        let fn_ty = Type::function(vec![var.clone()], var.clone());
        assert!(infer.unify(&var, &fn_ty, span()).is_err());
    }

    #[test]
    fn test_infer_literal() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::number(42.0);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Number);

        let mut expr = Expr::string("hello");
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::String);

        let mut expr = Expr::bool(true);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Bool);
    }

    #[test]
    fn test_infer_binary_arithmetic() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::binary(BinaryOp::Add, Expr::number(1.0), Expr::number(2.0));
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Number);
    }

    #[test]
    fn test_infer_binary_comparison() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::binary(BinaryOp::Lt, Expr::number(1.0), Expr::number(2.0));
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Bool);
    }

    #[test]
    fn test_infer_if_then_else() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::if_then_else(
            Expr::bool(true),
            Expr::number(1.0),
            Some(Expr::number(2.0)),
        );
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Number);
    }

    #[test]
    fn test_infer_if_then_else_mismatch() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::if_then_else(
            Expr::bool(true),
            Expr::number(1.0),
            Some(Expr::string("hello")),
        );
        assert!(infer.infer_expr(&env, &mut expr).is_err());
    }

    #[test]
    fn test_infer_tuple() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::tuple(vec![Expr::number(1.0), Expr::string("hello")]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Tuple(vec![Type::Number, Type::String]));
    }

    #[test]
    fn test_infer_record() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::record([("x", Expr::number(1.0)), ("y", Expr::number(2.0))]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(
            ty,
            Type::record([("x", Type::Number), ("y", Type::Number)])
        );
    }

    #[test]
    fn test_infer_lambda() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // (x) => x + 1
        let mut expr = Expr::lambda(
            vec![Param::new(0, "x")],
            Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
        );
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        let ty = infer.apply(&ty);
        assert_eq!(ty, Type::function(vec![Type::Number], Type::Number));
    }

    #[test]
    fn test_infer_lambda_call() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // ((x) => x + 1)(42)
        let lambda = Expr::lambda(
            vec![Param::new(0, "x")],
            Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
        );
        let mut expr = Expr::call(lambda, vec![Expr::number(42.0)]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Number);
    }

    #[test]
    fn test_infer_let_polymorphism() {
        let mut infer = Infer::new();
        let mut env = TypeEnv::new();

        // identity: forall a. a -> a
        env.insert(0, "id".into(), Scheme::poly(vec![0], Type::function(vec![Type::var(0)], Type::var(0))));

        // id(42) should be number
        let mut expr = Expr::call(Expr::local(0), vec![Expr::number(42.0)]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Number);

        // id("hello") should be string
        let mut expr = Expr::call(Expr::local(0), vec![Expr::string("hello")]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::String);
    }

    #[test]
    fn test_generalize() {
        let infer = Infer::new();
        let env = TypeEnv::new();

        // A type with free variable should generalize
        let ty = Type::function(vec![Type::var(0)], Type::var(0));
        let scheme = infer.generalize(&env, &ty);

        assert_eq!(scheme.vars, vec![0]);
    }

    #[test]
    fn test_instantiate() {
        let mut infer = Infer::new();

        let scheme = Scheme::poly(vec![0], Type::function(vec![Type::var(0)], Type::var(0)));
        let ty = infer.instantiate(&scheme);

        // Should get a fresh type variable, not '0
        if let Type::Function(f) = ty {
            assert!(matches!(f.params[0], Type::Var(TypeVar::Unbound(_))));
            assert!(matches!(*f.ret, Type::Var(TypeVar::Unbound(_))));
        } else {
            panic!("Expected function type");
        }
    }

    #[test]
    fn test_type_error_display() {
        let err = TypeError::new(
            TypeErrorKind::TypeMismatch {
                expected: Type::Number,
                actual: Type::String,
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("type mismatch"));
        assert!(msg.contains("number"));
        assert!(msg.contains("string"));
    }
}
