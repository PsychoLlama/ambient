//! Builders: the first interface to the language. See `spec/trees.md`.
//!
//! Expressions and types are made with the free functions here and in
//! [`ty`]. They are assembled into definitions, and definitions into a
//! module. [`ModuleBuilder::build`] is where an ill-formed tree is rejected;
//! type errors are left to the checker.

use std::collections::BTreeSet;
use std::fmt;

use crate::literal::Literal;
use crate::surface::{
    Arm, Definition, DefinitionKind, Expr, Function, Import, Module, Operator, Param, Path,
    Pattern, TypeDefinition, TypeParam,
};
use crate::symbol::Namespace;

/// The first segment of a path into the module's own package.
pub const OWN_PACKAGE: &str = "pkg";

/// Why a builder refused a tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildError {
    /// Not an identifier: empty, or contains something other than letters,
    /// digits, and underscores, or starts with a digit.
    InvalidName(String),
    /// A module defines a name twice in one namespace, or imports two
    /// things under one alias.
    DuplicateDefinition(String),
    DuplicateTypeParameter(String),
    DuplicateParameter {
        function: String,
        name: String,
    },
    DuplicateField(String),
    /// A comptime call must be a call to a function by name.
    ComptimeNotCall,
    /// One pattern binds a name twice.
    DuplicateBinder(String),
    /// An import must name a package and at least one thing inside it.
    ImportTooShort(Path),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(f, "`{name}` is not a name"),
            Self::DuplicateDefinition(name) => write!(f, "`{name}` is defined twice"),
            Self::DuplicateParameter { function, name } => {
                write!(f, "`{function}` has two parameters named `{name}`")
            }
            Self::DuplicateTypeParameter(name) => {
                write!(f, "two type parameters are named `{name}`")
            }
            Self::DuplicateField(name) => write!(f, "a record has two fields named `{name}`"),
            Self::ComptimeNotCall => {
                write!(f, "a comptime call must call a function by name")
            }
            Self::DuplicateBinder(name) => write!(f, "a pattern binds `{name}` twice"),
            Self::ImportTooShort(path) => write!(f, "import `{path}` names only a package"),
        }
    }
}

impl std::error::Error for BuildError {}

/// Type constructors.
pub mod ty {
    use crate::literal::Literal;
    use crate::surface::{Path, Type};

    #[must_use]
    pub fn never() -> Type {
        Type::Never
    }

    #[must_use]
    pub fn unknown() -> Type {
        Type::Unknown
    }

    /// The type of one boolean.
    #[must_use]
    pub fn boolean(value: bool) -> Type {
        Type::Literal(Literal::Bool(value))
    }

    /// The type of one integer.
    #[must_use]
    pub fn integer(value: i64) -> Type {
        Type::Literal(Literal::Int(value))
    }

    /// The type of one string.
    #[must_use]
    pub fn text(value: &str) -> Type {
        Type::Literal(Literal::String(value.to_owned()))
    }

    pub fn tuple(members: impl IntoIterator<Item = Type>) -> Type {
        Type::Tuple(members.into_iter().collect())
    }

    #[must_use]
    pub fn list(member: Type) -> Type {
        Type::List(Box::new(member))
    }

    pub fn union(members: impl IntoIterator<Item = Type>) -> Type {
        Type::Union(members.into_iter().collect())
    }

    #[must_use]
    pub fn null() -> Type {
        Type::Null
    }

    #[must_use]
    pub fn bool() -> Type {
        Type::Bool
    }

    #[must_use]
    pub fn int() -> Type {
        Type::Int
    }

    #[must_use]
    pub fn float() -> Type {
        Type::Float
    }

    #[must_use]
    pub fn string() -> Type {
        Type::String
    }

    pub fn record<'a>(fields: impl IntoIterator<Item = (&'a str, Type)>) -> Type {
        Type::Record(
            fields
                .into_iter()
                .map(|(name, ty)| (name.to_owned(), ty))
                .collect(),
        )
    }

    pub fn function(params: impl IntoIterator<Item = Type>, ret: Type) -> Type {
        Type::Function {
            params: params.into_iter().collect(),
            ret: Box::new(ret),
        }
    }

    /// A type named by a dotted path, such as `"Point"` or `"geometry.Point"`.
    #[must_use]
    pub fn named(path: &str) -> Type {
        Type::Named(Path::parse(path), Vec::new())
    }

    /// A generic type applied to arguments.
    pub fn apply(path: &str, args: impl IntoIterator<Item = Type>) -> Type {
        Type::Named(Path::parse(path), args.into_iter().collect())
    }
}

