//! How far ahead of UTC the machine's own time zone is. See "The
//! system package" in `docs/spec/targets.md`.

/// The milliseconds the machine's zone is ahead of UTC at `moment`,
/// milliseconds since the start of 1970: what its rules say of that
/// moment, daylight saving and all. Zero where the machine cannot say.
#[cfg(unix)]
#[allow(
    irrefutable_let_patterns,
    clippy::useless_conversion,
    reason = "how wide the C library's seconds are depends on the machine"
)]
pub(super) fn offset(moment: i64) -> i64 {
    let Ok(seconds) = libc::time_t::try_from(moment.div_euclid(1000)) else {
        return 0;
    };
    let mut local = std::mem::MaybeUninit::<libc::tm>::zeroed();
    // SAFETY: both pointers are to values that live through the call,
    // and `localtime_r` writes only the `tm` it is handed.
    let filled = unsafe { libc::localtime_r(&raw const seconds, local.as_mut_ptr()) };
    if filled.is_null() {
        return 0;
    }
    // SAFETY: a call that gave its argument back filled it in.
    let local = unsafe { local.assume_init() };
    i64::from(local.tm_gmtoff).saturating_mul(1000)
}

/// No machine but a Unix one is asked: see [`offset`].
#[cfg(not(unix))]
pub(super) fn offset(_moment: i64) -> i64 {
    0
}
