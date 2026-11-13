//! Relations between types: subtyping, unions, disjointness. See
//! `spec/types.md`.
//!
//! The checker works on resolved surface types. A name in one is either a
//! type parameter or a recursive type definition; a non-recursive definition
//! is transparent and is replaced by its expansion before anything looks at
//! it.

mod lower;
mod operations;
mod patterns;
mod relate;
mod show;

use std::collections::BTreeMap;

use crate::literal::Literal;
use crate::surface::{Key, Reference, Shape, Type};
use crate::symbol::Symbol;

pub type Ty = Type<Reference>;

#[derive(Clone, Debug)]
pub struct TypeInfo {
    /// How many type parameters the definition takes.
    pub params: usize,
    pub ty: Ty,
    /// Whether the definition can reach itself.
    pub recursive: bool,
}

#[derive(Clone, Debug)]
pub struct Signature {
    /// The bound of each type parameter; `unknown` where none was written.
    pub type_params: Vec<Ty>,
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// The abilities it takes from its caller.
    pub row: Vec<Symbol>,
    /// Whether this is a native handler, which only a target may name.
    pub native: bool,
}

/// An ability, as a perform or a handler sees it.
#[derive(Clone, Debug)]
pub struct AbilityInfo {
    /// The package that declares it and its tag: its identity apart from
    /// its signature, and what a row is ordered by.
    pub tag: (String, String),
    pub params: Vec<Ty>,
    pub ret: Ty,
}

/// A key, as a record that holds a value under it sees it.
#[derive(Clone, Debug)]
pub struct KeyInfo {
    /// The package that declares it and its tag: its identity apart from
    /// its type, and what a record's keys are ordered by.
    pub tag: (String, String),
    /// The type of what a record holds under it.
    pub ty: Ty,
}

/// What has methods that are not fields: a built-in shape, whose
/// methods are its module of the standard library, or a type
/// definition, whose methods are the module it is defined in. See
/// `spec/methods.md`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Receiver {
    Shape(Shape),
    Type(Symbol),
}

/// Everything about other definitions that checking one definition reads.
#[derive(Debug, Default)]
pub struct Env {
    /// Every type definition reachable from the definition being checked.
    pub types: BTreeMap<Symbol, TypeInfo>,
    /// The header of every function it refers to.
    pub functions: BTreeMap<Symbol, Signature>,
    /// Every ability it refers to, or that a type it reaches does.
    pub abilities: BTreeMap<Symbol, AbilityInfo>,
    /// Every key it refers to, or that a type it reaches does.
    pub keys: BTreeMap<Symbol, KeyInfo>,
    /// The function each method it may call by name means, for each
    /// receiver that has one of that name.
    pub methods: BTreeMap<(Receiver, String), Symbol>,
    /// The bound of each type parameter of the definition being checked.
    pub bounds: Vec<Ty>,
}

/// `ty` with each type parameter replaced by the argument in its position.
#[must_use]
pub fn substitute(ty: &Ty, args: &[Ty]) -> Ty {
    let all = |types: &[Ty]| types.iter().map(|ty| substitute(ty, args)).collect();
    match ty {
        Ty::Never
        | Ty::Unknown
        | Ty::Null
        | Ty::Bool
        | Ty::Int
        | Ty::Float
        | Ty::String
        | Ty::Tag
        | Ty::Literal(_)
        | Ty::Named(Reference::Local(_), _) => ty.clone(),
        Ty::Record(fields) => Ty::Record(
            fields
                .iter()
                .map(|(key, ty)| (key.clone(), substitute(ty, args)))
                .collect(),
        ),
        Ty::Tuple(members) => Ty::Tuple(all(members)),
        Ty::Union(members) => Ty::Union(all(members)),
        Ty::List(member) => Ty::List(Box::new(substitute(member, args))),
        Ty::Cell(member) => Ty::Cell(Box::new(substitute(member, args))),
        Ty::Method { needs, function } => Ty::Method {
            needs: Box::new(substitute(needs, args)),
            function: Box::new(substitute(function, args)),
        },
        Ty::Function { params, ret, row } => Ty::Function {
            params: all(params),
            ret: Box::new(substitute(ret, args)),
            row: row.clone(),
        },
        Ty::Named(Reference::Parameter(position), _) => {
            args.get(*position).cloned().unwrap_or(Ty::Never)
        }
        Ty::Named(name, inner) => Ty::Named(name.clone(), all(inner)),
    }
}

