//! Relations between types: subtyping, unions, disjointness. See
//! `spec/types.md`.
//!
//! The checker works on resolved surface types. A name in one is either a
//! type parameter or a recursive type definition; a non-recursive definition
//! is transparent and is replaced by its expansion before anything looks at
//! it.

mod show;

use std::collections::BTreeMap;

use crate::literal::Literal;
use crate::surface::{Key, Reference, Type};
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

/// Everything about other definitions that checking one definition reads.
#[derive(Debug, Default)]
pub struct Env {
    /// Every type definition reachable from the definition being checked.
    pub types: BTreeMap<Symbol, TypeInfo>,
    /// The header of every function it refers to.
    pub functions: BTreeMap<Symbol, Signature>,
    /// Every ability it refers to, or that a type it reaches does.
    pub abilities: BTreeMap<Symbol, AbilityInfo>,
    /// The bound of each type parameter of the definition being checked.
    pub bounds: Vec<Ty>,
}

/// The longest chain of assumptions a subtyping proof may build. Regular
/// types never come near it; it is what makes an irregular one fail rather
/// than run forever.
const DEPTH: usize = 256;

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

impl Env {
    fn recursive(&self, symbol: &Symbol) -> Option<&TypeInfo> {
        self.types.get(symbol).filter(|info| info.recursive)
    }

