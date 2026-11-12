//! Surface trees: what an author writes. See `spec/trees.md`.
//!
//! The trees are generic over how they name things. As authored, a name is a
//! [`Path`]; after resolution it is a [`Reference`]. Nothing else differs
//! between the two, so they share one shape.
//!
//! These types are plain data that anything may read. A [`Module`] can only
//! be made by a [`crate::builder`], which is what rejects ill-formed trees.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::literal::Literal;
use crate::symbol::Symbol;

/// A dotted name as written: `x`, `math.add`, `pkg.util.math`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Path(pub Vec<String>);

impl Path {
    /// Splits a dotted name into its segments.
    #[must_use]
    pub fn parse(dotted: &str) -> Self {
        Self(dotted.split('.').map(str::to_owned).collect())
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.join("."))
    }
}

/// What a name means once resolved.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Reference {
    /// A parameter or local binder, numbered in binding order within its
    /// function.
    Local(usize),
    /// A type parameter of the enclosing definition, by position.
    Parameter(usize),
    Definition(Symbol),
}

/// A file of definitions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Module {
    pub(crate) imports: Vec<Import>,
    pub(crate) definitions: Vec<Definition>,
}

impl Module {
    #[must_use]
    pub fn imports(&self) -> &[Import] {
        &self.imports
    }

    #[must_use]
    pub fn definitions(&self) -> &[Definition] {
        &self.definitions
    }
}

