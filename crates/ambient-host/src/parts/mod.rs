//! The parts of a program: everything an entry reaches, compiled. See
//! `docs/spec/build.md` and `docs/spec/targets.md`.
//!
//! The compiler's engine makes them (`Engine::link`); a host loads
//! them, and needs nothing else of the compiler to.

mod encode;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use encode::Unloadable;

use crate::core;
use crate::hash::Hash;
use crate::value::{Function, Value};

/// Why nothing is loaded into a program that was not linked for it.
pub const CLOSED: &str = "the program was not linked to be loaded into: see `Engine::link_open`";

/// The build of the host this is: its source, the runtime's, and the
/// versions of what they depend on. Nothing compiled for one build is
/// loaded by another.
pub const BUILD: &str = env!("AMBIENT_BUILD");

/// One function, compiled by a backend.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact(pub Vec<u8>);

/// Something a definition needs that is made once, when its code is
/// loaded, and kept as long as that code is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Constant {
    String(String),
    Bytes(Vec<u8>),
    /// A tag: its package and itself, as [`crate::literal::Literal::tag_text`]
    /// spells them.
    Tag(String),
    /// A field name, as the key records are indexed by.
    Field(String),
    /// A type, as the descriptor a value is tested against. Its slots are
    /// those of the definition it is a constant of.
    Type(core::Type),
    /// Type arguments that mention no type parameter: what a generic
    /// function used at them is made with. Their slots are those of the
    /// definition they are a constant of.
    Types(Vec<core::Type>),
    /// The generic function in a slot at type arguments that mention no
    /// type parameter, as a value: a closure that holds them.
    Instance(usize, Vec<core::Type>),
    /// The ability in a slot, as the key bindings are indexed by.
    Ability(usize),
    /// Where the definition, an extern, sits among the externs of the
    /// program it is loaded into. Not a value.
    Native,
    /// The hash of the definition itself, as a string: what a frame of a
    /// trace names it by. Known only once the program is linked.
    Own,
}

/// One definition of a program.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub hash: Hash,
    /// The part each slot refers to.
    pub slots: Vec<usize>,
    pub body: Body,
    /// If the part is an extern: its position in [`Parts::natives`].
    pub native: Option<usize>,
}

/// An extern a program reaches: what a host finds native code by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Native {
    pub package: String,
    /// The extern's module path and name, as one text.
    pub path: String,
    /// The abilities it performs, each by its package and its module
    /// path and name, in the order it is handed their handlers.
    pub row: Vec<(String, String)>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Body {
    Function {
        /// How many parameters it takes.
        arity: usize,
        code: Arc<Artifact>,
        /// What the function needs made when the program is loaded.
        constants: Vec<Constant>,
        /// Its type, which its definition carries the name of, unless
        /// that depends on type arguments. Its slots are the
        /// definition's.
        signature: Option<core::Type>,
        /// Its type as a function, if that depends on type arguments
        /// it is handed: what its definition carries a descriptor of,
        /// read in the arguments a value of it holds.
        described: Option<core::Type>,
        /// How many type arguments it is handed as it runs: as many as
        /// it has type parameters if it needs them, and none if not.
        types: usize,
    },
    /// A recursive type definition, which casts test values against.
    Type(core::Type),
    /// An ability or a target. Nothing is laid out for one: an ability
    /// is reached by a key made from its hash, and a target not at all.
    Inert,
}

/// A program: exactly the definitions its entry reaches, each once.
/// [`Parts::encode`] is its form for a host that has no compiler.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parts {
    pub parts: Vec<Part>,
    /// The entry's position in `parts`.
    pub entry: usize,
    /// The externs among `parts`, which a host must supply before the
    /// program runs.
    pub natives: Vec<Native>,
    /// Whether this is a program code can be loaded into, or code to
    /// load. Only then is every function sure to be able to suspend:
    /// see `Engine::link`.
    pub open: bool,
}

impl Parts {
    /// The entry as a value: what a host makes of a function it has
    /// loaded.
    ///
    /// # Errors
    ///
    /// Fails if the parts have no entry.
    pub fn function(&self) -> Result<Value, String> {
        let entry = self
            .parts
            .get(self.entry)
            .ok_or("the parts have no entry")?;
        Ok(Value::Function(Function {
            definition: Some(entry.hash),
            captures: Vec::new(),
            live: None,
        }))
    }

    /// The code of the program: each part's, with how many parameters
    /// it takes. A part that is no function has `None` there.
    #[must_use]
    pub fn codes(&self) -> Vec<Option<(usize, &Artifact)>> {
        let codes = self.parts.iter().map(|part| match &part.body {
            Body::Function { arity, code, .. } => Some((*arity, &**code)),
            Body::Type(_) | Body::Inert => None,
        });
        codes.collect()
    }
}
