//! The descriptors of a block: a type as the words a value is tested
//! against (`ambient_runtime::descriptor`).

use ambient_runtime::descriptor as d;

use super::{Field, Make, Place, Statics, Target, ability_key};
use crate::core::{Key, Type};
use crate::literal::Literal;

/// How many words a type definition's own node takes.
pub(super) fn root_size(ty: &Type) -> usize {
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
        | Type::Tag => 1,
        Type::Function { params, row, .. } => 5 + params.len() + row.len(),
        Type::Literal(_) => 3,
        Type::Record(fields) => 2 + fields.len() * 2,
        Type::Tuple(members) | Type::Union(members) => 2 + members.len(),
        Type::List(_) | Type::Cell(_) | Type::Parameter(_) => 2,
        Type::Reference(_, args) => 3 + args.len(),
    }
}

impl Statics {
    /// Writes a type definition's node at the place reserved for it.
    pub(super) fn root(&mut self, ty: &Type, slots: &[Target], at: usize) {
        if let Type::Reference(slot, args) = ty
            && args.is_empty()
        {
            self.set(at, Field::Word(d::ALIAS));
            self.set(at + 1, slots[slot.0].address.field());
        } else {
            self.node(ty, slots, at);
        }
    }

    /// The address of a descriptor for `ty`, adding one if `ty` is not
    /// already a definition's.
    pub(super) fn describe(&mut self, ty: &Type, slots: &[Target]) -> Place {
        if let Type::Reference(slot, args) = ty
            && args.is_empty()
        {
            return slots[slot.0].address;
        }
        let at = self.words;
        self.words += node_size(ty);
        self.node(ty, slots, at);
        Place::Here(at)
    }

    /// Writes the node for `ty` at `at`, and the nodes it refers to after
    /// everything written so far.
    fn node(&mut self, ty: &Type, slots: &[Target], at: usize) {
        let tag = |statics: &mut Self, tag| statics.set(at, Field::Word(tag));
        match ty {
            Type::Never => tag(self, d::NEVER),
            Type::Unknown => tag(self, d::UNKNOWN),
            Type::Null => tag(self, d::NULL),
            Type::Bool => tag(self, d::BOOL),
            Type::Int => tag(self, d::INT),
            Type::Float => tag(self, d::FLOAT),
            Type::String => tag(self, d::STRING),
            Type::Tag => tag(self, d::TAG),
            Type::Function { params, ret, row } => {
                tag(self, d::FUNCTION);
                self.children(params, slots, at + 1);
                let after = at + 2 + params.len();
                let ret = self.describe(ret, slots);
                self.set(after, ret.field());
                self.set(after + 1, Field::Word(row.len() as u64));
                for (position, slot) in row.iter().enumerate() {
                    let key = self.text(Make::Field, &ability_key(&slots[slot.0].hash));
                    self.set(after + 2 + position, key);
                }
                let name = self.type_name(ty, slots);
                self.set(after + 2 + row.len(), name);
            }
            Type::Literal(literal) => {
                tag(self, d::LITERAL);
                let (kind, payload) = match literal {
                    Literal::Bool(value) => (d::LITERAL_BOOL, Field::Word(u64::from(*value))),
                    Literal::Int(value) => (d::LITERAL_INT, Field::Word(value.cast_unsigned())),
                    Literal::Float(bits) => (d::LITERAL_FLOAT, Field::Word(*bits)),
                    Literal::String(text) => (d::LITERAL_STRING, self.text(Make::String, text)),
                    Literal::Tag { package, tag } => {
                        let text = Literal::tag_text(package, tag);
                        (d::LITERAL_TAG, self.text(Make::Tag, &text))
                    }
                };
                self.set(at + 1, Field::Word(kind));
                self.set(at + 2, payload);
            }
            Type::Record(fields) => {
                tag(self, d::RECORD);
                self.set(at + 1, Field::Word(fields.len() as u64));
                for (position, (key, ty)) in fields.iter().enumerate() {
                    let key = match key {
                        Key::Field(name) => self.text(Make::Field, name),
                        Key::Ability(slot) => {
                            self.text(Make::Field, &ability_key(&slots[slot.0].hash))
                        }
                    };
                    self.set(at + 2 + position * 2, key);
                    let child = self.describe(ty, slots);
                    self.set(at + 3 + position * 2, child.field());
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
                self.set(at + 1, child.field());
            }
            Type::Cell(member) => {
                tag(self, d::CELL);
                let child = self.describe(member, slots);
                self.set(at + 1, child.field());
            }
            Type::Parameter(position) => {
                tag(self, d::PARAMETER);
                self.set(at + 1, Field::Word(*position as u64));
            }
            Type::Reference(slot, args) => {
                tag(self, d::APPLY);
                self.set(at + 1, slots[slot.0].address.field());
                self.children(args, slots, at + 2);
            }
        }
    }

    /// Writes a count at `at` and then the address of a descriptor for
    /// each type.
    fn children(&mut self, types: &[Type], slots: &[Target], at: usize) {
        self.set(at, Field::Word(types.len() as u64));
        for (position, ty) in types.iter().enumerate() {
            let child = self.describe(ty, slots);
            self.set(at + 1 + position, child.field());
        }
    }
}