#[must_use]
pub fn null() -> Expr {
    Expr::Null
}

#[must_use]
pub fn boolean(value: bool) -> Expr {
    Expr::Literal(Literal::Bool(value))
}

#[must_use]
pub fn int(value: i64) -> Expr {
    Expr::Literal(Literal::Int(value))
}

#[must_use]
pub fn float(value: f64) -> Expr {
    Expr::Literal(Literal::float(value))
}

#[must_use]
pub fn string(value: &str) -> Expr {
    Expr::Literal(Literal::String(value.to_owned()))
}

pub fn tuple(members: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::Tuple(members.into_iter().collect())
}

pub fn list(members: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::List(members.into_iter().collect())
}

fn binary(operator: Operator, left: Expr, right: Expr) -> Expr {
    Expr::Binary {
        operator,
        left: Box::new(left),
        right: Box::new(right),
    }
}

#[must_use]
pub fn add(left: Expr, right: Expr) -> Expr {
    binary(Operator::Add, left, right)
}

#[must_use]
pub fn subtract(left: Expr, right: Expr) -> Expr {
    binary(Operator::Subtract, left, right)
}

#[must_use]
pub fn multiply(left: Expr, right: Expr) -> Expr {
    binary(Operator::Multiply, left, right)
}

#[must_use]
pub fn less(left: Expr, right: Expr) -> Expr {
    binary(Operator::Less, left, right)
}

#[must_use]
pub fn equal(left: Expr, right: Expr) -> Expr {
    binary(Operator::Equal, left, right)
}

/// Takes the first arm whose pattern matches `subject`.
pub fn match_(subject: Expr, arms: impl IntoIterator<Item = (Pattern, Expr)>) -> Expr {
    Expr::Match {
        subject: Box::new(subject),
        arms: arms
            .into_iter()
            .map(|(pattern, body)| Arm { pattern, body })
            .collect(),
    }
}

/// A function written in place. A parameter given no type takes it from
/// where the lambda is used.
pub fn lambda<'a>(
    params: impl IntoIterator<Item = (&'a str, Option<crate::surface::Type>)>,
    body: Expr,
) -> Expr {
    Expr::Lambda {
        params: params
            .into_iter()
            .map(|(name, ty)| (name.to_owned(), ty))
            .collect(),
        body: Box::new(body),
    }
}

/// Runs `call`, a call to a named function, during compilation, and
/// stands for its result.
#[must_use]
pub fn comptime(call: Expr) -> Expr {
    Expr::Comptime(Box::new(call))
}

/// `value` at type `ty` if the type describes it, and `otherwise` if not.
#[must_use]
pub fn cast(value: Expr, ty: crate::surface::Type, otherwise: Expr) -> Expr {
    Expr::Cast {
        value: Box::new(value),
        ty,
        otherwise: Box::new(otherwise),
    }
}

/// Pattern constructors.
pub mod pat {
    use crate::literal::Literal;
    use crate::surface::{Pattern, Type};

    /// Matches anything.
    #[must_use]
    pub fn any() -> Pattern {
        Pattern::Wildcard
    }

    /// Matches anything and binds it.
    #[must_use]
    pub fn bind(name: &str) -> Pattern {
        Pattern::Bind(name.to_owned(), Box::new(Pattern::Wildcard))
    }

    /// Matches what `pattern` matches and binds it.
    #[must_use]
    pub fn bind_as(name: &str, pattern: Pattern) -> Pattern {
        Pattern::Bind(name.to_owned(), Box::new(pattern))
    }

    #[must_use]
    pub fn boolean(value: bool) -> Pattern {
        Pattern::Literal(Literal::Bool(value))
    }

    #[must_use]
    pub fn int(value: i64) -> Pattern {
        Pattern::Literal(Literal::Int(value))
    }

    #[must_use]
    pub fn string(value: &str) -> Pattern {
        Pattern::Literal(Literal::String(value.to_owned()))
    }

