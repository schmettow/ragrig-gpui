//! Cooperative cancellation shared by UI and worker threads.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A cloneable, thread-safe cancellation flag.
///
/// The UI calls [`cancel`](Self::cancel) while a worker checks
/// [`is_cancelled`](Self::is_cancelled) at its own checkpoints. A finished run
/// can be followed by another one on the same handle after
/// [`reset`](Self::reset).
#[derive(Clone, Debug, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    /// Creates a fresh, uncancelled flag.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Every clone observes the request.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Returns whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Clears the flag so the handle can start another run.
    pub fn reset(&self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_one_flag() {
        let flag = CancelFlag::new();
        let worker = flag.clone();

        assert!(!worker.is_cancelled());
        flag.cancel();
        assert!(worker.is_cancelled());

        flag.reset();
        assert!(!worker.is_cancelled());
    }
}
