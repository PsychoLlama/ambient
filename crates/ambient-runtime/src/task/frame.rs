//! What a function keeps while its task is suspended: the frame it
//! saves as it unwinds and takes back as it is called again. See
//! "Suspension" in `spec/backends.md`.
//!
//! A function makes its frame with one call and then one for each
//! local, and takes it back the other way round. What makes a frame
//! gives the context back, so a function says the context once and
//! then only its locals.

use alloc::vec::Vec;

use super::{Frame, scheduler};
use crate::{Context, Word};

/// Starts a frame for the function that is suspending, at `point`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_push(context: *mut Context, point: u64) -> *mut Context {
    unsafe { scheduler(context) }.current().frames.push(Frame {
        point,
        position: 0,
        locals: Vec::new(),
    });
    context
}

/// Adds a local to the frame being made. The frame takes the local's
/// reference; the function gives it up by returning without releasing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_save(context: *mut Context, value: Word) -> *mut Context {
    if let Some(frame) = unsafe { scheduler(context) }.current().frames.last_mut() {
        frame.locals.push(value);
    }
    context
}

/// The last local the frame of the function being resumed still
/// holds, which takes it back: a function asks for its locals in the
/// reverse of the order it saved them in.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_load(context: *mut Context) -> Word {
    let frames = &mut unsafe { scheduler(context) }.current().frames;
    frames
        .last_mut()
        .and_then(|frame| frame.locals.pop())
        .unwrap_or(0)
}

/// The point the function being resumed was at. Its frame is over:
/// it has its locals back.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_point(context: *mut Context) -> u64 {
    let frames = &mut unsafe { scheduler(context) }.current().frames;
    frames.pop().map_or(0, |frame| frame.point)
}
