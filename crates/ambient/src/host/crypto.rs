//! What `system.crypto` declares: hashing, HMAC, and a comparison that
//! takes the same time whatever it compares. See "Cryptography" in
//! `spec/targets.md`.
//!
//! None of it reaches a world, so each is offered as verifiable: called
//! again, it gives what it gave.

use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use subtle::ConstantTimeEq;

use super::bytes;
use crate::backend::native::Natives;
use crate::system::PACKAGE;
use crate::value::Value;

/// A way of hashing, as a tag of `system.crypto` names it.
#[derive(Clone, Copy)]
enum Algorithm {
    Sha256,
    Sha512,
    Sha1,
}

impl Algorithm {
    fn read(args: &[Value], position: usize) -> Result<Self, String> {
        let Some(Value::Tag { package, tag }) = args.get(position) else {
            return Err(format!("argument {position} is not a tag"));
        };
        match (package.as_str(), tag.as_str()) {
            (PACKAGE, "crypto.sha256") => Ok(Self::Sha256),
            (PACKAGE, "crypto.sha512") => Ok(Self::Sha512),
            (PACKAGE, "crypto.sha1") => Ok(Self::Sha1),
            _ => Err(format!("`{tag}` is not a way of hashing")),
        }
    }

    fn hash(self, data: &[u8]) -> Vec<u8> {
        match self {
            Self::Sha256 => Sha256::digest(data).to_vec(),
            Self::Sha512 => Sha512::digest(data).to_vec(),
            Self::Sha1 => Sha1::digest(data).to_vec(),
        }
    }

    fn hmac(self, key: &[u8], data: &[u8]) -> Vec<u8> {
        /// The HMAC by one hash. A key of any length is one it takes.
        fn signed<M: Mac + hmac::digest::KeyInit>(key: &[u8], data: &[u8]) -> Vec<u8> {
            let Ok(mut mac) = <M as Mac>::new_from_slice(key) else {
                return Vec::new();
            };
            mac.update(data);
            mac.finalize().into_bytes().to_vec()
        }
        match self {
            Self::Sha256 => signed::<Hmac<Sha256>>(key, data),
            Self::Sha512 => signed::<Hmac<Sha512>>(key, data),
            Self::Sha1 => signed::<Hmac<Sha1>>(key, data),
        }
    }
}

/// `natives`, with the functions of `system.crypto`.
pub(super) fn offer(natives: Natives) -> Natives {
    natives
        .verifiable(PACKAGE, "crypto.hash", |args| {
            let hashed = Algorithm::read(args, 0)?.hash(bytes(args, 1)?);
            Ok(Value::Bytes(hashed))
        })
        .verifiable(PACKAGE, "crypto.hmac", |args| {
            let signed = Algorithm::read(args, 0)?.hmac(bytes(args, 1)?, bytes(args, 2)?);
            Ok(Value::Bytes(signed))
        })
        .verifiable(PACKAGE, "crypto.same", |args| {
            let same = bytes(args, 0)?.ct_eq(bytes(args, 1)?);
            Ok(Value::Bool(same.into()))
        })
}
