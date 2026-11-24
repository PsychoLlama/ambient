//! Modules: definitions, what they import, and what they expose.

use std::collections::BTreeSet;

use super::BuildError;
use super::validate::{
    check_expr, check_extern, check_function, check_name, check_params, check_path, check_target,
    check_type, check_type_params,
};
use crate::surface::{Definition, DefinitionKind, Import, Module, Path, constant_name};
use crate::symbol::Namespace;

#[must_use]
pub fn module() -> ModuleBuilder {
    ModuleBuilder::default()
}

#[derive(Clone, Debug, Default)]
pub struct ModuleBuilder {
    imports: Vec<Import>,
    definitions: Vec<Definition>,
    exposes: Vec<Path>,
}

impl ModuleBuilder {
    /// Imports a definition or a module by its full path, under its last
    /// segment: `"pkg.util.math"`, `"http.Request"`.
    #[must_use]
    pub fn import(self, path: &str) -> Self {
        let alias = path.rsplit('.').next().unwrap_or(path).to_owned();
        self.import_as(path, &alias)
    }

    #[must_use]
    pub fn import_as(mut self, path: &str, alias: &str) -> Self {
        self.imports.push(Import {
            path: Path::parse(path),
            alias: alias.to_owned(),
            public: false,
        });
        self
    }

    /// Imports a definition and re-exports it under its last segment:
    /// other modules reach it through this one too.
    #[must_use]
    pub fn export(mut self, path: &str) -> Self {
        let alias = path.rsplit('.').next().unwrap_or(path).to_owned();
        self.imports.push(Import {
            path: Path::parse(path),
            alias,
            public: true,
        });
        self
    }

    /// For a library module: lets other packages reach the module at
    /// `path` within this one, such as `"util.math"`.
    #[must_use]
    pub fn expose(mut self, path: &str) -> Self {
        self.exposes.push(Path::parse(path));
        self
    }

    #[must_use]
    pub fn define(mut self, definition: impl Into<Definition>) -> Self {
        self.definitions.push(definition.into());
        self
    }

    /// Finishes the module.
    ///
    /// # Errors
    ///
    /// Returns the first way in which the module is ill-formed.
    pub fn build(self) -> Result<Module, BuildError> {
        self.placed().map_err(|(_, error)| error)
    }

    /// Finishes the module, as [`Self::build`] does.
    ///
    /// # Errors
    ///
    /// Returns the first way in which the module is ill-formed, and
    /// which part of it is.
    pub fn placed(self) -> Result<Module, (Part, BuildError)> {
        let mut aliases = BTreeSet::new();
        for (index, import) in self.imports.iter().enumerate() {
            imported(import, &mut aliases).map_err(|error| (Part::Import(index), error))?;
        }
        let whole = |error| (Part::Module, error);
        self.exposes
            .iter()
            .try_for_each(check_path)
            .map_err(whole)?;
        let mut defined = BTreeSet::new();
        let alone = self.definitions.len() == 1;
        for (index, definition) in self.definitions.iter().enumerate() {
            define(definition, alone, &mut defined)
                .map_err(|error| (Part::Definition(index), error))?;
        }
        Ok(Module {
            imports: self.imports,
            definitions: self.definitions,
            exposes: self.exposes,
        })
    }
}

/// A part of a module, by its place among those of its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Import(usize),
    Definition(usize),
    /// Nothing smaller than the module.
    Module,
}

/// Checks an import, beside the names `aliases` holds of those before
/// it.
fn imported<'a>(import: &'a Import, aliases: &mut BTreeSet<&'a String>) -> Result<(), BuildError> {
    check_path(&import.path)?;
    check_name(&import.alias)?;
    if !aliases.insert(&import.alias) {
        return Err(BuildError::DuplicateDefinition(import.alias.clone()));
    }
    Ok(())
}

/// Checks a definition, beside the names `defined` holds of those
/// before it. `alone` is whether its module defines nothing else.
fn define<'a>(
    definition: &'a Definition,
    alone: bool,
    defined: &mut BTreeSet<(Namespace, &'a String)>,
) -> Result<(), BuildError> {
    {
        check_name(&definition.name)?;
        let namespace = match &definition.kind {
            DefinitionKind::Type(definition) => {
                check_type_params(definition.params.iter().map(|param| &param.name))?;
                for bound in definition.params.iter().filter_map(|p| p.bound.as_ref()) {
                    check_type(bound)?;
                }
                check_type(&definition.ty)?;
                Namespace::Type
            }
            DefinitionKind::Derived(call) => {
                check_expr(call)?;
                Namespace::Type
            }
            DefinitionKind::Generated(call) => {
                check_expr(call)?;
                Namespace::Value
            }
            DefinitionKind::Module(call) => {
                if !alone {
                    return Err(BuildError::GeneratedWithMore);
                }
                check_expr(call)?;
                Namespace::Value
            }
            DefinitionKind::Function(function) => {
                check_function(&definition.name, function)?;
                Namespace::Value
            }
            DefinitionKind::Constant(function) => {
                if !constant_name(&definition.name) {
                    return Err(BuildError::ConstantName(definition.name.clone()));
                }
                check_function(&definition.name, function)?;
                Namespace::Value
            }
            DefinitionKind::Ability(ability) => {
                check_params(&definition.name, &ability.params)?;
                check_type(&ability.ret)?;
                Namespace::Ability
            }
            DefinitionKind::Contract(contract) => {
                check_type(&contract.ty)?;
                Namespace::Contract
            }
            // A tag is named as a type and as a value, and a brand
            // as a type and as what brands a value.
            DefinitionKind::Tag | DefinitionKind::Brand => {
                if !defined.insert((Namespace::Value, &definition.name)) {
                    return Err(BuildError::DuplicateDefinition(definition.name.clone()));
                }
                Namespace::Type
            }
            DefinitionKind::Extern(native) => {
                check_extern(native)?;
                Namespace::Value
            }
            DefinitionKind::Intrinsic => Namespace::Value,
            DefinitionKind::Target(target) => {
                check_target(target)?;
                Namespace::Value
            }
        };
        if !defined.insert((namespace, &definition.name)) {
            return Err(BuildError::DuplicateDefinition(definition.name.clone()));
        }
    }
    Ok(())
}
