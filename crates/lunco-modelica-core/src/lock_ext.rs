//! Poison-tolerant `Mutex` locking (CQ-702).
//!
//! The compile/run worker pool, the experiment runner state, and the
//! AST parse cache all live behind process-global `std::sync::Mutex`es.
//! A plain `lock().unwrap()` turns a *single* panicked worker into a
//! permanent crash: the panic poisons the mutex, and every later
//! `unwrap()` on it hard-panics — so one bad compile bricks all future
//! compiles/runs for the process.
//!
//! The data behind these locks is plain state we can keep using after a
//! panic, so recovering the guard (`PoisonError::into_inner`) and
//! warning once is strictly better than cascading the crash. This
//! mirrors the recovery already applied to the lint mutex (CQ-513).

use std::sync::{Mutex, MutexGuard};

/// Extension giving [`Mutex`] a panic-tolerant lock.
pub(crate) trait LockExt<T: ?Sized> {
    /// Lock, recovering the guard if the mutex was poisoned by a prior
    /// panic (logging once) instead of propagating the poison.
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
