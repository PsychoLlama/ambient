//! Compile: a core tree, the interfaces of its slots, and a backend yield an
//! artifact. See `spec/build.md`.

use std::sync::Arc;

use super::{Artifact, Engine, Query, Stage, memo};
use crate::core::{Open, Tree};
use crate::error::Error;
use crate::hash::Hasher;
use crate::lower::{Form, deep};
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

    /// Whether a task is a stack where this backend's code runs, so
    /// that no function is compiled to suspend or to resume. See
    /// "Suspension" in `spec/backends.md`.
    fn stacks(&self) -> bool {
        false
    }
}

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

    /// A function's compiled form for one backend: with `still`, as
    /// part of a program in which nothing suspends, compiled without
    /// what resuming takes. Both forms are code for one hash.
    pub(super) fn compile_as(
        &self,
        symbol: &Symbol,
        backend: &dyn Backend,
        still: bool,
    ) -> Query<Artifact> {
        // Where a task is a stack, nothing is ever compiled to resume.
        let still = still || backend.stacks();
        let memo_key = (symbol.clone(), backend.name().to_owned(), still);
        deep(|| self.compiled(symbol, backend, still, &memo_key))
    }

    fn compiled(
        &self,
        symbol: &Symbol,
        backend: &dyn Backend,
        still: bool,
        memo_key: &(Symbol, String, bool),
    ) -> Query<Artifact> {
        memo(&self.compile, memo_key, || {
            let open = self.check(symbol)?;
            if !matches!(open.tree, Tree::Function(_) | Tree::Extern { .. }) {
                return Err(Error::NotCompilable(symbol.clone()));
            }

            let form = self.form(symbol, still)?;
            let mut key = Hasher::new("compile");
            key.bytes(backend.name().as_bytes());
            let flags = [still, form.handed];
            key.bytes(&flags.map(u8::from));
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
