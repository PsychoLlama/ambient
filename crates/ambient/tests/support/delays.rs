//! A host that answers later, for tests: each wait takes a number of
//! ticks, and the one that is due soonest is answered first. Nothing
//! waits on a real clock, so a run is the same every time.

#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use ambient::backend::native::{Answer, Natives, Waits};
use ambient::value::Value;

/// One wait: when it is due, the ticket it answers, and the answer.
type Due = (i64, u64, Result<Value, String>);

#[derive(Default)]
pub struct Delays {
    waits: Vec<Due>,
    /// The tickets of the waits tasks left, in the order they left.
    pub forsaken: Vec<u64>,
}

impl Waits for Delays {
    fn next(&mut self) -> Option<Answer> {
        let soonest = (0..self.waits.len()).min_by_key(|index| self.waits[*index].0)?;
        let (_, ticket, answer) = self.waits.remove(soonest);
        Some((ticket, answer))
    }

    fn forsake(&mut self, ticket: u64) {
        self.waits.retain(|wait| wait.1 != ticket);
        self.forsaken.push(ticket);
    }
}

/// Native code for `main.wait`, which answers its argument, a number of
/// ticks, after that many; a negative number is a wait that fails.
/// `delays` brings the answers.
pub fn waiting(delays: &Rc<RefCell<Delays>>) -> Natives {
    let starts = Rc::clone(delays);
    Natives::new()
        .reaching("app", "main.wait", move |running, args| {
            let ticket = running.later()?;
            let Some(Value::Int(ticks)) = args.first() else {
                return Err("not a number of ticks".to_owned());
            };
            let answer = if *ticks < 0 {
                Err("the wait failed".to_owned())
            } else {
                Ok(Value::Int(*ticks))
            };
            starts.borrow_mut().waits.push((*ticks, ticket, answer));
            Ok(Value::Null)
        })
        .answered_by(Rc::clone(delays) as _)
}
