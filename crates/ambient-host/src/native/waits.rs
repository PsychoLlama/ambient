//! Native code that answers later, and the host that brings the
//! answers. See "Native side" in `docs/spec/externs.md`.
//!
//! Native code that cannot answer at once takes a ticket
//! ([`super::Running::later`]) and starts whatever does the waiting. When
//! no task can run, the program asks its host for the next answer
//! ([`Waits::next`]) and hands it to the task that holds the ticket.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use ambient_native::Data;

use ambient_runtime::{Context, WAITING};

use super::load::Program;
use super::natives::Observation;
use super::posts::{Post, Posts};
use super::values::{Host, Inside};
use crate::value::{self, Value};

/// What a task is told when what was to make its answer gave none.
const UNMADE: &str = "a native bundle did not make the answer it posted";

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

    /// Hands over the posts of the program these answers are for. A
    /// host that waits on threads of its own has them wake it
    /// ([`Posts::wake_with`]), and gives nothing from `next` once
    /// something is posted; one that does not is waited out before a
    /// post is taken.
    fn attend(&mut self, _posts: Arc<Posts>) {}

    /// Says the program holds a handle no longer: one minted under the
    /// brand of this identity in a package, that the host knows by
    /// `number`. What it stood for may be closed.
    fn dropped(&mut self, _package: &str, _brand: &str, _number: u64) {}
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
                self.heard();
                if (*context).mode != WAITING {
                    return;
                }
                if let Some((ticket, post)) = self.posts.take() {
                    self.posted(context, ticket, post);
                    continue;
                }
                let answer = self
                    .waits
                    .as_ref()
                    .and_then(|waits| waits.borrow_mut().next());
                match answer {
                    Some((ticket, answer)) => self.answer(context, ticket, answer),
                    None if self.posts.ready() => {}
                    // Only a thread of a native bundle is still to
                    // answer.
                    None if self.posts.coming() => self.posts.wait(),
                    None => ambient_runtime::amb_strand(context),
                }
            }
        }
    }

    /// Tells the host of every handle the program dropped since it
    /// was last told. The runtime notes them as it reclaims them
    /// (`ambient_runtime::amb_handle_next`), wherever that is, so the
    /// host asks: before native code runs, when no task can, and when
    /// a call is over.
    pub(super) fn heard(&self) {
        loop {
            let brand = ambient_runtime::amb_handle_next();
            if brand == 0 {
                return;
            }
            let number = ambient_runtime::amb_handle_number();
            let Some(waits) = &self.waits else { continue };
            let text = value::Inspect::string(&mut Host, brand);
            let text = String::from_utf8_lossy(&text);
            let (package, brand) = text.split_once('\0').unwrap_or(("", &text));
            waits.borrow_mut().dropped(package, brand, number);
        }
    }

    /// Tells the host a task left the wait of `ticket`.
    pub(super) fn forsake(&self, ticket: u64) {
        self.asked.borrow_mut().remove(&ticket);
        self.posts.forsake(ticket);
        if let Some(waits) = &self.waits {
            waits.borrow_mut().forsake(ticket);
        }
    }

    /// Makes the answer a thread of a native bundle posted, and hands
    /// it to the task that holds `ticket`. With none, the task left
    /// its wait: the answer is made all the same, since what makes it
    /// is what frees what the thread handed over, and is let go.
    unsafe fn posted(&self, context: *mut Context, ticket: Option<u64>, post: Post) {
        let made = match post {
            // SAFETY: the bundle posted the two together, to be called
            // here, once.
            Post::Made(make, data) => match unsafe { make(data as *mut Data) } {
                ambient_runtime::UNWIND => Err(UNMADE.to_owned()),
                word => Ok(word),
            },
            Post::Refused(message) => Err(message),
        };
        let Some(ticket) = ticket else {
            if let Ok(word) = made {
                // SAFETY: the word was just made, and nothing holds it.
                unsafe { ambient_runtime::amb_release(word) };
            }
            return;
        };
        let asked = self.asked.borrow_mut().remove(&ticket);
        if let (Some(trace), Some(asked)) = (self.trace.borrow_mut().as_mut(), asked) {
            let result = made.clone().map(|word| value::decode(word, &mut Host));
            trace.push(Observation {
                result,
                ..asked.observation
            });
        }
        // SAFETY: the context is live, and the value is handed over.
        unsafe {
            match made {
                Ok(word) => ambient_runtime::amb_answer(context, ticket, word),
                Err(message) => {
                    *self.failure.borrow_mut() = Some(message);
                    ambient_runtime::amb_refuse(context, ticket);
                }
            }
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
