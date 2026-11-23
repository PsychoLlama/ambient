//! Externs and targets: what a program is given by the world it runs in.

use crate::surface::{Definition, DefinitionKind, Extern, Path, Target};

/// Starts a native implementation of the ability at a dotted path, found
/// by where it is declared. Only a target can bind it.
#[must_use]
pub fn native_handler(name: &str, ability: &str) -> ExternBuilder {
    ExternBuilder {
        name: name.to_owned(),
        public: false,
        native: Extern {
            ability: Path::parse(ability),
            row: Vec::new(),
            targets: Vec::new(),
            suspends: false,
        },
    }
}

#[derive(Clone, Debug)]
pub struct ExternBuilder {
    name: String,
    public: bool,
    native: Extern,
}

impl ExternBuilder {
    #[must_use]
    pub fn public(mut self) -> Self {
        self.public = true;
        self
    }

    /// Says a native handler may suspend the task that performs it.
    #[must_use]
    pub fn suspending(mut self) -> Self {
        self.native.suspends = true;
        self
    }

    /// Makes a native handler its ability's default under the target
    /// at a dotted path, and under whatever extends it.
    #[must_use]
    pub fn under(mut self, target: &str) -> Self {
        self.native.targets.push(Path::parse(target));
        self
    }

    /// Adds abilities, each a dotted path, to what the native code
    /// performs.
    #[must_use]
    pub fn performs<'a>(mut self, abilities: impl IntoIterator<Item = &'a str>) -> Self {
        self.native
            .row
            .extend(abilities.into_iter().map(Path::parse));
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
