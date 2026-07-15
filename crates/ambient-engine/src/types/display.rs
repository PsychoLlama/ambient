//! Display implementations for pretty printing.

use std::fmt;

use super::{AbilitySet, Type, uuid_to_source};

impl fmt::Display for AbilitySet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "{{}}"),
            Self::Concrete(abilities) => {
                write!(f, "{{")?;
                for (i, ability) in abilities.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "#{ability}")?;
                }
                write!(f, "}}")
            }
            Self::Var(id) => write!(f, "E{id}!"),
            Self::Row { concrete, tail } => {
                write!(f, "{{")?;
                for (i, ability) in concrete.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "#{ability}")?;
                }
                write!(f, ", E{tail}!}}")
            }
            Self::Unresolved(names) => {
                write!(f, "{{")?;
                for (i, name) in names.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{name}?")?;
                }
                write!(f, "}}")
            }
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit => write!(f, "()"),
            Self::Never => write!(f, "!"),
            Self::Error => write!(f, "<error>"),
            Self::Hole => write!(f, "_"),

            Self::Var(id) => write!(f, "'{id}"),

            // A rigid type parameter prints as its bare source name, so a
            // diagnostic about `T` reads `T` — not `'3` or `named:T`.
            Self::Param(name) => write!(f, "{name}"),

            // An associated-type projection prints as written:
            // `Self::Error`, `T::Error`.
            Self::Projection(p) => write!(f, "{}::{}", p.base, p.assoc),

            Self::Tuple(elems) => {
                write!(f, "(")?;
                for (i, elem) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{elem}")?;
                }
                write!(f, ")")
            }

            Self::Record(rec) => {
                write!(f, "{{ ")?;
                for (i, (name, ty)) in rec.fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{name}: {ty}")?;
                }
                write!(f, " }}")
            }

            Self::Function(func) => {
                write!(f, "(")?;
                for (i, param) in func.params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{param}")?;
                }
                write!(f, ") -> {}", func.ret)?;
                if !func.abilities.is_empty() {
                    write!(f, " with {}", func.abilities)?;
                }
                Ok(())
            }

            Self::Named(named) => {
                write!(f, "{}", named.name)?;
                if !named.args.is_empty() {
                    write!(f, "<")?;
                    for (i, arg) in named.args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{arg}")?;
                    }
                    write!(f, ">")?;
                }
                Ok(())
            }

            Self::Nominal(nom) => {
                if let Some(name) = &nom.name {
                    write!(f, "{name}")
                } else {
                    write!(f, "unique({})", uuid_to_source(&nom.uuid))
                }
            }

            Self::Handler(handler) => {
                write!(f, "Handler<#{}, {}>", handler.ability, handler.answer)
            }

            // The unresolved surface form (pre-`resolve_holes`); shown as
            // written, for diagnostics that render a raw annotation.
            Self::HandlerAnnotation(h) => match &h.answer {
                Some(answer) => write!(f, "Handler<{}, {answer}>", h.ability),
                None => write!(f, "Handler<{}>", h.ability),
            },

            Self::Forall(forall) => {
                write!(f, "forall ")?;
                for (i, var) in forall.vars.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "'{var}")?;
                }
                for (i, var) in forall.ability_vars.iter().enumerate() {
                    if !forall.vars.is_empty() || i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "E{var}!")?;
                }
                write!(f, ". {}", forall.body)
            }
        }
    }
}
