//! Cooperative cancellation for a single admitted native solve.
use std::{
    cell::RefCell,
    marker::PhantomData,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

thread_local! {
    static SOLVER_CANCELLATION: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Installs one run's cancellation flag until this thread-local scope ends.
/// Nested solves restore their caller's flag; the guard cannot move threads.
pub struct SolverCancellationGuard {
    previous: Option<Arc<AtomicBool>>,
    _thread: PhantomData<Rc<()>>,
}

impl SolverCancellationGuard {
    pub fn install(flag: Arc<AtomicBool>) -> Self {
        let previous = SOLVER_CANCELLATION.with(|slot| slot.replace(Some(flag)));
        Self {
            previous,
            _thread: PhantomData,
        }
    }
}

impl Drop for SolverCancellationGuard {
    fn drop(&mut self) {
        SOLVER_CANCELLATION.with(|slot| {
            slot.replace(self.previous.take());
        });
    }
}

#[inline]
pub fn solver_cancellation_requested() -> bool {
    SOLVER_CANCELLATION.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_is_thread_local_and_nested_scopes_restore_their_owner() {
        let outer = Arc::new(AtomicBool::new(false));
        let guard = SolverCancellationGuard::install(outer.clone());
        outer.store(true, Ordering::SeqCst);
        assert!(solver_cancellation_requested());
        std::thread::spawn(|| assert!(!solver_cancellation_requested()))
            .join()
            .expect("isolated thread");
        {
            let _inner = SolverCancellationGuard::install(Arc::new(AtomicBool::new(false)));
            assert!(!solver_cancellation_requested());
        }
        assert!(solver_cancellation_requested());
        drop(guard);
        assert!(!solver_cancellation_requested());
    }
}
