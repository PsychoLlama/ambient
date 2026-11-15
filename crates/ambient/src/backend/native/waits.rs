//! Native code that answers later, and the host that brings the
//! answers. See "Native side" in `spec/externs.md`.
//!
//! Native code that cannot answer at once takes a ticket
//! ([`super::Running::later`]) and starts whatever does the waiting. When
//! no task can run, the program asks its host for the next answer
//! ([`Waits::next`]) and hands it to the task that holds the ticket.

use std::cell::RefCell;
use std::rc::Rc;

use ambient_runtime::{Context, WAITING};

use super::host::{Inside, Observation};
use super::load::Program;
use crate::value::{self, Value};

/// The answer to a ticket: a result, or what went wrong.
pub type Answer = (u64, Result<Value, String>);

/// How a host brings the answers native code said would come later.
pub trait Waits {
    /// The next answer, waited for if none is ready. None if no answer
    /// is coming: the tasks that wait are then waiting for nothing.
    fn next(&mut self) -> Option<Answer>;

    /// Says the task that held `ticket` left its wait: the work may
    /// stop, and its answer is dropped if it comes.
    fn forsake(&mut self, _ticket: u64) {}
}

/// The answers of one host, shared by the native code that starts the
/// waits and the program that takes the answers.
pub type Answers = Rc<RefCell<dyn Waits>>;

/// A call of native code that is to answer later: what a run observes
/// of it once the answer is in.
pub(super) struct Asked {
    pub(super) observation: Observation,
}

impl Program {
    /// Gives suspended tasks their turns until the root can go on,
    /// taking answers from the host whenever none can run without one.
    ///
    /// # Safety
    ///
    /// `context` is the context of a call of this program whose entry
    /// unwound suspending.
    pub(super) unsafe fn schedule(&self, context: *mut Context) {
        // SAFETY: the context is live, and the runtime is called from
        // outside compiled code, as it must be to take an answer.
        unsafe {
            loop {
                ambient_runtime::amb_schedule(context);
                if (*context).mode != WAITING {
                    return;
                }
                let answer = self
                    .waits
                    .as_ref()
                    .and_then(|waits| waits.borrow_mut().next());
                let Some((ticket, answer)) = answer else {
                    ambient_runtime::amb_strand(context);
                    continue;
                };
                self.answer(context, ticket, answer);
            }
        }
    }

    /// Tells the host a task left the wait of `ticket`.
    pub(super) fn forsake(&self, ticket: u64) {
        self.asked.borrow_mut().remove(&ticket);
        if let Some(waits) = &self.waits {
            waits.borrow_mut().forsake(ticket);
        }
    }

    /// Hands the task that holds `ticket` its answer, and notes what
    /// the run observed.
    unsafe fn answer(&self, context: *mut Context, ticket: u64, answer: Result<Value, String>) {
        let asked = self.asked.borrow_mut().remove(&ticket);
        if let (Some(trace), Some(asked)) = (self.trace.borrow_mut().as_mut(), asked) {
            trace.push(Observation {
                result: answer.clone(),
                ..asked.observation
            });
        }
        // SAFETY: the context is live, and the value is handed over.
        unsafe {
            match answer {
                Ok(value) => {
                    let word = value::encode(&value, &mut Inside(self));
                    ambient_runtime::amb_answer(context, ticket, word);
                }
                Err(message) => {
                    *self.failure.borrow_mut() = Some(message);
                    ambient_runtime::amb_refuse(context, ticket);
                }
            }
        }
    }
}

/// What a call of native code that took a ticket leaves behind: what to
/// observe when the answer comes, or the refusal if it failed at once.
pub(super) unsafe fn asked(
    program: &Program,
    context: *mut Context,
    ticket: u64,
    observation: Observation,
) {
    if let Err(message) = &observation.result {
        *program.failure.borrow_mut() = Some(message.clone());
        // SAFETY: the context is the one the call was handed.
        unsafe { ambient_runtime::amb_refuse(context, ticket) };
        return;
    }
    program
        .asked
        .borrow_mut()
        .insert(ticket, Asked { observation });
}
