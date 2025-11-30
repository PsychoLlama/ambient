//! Type system for the Ambient language.
//!
//! This module implements Hindley-Milner type inference with support for:
//! - Primitive types (number, string, bool, unit)
//! - Composite types (tuples, records, functions)
//! - Polymorphic types (generics with type variables)
//! - Nominal types (unique types distinguished by UUID)
//!
//! The type system uses structural equivalence by default, with nominal
//! types providing opt-in name-based distinction.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use uuid::Uuid;

/// A unique identifier for type variables, used during unification.
pub type TypeVarId = u32;

/// Counter for generating fresh type variable IDs.
#[derive(Debug, Default)]
pub struct TypeVarGen {
    next_id: TypeVarId,
}

impl TypeVarGen {
    /// Create a new type variable generator.
    #[must_use]
    pub fn new() -> Self {
        Self { next_id: 0 }
    }

    /// Generate a fresh type variable.
    pub fn fresh(&mut self) -> Type {
        let id = self.next_id;
        self.next_id += 1;
        Type::Var(TypeVar::Unbound(id))
    }

    /// Generate a fresh type variable ID.
    pub fn fresh_id(&mut self) -> TypeVarId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

/// A type variable that may be unbound or linked to another type.
///
/// Type variables are used during inference to represent unknown types.
/// During unification, they get linked to concrete types or other variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeVar {
    /// An unbound type variable with a unique ID.
    Unbound(TypeVarId),

    /// A type variable that has been linked to another type.
    /// Uses interior mutability for efficient union-find during unification.
    Link(Rc<RefCell<Type>>),
}

/// Represents a type in the Ambient language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    // ─────────────────────────────────────────────────────────────────────────
    // Primitive types
    // ─────────────────────────────────────────────────────────────────────────
    /// Unit type `()`, represents absence of a meaningful value.
    Unit,

    /// Boolean type `bool`.
    Bool,

    /// 64-bit floating point number (the only numeric type).
    Number,

    /// UTF-8 string type.
    String,

    // ─────────────────────────────────────────────────────────────────────────
    // Composite types
    // ─────────────────────────────────────────────────────────────────────────
    /// Tuple type: fixed-size, heterogeneous collection.
    /// `(T1, T2, ..., Tn)`
    Tuple(Vec<Type>),

    /// Record type: named fields with types (structural typing).
    /// `{ field1: T1, field2: T2, ... }`
    Record(RecordType),

    /// Function type: parameters -> return type.
    /// `(P1, P2, ...) -> R`
    Function(FunctionType),

    // ─────────────────────────────────────────────────────────────────────────
    // Polymorphism
    // ─────────────────────────────────────────────────────────────────────────
    /// A type variable used during inference.
    Var(TypeVar),

    /// A quantified (forall) type scheme.
    /// `forall a b. (a -> b) -> List<a> -> List<b>`
    Forall(ForallType),

    // ─────────────────────────────────────────────────────────────────────────
    // Named types
    // ─────────────────────────────────────────────────────────────────────────
    /// A named type constructor with optional type arguments.
    /// `List<T>`, `Option<T>`, `Map<K, V>`
    Named(NamedType),

    /// A nominal type distinguished by UUID, incompatible with structurally
    /// identical types.
    /// `unique(uuid) type UserId { value: string }`
    Nominal(NominalType),

    // ─────────────────────────────────────────────────────────────────────────
    // Special types
    // ─────────────────────────────────────────────────────────────────────────
    /// The never type `!`, for expressions that never return.
    Never,

    /// Error type used during type checking to allow recovery.
    Error,
}

/// A record type with named fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordType {
    /// Fields sorted by name for consistent comparison.
    pub fields: Vec<(Arc<str>, Type)>,
}

impl RecordType {
    /// Create a new record type with the given fields.
    /// Fields are sorted by name for consistent structural comparison.
    #[must_use]
    pub fn new(mut fields: Vec<(Arc<str>, Type)>) -> Self {
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        Self { fields }
    }

    /// Get the type of a field by name.
    #[must_use]
    pub fn get_field(&self, name: &str) -> Option<&Type> {
        self.fields
            .binary_search_by(|(n, _)| n.as_ref().cmp(name))
            .ok()
            .map(|idx| &self.fields[idx].1)
    }
}

/// A function type with parameters and return type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionType {
    /// Parameter types.
    pub params: Vec<Type>,

    /// Return type.
    pub ret: Box<Type>,
}

