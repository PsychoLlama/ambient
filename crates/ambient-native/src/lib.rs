//! What a native bundle written in Rust is built against: the entry
//! points of the runtime it will be linked with, and nothing of the
//! runtime itself. See "Native bundles" in `docs/spec/externs.md`.
//!
//! A bundle is an archive whose functions are named for the externs
//! they are ([`natives!`]). Each takes the call it is in and its
//! arguments, a tuple it does not own, and gives a value its caller
//! owns. It reads and makes values through [`entries!`], which is the
//! one list of what a bundle may call: the C header `ambient` prints is
//! written from it, and what loads a program is checked against it.

#![no_std]

mod entries;
mod values;

pub use values::{bytes, fail, string, text};

/// A value, or a reference to one.
pub type Word = u64;

/// The call a function of a bundle is in. Nothing of it is the
/// function's to read: it is what the `amb_bundle_` entries take.
#[repr(C)]
pub struct Call {
    _opaque: [u8; 0],
}

/// What a bundle hands over for its own function to be called with:
/// nothing but the bundle reads it.
pub type Data = core::ffi::c_void;

/// What makes the answer to a wait, on the program's thread, of what
/// a thread of the bundle handed over. The program owns what it gives;
/// zero fails the task that waits.
pub type Make = unsafe extern "C" fn(data: *mut Data) -> Word;

/// What is told of each handle of a brand that its program holds no
/// longer: the number the bundle minted it with.
pub type Dropped = unsafe extern "C" fn(number: u64);

/// The layout number, written out: the one place it is.
#[macro_export]
macro_rules! layout {
    () => {
        33
    };
}

/// The layout a bundle built against this crate reads and makes values
/// in. A bundle is linked only into a program of the same number, and
/// is built again when it changes.
pub const LAYOUT: u32 = layout!();

/// The word that is null.
pub const NULL: Word = 0b0010;
/// The word that is `false`.
pub const FALSE: Word = 0b0110;
/// The word that is `true`.
pub const TRUE: Word = 0b1010;

/// What `amb_kind` says a value is.
pub mod kind {
    pub const NULL: u64 = 0;
    pub const BOOL: u64 = 1;
    pub const INT: u64 = 2;
    pub const FLOAT: u64 = 3;
    pub const STRING: u64 = 4;
    pub const TUPLE: u64 = 5;
    pub const LIST: u64 = 6;
    pub const RECORD: u64 = 7;
    pub const FUNCTION: u64 = 8;
    pub const TAG: u64 = 10;
    pub const BYTES: u64 = 15;
    pub const HANDLE: u64 = 16;

    /// Each kind a bundle is told of, by the name a header gives it.
    pub const NAMED: &[(&str, u64)] = &[
        ("NULL", NULL),
        ("BOOL", BOOL),
        ("INT", INT),
        ("FLOAT", FLOAT),
        ("STRING", STRING),
        ("TUPLE", TUPLE),
        ("LIST", LIST),
        ("RECORD", RECORD),
        ("FUNCTION", FUNCTION),
        ("TAG", TAG),
        ("BYTES", BYTES),
        ("HANDLE", HANDLE),
    ];
}

/// One entry point, as text: what a header is written from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: &'static str,
    /// What it does, a line of text to each line.
    pub about: &'static str,
    /// Each parameter's name and its type, as Rust writes it.
    pub takes: &'static [(&'static str, &'static str)],
    /// The type it gives, as Rust writes it; empty if it gives nothing.
    pub gives: &'static str,
}

/// Declares each entry as a function of the runtime a bundle is linked
/// with.
macro_rules! declared {
    ($($(#[doc = $about:literal])* fn $name:ident($($arg:ident: $ty:ty),*) $(-> $gives:ty)?;)*) => {
        unsafe extern "C" {
            $($(#[doc = $about])* pub fn $name($($arg: $ty),*) $(-> $gives)?;)*
        }
    };
}

/// Lists each entry as text.
macro_rules! listed {
    ($($(#[doc = $about:literal])* fn $name:ident($($arg:ident: $ty:ty),*) $(-> $gives:ty)?;)*) => {
        /// Every entry point a bundle may call, in the order a header
        /// lists them.
        pub const ENTRIES: &[Entry] = &[$(Entry {
            name: stringify!($name),
            about: concat!($($about, "\n"),*),
            takes: &[$((stringify!($arg), stringify!($ty))),*],
            gives: stringify!($($gives)?),
        }),*];
    };
}

entries!(declared);
entries!(listed);

/// Defines the functions of a bundle for externs of the package with
/// the ID given. Each is named for its extern's module path and name,
/// with an underscore for each dot: `main_connect` is `connect` of the
/// module `main`.
///
/// A function takes the call it is in and its arguments, a tuple it
/// does not own, and gives a value its caller owns. A bundle says this
/// once.
#[macro_export]
macro_rules! natives {
    (package $id:literal; $($(#[$meta:meta])* fn $name:ident($call:ident, $args:ident) $body:block)*) => {
        /// What says the layout this bundle was built for: a program
        /// of another asks for a name this is not, and does not link.
        #[unsafe(export_name = concat!("amb_built_", $id, "_", $crate::layout!()))]
        pub static AMBIENT_BUILT: u8 = 0;

        $(
            $(#[$meta])*
            #[unsafe(export_name = concat!("amb_x_", $id, "_", stringify!($name)))]
            pub unsafe extern "C" fn $name($call: *mut $crate::Call, $args: $crate::Word) -> $crate::Word {
                $body
            }
        )*
    };
}

#[cfg(test)]
mod tests {
    use super::ENTRIES;

    #[test]
    fn each_entry_is_listed_once_and_says_what_it_does() {
        for (place, entry) in ENTRIES.iter().enumerate() {
            assert!(!entry.about.is_empty(), "{}", entry.name);
            assert!(
                ENTRIES[..place]
                    .iter()
                    .all(|other| other.name != entry.name)
            );
        }
    }
}