    /// Matches a value the type describes.
    #[must_use]
    pub fn is(ty: Type) -> Pattern {
        Pattern::Is(ty)
    }

    /// Matches a record with at least these fields.
    pub fn record<'a>(fields: impl IntoIterator<Item = (&'a str, Pattern)>) -> Pattern {
        Pattern::Record(
            fields
                .into_iter()
                .map(|(name, pattern)| (name.to_owned(), pattern))
                .collect(),
        )
    }

    /// Matches a tuple of exactly this length.
    pub fn tuple(members: impl IntoIterator<Item = Pattern>) -> Pattern {
        Pattern::Tuple(members.into_iter().collect())
    }
}

/// A local or a definition named by a dotted path, such as `"x"` or
/// `"math.add"`.
#[must_use]
pub fn name(path: &str) -> Expr {
    Expr::Name(Path::parse(path))
}

/// A generic function with its type arguments written out.
pub fn instantiate(path: &str, args: impl IntoIterator<Item = crate::surface::Type>) -> Expr {
    Expr::Instantiate {
        function: Path::parse(path),
        args: args.into_iter().collect(),
    }
}

pub fn call(callee: Expr, args: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::Call {
        callee: Box::new(callee),
        args: args.into_iter().collect(),
    }
}

pub fn record<'a>(fields: impl IntoIterator<Item = (&'a str, Expr)>) -> Expr {
    Expr::Record(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

#[must_use]
pub fn field(target: Expr, name: &str) -> Expr {
    Expr::Field {
        target: Box::new(target),
        name: name.to_owned(),
    }
}

/// Binds `name` to `value` for the extent of `body`.
#[must_use]
pub fn let_(name: &str, value: Expr, body: Expr) -> Expr {
    Expr::Let {
        name: name.to_owned(),
        value: Box::new(value),
        body: Box::new(body),
    }
}

/// Starts a function. With nothing else said it is private, takes no
/// parameters, and returns `null`.
#[must_use]
pub fn function(name: &str) -> FunctionBuilder {
    FunctionBuilder {
        name: name.to_owned(),
        public: false,
        doc: None,
        function: Function {
            type_params: Vec::new(),
            params: Vec::new(),
            ret: ty::null(),
            body: null(),
        },
    }
}

/// A private type definition.
#[must_use]
pub fn type_(name: &str, ty: crate::surface::Type) -> Definition {
    Definition {
        name: name.to_owned(),
        public: false,
        doc: None,
        kind: DefinitionKind::Type(TypeDefinition {
            params: Vec::new(),
            ty,
        }),
    }
}

/// A private type definition with type parameters.
pub fn generic_type<'a>(
    name: &str,
    params: impl IntoIterator<Item = &'a str>,
    ty: crate::surface::Type,
) -> Definition {
    Definition {
        name: name.to_owned(),
        public: false,
        doc: None,
        kind: DefinitionKind::Type(TypeDefinition {
            params: params.into_iter().map(str::to_owned).collect(),
            ty,
        }),
    }
}

impl Definition {
    /// Makes the definition visible throughout its package.
    #[must_use]
    pub fn public(mut self) -> Self {
        self.public = true;
        self
    }

    #[must_use]
    pub fn doc(mut self, doc: &str) -> Self {
        self.doc = Some(doc.to_owned());
        self
    }
}

#[derive(Clone, Debug)]
pub struct FunctionBuilder {
    name: String,
    public: bool,
    doc: Option<String>,
    function: Function,
}

impl FunctionBuilder {
    #[must_use]
    pub fn public(mut self) -> Self {
        self.public = true;
        self
    }

    #[must_use]
    pub fn doc(mut self, doc: &str) -> Self {
        self.doc = Some(doc.to_owned());
        self
    }

    /// Adds a type parameter with no bound.
    #[must_use]
    pub fn type_param(mut self, name: &str) -> Self {
        self.function.type_params.push(TypeParam {
            name: name.to_owned(),
            bound: None,
        });
        self
    }

    /// Adds a type parameter that must be a subtype of `bound`.
    #[must_use]
    pub fn bounded(mut self, name: &str, bound: crate::surface::Type) -> Self {
        self.function.type_params.push(TypeParam {
            name: name.to_owned(),
            bound: Some(bound),
        });
        self
    }

    #[must_use]
    pub fn param(mut self, name: &str, ty: crate::surface::Type) -> Self {
        self.function.params.push(Param {
            name: name.to_owned(),
            ty,
        });
        self
    }

    #[must_use]
    pub fn returns(mut self, ty: crate::surface::Type) -> Self {
        self.function.ret = ty;
        self
    }

    /// Finishes the function with its body.
    #[must_use]
    pub fn body(mut self, body: Expr) -> Definition {
        self.function.body = body;
        Definition {
            name: self.name,
            public: self.public,
            doc: self.doc,
            kind: DefinitionKind::Function(self.function),
        }
    }
}

#[must_use]
pub fn module() -> ModuleBuilder {
    ModuleBuilder::default()
}

#[derive(Clone, Debug, Default)]
pub struct ModuleBuilder {
    imports: Vec<Import>,
    definitions: Vec<Definition>,
}

impl ModuleBuilder {
    /// Imports a definition or a module by its full path, under its last
    /// segment: `"pkg.util.math"`, `"http.Request"`.
    #[must_use]
    pub fn import(self, path: &str) -> Self {
        let alias = path.rsplit('.').next().unwrap_or(path).to_owned();
        self.import_as(path, &alias)
    }

    #[must_use]
    pub fn import_as(mut self, path: &str, alias: &str) -> Self {
        self.imports.push(Import {
            path: Path::parse(path),
            alias: alias.to_owned(),
        });
        self
    }

    #[must_use]
    pub fn define(mut self, definition: Definition) -> Self {
        self.definitions.push(definition);
        self
    }

    /// Finishes the module.
    ///
    /// # Errors
    ///
    /// Returns the first way in which the module is ill-formed.
    pub fn build(self) -> Result<Module, BuildError> {
        let mut aliases = BTreeSet::new();
        for import in &self.imports {
            if import.path.0.len() < 2 {
                return Err(BuildError::ImportTooShort(import.path.clone()));
            }
            check_path(&import.path)?;
            check_name(&import.alias)?;
            if !aliases.insert(&import.alias) {
                return Err(BuildError::DuplicateDefinition(import.alias.clone()));
            }
        }

        let mut defined = BTreeSet::new();
        for definition in &self.definitions {
            check_name(&definition.name)?;
            let namespace = match &definition.kind {
                DefinitionKind::Type(definition) => {
                    check_type_params(definition.params.iter())?;
                    check_type(&definition.ty)?;
                    Namespace::Type
                }
                DefinitionKind::Function(function) => {
                    check_function(&definition.name, function)?;
                    Namespace::Value
                }
            };
            if !defined.insert((namespace, &definition.name)) {
                return Err(BuildError::DuplicateDefinition(definition.name.clone()));
            }
        }

        Ok(Module {
            imports: self.imports,
            definitions: self.definitions,
        })
    }
}

fn check_name(name: &str) -> Result<(), BuildError> {
    let mut chars = name.chars();
    let starts = chars
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_');
    if starts && chars.all(|c| c.is_alphanumeric() || c == '_') {
        Ok(())
    } else {
        Err(BuildError::InvalidName(name.to_owned()))
    }
}

fn check_path(path: &Path) -> Result<(), BuildError> {
    path.0.iter().try_for_each(|segment| check_name(segment))
}

fn check_fields<'a>(names: impl Iterator<Item = &'a String>) -> Result<(), BuildError> {
    let mut seen = BTreeSet::new();
    for name in names {
        check_name(name)?;
        if !seen.insert(name) {
            return Err(BuildError::DuplicateField(name.clone()));
        }
    }
    Ok(())
}