impl FunctionType {
    /// Create a new function type.
    #[must_use]
    pub fn new(params: Vec<Type>, ret: Type) -> Self {
        Self {
            params,
            ret: Box::new(ret),
        }
    }
}

/// A quantified type scheme (forall).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForallType {
    /// Bound type variable IDs.
    pub vars: Vec<TypeVarId>,

    /// The quantified type.
    pub body: Box<Type>,
}

impl ForallType {
    /// Create a new forall type.
    #[must_use]
    pub fn new(vars: Vec<TypeVarId>, body: Type) -> Self {
        Self {
            vars,
            body: Box::new(body),
        }
    }
}

/// A named type constructor (like `List<T>` or `Option<T>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedType {
    /// The type constructor name.
    pub name: Arc<str>,

    /// Type arguments (empty for non-generic types).
    pub args: Vec<Type>,
}

impl NamedType {
    /// Create a new named type.
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>, args: Vec<Type>) -> Self {
        Self {
            name: name.into(),
            args,
        }
    }

    /// Create a non-generic named type.
    #[must_use]
    pub fn simple(name: impl Into<Arc<str>>) -> Self {
        Self::new(name, Vec::new())
    }
}

/// A nominal type distinguished by UUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NominalType {
    /// Unique identifier making this type distinct from structurally
    /// identical types.
    pub uuid: Uuid,

    /// The underlying structural type.
    pub inner: Box<Type>,

    /// Optional human-readable name for error messages.
    pub name: Option<Arc<str>>,
}

