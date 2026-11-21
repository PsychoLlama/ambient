//! A file only its owner may read or write. See "The system
//! package" in `docs/spec/targets.md`.

use std::io;

/// Makes the file at `path` hold `bytes`, open to its owner alone.
///
/// A new file is made with that mode, so no moment has it open to
/// others. One that was there keeps the mode it had when it is opened,
/// so it is narrowed first and emptied and written after: whoever
/// could read it before reads what it held before, and no more.
#[cfg(unix)]
pub(super) fn store(path: &str, bytes: &[u8]) -> io::Result<()> {
    use std::fs::{OpenOptions, Permissions};
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    const OWNER: u32 = 0o600;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(OWNER)
        .open(path)?;
    file.set_permissions(Permissions::from_mode(OWNER))?;
    file.set_len(0)?;
    file.write_all(bytes)
}

/// No machine but a Unix one is built for, and saying a file is kept
/// from others where nothing keeps it would be worse than refusing.
#[cfg(not(unix))]
pub(super) fn store(_path: &str, _bytes: &[u8]) -> io::Result<()> {
    Err(io::Error::other("this machine keeps no file to its owner"))
}
