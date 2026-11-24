//! Resolving a type: every name in one replaced by what it refers to.

use super::Resolver;
use crate::error::Error;
use crate::surface::{Pattern, Reference, Type, TypeMatch};
use crate::symbol::Namespace;

impl Resolver<'_> {
    pub(super) fn types(&self, types: &[Type]) -> Result<Vec<Type<Reference>>, Error> {
        types.iter().map(|ty| self.ty(ty)).collect()
    }

    /// A pattern of a type match resolved. Its binders bind no local:
    /// the arm's type names each by its place (`Type::Matched`).
    fn type_pattern(&self, pattern: &Pattern) -> Result<Pattern<Reference>, Error> {
        Ok(match pattern {
            Pattern::Wildcard => Pattern::Wildcard,
            Pattern::Literal(literal) => Pattern::Literal(literal.clone()),
            Pattern::Record(fields) => Pattern::Record(
                fields
                    .iter()
                    .map(|(name, pattern)| Ok((name.clone(), self.type_pattern(pattern)?)))
                    .collect::<Result<_, Error>>()?,
            ),
            Pattern::Tuple(members) => Pattern::Tuple(
                members
                    .iter()
                    .map(|member| self.type_pattern(member))
                    .collect::<Result<_, Error>>()?,
            ),
            Pattern::Is(ty) => Pattern::Is(self.ty(ty)?),
            Pattern::Bind(name, inner) => {
                Pattern::Bind(name.clone(), Box::new(self.type_pattern(inner)?))
            }
        })
    }

    pub(super) fn ty(&self, ty: &Type) -> Result<Type<Reference>, Error> {
        Ok(match ty {
            Type::Never => Type::Never,
            Type::Unknown => Type::Unknown,
            Type::Null => Type::Null,
            Type::Bool => Type::Bool,
            Type::Int => Type::Int,
            Type::Float => Type::Float,
            Type::String => Type::String,
            Type::Bytes => Type::Bytes,
            Type::Tag => Type::Tag,
            Type::Callable => Type::Callable,
            Type::Literal(literal) => Type::Literal(literal.clone()),
            Type::Brand(brand) => Type::Brand(brand.clone()),
            Type::Branded(ty, brands) => Type::Branded(Box::new(self.ty(ty)?), self.types(brands)?),
            Type::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|(key, ty)| Ok((self.key(key)?, self.ty(ty)?)))
                    .collect::<Result<_, Error>>()?,
            ),
            Type::Tuple(members) => Type::Tuple(members.try_each(|member| self.ty(member))?),
            Type::List(member) => Type::List(Box::new(self.ty(member)?)),
            Type::Cell(member) => Type::Cell(Box::new(self.ty(member)?)),
            Type::Iterator(member) => Type::Iterator(Box::new(self.ty(member)?)),
            Type::Method { needs, function } => Type::Method {
                needs: Box::new(self.ty(needs)?),
                function: Box::new(self.ty(function)?),
            },
            Type::Function { params, ret, row } => Type::Function {
                params: params.try_each(|param| self.ty(param))?,
                ret: Box::new(self.ty(ret)?),
                row: self.row(row)?,
            },
            Type::Union(members) => Type::Union(self.types(members)?),
            Type::Bound(index) => Type::Bound(*index),
            Type::Matched(index) => Type::Matched(*index),
            Type::Match(matching) => Type::Match(Box::new(TypeMatch {
                subject: self.ty(&matching.subject)?,
                arms: matching
                    .arms
                    .iter()
                    .map(|(pattern, ty)| Ok((self.type_pattern(pattern)?, self.ty(ty)?)))
                    .collect::<Result<_, Error>>()?,
            })),
            Type::Generic { bounds, function } => Type::Generic {
                bounds: self.types(bounds)?,
                function: Box::new(self.ty(function)?),
            },
            Type::Named(path, args) => {
                let parameter = match path.0.as_slice() {
                    [name] => self.type_params.iter().rposition(|param| param == name),
                    _ => None,
                };
                let reference = match parameter {
                    Some(position) => Reference::Parameter(position),
                    None => Reference::Definition(self.lookup(Namespace::Type, path)?),
                };
                Type::Named(reference, self.types(args)?)
            }
        })
    }
}
