//! Shared synchronization helpers for recoverable process-global state.

use std::sync::{Mutex, MutexGuard};

/// Extension giving [`Mutex`] a panic-tolerant lock.
pub trait LockExt<T: ?Sized> {
    /// Lock, recovering the guard if a prior holder poisoned the mutex.
    fn lock_or_recover(&self) -> MutexGuard<'_, T>;
}

impl<T: ?Sized> LockExt<T> for Mutex<T> {
    fn lock_or_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| {
            bevy::log::warn_once!(
                "recovered a poisoned Mutex (a prior holder panicked); \
                 continuing with the retained state"
            );
            poisoned.into_inner()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::LockExt;
    use std::sync::{Arc, Mutex};
    use std::thread;

    #[test]
    fn poisoned_mutex_keeps_its_state_available() {
        let state = Arc::new(Mutex::new(7));
        let poisoned = Arc::clone(&state);
        let _ = thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("poison test");
        })
        .join();

        assert_eq!(*state.lock_or_recover(), 7);
    }
}
