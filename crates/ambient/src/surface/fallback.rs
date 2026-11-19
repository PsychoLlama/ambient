//! `value ?? otherwise`: a match the reader arranges, which the checker
//! knows again so that an error is about what was written.

use super::{Arm, Expr, Path, Pattern, Type};

/// What begins the name the value is kept under where it is not null.
/// No program can write it.
const KEPT: &str = "__kept";

/// `value` where it is not null, and `otherwise` where it is. `number`
/// tells this fallback from the others of its definition.
#[must_use]
pub fn fallback(value: Expr, number: usize, otherwise: Expr) -> Expr {
    let kept = format!("{KEPT}{number}");
    let missed = Arm {
        pattern: Pattern::Is(Type::Null),
        body: otherwise,
    };
    let found = Arm {
        body: Expr::Name(Path(vec![kept.clone()])),
        pattern: Pattern::Bind(kept, Box::new(Pattern::Wildcard)),
    };
    Expr::Match {
        subject: Box::new(value),
        arms: vec![missed, found],
    }
}

/// Whether `arms` are those of a fallback.
#[must_use]
pub fn is_fallback<N>(arms: &[Arm<N>]) -> bool {
    matches!(arms, [_, Arm { pattern: Pattern::Bind(kept, _), .. }] if kept.starts_with(KEPT))
}