impl Ty {
    /// The type of what reading a record's key of this type gives: for
    /// a method, the function bound to the record.
    #[must_use]
    pub fn read(&self) -> Self {
        match self {
            Self::Method { function, .. } => (**function).clone(),
            other => other.clone(),
        }
    }
}

fn primitive_of(literal: &Literal) -> Ty {
    match literal {
        Literal::Bool(_) => Ty::Bool,
        Literal::Int(_) => Ty::Int,
        Literal::Float(_) => Ty::Float,
        Literal::String(_) => Ty::String,
        Literal::Tag { .. } => Ty::Tag,
    }
}

fn booleans() -> Ty {
    Ty::Union(vec![
        Ty::Literal(Literal::Bool(false)),
        Ty::Literal(Literal::Bool(true)),
    ])
}

/// Whether a type mentions a type parameter of the definition it is
/// written in.
fn mentions_parameter(ty: &Ty) -> bool {
    let any = |types: &[Ty]| types.iter().any(mentions_parameter);
    match ty {
        Ty::Named(Reference::Parameter(_), _) => true,
        Ty::Record(fields) => fields.iter().any(|(_, ty)| mentions_parameter(ty)),
        Ty::Tuple(members) | Ty::Union(members) | Ty::Named(_, members) => any(members),
        Ty::List(member) | Ty::Cell(member) => mentions_parameter(member),
        Ty::Method { needs, function } => mentions_parameter(needs) || mentions_parameter(function),
        Ty::Function { params, ret, .. } => any(params) || mentions_parameter(ret),
        _ => false,
    }
}

impl Env {
    fn recursive(&self, symbol: &Symbol) -> Option<&TypeInfo> {
        self.types.get(symbol).filter(|info| info.recursive)
    }

    /// Whether a value can be tested against a type as a program runs.
    /// A function says which type it is of and no more, so a function
    /// type in it must be one type whatever the type arguments in
    /// force: it may mention no type parameter. Inside a cell type
    /// anything may be, since a cell keeps the type it was made at.
    #[must_use]
    pub fn testable(&self, ty: &Ty) -> bool {
        self.tests(ty, &mut Vec::new())
    }

