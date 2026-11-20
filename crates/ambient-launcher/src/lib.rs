//! What a built program starts with: a host, and no compiler. See
//! "Building" in `docs/spec/packages.md` and "Native code" in
//! `docs/spec/linking.md`.
//!
//! It is built twice over: as the file `ambient build` puts a program
//! after, and as an archive that the machine's linker joins with the
//! native bundles a program reaches. The second is started by a `main`
//! the build writes, which hands over the functions it linked.

use std::ffi::{CStr, OsString, c_char, c_int};

use ambient_host::host::{System, World};
use ambient_host::launch;
use ambient_host::native::{Bundled, Natives};

/// A function of a native bundle, as the `main` a build writes lists
/// it: the extern it is for, by the package that declares it and its
/// module path and name, each as C text.
#[repr(C)]
pub struct Root {
    pub package: *const c_char,
    pub path: *const c_char,
    pub function: Bundled,
    /// What the bundle defines to say the layout it was built for.
    /// Nothing reads it: naming it is what holds the bundle to it.
    pub built: *const u8,
}

/// Runs the program this file carries, told `args`, with the native
/// code `linked` gives beside the standard target's. Gives what to
/// exit with, or nothing if it could not start.
pub fn launched(
    args: impl Iterator<Item = OsString>,
    linked: impl FnOnce() -> Natives,
) -> Option<i32> {
    let start = |program: &[u8], world: &_| {
        launch::start_with(program, world, |natives, _| natives.and(linked()))
    };
    launch::own_given(args, start).or_else(|| {
        System::new(Vec::new()).warn("error: this file carries no program\n");
        None
    })
}

/// The arguments a process was started with after its own name, as C
/// hands them to a `main`.
///
/// # Safety
///
/// `texts` is `count_given` texts, each ended by a zero.
unsafe fn given(count_given: c_int, texts: *const *const c_char) -> Vec<OsString> {
    let count = usize::try_from(count_given).unwrap_or(0);
    let arg = |index| {
        // SAFETY: the index is within the count, and each is C text.
        let text = unsafe { CStr::from_ptr(*texts.add(index)) };
        text.to_string_lossy().into_owned().into()
    };
    (1..count).map(arg).collect()
}

/// The start of an executable linked with native bundles: runs the
/// program the file carries with `count` functions of them, and gives
/// what the process is to exit with.
///
/// # Safety
///
/// `roots` is `count` roots, whose texts end in a zero and whose
/// functions keep to what [`Bundled`] says; `texts` is `count_given` texts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ambient_launch(
    roots: *const Root,
    count: usize,
    count_given: c_int,
    texts: *const *const c_char,
) -> c_int {
    let linked = || {
        let text = |text| unsafe { CStr::from_ptr(text) }.to_string_lossy();
        // SAFETY: the caller's to uphold.
        let roots = unsafe { std::slice::from_raw_parts(roots, count) };
        roots.iter().fold(Natives::new(), |natives, root| {
            // SAFETY: as above.
            unsafe { natives.bundled(&text(root.package), &text(root.path), root.function) }
        })
    };
    // SAFETY: the caller's to uphold.
    let told = unsafe { given(count_given, texts) };
    launched(told.into_iter(), linked).unwrap_or(launch::FAILED)
}
