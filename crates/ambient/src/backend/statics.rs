//! What a program needs in memory before it runs, and where: every
//! function's definition and closure, and every type's descriptor.
//!
//! Both backends load a program the same way, so the layout is planned
//! here once. A backend only has to write each [`Field`] where it says.

use ambient_runtime::descriptor as d;
use ambient_runtime::layout::{CLOSURE_WORDS, DEFINITION_WORDS};

use crate::core::Type;
use crate::engine::{Body, Parts};
use crate::hash::Hash;
use crate::lir::Constant;
use crate::literal::Literal;

/// What goes in one word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Word(u64),
    /// The address of another word of the block.
    Address(usize),
    /// The code of a part, however the backend refers to code.
    Code(usize),
    /// Something the runtime makes from bytes of [`Statics::text`].
    Text {
        make: Make,
        offset: usize,
        length: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Make {
    /// A string value.
    String,
    /// A field name.
    Field,
}

/// One block of words and what to write in it.
#[derive(Debug, Default)]
pub struct Statics {
    /// How many words the block has.
    pub words: usize,
    /// What each word that is not zero holds, by position.
    pub fields: Vec<(usize, Field)>,
    /// The bytes [`Field::Text`] refers to.
    pub text: Vec<u8>,
    /// Where the entry's closure is.
    pub entry: usize,
}

/// Plans the block for a program.
#[must_use]
pub fn plan(parts: &Parts) -> Statics {
    let mut statics = Statics::default();

    // Where each part is found: a function's closure, a type's descriptor.
    // These are fixed first, because parts refer to each other freely.
    let mut definitions = Vec::new();
    let mut roots = Vec::new();
    for part in &parts.parts {
        definitions.push(statics.words);
        match &part.body {
            Body::Function { constants, .. } => {
                let definition = DEFINITION_WORDS + part.slots.len() + constants.len();
                roots.push(statics.words + definition);
                statics.words += definition + CLOSURE_WORDS;
            }
            Body::Type(ty) => {
                roots.push(statics.words);
                statics.words += root_size(ty);
            }
            // Never read: an ability is reached by its key, a constant.
            Body::Ability => roots.push(0),
        }
    }
    statics.entry = roots[parts.entry];

    let mut header = [0u64; CLOSURE_WORDS];
    ambient_runtime::pinned_closure(&mut header, 0);
    for (index, part) in parts.parts.iter().enumerate() {
        let slots: Vec<usize> = part.slots.iter().map(|slot| roots[*slot]).collect();
        let at = definitions[index];
        match &part.body {
            Body::Function {
                arity, constants, ..
            } => {
                statics.set(at, Field::Code(index));
                statics.set(at + 1, Field::Word(*arity as u64));
                for (position, slot) in slots.iter().enumerate() {
                    statics.set(at + DEFINITION_WORDS + position, Field::Address(*slot));
                }
                let first = at + DEFINITION_WORDS + slots.len();
                for (position, constant) in constants.iter().enumerate() {
                    let field = match constant {
                        Constant::String(text) => statics.text(Make::String, text),
                        Constant::Field(name) => statics.text(Make::Field, name),
                        Constant::Type(ty) => Field::Address(statics.describe(ty, &slots)),
                        Constant::Ability(slot) => {
                            let ability = &parts.parts[part.slots[*slot]];
                            statics.text(Make::Field, &ability_key(&ability.hash))
                        }
                    };
                    statics.set(first + position, field);
                }
                let closure = roots[index];
                statics.set(closure, Field::Word(header[0]));
                statics.set(closure + 1, Field::Word(header[1]));
                statics.set(closure + 2, Field::Address(at));
            }
            Body::Type(ty) => statics.root(ty, &slots, at),
            Body::Ability => {}
        }
    }
    statics
}

/// The name bindings hold an ability under. No field has a name like it,
/// so bindings are records like any other.
fn ability_key(hash: &Hash) -> String {
    format!("@{hash}")
}

/// How many words a type definition's own node takes.
fn root_size(ty: &Type) -> usize {
    match ty {
        // A definition that is only another's name gets a node of its own
        // to be found at.
        Type::Reference(_, args) if args.is_empty() => 2,
        _ => node_size(ty),
    }
}

/// How many words a type's node takes, not counting the nodes it refers
/// to.
fn node_size(ty: &Type) -> usize {
    match ty {
        Type::Never
        | Type::Unknown
        | Type::Null
        | Type::Bool
        | Type::Int
        | Type::Float
        | Type::String
        | Type::Function { .. } => 1,
        Type::Literal(_) => 3,
        Type::Record(fields) => 2 + fields.len() * 2,
        Type::Tuple(members) | Type::Union(members) => 2 + members.len(),
        Type::List(_) | Type::Parameter(_) => 2,
        Type::Reference(_, args) => 3 + args.len(),
    }
}

impl Statics {
    fn set(&mut self, word: usize, field: Field) {
        self.fields.push((word, field));
    }

    fn text(&mut self, make: Make, text: &str) -> Field {
        let offset = self.text.len();
        self.text.extend_from_slice(text.as_bytes());
        Field::Text {
            make,
            offset,
            length: text.len(),
        }
    }

    /// Writes a type definition's node at the place reserved for it.
    fn root(&mut self, ty: &Type, slots: &[usize], at: usize) {
        if let Type::Reference(slot, args) = ty
            && args.is_empty()
        {
            self.set(at, Field::Word(d::ALIAS));
            self.set(at + 1, Field::Address(slots[slot.0]));
        } else {
            self.node(ty, slots, at);
        }
    }

    /// The address of a descriptor for `ty`, adding one if `ty` is not
    /// already a definition's.
    fn describe(&mut self, ty: &Type, slots: &[usize]) -> usize {
        if let Type::Reference(slot, args) = ty
            && args.is_empty()
        {
            return slots[slot.0];
        }
        let at = self.words;
        self.words += node_size(ty);
        self.node(ty, slots, at);
        at
    }

    /// Writes the node for `ty` at `at`, and the nodes it refers to after
    /// everything written so far.
    fn node(&mut self, ty: &Type, slots: &[usize], at: usize) {
        let tag = |statics: &mut Self, tag| statics.set(at, Field::Word(tag));
        match ty {
            Type::Never => tag(self, d::NEVER),
            Type::Unknown => tag(self, d::UNKNOWN),
            Type::Null => tag(self, d::NULL),
            Type::Bool => tag(self, d::BOOL),
            Type::Int => tag(self, d::INT),
            Type::Float => tag(self, d::FLOAT),
            Type::String => tag(self, d::STRING),
            Type::Function { .. } => tag(self, d::FUNCTION),
            Type::Literal(literal) => {
                tag(self, d::LITERAL);
                let (kind, payload) = match literal {
                    Literal::Bool(value) => (d::LITERAL_BOOL, Field::Word(u64::from(*value))),
                    Literal::Int(value) => (d::LITERAL_INT, Field::Word(value.cast_unsigned())),
                    Literal::Float(bits) => (d::LITERAL_FLOAT, Field::Word(*bits)),
                    Literal::String(text) => (d::LITERAL_STRING, self.text(Make::String, text)),
                };
                self.set(at + 1, Field::Word(kind));
                self.set(at + 2, payload);
            }
            Type::Record(fields) => {
                tag(self, d::RECORD);
                self.set(at + 1, Field::Word(fields.len() as u64));
                for (position, (name, ty)) in fields.iter().enumerate() {
                    let key = self.text(Make::Field, name);
                    self.set(at + 2 + position * 2, key);
                    let child = self.describe(ty, slots);
                    self.set(at + 3 + position * 2, Field::Address(child));
                }
            }
            Type::Tuple(members) | Type::Union(members) => {
                tag(
                    self,
                    if matches!(ty, Type::Tuple(_)) {
                        d::TUPLE
                    } else {
                        d::UNION
                    },
                );
                self.children(members, slots, at + 1);
            }
            Type::List(member) => {
                tag(self, d::LIST);
                let child = self.describe(member, slots);
                self.set(at + 1, Field::Address(child));
            }
            Type::Parameter(position) => {
                tag(self, d::PARAMETER);
                self.set(at + 1, Field::Word(*position as u64));
            }
            Type::Reference(slot, args) => {
                tag(self, d::APPLY);
                self.set(at + 1, Field::Address(slots[slot.0]));
                self.children(args, slots, at + 2);
            }
        }
    }

    /// Writes a count at `at` and then the address of a descriptor for
    /// each type.
    fn children(&mut self, types: &[Type], slots: &[usize], at: usize) {
        self.set(at, Field::Word(types.len() as u64));
        for (position, ty) in types.iter().enumerate() {
            let child = self.describe(ty, slots);
            self.set(at + 1 + position, Field::Address(child));
        }
    }
}