    /// [`Self::testable`], given the definitions already looked into.
    fn tests<'a>(&'a self, ty: &'a Ty, seen: &mut Vec<&'a Symbol>) -> bool {
        match ty {
            Ty::Function { .. } => !mentions_parameter(ty),
            // A test never yields a method: see `spec/methods.md`.
            Ty::Method { .. } => false,
            Ty::Record(fields) => fields.iter().all(|(_, ty)| self.tests(ty, seen)),
            Ty::Tuple(members) | Ty::Union(members) => {
                members.iter().all(|member| self.tests(member, seen))
            }
            Ty::List(member) => self.tests(member, seen),
            Ty::Named(name, args) => {
                let definition = match name {
                    Reference::Definition(symbol) if !seen.contains(&symbol) => {
                        seen.push(symbol);
                        self.types.get(symbol)
                    }
                    _ => None,
                };
                args.iter().all(|arg| self.tests(arg, seen))
                    && definition.is_none_or(|info| self.tests(&info.ty, seen))
            }
            _ => true,
        }
    }

    /// The type in the form every other operation expects: transparent
    /// names expanded, record fields ordered, unions simplified.
    #[must_use]
    pub fn normalize(&self, ty: &Ty) -> Ty {
        self.normal(ty, true)
    }

    /// [`Self::normalize`], with unions simplified only if `absorb`.
    fn normal(&self, ty: &Ty, absorb: bool) -> Ty {
        match ty {
            Ty::Never
            | Ty::Unknown
            | Ty::Null
            | Ty::Bool
            | Ty::Int
            | Ty::Float
            | Ty::String
            | Ty::Tag
            | Ty::Literal(_)
            | Ty::Named(Reference::Local(_) | Reference::Parameter(_), _) => ty.clone(),
            Ty::Record(fields) => {
                let mut fields: Vec<_> = fields
                    .iter()
                    .map(|(key, ty)| (key.clone(), self.normal(ty, absorb)))
                    .collect();
                fields.sort_by(|(a, _), (b, _)| a.cmp(b));
                Ty::Record(fields)
            }
            Ty::Tuple(members) => {
                Ty::Tuple(members.iter().map(|m| self.normal(m, absorb)).collect())
            }
            Ty::List(member) => Ty::List(Box::new(self.normal(member, absorb))),
            Ty::Cell(member) => Ty::Cell(Box::new(self.normal(member, absorb))),
            Ty::Method { needs, function } => Ty::Method {
                needs: Box::new(self.normal(needs, absorb)),
                function: Box::new(self.normal(function, absorb)),
            },
            Ty::Function { params, ret, row } => {
                let mut row = row.clone();
                row.sort();
                row.dedup();
                Ty::Function {
                    params: params.iter().map(|p| self.normal(p, absorb)).collect(),
                    ret: Box::new(self.normal(ret, absorb)),
                    row,
                }
            }
            Ty::Union(members) => {
                let members = members.iter().map(|m| self.normal(m, absorb)).collect();
                self.join(members, absorb)
            }
            Ty::Named(Reference::Definition(symbol), args) => {
                let args: Vec<Ty> = args.iter().map(|arg| self.normal(arg, absorb)).collect();
                match self.types.get(symbol) {
                    Some(info) if !info.recursive => {
                        self.normal(&substitute(&info.ty, &args), absorb)
                    }
                    _ => Ty::Named(Reference::Definition(symbol.clone()), args),
                }
            }
        }
    }

    /// A normalized type with any recursive name at its head replaced by
    /// its definition, so its outermost constructor can be inspected.
    ///
    /// The unions of the definition are not simplified: simplifying asks
    /// whether one member is a subtype of another, which unfolds, and a
    /// definition that holds a union with itself in it would never be done.
    #[must_use]
    pub fn unfold(&self, ty: &Ty) -> Ty {
        let mut ty = ty.clone();
        // A contractive type reaches a constructor within as many steps as
        // there are definitions.
        for _ in 0..=self.types.len() {
            let Ty::Named(Reference::Definition(symbol), args) = &ty else {
                break;
            };
            let Some(info) = self.recursive(symbol) else {
                break;
            };
            ty = self.normal(&substitute(&info.ty, args), false);
        }
        ty
    }

    /// The union of normalized types, simplified: nested unions flattened,
    /// members that another member already covers dropped.
    #[must_use]
    pub fn union(&self, members: Vec<Ty>) -> Ty {
        self.join(members, true)
    }

    /// [`Self::union`], dropping covered members only if `absorb`.
    fn join(&self, members: Vec<Ty>, absorb: bool) -> Ty {
        let mut flat = Vec::new();
        for member in members {
            match member {
                Ty::Union(inner) => flat.extend(inner),
                Ty::Never => {}
                other => flat.push(other),
            }
        }
        let mut kept: Vec<Ty> = Vec::new();
        for member in flat {
            if !absorb {
                kept.push(member);
                continue;
            }
            if kept.iter().any(|other| self.subtype(&member, other)) {
                continue;
            }
            kept.retain(|other| !self.subtype(other, &member));
            kept.push(member);
        }
        let boolean = |value| Ty::Literal(Literal::Bool(value));
        if kept.contains(&boolean(true)) && kept.contains(&boolean(false)) {
            kept.retain(|member| !matches!(member, Ty::Literal(Literal::Bool(_))));
            kept.push(Ty::Bool);
        }
        kept.sort();
        kept.dedup();
        match kept.len() {
            0 => Ty::Never,
            1 => kept.remove(0),
            _ => Ty::Union(kept),
        }
    }

    /// The type as an error message shows it. This is not a syntax.
    #[must_use]
    pub fn show(&self, ty: &Ty) -> String {
        let mut out = String::new();
        show::show(ty, &mut out);
        out
    }
}

