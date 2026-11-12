//! Walking values without recursion. A value can nest deeper than the
//! stack, so nothing that goes through one may recurse as deep as it
//! nests: not building, cloning, comparing, or dropping it.

use super::{Function, Value};

/// Puts a value together from the values that go inside it.
pub(super) type Finish<'a> = Box<dyn FnOnce(Vec<Value>) -> Value + 'a>;

/// What a source of a value turns out to be.
pub(super) enum Step<'a, S> {
    Leaf(Value),
    /// The sources of what is inside it, in order, and how to put the
    /// results together.
    Inside(Vec<S>, Finish<'a>),
}

/// The value `root` is a source of, where `step` says what each source
/// is.
pub(super) fn build<'a, S>(root: S, mut step: impl FnMut(S) -> Step<'a, S>) -> Value {
    // The values being built, outermost first: the sources still to
    // go, last first, and the values already made.
    let mut open: Vec<(Vec<S>, Vec<Value>, Finish<'a>)> = Vec::new();
    let mut next = root;
    loop {
        let mut value = match step(next) {
            Step::Leaf(value) => value,
            Step::Inside(mut inside, finish) => {
                inside.reverse();
                match inside.pop() {
                    Some(first) => {
                        open.push((inside, Vec::new(), finish));
                        next = first;
                        continue;
                    }
                    None => finish(Vec::new()),
                }
            }
        };
        // The value goes inside whatever is open, and may complete it.
        loop {
            let Some((inside, made, _)) = open.last_mut() else {
                return value;
            };
            made.push(value);
            if let Some(source) = inside.pop() {
                next = source;
                break;
            }
            let Some((_, made, finish)) = open.pop() else {
                return Value::Null;
            };
            value = finish(made);
        }
    }
}

impl Value {
    /// Takes the values directly inside this one out of it.
    fn take_members(&mut self) -> Vec<Value> {
        match self {
            Self::Tuple(members) | Self::List(members) => std::mem::take(members),
            Self::Record(fields) => std::mem::take(fields).into_values().collect(),
            Self::Function(function) => std::mem::take(&mut function.captures),
            _ => Vec::new(),
        }
    }
}

impl Drop for Value {
    fn drop(&mut self) {
        let mut pending = self.take_members();
        while let Some(mut value) = pending.pop() {
            pending.append(&mut value.take_members());
        }
    }
}

impl Clone for Value {
    fn clone(&self) -> Self {
        build(self, |value| match value {
            Self::Null => Step::Leaf(Self::Null),
            Self::Bool(value) => Step::Leaf(Self::Bool(*value)),
            Self::Int(value) => Step::Leaf(Self::Int(*value)),
            Self::Float(value) => Step::Leaf(Self::Float(*value)),
            Self::String(text) => Step::Leaf(Self::String(text.clone())),
            Self::Tag { package, tag } => Step::Leaf(Self::Tag {
                package: package.clone(),
                tag: tag.clone(),
            }),
            Self::Cell => Step::Leaf(Self::Cell),
            Self::Token => Step::Leaf(Self::Token),
            Self::Tuple(members) => Step::Inside(members.iter().collect(), Box::new(Self::Tuple)),
            Self::List(members) => Step::Inside(members.iter().collect(), Box::new(Self::List)),
            Self::Record(fields) => {
                let record =
                    |values: Vec<Self>| Self::Record(fields.keys().cloned().zip(values).collect());
                Step::Inside(fields.values().collect(), Box::new(record))
            }
            Self::Function(function) => {
                let definition = function.definition;
                let made = move |captures| {
                    Self::Function(Function {
                        definition,
                        captures,
                    })
                };
                Step::Inside(function.captures.iter().collect(), Box::new(made))
            }
        })
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        let mut pending = vec![(self, other)];
        while let Some(pair) = pending.pop() {
            let same = match pair {
                (Self::Null, Self::Null)
                | (Self::Cell, Self::Cell)
                | (Self::Token, Self::Token) => true,
                (Self::Bool(left), Self::Bool(right)) => left == right,
                (Self::Int(left), Self::Int(right)) => left == right,
                (Self::Float(left), Self::Float(right)) => left == right,
                (Self::String(left), Self::String(right)) => left == right,
                (
                    Self::Tag { package, tag },
                    Self::Tag {
                        package: other_package,
                        tag: other_tag,
                    },
                ) => package == other_package && tag == other_tag,
                (Self::Tuple(left), Self::Tuple(right)) | (Self::List(left), Self::List(right)) => {
                    pending.extend(left.iter().zip(right));
                    left.len() == right.len()
                }
                (Self::Record(left), Self::Record(right)) => {
                    pending.extend(left.values().zip(right.values()));
                    left.keys().eq(right.keys())
                }
                (Self::Function(left), Self::Function(right)) => {
                    pending.extend(left.captures.iter().zip(&right.captures));
                    left.definition == right.definition
                        && left.captures.len() == right.captures.len()
                }
                _ => false,
            };
            if !same {
                return false;
            }
        }
        true
    }
}
