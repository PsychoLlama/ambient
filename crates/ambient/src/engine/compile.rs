//! Compile: a core tree, the interfaces of its slots, and a backend yield an
//! artifact. See `spec/build.md`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{Engine, Query, Stage, memo};
use crate::core::{Open, Tree};
use crate::error::Error;
use crate::hash::Hasher;
use crate::lower::Form;
use crate::symbol::Symbol;

/// Turns one function into code.
pub trait Backend {
    /// Distinguishes this backend's artifacts from every other's.
    fn name(&self) -> &str;

    /// Compiles one function.
    ///
    /// Slots are compiled as indirections: the result must not depend on
    /// what a slot's definition contains, only on its interface.
    ///
    /// # Errors
    ///
    /// Returns a message if the backend cannot compile the definition.
    ///
    /// `form` says what the definition's tree does not, as
    /// [`crate::lower::lower`] has it.
    fn compile(&self, definition: &Open, form: &Form) -> Result<Vec<u8>, String>;
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact(pub Vec<u8>);

impl Engine<'_> {
    /// A function's compiled form for one backend.
    ///
    /// # Errors
    ///
    /// Fails if the symbol is not a function, does not check, or the backend
    /// refuses it.
    pub fn compile(&self, symbol: &Symbol, backend: &dyn Backend) -> Query<Artifact> {
        self.compile_as(symbol, backend, false)
    }

    /// A function's compiled form for one backend, metered or not. The
    /// two are different code for one hash.
    pub(super) fn compile_as(
        &self,
        symbol: &Symbol,
        backend: &dyn Backend,
        metered: bool,
    ) -> Query<Artifact> {
        let memo_key = (symbol.clone(), backend.name().to_owned(), metered);
        memo(&self.compile, &memo_key, || {
            let open = self.check(symbol)?;
            if !matches!(open.tree, Tree::Function(_) | Tree::Extern { .. }) {
                return Err(Error::NotCompilable(symbol.clone()));
            }

            let form = self.form(symbol, metered)?;
            let mut key = Hasher::new("compile");
            key.bytes(backend.name().as_bytes());
            key.bytes(&[u8::from(metered), u8::from(form.handed)]);
            key.bytes(
                &form
                    .slots
                    .iter()
                    .map(|slot| u8::from(*slot))
                    .collect::<Vec<_>>(),
            );
            key.bytes(&open.tree.encode());
            for slot in &open.slots {
                key.hash(&self.interface(slot)?.digest);
            }

            let compile = || {
                backend
                    .compile(&open, &form)
                    .map(Artifact)
                    .map_err(|message| Error::Backend {
                        symbol: symbol.clone(),
                        message,
                    })
            };
            // A received definition is never written to a store.
            if self.received(symbol).is_some() {
                return compile().map(Arc::new);
            }
            self.stage(Stage::Compile, symbol, key.finish(), compile)
        })
    }
}