/// Brings one definition or one module into scope under `alias`.
///
/// Whether the path's last segment is a module or a definition is decided by
/// resolution, not by the import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    pub path: Path,
    pub alias: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition<N = Path> {
    pub name: String,
    pub public: bool,
    pub doc: Option<String>,
    pub kind: DefinitionKind<N>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefinitionKind<N = Path> {
    Type(TypeDefinition<N>),
    Function(Function<N>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDefinition<N = Path> {
    /// The names of its type parameters.
    pub params: Vec<String>,
    pub ty: Type<N>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeParam<N = Path> {
    pub name: String,
    /// An upper bound on what the parameter may be instantiated with.
    pub bound: Option<Type<N>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function<N = Path> {
    pub type_params: Vec<TypeParam<N>>,
    pub params: Vec<Param<N>>,
    pub ret: Type<N>,
    pub body: Expr<N>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param<N = Path> {
    pub name: String,
    pub ty: Type<N>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Type<N = Path> {
    Never,
    Unknown,
    Null,
    Bool,
    Int,
    Float,
    String,
    Literal(Literal),
    Record(Vec<(String, Type<N>)>),
    Tuple(Vec<Type<N>>),
    List(Box<Type<N>>),
    Function {
        params: Vec<Type<N>>,
        ret: Box<Type<N>>,
    },
    Union(Vec<Type<N>>),
    /// A type parameter, or a type definition applied to arguments.
    Named(N, Vec<Type<N>>),
}

/// An operator. Which primitive operation it means depends on its operands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operator {
    Add,
    Subtract,
    Multiply,
    Less,
    Equal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pattern<N = Path> {
    /// Matches anything.
    Wildcard,
    Literal(Literal),
    /// Matches a record that has at least these fields, each matching.
    Record(Vec<(String, Pattern<N>)>),
    /// Matches a tuple of exactly this length, each member matching.
    Tuple(Vec<Pattern<N>>),
    /// Matches a value the type describes.
    Is(Type<N>),
    /// Matches what the inner pattern matches, and binds it to a name.
    Bind(String, Box<Pattern<N>>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arm<N = Path> {
    pub pattern: Pattern<N>,
    pub body: Expr<N>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr<N = Path> {
    Null,
    Literal(Literal),
    Name(N),
    /// A generic function with its type arguments written out.
    Instantiate {
        function: N,
        args: Vec<Type<N>>,
    },
    Call {
        callee: Box<Expr<N>>,
        args: Vec<Expr<N>>,
    },
    Record(Vec<(String, Expr<N>)>),
    Tuple(Vec<Expr<N>>),
    List(Vec<Expr<N>>),
    Field {
        target: Box<Expr<N>>,
        name: String,
    },
    /// Binds `name` to `value` for the extent of `body`.
    Let {
        name: String,
        value: Box<Expr<N>>,
        body: Box<Expr<N>>,
    },
    Binary {
        operator: Operator,
        left: Box<Expr<N>>,
        right: Box<Expr<N>>,
    },
    /// Takes the first arm whose pattern matches the subject.
    Match {
        subject: Box<Expr<N>>,
        arms: Vec<Arm<N>>,
    },
    /// A function written in place. A parameter without a type takes it
    /// from where the lambda is used.
    Lambda {
        params: Vec<(String, Option<Type<N>>)>,
        body: Box<Expr<N>>,
    },
    /// `value` at type `ty` if the type describes it, and `otherwise` if not.
    Cast {
        value: Box<Expr<N>>,
        ty: Type<N>,
        otherwise: Box<Expr<N>>,
    },
}

impl<N> Type<N> {
    /// Calls `visit` with every name in the type, in order.
    pub fn names<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        match self {
            Self::Never
            | Self::Unknown
            | Self::Null
            | Self::Bool
            | Self::Int
            | Self::Float
            | Self::String
            | Self::Literal(_) => {}
            Self::Record(fields) => fields.iter().for_each(|(_, ty)| ty.names(visit)),
            Self::Tuple(members) | Self::Union(members) => {
                for ty in members {
                    ty.names(visit);
                }
            }
            Self::List(member) => member.names(visit),
            Self::Function { params, ret } => {
                for ty in params {
                    ty.names(visit);
                }
                ret.names(visit);
            }
            Self::Named(name, args) => {
                visit(name);
                for arg in args {
                    arg.names(visit);
                }
            }
        }
    }
}

impl<N> Pattern<N> {
    /// Calls `visit` with every name in the pattern's types, in order.
    pub fn names<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        match self {
            Self::Wildcard | Self::Literal(_) => {}
            Self::Record(fields) => fields.iter().for_each(|(_, p)| p.names(visit)),
            Self::Tuple(members) => members.iter().for_each(|p| p.names(visit)),
            Self::Is(ty) => ty.names(visit),
            Self::Bind(_, inner) => inner.names(visit),
        }
    }
}

impl<N> Expr<N> {
    /// Calls `visit` with every name in the expression, in order.
    pub fn names<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        match self {
            Self::Null | Self::Literal(_) => {}
            Self::Name(name) => visit(name),
            Self::Instantiate { function, args } => {
                visit(function);
                for arg in args {
                    arg.names(visit);
                }
            }
            Self::Call { callee, args } => {
                callee.names(visit);
                for arg in args {
                    arg.names(visit);
                }
            }
            Self::Record(fields) => fields.iter().for_each(|(_, value)| value.names(visit)),
            Self::Tuple(members) | Self::List(members) => {
                for member in members {
                    member.names(visit);
                }
            }
            Self::Field { target, .. } => target.names(visit),
            Self::Let { value, body, .. } => {
                value.names(visit);
                body.names(visit);
            }
            Self::Binary { left, right, .. } => {
                left.names(visit);
                right.names(visit);
            }
            Self::Match { subject, arms } => {
                subject.names(visit);
                for arm in arms {
                    arm.pattern.names(visit);
                    arm.body.names(visit);
                }
            }
            Self::Lambda { params, body } => {
                for ty in params.iter().filter_map(|(_, ty)| ty.as_ref()) {
                    ty.names(visit);
                }
                body.names(visit);
            }
            Self::Cast {
                value,
                ty,
                otherwise,
            } => {
                value.names(visit);
                ty.names(visit);
                otherwise.names(visit);
            }
        }
    }
}

impl<N> Pattern<N> {
    /// How many names the pattern binds.
    #[must_use]
    pub fn binders(&self) -> usize {
        match self {
            Self::Wildcard | Self::Literal(_) | Self::Is(_) => 0,
            Self::Record(fields) => fields.iter().map(|(_, p)| p.binders()).sum(),
            Self::Tuple(members) => members.iter().map(Self::binders).sum(),
            Self::Bind(_, inner) => 1 + inner.binders(),
        }
    }
}

impl<N> Expr<N> {
    /// How many names are bound anywhere inside the expression. Locals are
    /// numbered in binding order, so this is how many numbers it uses.
    #[must_use]
    pub fn binders(&self) -> usize {
        let all = |exprs: &[Self]| exprs.iter().map(Self::binders).sum::<usize>();
        match self {
            Self::Null | Self::Literal(_) | Self::Name(_) | Self::Instantiate { .. } => 0,
            Self::Call { callee, args } => callee.binders() + all(args),
            Self::Record(fields) => fields.iter().map(|(_, value)| value.binders()).sum(),
            Self::Tuple(members) | Self::List(members) => all(members),
            Self::Field { target, .. } => target.binders(),
            Self::Let { value, body, .. } => 1 + value.binders() + body.binders(),
            Self::Binary { left, right, .. } => left.binders() + right.binders(),
            Self::Match { subject, arms } => {
                let arms: usize = arms
                    .iter()
                    .map(|arm| arm.pattern.binders() + arm.body.binders())
                    .sum();
                subject.binders() + arms
            }
            Self::Lambda { params, body } => params.len() + body.binders(),
            Self::Cast {
                value, otherwise, ..
            } => value.binders() + otherwise.binders(),
        }
    }
}

impl<N> Function<N> {
    /// Calls `visit` with every name in the header, in order.
    pub fn header_names<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        for bound in self.type_params.iter().filter_map(|p| p.bound.as_ref()) {
            bound.names(visit);
        }
        self.params.iter().for_each(|param| param.ty.names(visit));
        self.ret.names(visit);
    }
}

impl<N> Definition<N> {
    /// Calls `visit` with every name in the definition, in order.
    pub fn names<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        match &self.kind {
            DefinitionKind::Type(definition) => definition.ty.names(visit),
            DefinitionKind::Function(function) => {
                function.header_names(visit);
                function.body.names(visit);
            }
        }
    }
}