fn check_type_params<'a>(names: impl Iterator<Item = &'a String>) -> Result<(), BuildError> {
    let mut seen = BTreeSet::new();
    for name in names {
        check_name(name)?;
        if !seen.insert(name) {
            return Err(BuildError::DuplicateTypeParameter(name.clone()));
        }
    }
    Ok(())
}

fn check_function(name: &str, function: &Function) -> Result<(), BuildError> {
    check_type_params(function.type_params.iter().map(|param| &param.name))?;
    for bound in function.type_params.iter().filter_map(|p| p.bound.as_ref()) {
        check_type(bound)?;
    }
    let mut seen = BTreeSet::new();
    for param in &function.params {
        check_name(&param.name)?;
        check_type(&param.ty)?;
        if !seen.insert(&param.name) {
            return Err(BuildError::DuplicateParameter {
                function: name.to_owned(),
                name: param.name.clone(),
            });
        }
    }
    check_type(&function.ret)?;
    check_expr(&function.body)
}

fn check_type(ty: &crate::surface::Type) -> Result<(), BuildError> {
    use crate::surface::Type;
    match ty {
        Type::Never
        | Type::Unknown
        | Type::Null
        | Type::Bool
        | Type::Int
        | Type::Float
        | Type::String
        | Type::Literal(_) => Ok(()),
        Type::Record(fields) => {
            check_fields(fields.iter().map(|(name, _)| name))?;
            fields.iter().try_for_each(|(_, ty)| check_type(ty))
        }
        Type::Tuple(members) | Type::Union(members) => members.iter().try_for_each(check_type),
        Type::List(member) => check_type(member),
        Type::Function { params, ret } => {
            params.iter().try_for_each(check_type)?;
            check_type(ret)
        }
        Type::Named(path, args) => {
            check_path(path)?;
            args.iter().try_for_each(check_type)
        }
    }
}

