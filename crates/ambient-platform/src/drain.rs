//! Drain signaling: cooperative cancellation for one computation (see
//! `ref/live-upgrade.md`, "Drain").
//!
//! A [`DrainSignal`] is the per-computation handle a draining host holds.
//! Requesting a drain does three things, in escalating order:
//!
//! 1. **Wakes blocked interruptible natives.** [`install_drain_natives`]
//!    overrides the interruptible subset of blocking platform natives
//!    (`tcp_accept`, `tcp_receive`, `tcp_receive_raw`,
//!    `time_wait`) on the
//!    computation's VM with variants that race the blocking operation
//!    against the signal. An interrupted native returns
//!    [`VmError::Interrupted`] carrying the `Drain::requested` anchors
//!    (`ambient_core::drain`), and the VM delivers the unwind at the
//!    native's own call site — the nearest `Drain::requested` arm runs
//!    cleanup and yields the computation's value.
//! 2. **Marks the signal**, so the *next* interruptible perform unwinds
//!    immediately even if nothing was blocked when the request arrived.
//!    Code between interruptible performs always runs to completion.
//! 3. **Arms the deadline** ([`DrainSignal::request_with_deadline`]): a
//!    computation that has not completed when the deadline expires is
//!    hard-stopped at the VM's next opcode boundary
//!    ([`VmError::HardStopped`]) — the fault-budget precedent's answer
//!    to a broken code path that would otherwise wedge a deploy. The
//!    driving host parks the computation instead of restarting it.
//!
//! A signal is one-way: once requested it never resets, and every later
//! interruptible perform on the same signal unwinds immediately.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use ambient_ability::{Value, VmError};
use ambient_engine::vm::Vm;

use crate::tcp_state::{TcpError, TcpState};
use crate::{extract_number, into_result, native_uuid};

/// The error an interrupted native returns: the VM responds by
/// performing `Drain::requested!` — a host-constructed suspended never
/// value — at the native's own call site.
#[must_use]
pub fn drain_interrupt() -> VmError {
    VmError::Interrupted {
        ability_id: ambient_core::drain::ability_id(),
        method: ambient_core::drain::requested_method_key(),
    }
}

/// Condvar-guarded signal state.
#[derive(Default)]
struct Flags {
    /// A drain has been requested (one-way).
    requested: bool,
    /// The computation has finished (reported by the driver); disarms a
    /// pending deadline watchdog.
    completed: bool,
}

/// Per-computation drain handle: the request flag, the wakeup channels
/// for blocked waits (one thread-side, one tokio-side), and the
/// hard-stop backstop wired into the computation's VM.
#[derive(Default)]
pub struct DrainSignal {
    flags: Mutex<Flags>,
    /// Wakes thread-blocking waits: [`Self::sleep`] and the deadline
    /// watchdog.
    cv: Condvar,
    /// Wakes tokio-bridged waits: the `accept`/`receive` cancel futures.
    notify: tokio::sync::Notify,
    /// Set by the deadline watchdog; observed by the VM between opcodes
    /// (see `Vm::set_interrupt_flag`).
    hard_stop: Arc<AtomicBool>,
}

impl DrainSignal {
    /// A fresh, unrequested signal.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A poisoned flag lock means a panicking thread died holding it;
    /// the flags are plain bools with no invariant between them, so the
    /// state is still meaningful — keep signaling rather than wedging
    /// the drain.
    fn lock(&self) -> MutexGuard<'_, Flags> {
        self.flags.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Request a drain: every interruptible perform from now on unwinds
    /// with `Drain::requested`, and anything currently blocked in an
    /// interruptible native is woken. One-way and idempotent.
    pub fn request(&self) {
        self.lock().requested = true;
        self.cv.notify_all();
        self.notify.notify_waiters();
    }

    /// [`Self::request`], then hard-stop the computation if it has not
    /// [completed](Self::mark_complete) within `deadline`: a watchdog
    /// thread sets the VM's interrupt flag, and the run ends with
    /// `VmError::HardStopped` at the next opcode boundary. (A native
    /// blocked in the host past the deadline is not interrupted by
    /// this — only opcode boundaries observe the flag.)
    pub fn request_with_deadline(self: &Arc<Self>, deadline: Duration) {
        self.request();
        let signal = Arc::clone(self);
        std::thread::spawn(move || {
            let end = Instant::now() + deadline;
            let mut flags = signal.lock();
            while !flags.completed {
                let now = Instant::now();
                if now >= end {
                    signal.hard_stop.store(true, Ordering::SeqCst);
                    return;
                }
                flags = signal
                    .cv
                    .wait_timeout(flags, end - now)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
        });
    }

    /// Report that the drained computation has finished (however it
    /// ended), disarming a pending deadline watchdog so it never
    /// hard-stops a VM that might be reused.
    pub fn mark_complete(&self) {
        self.lock().completed = true;
        self.cv.notify_all();
    }

    /// Whether a drain has been requested.
    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.lock().requested
    }

