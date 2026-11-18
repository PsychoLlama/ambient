//! An annotation on an expression: the expression, with something
//! said of it to a build. See `spec/annotations.md`.

use super::{Checker, core};
use crate::core::Annotation;
use crate::error::Error;
use crate::exchange;
use crate::literal::Literal;
use crate::surface::{Expr, Reference, data};
use crate::types::Ty;
use crate::value::Value;

impl Checker<'_> {
    /// `expr`, which is of the type it would be with nothing said of
    /// it, under what `key` and `value` say.
    pub(super) fn annotated(
        &mut self,
        key: &Reference,
        value: &Expr<Reference>,
        expr: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        let wrong = |key| Error::NotAnnotation {
            symbol: self.symbol.clone(),
            key,
        };
        let tag = match key {
            Reference::Definition(named) => self.tag(named),
            _ => None,
        };
        let Some(Literal::Tag { package, tag }) = tag else {
            return Err(wrong(true));
        };
        // A name is a tag, or else a definition, which data refers to.
        let named = |name: &Reference| match name {
            Reference::Definition(named) => Some(match self.tag(named) {
                Some(Literal::Tag { package, tag }) => Value::Tag { package, tag },
                _ => super::reference(named),
            }),
            _ => None,
        };
        let value = data(value, &named).and_then(|value| exchange::encode(&value).ok());
        let annotation = Annotation {
            package,
            tag,
            value: value.ok_or_else(|| wrong(false))?,
        };
        let (value, ty) = self.infer(expr)?;
        let value = Box::new(value);
        Ok((core::Expr::Annotated { annotation, value }, ty))
    }
}
