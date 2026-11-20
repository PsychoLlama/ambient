//! A constant's value, laid out in a block: every object it holds is
//! one the block has, so reading it makes nothing. See "Cost" in
//! `docs/spec/constants.md`.
//!
//! A value is read by the forms `core::Expr::is_value_form` allows and
//! no other. Whatever else it met would have been refused where the
//! constant was checked, so here it is null.

use std::collections::BTreeMap;

use ambient_runtime::{Fixed, NULL, fixed_count, fixed_header, fixed_int, fixed_list};

use super::{Field, Make, Place, Statics, ability_key};
use crate::core::{Expr, Key, Slot, Type};
use crate::literal::Literal;
use crate::parts::{Body, Parts};

/// What laying values out reads of the program, and the constants laid
/// out so far.
pub(super) struct Values<'a> {
    pub(super) parts: &'a Parts,
    /// Where each part's closure or descriptor is.
    pub(super) roots: &'a [Place],
    /// Where each part's definition is.
    pub(super) definitions: &'a [Place],
    /// What each constant's value is, by its part. One being laid out
    /// holds nothing yet.
    made: BTreeMap<usize, Option<Field>>,
}

impl<'a> Values<'a> {
    pub(super) fn new(parts: &'a Parts, roots: &'a [Place], definitions: &'a [Place]) -> Self {
        Self {
            parts,
            roots,
            definitions,
            made: BTreeMap::new(),
        }
    }

    /// The word that is the value of the constant that is part `part`.
    /// It is laid out once, so every constant that holds this one
    /// holds the same objects.
    pub(super) fn constant(&mut self, part: usize, statics: &mut Statics) -> Field {
        match self.made.get(&part) {
            Some(Some(field)) => return *field,
            // A constant that holds itself was refused when it was
            // checked. Nothing here goes round for one that was not.
            Some(None) => return Field::Word(NULL),
            None => {}
        }
        let Body::Constant(value) = &self.parts.parts[part].body else {
            return Field::Word(NULL);
        };
        self.made.insert(part, None);
        let field = self.value(value, part, statics);
        self.made.insert(part, Some(field));
        field
    }

    /// The part a slot of part `of` refers to.
    fn target(&self, of: usize, slot: Slot) -> usize {
        self.parts.parts[of].slots[slot.0]
    }

    /// The word that is `value`, an expression of part `of`.
    fn value(&mut self, value: &Expr, of: usize, statics: &mut Statics) -> Field {
        match value {
            Expr::Literal(literal) => literal_field(literal, statics),
            Expr::Constant(slot) => self.constant(self.target(of, *slot), statics),
            Expr::Reference(slot, types) => self.function(of, *slot, types, statics),
            Expr::Instantiate { function, types } => match &**function {
                Expr::Reference(slot, _) => self.function(of, *slot, types, statics),
                _ => Field::Word(NULL),
            },
            Expr::Tuple(members) => {
                let members = self.values(members, of, statics);
                Field::Address(statics.object(Fixed::Tuple, &members))
            }
            Expr::List(members) => {
                let members = self.values(members, of, statics);
                Field::Address(statics.list(&members))
            }
            Expr::Record(fields) => {
                let mut words = Vec::new();
                for (key, value) in fields {
                    words.push(self.key(key, of, statics));
                    words.push(self.value(value, of, statics));
                }
                let record = statics.object(Fixed::Record, &words);
                statics.settle.push(record);
                Field::Address(record)
            }
            Expr::Closure { .. } => self.closure(value, Fixed::Closure, of, statics),
            Expr::Method(closure) => self.closure(closure, Fixed::Method, of, statics),
            Expr::Widen { value, .. }
            | Expr::Brand { value, .. }
            | Expr::Annotated { value, .. }
            | Expr::At(_, value) => self.value(value, of, statics),
            _ => Field::Word(NULL),
        }
    }

    fn values(&mut self, values: &[Expr], of: usize, statics: &mut Statics) -> Vec<Field> {
        let value = |value| self.value(value, of, statics);
        values.iter().map(value).collect()
    }

    /// The key a record holds a field under: a name, or what bindings
    /// hold an ability under.
    fn key(&self, key: &Key, of: usize, statics: &mut Statics) -> Field {
        match key {
            Key::Field(name) => statics.text(Make::Field, name),
            Key::Ability(slot) => {
                let hash = self.parts.parts[self.target(of, *slot)].hash;
                statics.text(Make::Field, &ability_key(&hash))
            }
        }
    }