fn check_pattern<'a>(
    pattern: &'a Pattern,
    bound: &mut BTreeSet<&'a String>,
) -> Result<(), BuildError> {
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) => Ok(()),
        Pattern::Record(fields) => {
            check_fields(fields.iter().map(|(name, _)| name))?;
            fields
                .iter()
                .try_for_each(|(_, pattern)| check_pattern(pattern, bound))
        }
        Pattern::Tuple(members) => members
            .iter()
            .try_for_each(|member| check_pattern(member, bound)),
        Pattern::Is(ty) => check_type(ty),
        Pattern::Bind(name, inner) => {
            check_name(name)?;
            if !bound.insert(name) {
                return Err(BuildError::DuplicateBinder(name.clone()));
            }
            check_pattern(inner, bound)
        }
    }
}

fn check_expr(expr: &Expr) -> Result<(), BuildError> {
    match expr {
        Expr::Null | Expr::Literal(_) => Ok(()),
        Expr::Name(path) => check_path(path),
        Expr::Instantiate { function, args } => {
            check_path(function)?;
            args.iter().try_for_each(check_type)
        }
        Expr::Call { callee, args } => {
            check_expr(callee)?;
            args.iter().try_for_each(check_expr)
        }
        Expr::Record(fields) => {
            check_fields(fields.iter().map(|(name, _)| name))?;
            fields.iter().try_for_each(|(_, value)| check_expr(value))
        }
        Expr::Tuple(members) | Expr::List(members) => members.iter().try_for_each(check_expr),
        Expr::Field { target, name } => {
            check_name(name)?;
            check_expr(target)
        }
        Expr::Let { name, value, body } => {
            check_name(name)?;
            check_expr(value)?;
            check_expr(body)
        }
        Expr::Binary { left, right, .. } => {
            check_expr(left)?;
            check_expr(right)
        }
        Expr::Match { subject, arms } => {
            check_expr(subject)?;
            arms.iter().try_for_each(|arm| {
                check_pattern(&arm.pattern, &mut BTreeSet::new())?;
                check_expr(&arm.body)
            })
        }
        Expr::Lambda { params, body } => {
            let mut seen = BTreeSet::new();
            for (name, ty) in params {
                check_name(name)?;
                if !seen.insert(name) {
                    return Err(BuildError::DuplicateBinder(name.clone()));
                }
                if let Some(ty) = ty {
                    check_type(ty)?;
                }
            }
            check_expr(body)
        }
        Expr::Comptime(call) => {
            let named =
                matches!(&**call, Expr::Call { callee, .. } if matches!(**callee, Expr::Name(_)));
            if !named {
                return Err(BuildError::ComptimeNotCall);
            }
            check_expr(call)
        }
        Expr::Cast {
            value,
            ty,
            otherwise,
        } => {
            check_expr(value)?;
            check_type(ty)?;
            check_expr(otherwise)
        }
    }
}
