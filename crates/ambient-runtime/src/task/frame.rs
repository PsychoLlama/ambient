//! What a function keeps while its task is suspended: the frame it
//! saves as it unwinds and takes back as it is called again. See
//! "Suspension" in `spec/backends.md`.

use alloc::vec::Vec;

use super::{Frame, scheduler};
use crate::{Context, Word};

/// Starts a frame for the function that is suspending, at `point`:
/// `position` of `definition`, a string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_push(
    context: *mut Context,
    point: u64,
    definition: Word,
    position: u64,
) {
    unsafe { scheduler(context) }.current().frames.push(Frame {
        point,
        definition,
        position,
        locals: Vec::new(),
    });
}

/// Adds a local to the frame being made. The frame takes the local's
/// reference; the function gives it up by returning without releasing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_save(context: *mut Context, value: Word) {
    if let Some(frame) = unsafe { scheduler(context) }.current().frames.last_mut() {
        frame.locals.push(value);
    }
}

/// The point the function being resumed was at.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_point(context: *mut Context) -> u64 {
    let frames = &unsafe { scheduler(context) }.current().frames;
    frames.last().map_or(0, |frame| frame.point)
}

/// A local of the function being resumed, which takes it back.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_load(context: *mut Context, index: u64) -> Word {
    let frames = &unsafe { scheduler(context) }.current().frames;
    frames
        .last()
        .and_then(|frame| frame.locals.get(index as usize).copied())
        .unwrap_or(0)
}

/// Ends the frame of the function being resumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_drop(context: *mut Context) {
    unsafe { scheduler(context) }.current().frames.pop();
}
