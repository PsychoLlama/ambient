//! Externs and targets: what a program is given by the world it runs in.

use super::ty;
use crate::surface::{Definition, DefinitionKind, Extern, ExternKind, Param, Path, Target};

/// Starts a pure function implemented in native code, which is found by
/// where it is declared. With nothing else said it is private, takes no parameters, and
/// returns `null`.
#[must_use]
pub fn extern_function(name: &str) -> ExternBuilder {
    let kind = ExternKind::Function {
        params: Vec::new(),
        ret: ty::null(),
        row: Vec::new(),
    };
    ExternBuilder::new(name, kind)
}

/// Starts a native implementation of the ability at a dotted path, found
/// by where it is declared. Only a target can bind it.
#[must_use]
pub fn native_handler(name: &str, ability: &str) -> ExternBuilder {
    let kind = ExternKind::Handler {
        ability: Path::parse(ability),
        row: Vec::new(),
    };
    ExternBuilder::new(name, kind)
}

#[derive(Clone, Debug)]
pub struct ExternBuilder {
    name: String,
    public: bool,
    native: Extern,
}

impl ExternBuilder {
    fn new(name: &str, kind: ExternKind) -> Self {
        Self {
            name: name.to_owned(),
            public: false,
            native: Extern {
                kind,
                suspends: false,
            },
        }
    }

    #[must_use]
    pub fn public(mut self) -> Self {
        self.public = true;
        self
    }

    /// Adds a parameter. A native handler has none of its own: it takes
    /// what its ability takes.
    #[must_use]
    pub fn param(mut self, name: &str, ty: crate::surface::Type) -> Self {
        if let ExternKind::Function { params, .. } = &mut self.native.kind {
            params.push(Param {
                name: name.to_owned(),
                ty,
            });
        }
        self
    }

    /// Sets what a native function returns. A native handler returns what
    /// its ability returns.
    #[must_use]
    pub fn returns(mut self, ty: crate::surface::Type) -> Self {
        if let ExternKind::Function { ret, .. } = &mut self.native.kind {
            *ret = ty;
        }
        self
    }

    /// Says a native handler may suspend the task that performs it.
    #[must_use]
    pub fn suspending(mut self) -> Self {
        self.native.suspends = true;
        self
    }

    /// Adds abilities, each a dotted path, to what the native code
    /// performs.
    #[must_use]
    pub fn performs<'a>(mut self, abilities: impl IntoIterator<Item = &'a str>) -> Self {
        let (ExternKind::Handler { row, .. } | ExternKind::Function { row, .. }) =
            &mut self.native.kind;
        row.extend(abilities.into_iter().map(Path::parse));
        self
    }
}

impl From<ExternBuilder> for Definition {
    fn from(builder: ExternBuilder) -> Self {
        Definition {
            name: builder.name,
            public: builder.public,
            doc: None,
            annotations: Vec::new(),
            kind: DefinitionKind::Extern(builder.native),
        }
    }
}

/// Starts a target: the world a program runs in. With nothing else said it
/// is private and binds nothing.
#[must_use]
pub fn target(name: &str) -> TargetBuilder {
    TargetBuilder {
        name: name.to_owned(),
        public: false,
        target: Target {
            extends: Vec::new(),
            tool: None,
            bindings: Vec::new(),
        },
    }
}

#[derive(Clone, Debug)]
pub struct TargetBuilder {
    name: String,
    public: bool,
    target: Target,
}

impl TargetBuilder {
    #[must_use]
    pub fn public(mut self) -> Self {
        self.public = true;
        self
    }

    /// Starts from the bindings of the target at a dotted path.
    #[must_use]
    pub fn extends(mut self, target: &str) -> Self {
        self.target.extends.push(Path::parse(target));
        self
    }

    /// Names the tool at a dotted path as what builds a program under
    /// the target.
    #[must_use]
    pub fn built_by(mut self, tool: &str) -> Self {
        self.target.tool = Some(Path::parse(tool));
        self
    }

    /// Binds the ability at one dotted path to the function or native
    /// handler at another, replacing what an extended target bound.
    #[must_use]
    pub fn bind(mut self, ability: &str, handler: &str) -> Self {
        self.target
            .bindings
            .push((Path::parse(ability), Path::parse(handler)));
        self
    }
}

impl From<TargetBuilder> for Definition {
    fn from(builder: TargetBuilder) -> Self {
        Definition {
            name: builder.name,
            public: builder.public,
            doc: None,
            annotations: Vec::new(),
            kind: DefinitionKind::Target(builder.target),
        }
    }
}
