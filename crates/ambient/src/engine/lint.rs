//! Lints: the functions a package's manifest lists, which checking the
//! package runs for what each finds wrong. See "Commands" in
//! `docs/spec/tools.md`.

use super::Engine;
use super::asked::{definition, record};
use crate::error::Error;
use crate::symbol::{PackageId, Symbol};
use crate::value::Value;

/// What a lint gave, as the errors they are. What a checked lint
/// gives is diagnostics, whatever it did.
fn found(lint: &Symbol, given: &Value) -> Vec<Error> {
    let diagnostic = |given: &Value| {
        let Value::Record(fields) = given else {
            return None;
        };
        let Some(Value::String(message)) = fields.get("message") else {
            return None;
        };
        let on = fields.get("on")?.clone();
        Some(Error::Lint {
            symbol: definition(&[on], 0).ok()?,
            lint: lint.clone(),
            message: message.clone(),
        })
    };
    match given {
        Value::List(given) => given.iter().filter_map(diagnostic).collect(),
        _ => Vec::new(),
    }
}

impl Engine<'_> {
    /// What the lints of `package` find wrong with it, in the order
    /// its manifest lists them: each is handed a selection of the
    /// package and its modules, and of every name that holds `name`.
    ///
    /// # Errors
    ///
    /// Fails if what is listed is not a lint, or as [`Self::tool`]
    /// does.
    pub fn linted(&self, package: &PackageId, name: &str) -> Result<Vec<Error>, Error> {
        let text = |text: &String| Value::String(text.clone());
        let module = |module: crate::symbol::ModuleId| {
            record([
                ("package", text(&package.0)),
                (
                    "path",
                    Value::List(module.path.0.iter().map(text).collect()),
                ),
            ])
        };
        let modules = self.sources.modules(package).into_iter().map(module);
        let selection = record([
            ("packages", Value::List(vec![text(&package.0)])),
            ("modules", Value::List(modules.collect())),
            ("name", Value::String(name.to_owned())),
        ]);
        let mut diagnostics = Vec::new();
        for lint in self.sources.lints(package) {
            if !self.typed(&lint, "Selection", "Diagnostics")? {
                return Err(Error::NotLint(lint));
            }
            let given = self.tool(&lint, std::slice::from_ref(&selection))?;
            diagnostics.extend(found(&lint, &given));
        }
        Ok(diagnostics)
    }
}
