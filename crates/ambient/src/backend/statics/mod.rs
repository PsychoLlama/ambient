//! What a program needs in memory before it runs, and where: every
//! function's definition and closure, and every type's descriptor.
//!
//! Both backends load a program the same way, so the layout is planned
//! here once. A backend only has to write each [`Field`] where it says.
//!
//! A block loaded into a program that is running is a unit
//! (`ambient_runtime::UNIT_WORDS`): it begins with the unit's record,
//! and a word that names the unit goes before whatever can be referred
//! to from outside the block. In a program's first block that word is
//! zero.

use ambient_runtime::UNIT_WORDS;
use ambient_runtime::layout::{CLOSURE_WORDS, DEFINITION_WORDS};

mod descriptors;

use descriptors::root_size;

use super::identity;
use crate::core::Type;
use crate::engine::{Body, Parts};
use crate::hash::Hash;
use crate::lir::Constant;

/// What goes in one word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Word(u64),
    /// The address of another word of the block.
    Address(usize),
    /// Code, however the backend refers to code: by its position in
    /// [`Parts::codes`].
    Code(usize),
    /// The closure or descriptor of a part the block lays out, by the
    /// part's position: where [`Statics::placed`] says, unless whoever
    /// loads the block finds the program has the part already.
    Root(usize),
    /// The definition of such a part, likewise.
    Definition(usize),
    /// An extern's position among the program's: the one given, among
    /// the block's own, after however many the program had.
    Native(usize),
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
    /// A tag value.
    Tag,
}

/// Where something a block refers to is: in the block, by position;
/// in a block the program is known to have, by address; or wherever a
/// part the block lays out turns out to be, as [`Field::Root`] and
/// [`Field::Definition`] say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Here(usize),
    There(u64),
    Root(usize),
    Definition(usize),
}

impl Place {
    fn field(self) -> Field {
        match self {
            Self::Here(word) => Field::Address(word),
            Self::There(address) => Field::Word(address),
            Self::Root(part) => Field::Root(part),
            Self::Definition(part) => Field::Definition(part),
        }
    }
}

/// A definition a program already has: where its closure or descriptor
/// is, where its definition is if it is a function, and the unit both
/// are in, or zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Known {
    pub root: u64,
    pub definition: u64,
    pub unit: u64,
}

/// The program a block is planned for, if it is not the program's first.
pub struct Base<'a> {
    /// The definitions it is known to have, by hash. A part that is one
    /// of them is not laid out again.
    pub known: &'a dyn Fn(&Hash) -> Option<Known>,
    /// Whether the block is a unit: one reclaimed before the program is.
    pub loaded: bool,
}

fn unknown(_: &Hash) -> Option<Known> {
    None
}

impl Base<'_> {
    /// No program: the block is a program's first.
    #[must_use]
    pub fn none() -> Self {
        Base {
            known: &unknown,
            loaded: false,
        }
    }
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
    pub entry: Option<Place>,
    /// Where each part that was laid out is: its closure or descriptor,
    /// and its definition. Nothing for a part the program had.
    pub placed: Vec<Option<(usize, usize)>>,
    /// Whether the block is a unit, and so begins with its record.
    pub loaded: bool,
}

/// Plans the block for a program, or for what is added to one.
#[must_use]
pub fn plan(parts: &Parts, base: &Base) -> Statics {
    let mut statics = Statics {
        loaded: base.loaded,
        words: if base.loaded { UNIT_WORDS } else { 0 },
        ..Statics::default()
    };

    let (definitions, roots) = statics.reserve(parts, base);
    statics.entry = Some(roots[parts.entry]);

    let mut header = [0u64; CLOSURE_WORDS];
    ambient_runtime::fixed_closure(&mut header, 0, 0, base.loaded);
    for (index, part) in parts.parts.iter().enumerate() {
        let slots: Vec<Target> = part
            .slots
            .iter()
            .map(|slot| Target {
                address: roots[*slot],
                definition: definitions[*slot],
                hash: parts.parts[*slot].hash,
            })
            .collect();
        let Some((closure, at)) = statics.placed[index] else {
            continue;
        };
        match &part.body {
            Body::Function {
                arity,
                constants,
                metered,
                signature,
                described,
                types,
                ..
            } => {
                // Without a metered form, the plain one stands in for it.
                let plain = index * 2;
                statics.set(at, Field::Code(plain));
                statics.set(at + 1, Field::Code(plain + usize::from(metered.is_some())));
                statics.set(at + 2, Field::Word(*arity as u64));
                let hash = statics.text(Make::String, &part.hash.to_string());
                statics.set(at + 3, hash);
                if let Some(signature) = signature {
                    let name = statics.type_name(signature, &slots);
                    statics.set(at + 4, name);
                }
                statics.set(at + 5, Field::Word(*types as u64));
                if let Some(described) = described {
                    let node = statics.constant(described, &slots);
                    statics.set(at + 6, node.field());
                }
                for (position, slot) in slots.iter().enumerate() {
                    statics.set(at + DEFINITION_WORDS + position, slot.address.field());
                }
                let first = at + DEFINITION_WORDS + slots.len();
                for (position, constant) in constants.iter().enumerate() {
                    let field = match constant {
                        Constant::String(text) => statics.text(Make::String, text),
                        Constant::Tag(text) => statics.text(Make::Tag, text),
                        Constant::Field(name) => statics.text(Make::Field, name),
                        Constant::Type(ty) => statics.constant(ty, &slots).field(),
                        Constant::Types(types) => Field::Address(statics.types(types, &slots)),
                        Constant::Instance(slot, types) => {
                            Field::Address(statics.instance(&slots[*slot], types, &slots))
                        }
                        Constant::Ability(slot) => {
                            statics.text(Make::Field, &ability_key(&slots[*slot].hash))
                        }
                        Constant::AbilityBudget(slot) => {
                            let key = format!("{}$", ability_key(&slots[*slot].hash));
                            statics.text(Make::Field, &key)
                        }
                        Constant::Native => Field::Native(part.native.unwrap_or(0)),
                        Constant::Own => statics.text(Make::String, &part.hash.to_string()),
                    };
                    statics.set(first + position, field);
                }
                statics.set(closure, Field::Word(header[0]));
                statics.set(closure + 1, Field::Word(header[1]));
                statics.set(closure + 2, Field::Address(at));
            }
            Body::Type(ty) => statics.root(ty, &slots, at),
            Body::Inert => {}
        }
    }
    statics
}

