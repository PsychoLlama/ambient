//! Definitions and modules.

use std::collections::BTreeSet;

use super::validate::{
    check_expr, check_extern, check_function, check_name, check_params, check_path, check_target,
    check_type, check_type_params,
};
use super::{BuildError, null, ty};
use crate::surface::{
    Ability, Annotation, ContractDefinition, Definition, DefinitionKind, Expr, Function, Import,
    Module, Param, Path, TypeDefinition, TypeParam, constant_name,
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
/// parameters, and returns `null`. Where it is declared tells it from
/// another ability with the same signature.
#[must_use]
pub fn ability(name: &str) -> AbilityBuilder {
    AbilityBuilder {
        name: name.to_owned(),
        public: false,
        doc: None,
        ability: Ability {
            type_params: Vec::new(),
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

    /// A type parameter of the ability's signature.
    #[must_use]
    pub fn type_param(mut self, name: &str) -> Self {
        self.ability.type_params.push(TypeParam {
            name: name.to_owned(),
            bound: None,
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
            annotations: Vec::new(),
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
        annotations: Vec::new(),
        kind: DefinitionKind::Type(TypeDefinition {
            params: Vec::new(),
            ty,
        }),
    }
}

/// The definition a generated module is written as: the call that
/// gives everything it defines.
#[must_use]
pub fn generates(call: crate::surface::Expr) -> Definition {
    Definition {
        name: crate::surface::GENERATOR.to_owned(),
        public: false,
        doc: None,
        annotations: Vec::new(),
        kind: DefinitionKind::Module(crate::surface::Expr::Comptime(Box::new(call))),
    }
}

/// A private function that is whatever tree a comptime call gives.
#[must_use]
pub fn generated(name: &str, call: crate::surface::Expr) -> Definition {
    Definition {
        name: name.to_owned(),
        public: false,
        doc: None,
        annotations: Vec::new(),
        kind: DefinitionKind::Generated(crate::surface::Expr::Comptime(Box::new(call))),
    }
}

/// A private type that is whatever tree a comptime call gives.
#[must_use]
pub fn derived(name: &str, call: crate::surface::Expr) -> Definition {
    Definition {
        name: name.to_owned(),
        public: false,
        doc: None,
        annotations: Vec::new(),
        kind: DefinitionKind::Derived(crate::surface::Expr::Comptime(Box::new(call))),
    }
}

/// A private contract: what a record holds a value of type `ty` under.
#[must_use]
pub fn contract(name: &str, ty: crate::surface::Type) -> Definition {
    Definition {
        name: name.to_owned(),
        public: false,
        doc: None,
        annotations: Vec::new(),
        kind: DefinitionKind::Contract(ContractDefinition { ty }),
    }
}

/// A private tag: a name for one value, which is told from every
/// other by where the tag is declared.
#[must_use]
pub fn tag(name: &str) -> Definition {
    Definition {
        name: name.to_owned(),
        public: false,
        doc: None,
        annotations: Vec::new(),
        kind: DefinitionKind::Tag,
    }
}

/// A public intrinsic: a name the compiler knows, which only the
/// standard library declares.
#[must_use]
pub fn intrinsic(name: &str) -> Definition {
    Definition {
        name: name.to_owned(),
        public: true,
        doc: None,
        annotations: Vec::new(),
        kind: DefinitionKind::Intrinsic,
    }
}

/// A private brand: a mark on a type, told from every other by where
/// the brand is declared.
#[must_use]
pub fn brand(name: &str) -> Definition {
    Definition {
        name: name.to_owned(),
        public: false,
        doc: None,
        annotations: Vec::new(),
        kind: DefinitionKind::Brand,
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
        annotations: Vec::new(),
        kind: DefinitionKind::Type(TypeDefinition {
            params: params.into_iter().map(str::to_owned).collect(),
            ty,
        }),
    }
}

impl Definition {
    /// Annotates the definition: `key` is the dotted path of a tag, and
    /// `value` plain data.
    #[must_use]
    pub fn annotated(mut self, key: &str, value: Expr) -> Self {
        let key = Path::parse(key);
        self.annotations.push(Annotation { key, value });
        self
    }

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
            annotations: Vec::new(),
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
                check_type_params(definition.params.iter())?;
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
                check_extern(&definition.name, native)?;
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
