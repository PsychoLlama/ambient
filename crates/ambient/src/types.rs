//! Relations between types: subtyping, unions, disjointness. See
//! `spec/types.md`.
//!
//! The checker works on resolved surface types. A name in one is either a
//! type parameter or a recursive type definition; a non-recursive definition
//! is transparent and is replaced by its expansion before anything looks at
//! it.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::literal::Literal;
use crate::surface::{Reference, Type};
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
}

/// Everything about other definitions that checking one definition reads.
#[derive(Debug, Default)]
pub struct Env {
    /// Every type definition reachable from the definition being checked.
    pub types: BTreeMap<Symbol, TypeInfo>,
    /// The header of every function it refers to.
    pub functions: BTreeMap<Symbol, Signature>,
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
        | Ty::Literal(_)
        | Ty::Named(Reference::Local(_), _) => ty.clone(),
        Ty::Record(fields) => Ty::Record(
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), substitute(ty, args)))
                .collect(),
        ),
        Ty::Tuple(members) => Ty::Tuple(all(members)),
        Ty::Union(members) => Ty::Union(all(members)),
        Ty::List(member) => Ty::List(Box::new(substitute(member, args))),
        Ty::Function { params, ret } => Ty::Function {
            params: all(params),
            ret: Box::new(substitute(ret, args)),
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
            | Ty::Literal(_)
            | Ty::Named(Reference::Local(_) | Reference::Parameter(_), _) => ty.clone(),
            Ty::Record(fields) => {
                let mut fields: Vec<_> = fields
                    .iter()
                    .map(|(name, ty)| (name.clone(), self.normalize(ty)))
                    .collect();
                fields.sort_by(|(a, _), (b, _)| a.cmp(b));
                Ty::Record(fields)
            }
            Ty::Tuple(members) => Ty::Tuple(members.iter().map(|m| self.normalize(m)).collect()),
            Ty::List(member) => Ty::List(Box::new(self.normalize(member))),
            Ty::Function { params, ret } => Ty::Function {
                params: params.iter().map(|p| self.normalize(p)).collect(),
                ret: Box::new(self.normalize(ret)),
            },
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
            (Ty::Record(have), Ty::Record(want)) => want.iter().all(|(name, want)| {
                field(have, name).is_some_and(|have| self.sub(have, want, assumed))
            }),
            (Ty::Tuple(have), Ty::Tuple(want)) => {
                have.len() == want.len()
                    && have.iter().zip(want).all(|(h, w)| self.sub(h, w, assumed))
            }
            (Ty::Tuple(have), Ty::List(want)) => have.iter().all(|h| self.sub(h, want, assumed)),
            (Ty::List(have), Ty::List(want)) => self.sub(have, want, assumed),
            (
                Ty::Function { params, ret },
                Ty::Function {
                    params: want_params,
                    ret: want_ret,
                },
            ) => {
                params.len() == want_params.len()
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
                .any(|(name, a)| field(b, name).is_some_and(|b| self.apart(a, b, next))),
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
        show(ty, &mut out);
        out
    }
}

/// The type of the field `name` among `fields`.
#[must_use]
pub fn field<'a>(fields: &'a [(String, Ty)], name: &str) -> Option<&'a Ty> {
    fields
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, ty)| ty)
}

fn show_all(types: &[Ty], separator: &str, out: &mut String) {
    for (index, ty) in types.iter().enumerate() {
        if index > 0 {
            out.push_str(separator);
        }
        show(ty, out);
    }
}

fn show(ty: &Ty, out: &mut String) {
    match ty {
        Ty::Never => out.push_str("never"),
        Ty::Unknown => out.push_str("unknown"),
        Ty::Null => out.push_str("null"),
        Ty::Bool => out.push_str("bool"),
        Ty::Int => out.push_str("int"),
        Ty::Float => out.push_str("float"),
        Ty::String => out.push_str("string"),
        Ty::Literal(literal) => {
            let _ = write!(out, "{literal}");
        }
        Ty::Record(fields) => {
            out.push('{');
            for (index, (name, ty)) in fields.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "{name}: ");
                show(ty, out);
            }
            out.push('}');
        }
        Ty::Tuple(members) => {
            out.push('(');
            show_all(members, ", ", out);
            out.push(')');
        }
        Ty::List(member) => {
            out.push_str("List<");
            show(member, out);
            out.push('>');
        }
        Ty::Function { params, ret } => {
            out.push_str("Fn(");
            show_all(params, ", ", out);
            out.push_str("): ");
            show(ret, out);
        }
        Ty::Union(members) => show_all(members, " | ", out),
        Ty::Named(Reference::Local(number) | Reference::Parameter(number), _) => {
            let _ = write!(out, "T{number}");
        }
        Ty::Named(Reference::Definition(symbol), args) => {
            out.push_str(&symbol.name);
            if !args.is_empty() {
                out.push('<');
                show_all(args, ", ", out);
                out.push('>');
            }
        }
    }
}
