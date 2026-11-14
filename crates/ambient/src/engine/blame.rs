//! Where in a definition an error is: which node of its body, counted
//! in the order [`Expr::children`] walks them. A reader that kept where
//! it found each node (`syntax::Placed`) turns that into a line.

use super::Engine;
use crate::error::Error;
use crate::surface::{DefinitionKind, Expr, Key, Path};

impl Engine<'_> {
    /// The node of its definition's body that `error` is at, if it is
    /// at one. An error about a name is at the first node that names
    /// it; any other is where checking found it.
    #[must_use]
    pub fn blame(&self, error: &Error) -> Option<usize> {
        let (Error::UnknownName { symbol, path } | Error::PrivateName { symbol, path }) = error
        else {
            return self.blamed.borrow().get(error.symbol()?).copied();
        };
        let module = self.read(&symbol.module).ok()?;
        let entry = module
            .table
            .0
            .get(&(symbol.namespace, symbol.name.clone()))?;
        let DefinitionKind::Function(function) = &module.definitions[entry.index].kind else {
            return None;
        };
        let mut passed = 0;
        naming(&function.body, path, &mut passed).then_some(passed)
    }
}

/// Whether a node of `expr` names `path` itself, having counted the
/// nodes before the first that does into `passed`.
fn naming(expr: &Expr, path: &Path, passed: &mut usize) -> bool {
    let keyed = |key: &Key| matches!(key, Key::Ability(named) if named == path);
    let names = match expr {
        Expr::Name(named)
        | Expr::AbilityId(named)
        | Expr::Perform { ability: named, .. }
        | Expr::Instantiate {
            function: named, ..
        } => named == path,
        Expr::Field { key, .. } | Expr::Send { method: key, .. } => keyed(key),
        Expr::Record(fields) => fields.iter().any(|(key, _)| keyed(key)),
        _ => false,
    };
    if names {
        return true;
    }
    *passed += 1;
    let mut found = false;
    expr.children(&mut |child| found = found || naming(child, path, passed));
    found
}
