//! What a real process is asked from outside: to stop, or that its
//! terminal is gone. See "Standard targets" in `docs/spec/targets.md`.
//!
//! Nothing is listened for until a task waits. From then on the
//! process is no longer ended by being asked: the program was told,
//! and what it does is its own.

use std::collections::VecDeque;
use std::sync::mpsc::Sender;

use super::pending::Sent;

/// What a process was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// To stop, from its terminal.
    Interrupt,
    /// To stop.
    Terminate,
    /// Nothing: its terminal is gone. By custom, to read its settings
    /// again.
    Hangup,
}

impl Signal {
    /// What the tag that names it is called in `system.process`.
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "interrupt",
            Self::Terminate => "terminate",
            Self::Hangup => "hangup",
        }
    }
}

#[derive(Default)]
pub(super) struct Signals {
    /// Whether a thread listens yet.
    listening: bool,
    waiting: VecDeque<u64>,
    /// What was asked that no task has been told, each once.
    untold: VecDeque<Signal>,
}

#[cfg(unix)]
fn listen(sender: Sender<Sent>) {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP]) else {
        return;
    };
    // Ends when the program is gone.
    std::thread::spawn(move || {
        for signal in signals.forever() {
            let signal = match signal {
                SIGINT => Signal::Interrupt,
                SIGTERM => Signal::Terminate,
                _ => Signal::Hangup,
            };
            if sender.send(Sent::Signal(signal)).is_err() {
                return;
            }
        }
    });
}

/// Nothing asks a process anything here.
#[cfg(not(unix))]
fn listen(_sender: Sender<Sent>) {}

impl Signals {
    /// Whether a task waits to be told.
    pub(super) fn waits(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// What the process was asked that no task was told, or the
    /// ticket waits to be told.
    pub(super) fn wait(&mut self, ticket: u64, sender: &Sender<Sent>) -> Option<Signal> {
        if !self.listening {
            self.listening = true;
            listen(sender.clone());
        }
        let untold = self.untold.pop_front();
        if untold.is_none() {
            self.waiting.push_back(ticket);
        }
        untold
    }

    /// Tells the task that has waited longest, or keeps what was asked
    /// until one waits: once, however often it was asked.
    pub(super) fn came(&mut self, signal: Signal) -> Option<(u64, Signal)> {
        let Some(ticket) = self.waiting.pop_front() else {
            if !self.untold.contains(&signal) {
                self.untold.push_back(signal);
            }
            return None;
        };
        Some((ticket, signal))
    }

    pub(super) fn forsake(&mut self, ticket: u64) {
        self.waiting.retain(|waiting| *waiting != ticket);
    }
}

#[cfg(test)]
mod tests {
    use super::{Signal, Signals};

    #[test]
    fn what_was_asked_before_a_task_waits_is_told_once() {
        let mut signals = Signals {
            listening: true,
            ..Signals::default()
        };
        let (sender, _sent) = std::sync::mpsc::channel();

        assert_eq!(signals.came(Signal::Terminate), None);
        assert_eq!(signals.came(Signal::Terminate), None);

        assert_eq!(signals.wait(1, &sender), Some(Signal::Terminate));
        assert_eq!(signals.wait(2, &sender), None);
        assert_eq!(signals.came(Signal::Hangup), Some((2, Signal::Hangup)));
    }
}
