//! What the runtime keeps for as long as a program runs, and what it
//! does when it cannot go on.
//!
//! Natively the runtime is linked into a process that may run programs
//! on many threads, so its state is locked or kept per thread. In
//! WebAssembly a program is alone in its memory and runs on one thread,
//! and the runtime is built without the standard library: what a
//! program does not reach it does not carry (`spec/linking.md`), and
//! locks and thread-local storage would be reached by every program.

/// State every thread of a process shares.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Shared<T>(std::sync::Mutex<T>);

#[cfg(not(target_arch = "wasm32"))]
impl<T> Shared<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self(std::sync::Mutex::new(value))
    }

    /// Reads or changes the state. `work` must not come back here.
    pub(crate) fn with<R>(&self, work: impl FnOnce(&mut T) -> R) -> R {
        // State a panic left behind is still the state.
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        work(&mut state)
    }
}

/// State of a program that is alone in its memory.
#[cfg(target_arch = "wasm32")]
pub(crate) struct Shared<T>(core::cell::UnsafeCell<T>);

// There is one thread.
#[cfg(target_arch = "wasm32")]
unsafe impl<T> Sync for Shared<T> {}

#[cfg(target_arch = "wasm32")]
impl<T> Shared<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self(core::cell::UnsafeCell::new(value))
    }

    /// Reads or changes the state. `work` must not come back here.
    pub(crate) fn with<R>(&self, work: impl FnOnce(&mut T) -> R) -> R {
        // Nothing else holds it: there is one thread, and `work` does
        // not come back.
        work(unsafe { &mut *self.0.get() })
    }
}

/// Ends the program: the runtime cannot go on.
pub(crate) fn abort() -> ! {
    #[cfg(not(target_arch = "wasm32"))]
    std::process::abort();
    #[cfg(target_arch = "wasm32")]
    core::arch::wasm32::unreachable();
}

/// A panic in WebAssembly is a trap: nothing reads its message, so
/// nothing that would format one is carried.
#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
