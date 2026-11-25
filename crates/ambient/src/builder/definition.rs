//! Definitions and modules.

use super::{null, ty};
use crate::surface::{
    Ability, Annotation, ContractDefinition, Definition, DefinitionKind, Expr, Function, Param,
    Path, TypeDefinition, TypeParam,
};

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
            opaque: false,
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
    bounded_type(name, params.into_iter().map(|param| (param, None)), ty)
}

/// A private type definition with type parameters, each with its bound
/// if it has one. A bound may name any of the parameters.
pub fn bounded_type<'a>(
    name: &str,
    params: impl IntoIterator<Item = (&'a str, Option<crate::surface::Type>)>,
    ty: crate::surface::Type,
) -> Definition {
    let param = |(name, bound): (&str, _)| TypeParam {
        name: name.to_owned(),
        bound,
    };
    Definition {
        name: name.to_owned(),
        public: false,
        doc: None,
        annotations: Vec::new(),
        kind: DefinitionKind::Type(TypeDefinition {
            params: params.into_iter().map(param).collect(),
            ty,
            opaque: false,
        }),
    }
}

/// A private opaque type definition: one only its own module sees the
/// content of, with each parameter by its name and bound. See "Opaque
/// types" in `docs/spec/types.md`.
#[must_use]
pub fn opaque_type<'a>(
    name: &str,
    params: impl IntoIterator<Item = (&'a str, Option<crate::surface::Type>)>,
    ty: crate::surface::Type,
) -> Definition {
    let mut definition = bounded_type(name, params, ty);
    if let DefinitionKind::Type(written) = &mut definition.kind {
        written.opaque = true;
    }
    definition
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