impl NominalType {
    /// Create a new nominal type.
    #[must_use]
    pub fn new(uuid: Uuid, inner: Type, name: Option<impl Into<Arc<str>>>) -> Self {
        Self {
            uuid,
            inner: Box::new(inner),
            name: name.map(Into::into),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Type constructors (convenience methods)
// ─────────────────────────────────────────────────────────────────────────────

impl Type {
    /// Create a function type.
    #[must_use]
    pub fn function(params: Vec<Type>, ret: Type) -> Self {
        Self::Function(FunctionType::new(params, ret))
    }

    /// Create a tuple type.
    #[must_use]
    pub fn tuple(elements: Vec<Type>) -> Self {
        Self::Tuple(elements)
    }

    /// Create a record type.
    #[must_use]
    pub fn record(fields: impl IntoIterator<Item = (impl Into<Arc<str>>, Type)>) -> Self {
        let fields: Vec<_> = fields.into_iter().map(|(k, v)| (k.into(), v)).collect();
        Self::Record(RecordType::new(fields))
    }

    /// Create a named type with arguments.
    #[must_use]
    pub fn named(name: impl Into<Arc<str>>, args: Vec<Type>) -> Self {
        Self::Named(NamedType::new(name, args))
    }

    /// Create a simple named type (no arguments).
    #[must_use]
    pub fn named_simple(name: impl Into<Arc<str>>) -> Self {
        Self::Named(NamedType::simple(name))
    }

    /// Create an unbound type variable.
    #[must_use]
    pub fn var(id: TypeVarId) -> Self {
        Self::Var(TypeVar::Unbound(id))
    }

    /// Create a nominal type.
    #[must_use]
    pub fn nominal(uuid: Uuid, inner: Type, name: Option<impl Into<Arc<str>>>) -> Self {
        Self::Nominal(NominalType::new(uuid, inner, name))
    }

    /// Create a forall (polymorphic) type.
    #[must_use]
    pub fn forall(vars: Vec<TypeVarId>, body: Type) -> Self {
        if vars.is_empty() {
            body
        } else {
            Self::Forall(ForallType::new(vars, body))
        }
    }

    /// Check if this type is a concrete (non-variable) type.
    #[must_use]
    pub fn is_concrete(&self) -> bool {
        match self {
            Self::Var(_) => false,
            Self::Tuple(elems) => elems.iter().all(Type::is_concrete),
            Self::Record(rec) => rec.fields.iter().all(|(_, t)| t.is_concrete()),
            Self::Function(f) => {
                f.params.iter().all(Type::is_concrete) && f.ret.is_concrete()
            }
            Self::Named(n) => n.args.iter().all(Type::is_concrete),
            Self::Nominal(n) => n.inner.is_concrete(),
            Self::Forall(f) => f.body.is_concrete(),
            _ => true,
        }
    }

    /// Resolve type variable links to get the actual type.
    /// This follows chains of linked type variables.
    #[must_use]
    pub fn resolve(&self) -> Type {
        match self {
            Self::Var(TypeVar::Link(link)) => link.borrow().resolve(),
            other => other.clone(),
        }
    }

    /// Collect all free type variables in this type.
    #[must_use]
    pub fn free_vars(&self) -> Vec<TypeVarId> {
        let mut vars = Vec::new();
        self.collect_free_vars(&mut vars);
        vars.sort_unstable();
        vars.dedup();
        vars
    }

    fn collect_free_vars(&self, vars: &mut Vec<TypeVarId>) {
        match self {
            Self::Var(TypeVar::Unbound(id)) => vars.push(*id),
            Self::Var(TypeVar::Link(link)) => link.borrow().collect_free_vars(vars),
            Self::Tuple(elems) => {
                for elem in elems {
                    elem.collect_free_vars(vars);
                }
            }
            Self::Record(rec) => {
                for (_, t) in &rec.fields {
                    t.collect_free_vars(vars);
                }
            }
            Self::Function(f) => {
                for p in &f.params {
                    p.collect_free_vars(vars);
                }
                f.ret.collect_free_vars(vars);
            }
            Self::Named(n) => {
                for arg in &n.args {
                    arg.collect_free_vars(vars);
                }
            }
            Self::Nominal(n) => n.inner.collect_free_vars(vars),
            Self::Forall(f) => {
                // Bound variables are not free
                let mut body_vars = Vec::new();
                f.body.collect_free_vars(&mut body_vars);
                for var in body_vars {
                    if !f.vars.contains(&var) {
                        vars.push(var);
                    }
                }
            }
            _ => {}
        }
    }

    /// Substitute type variables with other types.
    #[must_use]
    pub fn substitute(&self, subst: &HashMap<TypeVarId, Type>) -> Type {
        match self {
            Self::Var(TypeVar::Unbound(id)) => {
                subst.get(id).cloned().unwrap_or_else(|| self.clone())
            }
            Self::Var(TypeVar::Link(link)) => link.borrow().substitute(subst),
            Self::Tuple(elems) => {
                Self::Tuple(elems.iter().map(|e| e.substitute(subst)).collect())
            }
            Self::Record(rec) => Self::Record(RecordType::new(
                rec.fields
                    .iter()
                    .map(|(n, t)| (n.clone(), t.substitute(subst)))
                    .collect(),
            )),
            Self::Function(f) => Self::Function(FunctionType::new(
                f.params.iter().map(|p| p.substitute(subst)).collect(),
                f.ret.substitute(subst),
            )),
            Self::Named(n) => Self::Named(NamedType::new(
                n.name.clone(),
                n.args.iter().map(|a| a.substitute(subst)).collect(),
            )),
            Self::Nominal(n) => Self::Nominal(NominalType::new(
                n.uuid,
                n.inner.substitute(subst),
                n.name.clone(),
            )),
            Self::Forall(f) => {
                // Don't substitute bound variables
                let mut new_subst = subst.clone();
                for var in &f.vars {
                    new_subst.remove(var);
                }
                Self::Forall(ForallType::new(f.vars.clone(), f.body.substitute(&new_subst)))
            }
            _ => self.clone(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Display implementations for pretty printing
// ─────────────────────────────────────────────────────────────────────────────

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit => write!(f, "()"),
            Self::Bool => write!(f, "bool"),
            Self::Number => write!(f, "number"),
            Self::String => write!(f, "string"),
            Self::Never => write!(f, "!"),
            Self::Error => write!(f, "<error>"),

            Self::Var(TypeVar::Unbound(id)) => write!(f, "'{id}"),
            Self::Var(TypeVar::Link(link)) => write!(f, "{}", link.borrow()),

            Self::Tuple(elems) => {
                write!(f, "(")?;
                for (i, elem) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{elem}")?;
                }
                write!(f, ")")
            }

            Self::Record(rec) => {
                write!(f, "{{ ")?;
                for (i, (name, ty)) in rec.fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{name}: {ty}")?;
                }
                write!(f, " }}")
            }

            Self::Function(func) => {
                write!(f, "(")?;
                for (i, param) in func.params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{param}")?;
                }
                write!(f, ") -> {}", func.ret)
            }

            Self::Named(named) => {
                write!(f, "{}", named.name)?;
                if !named.args.is_empty() {
                    write!(f, "<")?;
                    for (i, arg) in named.args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{arg}")?;
                    }
                    write!(f, ">")?;
                }
                Ok(())
            }

            Self::Nominal(nom) => {
                if let Some(name) = &nom.name {
                    write!(f, "{name}")
                } else {
                    write!(f, "unique({})", nom.uuid)
                }
            }

            Self::Forall(forall) => {
                write!(f, "forall ")?;
                for (i, var) in forall.vars.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "'{var}")?;
                }
                write!(f, ". {}", forall.body)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_primitive_types_display() {
        assert_eq!(Type::Unit.to_string(), "()");
        assert_eq!(Type::Bool.to_string(), "bool");
        assert_eq!(Type::Number.to_string(), "number");
        assert_eq!(Type::String.to_string(), "string");
        assert_eq!(Type::Never.to_string(), "!");
    }

    #[test]
    fn test_tuple_type_display() {
        let tuple = Type::tuple(vec![Type::Number, Type::String]);
        assert_eq!(tuple.to_string(), "(number, string)");
    }

    #[test]
    fn test_record_type_display() {
        let record = Type::record([("x", Type::Number), ("y", Type::Number)]);
        assert_eq!(record.to_string(), "{ x: number, y: number }");
    }

    #[test]
    fn test_function_type_display() {
        let func = Type::function(vec![Type::Number, Type::Number], Type::Number);
        assert_eq!(func.to_string(), "(number, number) -> number");
    }

    #[test]
    fn test_named_type_display() {
        let list = Type::named("List", vec![Type::Number]);
        assert_eq!(list.to_string(), "List<number>");

        let map = Type::named("Map", vec![Type::String, Type::Number]);
        assert_eq!(map.to_string(), "Map<string, number>");
    }

    #[test]
    fn test_type_var_display() {
        let var = Type::var(0);
        assert_eq!(var.to_string(), "'0");
    }

    #[test]
    fn test_forall_type_display() {
        let forall = Type::forall(
            vec![0, 1],
            Type::function(vec![Type::var(0)], Type::var(1)),
        );
        assert_eq!(forall.to_string(), "forall '0 '1. ('0) -> '1");
    }

    #[test]
    fn test_type_var_generator() {
        let mut gen = TypeVarGen::new();
        let v1 = gen.fresh();
        let v2 = gen.fresh();
        let v3 = gen.fresh();

        assert_eq!(v1, Type::var(0));
        assert_eq!(v2, Type::var(1));
        assert_eq!(v3, Type::var(2));
    }

    #[test]
    fn test_record_field_access() {
        let record = if let Type::Record(rec) = Type::record([
            ("x", Type::Number),
            ("y", Type::String),
        ]) {
            rec
        } else {
            panic!("Expected record type");
        };

        assert_eq!(record.get_field("x"), Some(&Type::Number));
        assert_eq!(record.get_field("y"), Some(&Type::String));
        assert_eq!(record.get_field("z"), None);
    }

    #[test]
    fn test_free_vars() {
        let t = Type::function(vec![Type::var(0)], Type::var(1));
        let vars = t.free_vars();
        assert_eq!(vars, vec![0, 1]);
    }

    #[test]
    fn test_free_vars_in_forall() {
        // forall '0. ('0 -> '1) should have '1 free, '0 bound
        let t = Type::forall(
            vec![0],
            Type::function(vec![Type::var(0)], Type::var(1)),
        );
        let vars = t.free_vars();
        assert_eq!(vars, vec![1]);
    }

    #[test]
    fn test_substitute() {
        let t = Type::function(vec![Type::var(0)], Type::var(1));
        let mut subst = HashMap::new();
        subst.insert(0, Type::Number);
        subst.insert(1, Type::String);

        let result = t.substitute(&subst);
        assert_eq!(result, Type::function(vec![Type::Number], Type::String));
    }

    #[test]
    fn test_is_concrete() {
        assert!(Type::Number.is_concrete());
        assert!(Type::function(vec![Type::Number], Type::String).is_concrete());
        assert!(!Type::var(0).is_concrete());
        assert!(!Type::function(vec![Type::var(0)], Type::Number).is_concrete());
    }

    #[test]
    fn test_nominal_type_inequality() {
        let uuid1 = Uuid::new_v4();
        let uuid2 = Uuid::new_v4();

        let nominal1 = Type::nominal(uuid1, Type::String, Some("UserId"));
        let nominal2 = Type::nominal(uuid2, Type::String, Some("OrderId"));

        // Same structure, different UUIDs -> different types
        assert_ne!(nominal1, nominal2);
    }

    #[test]
    fn test_nominal_type_equality() {
        let uuid = Uuid::new_v4();

        let nominal1 = Type::nominal(uuid, Type::String, Some("UserId"));
        let nominal2 = Type::nominal(uuid, Type::String, Some("UserId"));

        // Same UUID -> same type
        assert_eq!(nominal1, nominal2);
    }
}
