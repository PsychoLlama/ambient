//! Compiled parts as bytes: what a host that has no compiler is given
//! to load. See "Packs" in `spec/exchange.md`.
//!
//! The bytes are trusted as the code is (`spec/exchange.md`, "Trust"):
//! decoding refuses what is not parts at all, or was made for another
//! backend, and nothing subtler.

use serde::{Deserialize, Serialize};

use super::{Backend, Body, Part, Parts};
use crate::core::{Slot, Type};
use crate::lir::Constant;

/// What the bytes begin with. Bump it when [`Parts`] changes, or
/// anything compiled code relies on: a layout, the calling convention,
/// an entry point of the runtime.
const FORMAT: u32 = 10;

/// Why bytes cannot be loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unloadable {
    /// They are not parts, or refer outside themselves.
    Malformed,
    /// They are parts as another version of the compiler wrote them.
    Format(u32),
    /// They were compiled for another backend, named here.
    Backend(String),
}

impl std::fmt::Display for Unloadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed => write!(f, "not compiled parts"),
            Self::Format(format) => write!(f, "compiled parts of format {format}, not {FORMAT}"),
            Self::Backend(name) => write!(f, "compiled for {name}"),
        }
    }
}

impl std::error::Error for Unloadable {}

/// What comes before the parts, and says whether to read them.
#[derive(Serialize, Deserialize)]
struct Header {
    format: u32,
    backend: String,
}

impl Parts {
    /// The parts as bytes, for a host with `backend` and no compiler.
    #[must_use]
    pub fn encode(&self, backend: &dyn Backend) -> Vec<u8> {
        let header = Header {
            format: FORMAT,
            backend: backend.name().to_owned(),
        };
        let mut bytes = crate::hash::encode(&header);
        bytes.extend(crate::hash::encode(self));
        bytes
    }

    /// The parts `bytes` hold, if they were compiled for the backend
    /// `name` names ([`Backend::name`]).
    ///
    /// # Errors
    ///
    /// Fails if the bytes are not parts, or not this backend's.
    pub fn decode(bytes: &[u8], name: &str) -> Result<Self, Unloadable> {
        let malformed = |_| Unloadable::Malformed;
        let (header, rest): (Header, _) = postcard::take_from_bytes(bytes).map_err(malformed)?;
        if header.format != FORMAT {
            return Err(Unloadable::Format(header.format));
        }
        if header.backend != name {
            return Err(Unloadable::Backend(header.backend));
        }
        let parts: Self = postcard::from_bytes(rest).map_err(malformed)?;
        if parts.closed() {
            Ok(parts)
        } else {
            Err(Unloadable::Malformed)
        }
    }

    /// Whether everything the parts refer to by position is there.
    fn closed(&self) -> bool {
        let part = |part: &Part| {
            part.slots.iter().all(|slot| *slot < self.parts.len())
                && part.native.is_none_or(|native| native < self.natives.len())
                && part.body.closed(part.slots.len())
        };
        self.entry < self.parts.len() && self.parts.iter().all(part)
    }
}

impl Body {
    /// Whether every slot the body names is one of its part's `slots`.
    fn closed(&self, slots: usize) -> bool {
        let slot = |slot: &usize| *slot < slots;
        let ty = |ty: &Type| ty.slots().iter().all(|Slot(position)| slot(position));
        match self {
            Self::Function {
                constants,
                signature,
                ..
            } => {
                let constant = |constant: &Constant| match constant {
                    Constant::Type(of) => ty(of),
                    Constant::Types(types) => types.iter().all(ty),
                    Constant::Instance(function, types) => slot(function) && types.iter().all(ty),
                    Constant::Ability(ability) | Constant::AbilityBudget(ability) => slot(ability),
                    Constant::String(_)
                    | Constant::Tag(_)
                    | Constant::Field(_)
                    | Constant::Native
                    | Constant::Own => true,
                };
                constants.iter().all(constant) && signature.as_ref().is_none_or(ty)
            }
            Self::Type(of) => ty(of),
            Self::Inert => true,
        }
    }
}