    /// A function named, as a value: its closure, or one over it that
    /// holds the type arguments it is handed as it runs.
    fn function(&mut self, of: usize, slot: Slot, types: &[Type], statics: &mut Statics) -> Field {
        let target = self.target(of, slot);
        let handed =
            matches!(self.parts.parts[target].body, Body::Function { types, .. } if types > 0);
        if !handed || types.is_empty() {
            return self.roots[target].field();
        }
        let slots = statics.targets(self.parts, of, self.roots, self.definitions);
        let definition = self.definitions[target];
        Field::Address(statics.instance(definition, types, &slots))
    }

    /// A closure a lambda written in a value makes: its definition,
    /// what it captured, and the type arguments it is handed.
    fn closure(&mut self, closure: &Expr, shape: Fixed, of: usize, statics: &mut Statics) -> Field {
        let Expr::Closure {
            slot,
            type_args,
            captures,
            ..
        } = closure
        else {
            return Field::Word(NULL);
        };
        let target = self.target(of, *slot);
        let mut words = vec![self.definitions[target].field()];
        words.extend(self.values(captures, of, statics));
        let handed =
            matches!(self.parts.parts[target].body, Body::Function { types, .. } if types > 0);
        if handed && !type_args.is_empty() {
            let slots = statics.targets(self.parts, of, self.roots, self.definitions);
            words.push(Field::Address(statics.types(type_args, &slots)));
        }
        Field::Address(statics.object(shape, &words))
    }
}

/// The word that is a literal: itself if it is immediate, the one tag
/// the runtime has for a tag, and an object of the block otherwise.
///
/// A string is laid out and not made, though the runtime makes a
/// function's: a record's fields are put in order after they are
/// written, so nothing could say afterwards which word of one held a
/// string for a unit to give back.
fn literal_field(literal: &Literal, statics: &mut Statics) -> Field {
    match literal {
        Literal::Bool(true) => Field::Word(ambient_runtime::TRUE),
        Literal::Bool(false) => Field::Word(ambient_runtime::FALSE),
        Literal::Int(value) => match fixed_int(*value) {
            Some(word) => Field::Word(word),
            // The integer's bits, as the word it is kept in.
            None => {
                Field::Address(statics.object(Fixed::Int, &[Field::Word(value.cast_unsigned())]))
            }
        },
        Literal::Float(bits) => Field::Address(statics.object(Fixed::Float, &[Field::Word(*bits)])),
        Literal::String(text) => {
            Field::Address(statics.text_object(Fixed::String, text.as_bytes()))
        }
        Literal::Bytes(bytes) => Field::Address(statics.text_object(Fixed::Bytes, bytes)),
        Literal::Tag { package, tag } => statics.text(Make::Tag, &Literal::tag_text(package, tag)),
    }
}

impl Statics {
    /// An object of `shape` that holds `words`, laid out in the block,
    /// and where it is.
    fn object(&mut self, shape: Fixed, words: &[Field]) -> usize {
        self.sized(shape, words.len() * 8, words)
    }

    /// A string or bytes that holds `bytes`: they are packed into
    /// words lowest byte first, which is how every machine a backend
    /// emits for keeps a word, as an object's header already counts on.
    fn text_object(&mut self, shape: Fixed, bytes: &[u8]) -> usize {
        let word = |chunk: &[u8]| {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            Field::Word(u64::from_le_bytes(word))
        };
        let words: Vec<Field> = bytes.chunks(8).map(word).collect();
        self.sized(shape, bytes.len(), &words)
    }

    /// An object of `shape` whose payload is `size` bytes, held in
    /// `words`.
    fn sized(&mut self, shape: Fixed, size: usize, words: &[Field]) -> usize {
        let header = fixed_header(shape, size, self.loaded);
        let at = self.held();
        self.words += header.len() + words.len();
        self.set(at, Field::Word(header[0]));
        self.set(at + 1, Field::Word(header[1]));
        for (position, word) in words.iter().enumerate() {
            self.set(at + 2 + position, *word);
        }
        at
    }

    /// A list of `members`: a view of a buffer that has room for no
    /// more, so that a push onto it copies them and writes nothing
    /// here.
    fn list(&mut self, members: &[Field]) -> usize {
        let view = fixed_list(members.len());
        let buffer = if members.is_empty() {
            Field::Word(NULL)
        } else {
            let mut held = vec![Field::Word(fixed_count(members.len()))];
            held.extend_from_slice(members);
            Field::Address(self.object(Fixed::Buffer, &held))
        };
        self.object(
            Fixed::List,
            &[buffer, Field::Word(view[0]), Field::Word(view[1])],
        )
    }
}
