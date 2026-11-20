//! Constants. See `docs/spec/constants.md`.

use super::definition::{FunctionBuilder, function};
use crate::surface::{Definition, DefinitionKind, Expr};

/// Starts a constant of type `ty`. With nothing else said it is
/// private. See `docs/spec/constants.md`.
#[must_use]
pub fn constant(name: &str, ty: crate::surface::Type) -> ConstantBuilder {
    ConstantBuilder(function(name).returns(ty))
}

/// A constant under construction: a function of no parameters, which
/// is how a constant is held.
#[derive(Clone, Debug)]
pub struct ConstantBuilder(FunctionBuilder);

impl ConstantBuilder {
    #[must_use]
    pub fn public(self) -> Self {
        Self(self.0.public())
    }

    #[must_use]
    pub fn doc(self, doc: &str) -> Self {
        Self(self.0.doc(doc))
    }

    /// Finishes the constant with its value.
    #[must_use]
    pub fn value(self, value: Expr) -> Definition {
        let mut definition = self.0.body(value);
        if let DefinitionKind::Function(function) = definition.kind {
            definition.kind = DefinitionKind::Constant(function);
        }
        definition
    }
}