/// What a normalized record type is keyed by.
pub type TyKey = Key<Reference>;

/// The type of the field `name` among `fields`.
#[must_use]
pub fn field<'a>(fields: &'a [(TyKey, Ty)], name: &str) -> Option<&'a Ty> {
    fields
        .iter()
        .find(|(key, _)| key.is_field(name))
        .map(|(_, ty)| ty)
}

/// The keys among `fields` that hold a method, in the order a core
/// tree lists keys.
#[must_use]
pub fn methods(env: &Env, fields: &[(TyKey, Ty)]) -> Vec<TyKey> {
    let held = env.ordered(fields).into_iter();
    held.filter(|(_, ty)| matches!(ty, Ty::Method { .. }))
        .map(|(key, _)| key.clone())
        .collect()
}

/// The type under `key` among `fields`.
#[must_use]
pub fn entry<'a>(fields: &'a [(TyKey, Ty)], key: &TyKey) -> Option<&'a Ty> {
    fields
        .iter()
        .find(|(found, _)| found == key)
        .map(|(_, ty)| ty)
}

/// How far each ability's handler is from needing nothing: `0` if its row
/// is empty, and otherwise one more than the furthest ability in its row.
/// `rows` holds the row of the handler bound to each ability; an ability
/// in a row but not in `rows` counts for nothing. `None` if a handler
/// needs itself, however indirectly.
#[must_use]
pub fn layers(rows: &BTreeMap<Symbol, Vec<Symbol>>) -> Option<BTreeMap<Symbol, usize>> {
    fn layer(
        ability: &Symbol,
        rows: &BTreeMap<Symbol, Vec<Symbol>>,
        done: &mut BTreeMap<Symbol, usize>,
        visiting: &mut Vec<Symbol>,
    ) -> Option<usize> {
        if let Some(known) = done.get(ability) {
            return Some(*known);
        }
        if visiting.contains(ability) {
            return None;
        }
        visiting.push(ability.clone());
        let mut depth = 0;
        for needed in rows.get(ability).into_iter().flatten() {
            if rows.contains_key(needed) {
                depth = depth.max(layer(needed, rows, done, visiting)? + 1);
            }
        }
        visiting.pop();
        done.insert(ability.clone(), depth);
        Some(depth)
    }
    let mut done = BTreeMap::new();
    for ability in rows.keys() {
        layer(ability, rows, &mut done, &mut Vec::new())?;
    }
    Some(done)
}

/// The abilities a normalized row names.
#[must_use]
pub fn abilities(row: &[Reference]) -> Vec<Symbol> {
    row.iter()
        .filter_map(|ability| match ability {
            Reference::Definition(symbol) => Some(symbol.clone()),
            Reference::Local(_) | Reference::Parameter(_) => None,
        })
        .collect()
}

/// What a trace is: a list of frames, each a definition's hash and a
/// position in the definition. See `spec/errors.md`.
#[must_use]
pub fn trace() -> Ty {
    let field = |name: &str, ty| (Key::Field(name.to_owned()), ty);
    let frame = Ty::Record(vec![
        field("definition", Ty::String),
        field("position", Ty::Int),
    ]);
    Ty::List(Box::new(frame))
}

/// What a boundary gives for a fault: an error, as `spec/errors.md`
/// shapes one.
#[must_use]
pub fn error() -> Ty {
    let field = |name: &str, ty| (Key::Field(name.to_owned()), ty);
    Ty::Record(vec![
        field("kind", Ty::Tag),
        field("message", Ty::String),
        field("cause", Ty::Null),
        field("trace", trace()),
    ])
}