/// What a slot of the definition being laid out refers to.
struct Target {
    /// Where its closure or descriptor is.
    address: Place,
    /// Where its definition is, if it is a function.
    definition: Place,
    hash: Hash,
}

/// The name bindings hold an ability under. No field has a name like it,
/// so bindings are records like any other.
fn ability_key(hash: &Hash) -> String {
    format!("@{hash}")
}

impl Statics {
    /// Makes room for each part the program lacks, and says where each
    /// part's definition and its closure or descriptor are. These are
    /// fixed before anything is written, because parts refer to each
    /// other freely.
    fn reserve(&mut self, parts: &Parts, base: &Base) -> (Vec<Place>, Vec<Place>) {
        // Where each part is found: a function's closure, a type's descriptor.
        // These are fixed first, because parts refer to each other freely.
        let mut definitions = Vec::new();
        let mut roots = Vec::new();
        for (parts_placed, part) in parts.parts.iter().enumerate() {
            let known = match part.body {
                Body::Inert => None,
                _ => (base.known)(&part.hash),
            };
            if let Some(known) = known {
                definitions.push(Place::There(known.definition));
                roots.push(Place::There(known.root));
                self.placed.push(None);
                continue;
            }
            let at = self.held();
            definitions.push(Place::Definition(parts_placed));
            let root = match &part.body {
                Body::Function { constants, .. } => {
                    self.words += DEFINITION_WORDS + part.slots.len() + constants.len();
                    let closure = self.held();
                    self.words += CLOSURE_WORDS;
                    closure
                }
                Body::Type(ty) => {
                    self.words += root_size(ty);
                    at
                }
                // Never read: an ability is reached by its key, a constant.
                Body::Inert => 0,
            };
            roots.push(Place::Root(parts_placed));
            self.placed.push(Some((root, at)));
        }
        (definitions, roots)
    }

    /// Makes room for the word that names the block's unit, and says
    /// where what it precedes begins.
    fn held(&mut self) -> usize {
        if self.loaded {
            self.set(self.words, Field::Address(0));
        }
        self.words += 1;
        self.words
    }

    /// The address of a descriptor something outside the block may
    /// keep: a cell made at the type, or type arguments.
    fn constant(&mut self, ty: &Type, slots: &[Target]) -> Place {
        if !matches!(ty, Type::Reference(_, args) if args.is_empty()) {
            self.held();
        }
        self.describe(ty, slots)
    }

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

    /// The name of a function type, as one interned word, or zero if the
    /// type mentions a type parameter.
    fn type_name(&mut self, ty: &Type, slots: &[Target]) -> Field {
        match identity::name(ty, &|slot| slots[slot.0].hash) {
            Some(name) => self.text(Make::Field, &name),
            None => Field::Word(0),
        }
    }

    /// The address of type arguments laid out in the block: an object
    /// that holds a descriptor for each, read in nothing.
    fn types(&mut self, types: &[Type], slots: &[Target]) -> usize {
        let mut header = [0u64; 2];
        ambient_runtime::fixed_scope(&mut header, types.len(), self.loaded);
        let at = self.held();
        self.words += header.len() + types.len() * 2;
        self.set(at, Field::Word(header[0]));
        self.set(at + 1, Field::Word(header[1]));
        for (position, ty) in types.iter().enumerate() {
            let node = self.constant(ty, slots);
            self.set(at + 2 + position * 2, node.field());
            self.set(at + 3 + position * 2, Field::Word(ambient_runtime::NULL));
        }
        at
    }

    /// The address of a closure laid out in the block, over a generic
    /// function at type arguments.
    fn instance(&mut self, function: &Target, types: &[Type], slots: &[Target]) -> usize {
        let mut header = [0u64; CLOSURE_WORDS];
        ambient_runtime::fixed_closure(&mut header, 0, 1, self.loaded);
        let at = self.held();
        self.words += CLOSURE_WORDS + 1;
        self.set(at, Field::Word(header[0]));
        self.set(at + 1, Field::Word(header[1]));
        self.set(at + 2, function.definition.field());
        let types = self.types(types, slots);
        self.set(at + 3, Field::Address(types));
        at
    }
}