    /// The hard-stop flag the deadline watchdog sets — wire it into the
    /// computation's VM with `Vm::set_interrupt_flag`
    /// ([`install_drain_natives`] does).
    #[must_use]
    pub fn hard_stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.hard_stop)
    }

    /// Resolves when a drain is requested — the cancel future the
    /// interruptible network natives race their blocking operation
    /// against. Registers with the wakeup channel *before* checking the
    /// flag, so a request can never fall between the check and the wait.
    pub async fn cancelled(&self) {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_requested() {
            return;
        }
        notified.await;
    }

    /// Block for `duration` or until a drain is requested, whichever
    /// comes first. Returns `true` when interrupted — the interruptible
    /// `time_wait`'s wait, without needing an async runtime.
    #[must_use]
    pub fn sleep(&self, duration: Duration) -> bool {
        let deadline = Instant::now() + duration;
        let mut flags = self.lock();
        loop {
            if flags.requested {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            flags = self
                .cv
                .wait_timeout(flags, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// Translate a cancellable network outcome: an interruption becomes the
/// `Drain::requested` delivery, anything else keeps the ordinary
/// operational-failure shape (`into_result` turns the exception into an
/// in-language `Err`).
fn tcp_error(op: &str, error: TcpError) -> VmError {
    match error {
        TcpError::Interrupted => drain_interrupt(),
        other => VmError::exception(format!("Tcp.{op}: {other}")),
    }
}

/// Make a VM's blocking natives drain-aware: override the interruptible
/// subset (`tcp_accept`, `tcp_receive`, `tcp_receive_raw`,
/// `time_wait`) with variants bound to `signal`, and wire the signal's
/// hard-stop flag into the VM. Implementations are uuid-keyed, so these
/// overrides win over the registry-installed ones — per-VM wiring,
/// exactly like the task runtime's `task_*` natives.
///
/// Every override checks the signal *before* blocking, so once a drain
/// is requested each subsequent interruptible perform unwinds
/// immediately and deterministically.
pub fn install_drain_natives(vm: &mut Vm, network: &Arc<TcpState>, signal: &Arc<DrainSignal>) {
    vm.set_interrupt_flag(signal.hard_stop_flag());

    // time_wait(duration) -> (), interruptible.
    let s = Arc::clone(signal);
    vm.register_native_impl(
        native_uuid("time_wait"),
        Arc::new(move |args: Vec<Value>| {
            let duration = args
                .first()
                .and_then(crate::time::duration_from_value)
                .unwrap_or_default();
            if s.sleep(duration) {
                return Err(drain_interrupt());
            }
            Ok(Value::Unit)
        }),
    );

    // tcp_accept(listener) -> Result<ConnectionId, string>, interruptible.
    let s = Arc::clone(signal);
    let net = Arc::clone(network);
    vm.register_native_impl(
        native_uuid("tcp_accept"),
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
                if s.is_requested() {
                    return Err(drain_interrupt());
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let listener_id = extract_number(&args)? as u64;
                let id = net
                    .accept_interruptible(listener_id, s.cancelled())
                    .map_err(|e| tcp_error("accept", e))?;
                #[allow(clippy::cast_precision_loss)]
                Ok(Value::Number(id as f64))
            })())
        }),
    );

    // tcp_receive(conn) -> Result<Binary, string>, interruptible.
    let s = Arc::clone(signal);
    let net = Arc::clone(network);
    vm.register_native_impl(
        native_uuid("tcp_receive"),
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
                if s.is_requested() {
                    return Err(drain_interrupt());
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let conn_id = extract_number(&args)? as u64;
                let data = net
                    .receive_interruptible(conn_id, s.cancelled())
                    .map_err(|e| tcp_error("receive", e))?;
                Ok(Value::binary(data))
            })())
        }),
    );

    // tcp_receive_raw(conn) -> Result<Binary, string>, interruptible.
    let s = Arc::clone(signal);
    let net = Arc::clone(network);
    vm.register_native_impl(
        native_uuid("tcp_receive_raw"),
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
                if s.is_requested() {
                    return Err(drain_interrupt());
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let conn_id = extract_number(&args)? as u64;
                let data = net
                    .receive_raw_interruptible(conn_id, s.cancelled())
                    .map_err(|e| tcp_error("receive_raw", e))?;
                Ok(Value::binary(data))
            })())
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_runs_to_the_deadline_without_a_request() {
        let signal = DrainSignal::new();
        let start = Instant::now();
        assert!(!signal.sleep(Duration::from_millis(20)));
        assert!(start.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn sleep_is_interrupted_by_a_request() {
        let signal = DrainSignal::new();
        let sleeper = Arc::clone(&signal);
        let waiter = std::thread::spawn(move || sleeper.sleep(Duration::from_mins(1)));
        std::thread::sleep(Duration::from_millis(10));
        signal.request();
        assert!(
            waiter.join().expect("sleeper joins"),
            "sleep must report the interrupt"
        );
    }

    #[test]
    fn sleep_after_a_request_returns_immediately() {
        let signal = DrainSignal::new();
        signal.request();
        let start = Instant::now();
        assert!(signal.sleep(Duration::from_mins(1)));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn deadline_sets_the_hard_stop_flag() {
        let signal = DrainSignal::new();
        signal.request_with_deadline(Duration::from_millis(10));
        std::thread::sleep(Duration::from_millis(100));
        assert!(signal.hard_stop_flag().load(Ordering::SeqCst));
    }

    #[test]
    fn completion_disarms_the_deadline() {
        let signal = DrainSignal::new();
        signal.request_with_deadline(Duration::from_millis(50));
        signal.mark_complete();
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            !signal.hard_stop_flag().load(Ordering::SeqCst),
            "a completed computation must never be hard-stopped"
        );
    }

    #[test]
    fn cancelled_resolves_for_a_prior_request() {
        // The registered-before-checked contract: a request that lands
        // before the wait starts must still resolve it.
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let signal = DrainSignal::new();
        signal.request();
        runtime.block_on(signal.cancelled());
    }

    #[test]
    fn cancelled_resolves_for_a_concurrent_request() {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let signal = DrainSignal::new();
        let requester = Arc::clone(&signal);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            requester.request();
        });
        runtime.block_on(signal.cancelled());
        handle.join().expect("requester joins");
    }
}
