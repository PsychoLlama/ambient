//! Whether a type that was written is one at all: its definitions
//! applied to as many arguments as they take, only brands put on it,
//! and what it holds under each key allowed there.

use super::Checker;
use super::header::visit_types;
use crate::error::Error;
use crate::surface::Reference;
use crate::types::{Ty, Unbounded};

impl Checker<'_> {
    /// Fails if a type applies a definition to the wrong number of
    /// arguments, or cannot be tested for as a program runs.
    pub(super) fn tested(&self, ty: &Ty) -> Result<(), Error> {
        self.well_formed(ty)?;
        if self.env.testable(ty) {
            return Ok(());
        }
        Err(Error::Untestable {
            symbol: self.symbol.clone(),
            found: self.env.show(ty),
        })
    }

    /// Fails if a type applies a definition to the wrong number of
    /// arguments, or to one outside the bound of its parameter.
    pub(super) fn well_formed(&self, ty: &Ty) -> Result<(), Error> {
        let mut result = Ok(());
        visit_types(ty, &mut |ty| {
            if let Ty::Record(fields) = ty {
                let held = fields.iter().try_for_each(|(key, ty)| self.held(key, ty));
                result = std::mem::replace(&mut result, Ok(())).and(held);
            }
            // Records intersect to a record; what is left of one that
            // is still an intersection has only brands on it.
            if let (Ty::Branded(..), Ty::Branded(under, brands)) = (ty, self.env.normalize(ty)) {
                let brand = |ty: &Ty| matches!(ty, Ty::Brand(_));
                if brand(&under) || !brands.iter().all(brand) {
                    result = Err(Error::Intersection {
                        symbol: self.symbol.clone(),
                    });
                }
            }
            let Ty::Named(name, args) = ty else { return };
            let expected = match name {
                Reference::Definition(symbol) => {
                    self.env.types.get(symbol).map(|info| info.bounds.len())
                }
                Reference::Local(_) | Reference::Parameter(_) => Some(0),
            };
            if let Some(expected) = expected
                && expected != args.len()
            {
                result = Err(Error::TypeArguments {
                    symbol: self.symbol.clone(),
                    expected,
                    found: args.len(),
                });
            }
        });
        result?;
        match self.env.unbounded(ty) {
            Some(Unbounded { bound, found }) => Err(self.mismatch(&bound, &found)),
            None => Ok(()),
        }
    }
}
