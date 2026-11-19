//! Resolving a type: every name in one replaced by what it refers to.

use super::Resolver;
use crate::error::Error;
use crate::surface::{Reference, Type};
use crate::symbol::Namespace;

impl Resolver<'_> {
    pub(super) fn types(&self, types: &[Type]) -> Result<Vec<Type<Reference>>, Error> {
        types.iter().map(|ty| self.ty(ty)).collect()
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
            Type::Literal(literal) => Type::Literal(literal.clone()),
            Type::Brand(brand) => Type::Brand(brand.clone()),
            Type::Branded(ty, brands) => Type::Branded(Box::new(self.ty(ty)?), self.types(brands)?),
            Type::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|(key, ty)| Ok((self.key(key)?, self.ty(ty)?)))
                    .collect::<Result<_, Error>>()?,
            ),
            Type::Tuple(members) => Type::Tuple(self.types(members)?),
            Type::List(member) => Type::List(Box::new(self.ty(member)?)),
            Type::Cell(member) => Type::Cell(Box::new(self.ty(member)?)),
            Type::Iterator(member) => Type::Iterator(Box::new(self.ty(member)?)),
            Type::Method { needs, function } => Type::Method {
                needs: Box::new(self.ty(needs)?),
                function: Box::new(self.ty(function)?),
            },
            Type::Function { params, ret, row } => Type::Function {
                params: self.types(params)?,
                ret: Box::new(self.ty(ret)?),
                row: self.row(row)?,
            },
            Type::Union(members) => Type::Union(self.types(members)?),
            Type::Bound(index) => Type::Bound(*index),
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
