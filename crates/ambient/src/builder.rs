//! Builders: the first interface to the language. See `spec/trees.md`.
//!
//! Expressions and types are made with the free functions here and in
//! [`ty`]. They are assembled into definitions, and definitions into a
//! module. [`ModuleBuilder::build`] is where an ill-formed tree is rejected;
//! type errors are left to the checker.

use std::collections::BTreeSet;
use std::fmt;

use crate::surface::{Definition, DefinitionKind, Expr, Function, Import, Module, Param, Path};
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
    DuplicateParameter {
        function: String,
        name: String,
    },
    DuplicateField(String),
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
            Self::DuplicateField(name) => write!(f, "a record has two fields named `{name}`"),
            Self::ImportTooShort(path) => write!(f, "import `{path}` names only a package"),
        }
    }
}

impl std::error::Error for BuildError {}

/// Type constructors.
pub mod ty {
    use crate::surface::{Path, Type};

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
        Type::Named(Path::parse(path))
    }
}

#[must_use]
pub fn null() -> Expr {
    Expr::Null
}

#[must_use]
pub fn boolean(value: bool) -> Expr {
    Expr::Bool(value)
}

#[must_use]
pub fn int(value: i64) -> Expr {
    Expr::Int(value)
}

#[must_use]
pub fn float(value: f64) -> Expr {
    Expr::Float(value)
}

#[must_use]
pub fn string(value: &str) -> Expr {
    Expr::String(value.to_owned())
}

/// A local or a definition named by a dotted path, such as `"x"` or
/// `"math.add"`.
#[must_use]
pub fn name(path: &str) -> Expr {
    Expr::Name(Path::parse(path))
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
        kind: DefinitionKind::Type(ty),
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
                DefinitionKind::Type(ty) => {
                    check_type(ty)?;
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

fn check_function(name: &str, function: &Function) -> Result<(), BuildError> {
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
        Type::Null | Type::Bool | Type::Int | Type::Float | Type::String => Ok(()),
        Type::Record(fields) => {
            check_fields(fields.iter().map(|(name, _)| name))?;
            fields.iter().try_for_each(|(_, ty)| check_type(ty))
        }
        Type::Function { params, ret } => {
            params.iter().try_for_each(check_type)?;
            check_type(ret)
        }
        Type::Named(path) => check_path(path),
    }
}

fn check_expr(expr: &Expr) -> Result<(), BuildError> {
    match expr {
        Expr::Null | Expr::Bool(_) | Expr::Int(_) | Expr::Float(_) | Expr::String(_) => Ok(()),
        Expr::Name(path) => check_path(path),
        Expr::Call { callee, args } => {
            check_expr(callee)?;
            args.iter().try_for_each(check_expr)
        }
        Expr::Record(fields) => {
            check_fields(fields.iter().map(|(name, _)| name))?;
            fields.iter().try_for_each(|(_, value)| check_expr(value))
        }
        Expr::Field { target, name } => {
            check_name(name)?;
            check_expr(target)
        }
        Expr::Let { name, value, body } => {
            check_name(name)?;
            check_expr(value)?;
            check_expr(body)
        }
    }
}
