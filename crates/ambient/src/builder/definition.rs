//! Definitions and modules.

use std::collections::BTreeSet;

use super::validate::{
    check_function, check_name, check_params, check_path, check_type, check_type_params,
};
use super::{BuildError, null, ty};
use crate::surface::{
    Ability, Definition, DefinitionKind, Expr, Function, Import, Module, Param, Path,
    TypeDefinition, TypeParam,
};
use crate::symbol::Namespace;

/// Starts a function. With nothing else said it is private, takes no
/// parameters, and returns `null`.
#[must_use]
pub fn function(name: &str) -> FunctionBuilder {
    FunctionBuilder {
        name: name.to_owned(),
        public: false,
        doc: None,
        function: Function {
            type_params: Vec::new(),
            params: Vec::new(),
            ret: ty::null(),
            row: Vec::new(),
            body: null(),
        },
    }
}

/// Starts an ability. With nothing else said it is private, takes no
/// parameters, and returns `null`.
///
/// `tag` is what tells it from another ability with the same signature; it
/// must be unique within the package.
#[must_use]
pub fn ability(name: &str, tag: &str) -> AbilityBuilder {
    AbilityBuilder {
        name: name.to_owned(),
        public: false,
        doc: None,
        ability: Ability {
            tag: tag.to_owned(),
            params: Vec::new(),
            ret: ty::null(),
        },
    }
}

#[derive(Clone, Debug)]
pub struct AbilityBuilder {
    name: String,
    public: bool,
    doc: Option<String>,
    ability: Ability,
}

impl AbilityBuilder {
    #[must_use]
    pub fn public(mut self) -> Self {
        self.public = true;
        self
    }

    #[must_use]
    pub fn doc(mut self, doc: &str) -> Self {
        self.doc = Some(doc.to_owned());
        self
    }

    #[must_use]
    pub fn param(mut self, name: &str, ty: crate::surface::Type) -> Self {
        self.ability.params.push(Param {
            name: name.to_owned(),
            ty,
        });
        self
    }

    /// What a perform evaluates to. An ability that returns `never` aborts.
    #[must_use]
    pub fn returns(mut self, ty: crate::surface::Type) -> Self {
        self.ability.ret = ty;
        self
    }
}

impl From<AbilityBuilder> for Definition {
    fn from(builder: AbilityBuilder) -> Self {
        Definition {
            name: builder.name,
            public: builder.public,
            doc: builder.doc,
            kind: DefinitionKind::Ability(builder.ability),
        }
    }
}

/// A private type definition.
#[must_use]
pub fn type_(name: &str, ty: crate::surface::Type) -> Definition {
    Definition {
        name: name.to_owned(),
        public: false,
        doc: None,
        kind: DefinitionKind::Type(TypeDefinition {
            params: Vec::new(),
            ty,
        }),
    }
}

/// A private type definition with type parameters.
pub fn generic_type<'a>(
    name: &str,
    params: impl IntoIterator<Item = &'a str>,
    ty: crate::surface::Type,
) -> Definition {
    Definition {
        name: name.to_owned(),
        public: false,
        doc: None,
        kind: DefinitionKind::Type(TypeDefinition {
            params: params.into_iter().map(str::to_owned).collect(),
            ty,
        }),
    }
}

impl Definition {
    /// Makes the definition visible throughout its package.
    #[must_use]
    pub fn public(mut self) -> Self {
        self.public = true;
        self
    }

    #[must_use]
    pub fn doc(mut self, doc: &str) -> Self {
        self.doc = Some(doc.to_owned());
        self
    }
}

#[derive(Clone, Debug)]
pub struct FunctionBuilder {
    name: String,
    public: bool,
    doc: Option<String>,
    function: Function,
}

impl FunctionBuilder {
    #[must_use]
    pub fn public(mut self) -> Self {
        self.public = true;
        self
    }

    #[must_use]
    pub fn doc(mut self, doc: &str) -> Self {
        self.doc = Some(doc.to_owned());
        self
    }

    /// Adds a type parameter with no bound.
    #[must_use]
    pub fn type_param(mut self, name: &str) -> Self {
        self.function.type_params.push(TypeParam {
            name: name.to_owned(),
            bound: None,
        });
        self
    }

    /// Adds a type parameter that must be a subtype of `bound`.
    #[must_use]
    pub fn bounded(mut self, name: &str, bound: crate::surface::Type) -> Self {
        self.function.type_params.push(TypeParam {
            name: name.to_owned(),
            bound: Some(bound),
        });
        self
    }

    #[must_use]
    pub fn param(mut self, name: &str, ty: crate::surface::Type) -> Self {
        self.function.params.push(Param {
            name: name.to_owned(),
            ty,
        });
        self
    }

    #[must_use]
    pub fn returns(mut self, ty: crate::surface::Type) -> Self {
        self.function.ret = ty;
        self
    }

    /// Adds abilities, each a dotted path, to what the function takes from
    /// its caller.
    #[must_use]
    pub fn performs<'a>(mut self, row: impl IntoIterator<Item = &'a str>) -> Self {
        self.function.row.extend(row.into_iter().map(Path::parse));
        self
    }

    /// Finishes the function with its body.
    #[must_use]
    pub fn body(mut self, body: Expr) -> Definition {
        self.function.body = body;
        Definition {
            name: self.name,
            public: self.public,
            doc: self.doc,
            kind: DefinitionKind::Function(self.function),
        }
    }
}

#[must_use]
pub fn module() -> ModuleBuilder {
    ModuleBuilder::default()
}

#[derive(Clone, Debug, Default)]
pub struct ModuleBuilder {
    imports: Vec<Import>,
    definitions: Vec<Definition>,
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
        });
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
        let mut aliases = BTreeSet::new();
        for import in &self.imports {
            if import.path.0.len() < 2 {
                return Err(BuildError::ImportTooShort(import.path.clone()));
            }
            check_path(&import.path)?;
            check_name(&import.alias)?;
            if !aliases.insert(&import.alias) {
                return Err(BuildError::DuplicateDefinition(import.alias.clone()));
            }
        }

        let mut defined = BTreeSet::new();
        for definition in &self.definitions {
            check_name(&definition.name)?;
            let namespace = match &definition.kind {
                DefinitionKind::Type(definition) => {
                    check_type_params(definition.params.iter())?;
                    check_type(&definition.ty)?;
                    Namespace::Type
                }
                DefinitionKind::Function(function) => {
                    check_function(&definition.name, function)?;
                    Namespace::Value
                }
                DefinitionKind::Ability(ability) => {
                    check_params(&definition.name, &ability.params)?;
                    check_type(&ability.ret)?;
                    Namespace::Ability
                }
            };
            if !defined.insert((namespace, &definition.name)) {
                return Err(BuildError::DuplicateDefinition(definition.name.clone()));
            }
        }

        Ok(Module {
            imports: self.imports,
            definitions: self.definitions,
        })
    }
}