    /// The type in the form every other operation expects: transparent
    /// names expanded, record fields ordered, unions simplified.
    #[must_use]
    pub fn normalize(&self, ty: &Ty) -> Ty {
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
                    .map(|(key, ty)| (key.clone(), self.normalize(ty)))
                    .collect();
                fields.sort_by(|(a, _), (b, _)| a.cmp(b));
                Ty::Record(fields)
            }
            Ty::Tuple(members) => Ty::Tuple(members.iter().map(|m| self.normalize(m)).collect()),
            Ty::List(member) => Ty::List(Box::new(self.normalize(member))),
            Ty::Cell(member) => Ty::Cell(Box::new(self.normalize(member))),
            Ty::Function { params, ret, row } => {
                let mut row = row.clone();
                row.sort();
                row.dedup();
                Ty::Function {
                    params: params.iter().map(|p| self.normalize(p)).collect(),
                    ret: Box::new(self.normalize(ret)),
                    row,
                }
            }
            Ty::Union(members) => self.union(members.iter().map(|m| self.normalize(m)).collect()),
            Ty::Named(Reference::Definition(symbol), args) => {
                let args: Vec<Ty> = args.iter().map(|arg| self.normalize(arg)).collect();
                match self.types.get(symbol) {
                    Some(info) if !info.recursive => self.normalize(&substitute(&info.ty, &args)),
                    _ => Ty::Named(Reference::Definition(symbol.clone()), args),
                }
            }
        }
    }

    /// A normalized type with any recursive name at its head replaced by
    /// its definition, so its outermost constructor can be inspected.
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
            ty = self.normalize(&substitute(&info.ty, args));
        }
        ty
    }

    /// The union of normalized types, simplified: nested unions flattened,
    /// members that another member already covers dropped.
    #[must_use]
    pub fn union(&self, members: Vec<Ty>) -> Ty {
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
        match kept.len() {
            0 => Ty::Never,
            1 => kept.remove(0),
            _ => Ty::Union(kept),
        }
    }

    /// Whether every value `sub` describes is one `sup` describes. Both are
    /// normalized.
    #[must_use]
    pub fn subtype(&self, sub: &Ty, sup: &Ty) -> bool {
        self.sub(sub, sup, &mut Vec::new())
    }

    /// `assumed` holds the judgements this one is being proved under.
    /// Meeting one again proves it: subtyping is coinductive.
    fn sub(&self, sub: &Ty, sup: &Ty, assumed: &mut Vec<(Ty, Ty)>) -> bool {
        if sub == sup {
            return true;
        }
        match (sub, sup) {
            (Ty::Never, _) | (_, Ty::Unknown) => true,
            (Ty::Named(Reference::Definition(_), _), _)
            | (_, Ty::Named(Reference::Definition(_), _)) => {
                let judgement = (sub.clone(), sup.clone());
                if assumed.contains(&judgement) {
                    return true;
                }
                if assumed.len() >= DEPTH {
                    return false;
                }
                let (below, above) = (self.unfold(sub), self.unfold(sup));
                if below == *sub && above == *sup {
                    return false;
                }
                let depth = assumed.len();
                assumed.push(judgement);
                let holds = self.sub(&below, &above, assumed);
                if !holds {
                    // A judgement that failed must not stay assumed.
                    assumed.truncate(depth);
                }
                holds
            }
            (Ty::Union(members), _) => members.iter().all(|m| self.sub(m, sup, assumed)),
            // A type parameter is known only by its bound.
            (Ty::Named(Reference::Parameter(position), _), _) => {
                let member = matches!(sup, Ty::Union(members) if members.contains(sub));
                member
                    || self
                        .bounds
                        .get(*position)
                        .is_some_and(|bound| self.sub(bound, sup, assumed))
            }
            (Ty::Bool, Ty::Union(_)) => self.sub(&booleans(), sup, assumed),
            (_, Ty::Union(members)) => members.iter().any(|m| self.sub(sub, m, assumed)),
            (Ty::Literal(literal), _) => primitive_of(literal) == *sup,
            (Ty::Record(have), Ty::Record(want)) => want.iter().all(|(key, want)| {
                entry(have, key).is_some_and(|have| self.sub(have, want, assumed))
            }),
            (Ty::Tuple(have), Ty::Tuple(want)) => {
                have.len() == want.len()
                    && have.iter().zip(want).all(|(h, w)| self.sub(h, w, assumed))
            }
            (Ty::Tuple(have), Ty::List(want)) => have.iter().all(|h| self.sub(h, want, assumed)),
            (Ty::List(have), Ty::List(want)) => self.sub(have, want, assumed),
            // A cell is written as well as read, so it is the same type
            // or it is not a subtype.
            (Ty::Cell(have), Ty::Cell(want)) => {
                self.sub(have, want, assumed) && self.sub(want, have, assumed)
            }
            (
                Ty::Function { params, ret, row },
                Ty::Function {
                    params: want_params,
                    ret: want_ret,
                    row: want_row,
                },
            ) => {
                // A function that takes less from its caller can stand in
                // for one that takes more.
                row.iter().all(|ability| want_row.contains(ability))
                    && params.len() == want_params.len()
                    && want_params
                        .iter()
                        .zip(params)
                        .all(|(want, have)| self.sub(want, have, assumed))
                    && self.sub(ret, want_ret, assumed)
            }
            _ => false,
        }
    }

    /// Whether no value is described by both types. Types overlap unless
    /// this proves otherwise.
    #[must_use]
    pub fn disjoint(&self, left: &Ty, right: &Ty) -> bool {
        self.apart(left, right, 0)
    }

    fn apart(&self, left: &Ty, right: &Ty, depth: usize) -> bool {
        // Past any recursive type's own depth nothing new can be learned;
        // answering "may overlap" is always safe.
        if depth > self.types.len() + 8 {
            return false;
        }
        let (left, right) = (self.unfold(left), self.unfold(right));
        let next = depth + 1;
        match (&left, &right) {
            (Ty::Never, _) | (_, Ty::Never) => true,
            // Two list types share the empty list; two function types may
            // share a function.
            (Ty::Named(Reference::Parameter(position), _), other)
            | (other, Ty::Named(Reference::Parameter(position), _)) => self
                .bounds
                .get(*position)
                .is_some_and(|bound| self.apart(bound, other, next)),
            (Ty::Unknown | Ty::Named(..), _)
            | (_, Ty::Unknown | Ty::Named(..))
            | (Ty::List(_), Ty::List(_))
            | (Ty::Function { .. }, Ty::Function { .. }) => false,
            (Ty::Union(members), other) | (other, Ty::Union(members)) => {
                members.iter().all(|m| self.apart(m, other, next))
            }
            (Ty::Literal(a), Ty::Literal(b)) => a != b,
            (Ty::Literal(literal), other) | (other, Ty::Literal(literal)) => {
                primitive_of(literal) != *other
            }
            (Ty::Record(a), Ty::Record(b)) => a
                .iter()
                .any(|(key, a)| entry(b, key).is_some_and(|b| self.apart(a, b, next))),
            (Ty::Tuple(a), Ty::Tuple(b)) => {
                a.len() != b.len() || a.iter().zip(b).any(|(a, b)| self.apart(a, b, next))
            }
            (Ty::Tuple(members), Ty::List(member)) | (Ty::List(member), Ty::Tuple(members)) => {
                members.iter().any(|m| self.apart(m, member, next))
            }
            _ => left != right,
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
