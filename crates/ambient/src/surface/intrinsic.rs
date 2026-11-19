//! Intrinsics: the names the compiler knows. See "Intrinsics" in
//! `spec/definitions.md`.

use super::{CellOp, Operator};

/// The module of the standard library that every intrinsic is declared
/// in. Other modules give some of them again under their own names.
pub const MODULE: &str = "intrinsics";

/// What a call of an intrinsic is: the node of a tree that the call is
/// written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intrinsic {
    /// An operation on its operands.
    Operation(Operator),
    /// Something done with a cell.
    Cell(CellOp),
    /// What an observer is told an ability is.
    AbilityId,
    /// A boundary around a body.
    Contain,
    /// A recording of a body, for an observer.
    Observe,
    /// A group around a body.
    Group,
    /// A task of the group it is written in.
    Spawn,
    /// What every branch gives, each run as a task.
    All,
    /// What the first branch to be over gives.
    First,
}

/// The intrinsics that are no operation, by name.
const NAMED: [(&str, Intrinsic); 11] = [
    ("cell", Intrinsic::Cell(CellOp::New)),
    ("read", Intrinsic::Cell(CellOp::Read)),
    ("replace", Intrinsic::Cell(CellOp::Replace)),
    ("wait", Intrinsic::Cell(CellOp::Wait)),
    ("ability_id", Intrinsic::AbilityId),
    ("contain", Intrinsic::Contain),
    ("observe", Intrinsic::Observe),
    ("group", Intrinsic::Group),
    ("spawn", Intrinsic::Spawn),
    ("all", Intrinsic::All),
    ("first", Intrinsic::First),
];

impl Intrinsic {
    /// The intrinsic a name is for, if it is for one.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        let named = NAMED.iter().find(|(known, _)| *known == name);
        named
            .map(|(_, intrinsic)| *intrinsic)
            .or_else(|| Operator::named(name).map(Self::Operation))
    }

    /// The name of every intrinsic.
    pub fn names() -> impl Iterator<Item = &'static str> {
        let operations = Operator::ALL.into_iter().map(Operator::name);
        NAMED.iter().map(|(name, _)| *name).chain(operations)
    }

    /// Whether a call of it takes a body: a function of no parameters
    /// written in place, which is run where the call is.
    #[must_use]
    pub fn takes_bodies(self) -> bool {
        matches!(
            self,
            Self::Contain | Self::Observe | Self::Group | Self::Spawn | Self::All | Self::First
        )
    }
}
