//! What a built program starts with: a host, and no compiler. See
//! "Building" in `docs/spec/packages.md` and "Native code" in
//! `docs/spec/linking.md`.
//!
//! It is built as an archive, which the machine's linker joins with
//! the native bundles a program reaches into the file `ambient build`
//! puts the program after. That file is started by a `main` the build
//! writes, which names what it linked: the functions of bundles, and
//! the groups of the standard target's native side. Nothing here names
//! a group, so an executable holds only those its `main` does.

use std::ffi::{CStr, OsString, c_char, c_int};

use ambient_host::host::{Linked, System, World};
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

/// A group of the standard target's native side, as the `main` a
/// build writes lists it: the function that links it
/// (`ambient_host::host::GROUPS`).
pub type Group = extern "C" fn(&mut Linked);

/// Runs the program this file carries, told `args`, in a process
/// linked with `linked`, with the native code `bundled` gives beside
/// the standard target's. Gives what to exit with, or nothing if it
/// could not start.
pub fn launched(
    args: impl Iterator<Item = OsString>,
    linked: &Linked,
    bundled: impl FnOnce() -> Natives,
) -> Option<i32> {
    let start = |program: &[u8], world: &_| {
        launch::start_linked(program, world, linked, |natives, _| natives.and(bundled()))
    };
    launch::own_given(args, linked, start).or_else(|| {
        System::linked(Vec::new(), linked).warn("error: this file carries no program\n");
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

/// What a `main` a build writes linked: the functions of native
/// bundles, and the groups of the standard target's native side.
#[repr(C)]
pub struct Reached {
    pub roots: *const Root,
    pub count: usize,
    pub groups: *const Group,
    pub group_count: usize,
}

/// The start of an executable: runs the program the file carries with
/// what `reached` lists, and gives what the process is to exit with.
///
/// # Safety
///
/// `reached` lists as many roots and groups as it counts; a root's
/// texts end in a zero and its function keeps to what [`Bundled`]
/// says; `texts` is `count_given` texts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ambient_launch(
    reached: &Reached,
    count_given: c_int,
    texts: *const *const c_char,
) -> c_int {
    let (roots, count) = (reached.roots, reached.count);
    // SAFETY: the caller's to uphold.
    let groups = unsafe { std::slice::from_raw_parts(reached.groups, reached.group_count) };
    let mut linked = Linked::default();
    for group in groups {
        group(&mut linked);
    }
    let bundled = || {
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
    launched(told.into_iter(), &linked, bundled).unwrap_or(launch::FAILED)
}
