//! Bounded worker threads with explicit affinity for thread-local solver state.

use crossbeam_channel::{Sender, unbounded};
use std::sync::Arc;
use std::thread::JoinHandle;

type Job<C> = Box<dyn FnOnce(&mut C) + Send + 'static>;

enum Request<C> {
    Run(Job<C>),
    Stop,
}

/// A mailbox for one persistent owner; its context never crosses threads.
pub(super) struct Lane<C> {
    tx: Sender<Request<C>>,
}

impl<C> Clone for Lane<C> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

impl<C> Lane<C> {
    pub(super) fn submit(&self, job: impl FnOnce(&mut C) + Send + 'static) -> Result<(), String> {
        self.tx
            .send(Request::Run(Box::new(job)))
            .map_err(|_| "Modelica solver owner is unavailable".to_owned())
    }
}

/// Shares the existing bounded solver budget between preparation and live work.
/// Shutdown is explicit, so retained mailbox handles cannot keep workers alive.
pub(super) struct ThreadAffinePool<C> {
    lanes: Vec<Lane<C>>,
    threads: Vec<JoinHandle<()>>,
    next_lane: usize,
    failure: Arc<dyn Fn(String) + Send + Sync>,
}

impl<C: 'static> ThreadAffinePool<C> {
    pub(super) fn new(
        count: usize,
        factory: impl Fn() -> C + Send + Sync + 'static,
        failure: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        if count == 0 {
            return Err("Modelica solver worker capacity must be positive".to_owned());
        }
        let factory = Arc::new(factory);
        let failure: Arc<dyn Fn(String) + Send + Sync> = Arc::new(failure);
        let mut pool = Self {
            lanes: Vec::with_capacity(count),
            threads: Vec::with_capacity(count),
            next_lane: 0,
            failure,
        };
        for index in 0..count {
            let (tx, rx) = unbounded();
            let factory = Arc::clone(&factory);
            let failure = Arc::clone(&pool.failure);
            let thread = std::thread::Builder::new()
                .name(format!("modelica-solve-{index}"))
                .stack_size(16 * 1024 * 1024)
                .spawn(move || {
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut context = factory();
                        while let Ok(request) = rx.recv() {
                            match request {
                                Request::Run(job) => job(&mut context),
                                Request::Stop => break,
                            }
                        }
                    }));
                    if let Err(payload) = outcome {
                        let message = payload
                            .downcast_ref::<&str>()
                            .copied()
                            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                            .unwrap_or("unknown panic payload");
                        failure(format!("Modelica solver owner {index} panicked: {message}"));
                    }
                })
                .map_err(|error| format!("cannot start Modelica solver owner {index}: {error}"))?;
            pool.lanes.push(Lane { tx });
            pool.threads.push(thread);
        }
        Ok(pool)
    }

    pub(super) fn worker_count(&self) -> usize {
        self.lanes.len()
    }

    #[cfg(test)]
    pub(super) fn lane(&self, index: usize) -> Result<Lane<C>, String> {
        self.lanes
            .get(index)
            .cloned()
            .ok_or_else(|| format!("Modelica solver lane {index} is outside the worker budget"))
    }

    /// Select preparation or a new session's owner in stable submission order.
    pub(super) fn next_lane(&mut self) -> Lane<C> {
        let lane = self.lanes[self.next_lane].clone();
        self.next_lane = (self.next_lane + 1) % self.lanes.len();
        lane
    }

    pub(super) fn spawn(&mut self, job: impl FnOnce() + Send + 'static) {
        if let Err(error) = self.next_lane().submit(move |_| job()) {
            (self.failure)(error);
        }
    }
}

impl<C> Drop for ThreadAffinePool<C> {
    fn drop(&mut self) {
        for lane in &self.lanes {
            let _ = lane.tx.send(Request::Stop);
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    #[test]
    fn jobs_retain_non_send_context_and_retire_on_the_owner_thread() {
        struct LocalContext {
            owner: std::thread::ThreadId,
            calls: Rc<Cell<usize>>,
            retired: Arc<Mutex<Vec<std::thread::ThreadId>>>,
        }
        impl Drop for LocalContext {
            fn drop(&mut self) {
                assert_eq!(self.owner, std::thread::current().id());
                self.retired.lock().unwrap().push(self.owner);
            }
        }
        let retired = Arc::new(Mutex::new(Vec::new()));
        let factory_retired = Arc::clone(&retired);
        let mut pool = ThreadAffinePool::new(
            2,
            move || LocalContext {
                owner: std::thread::current().id(),
                calls: Rc::new(Cell::new(0)),
                retired: Arc::clone(&factory_retired),
            },
            |error| panic!("unexpected pool failure: {error}"),
        )
        .unwrap();
        let (tx, rx) = crossbeam_channel::unbounded();
        for lane in [0, 1, 0, 1] {
            let tx = tx.clone();
            pool.lane(lane)
                .unwrap()
                .submit(move |context| {
                    assert_eq!(context.owner, std::thread::current().id());
                    context.calls.set(context.calls.get() + 1);
                    tx.send((lane, context.owner, context.calls.get())).unwrap();
                })
                .unwrap();
        }
        let mut rows: Vec<_> = (0..4)
            .map(|_| rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap())
            .collect();
        rows.sort_by_key(|row| (row.0, row.2));
        assert_eq!(rows[0].1, rows[1].1);
        assert_eq!(rows[2].1, rows[3].1);
        assert_ne!(rows[0].1, rows[2].1);
        assert_eq!(
            rows.iter().map(|row| row.2).collect::<Vec<_>>(),
            [1, 2, 1, 2]
        );
        assert_eq!(pool.worker_count(), 2);
        let (prepared_tx, prepared_rx) = crossbeam_channel::bounded(1);
        pool.spawn(move || {
            prepared_tx.send(std::thread::current().id()).unwrap();
        });
        assert_eq!(
            prepared_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            rows[0].1
        );
        let (started_tx, started_rx) = crossbeam_channel::unbounded();
        let (release_tx, release_rx) = crossbeam_channel::unbounded();
        for lane in 0..2 {
            let started_tx = started_tx.clone();
            let release_rx = release_rx.clone();
            pool.lane(lane)
                .unwrap()
                .submit(move |_| {
                    started_tx.send(lane).unwrap();
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                })
                .unwrap();
        }
        let first = started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let second = started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_ne!(first, second);
        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();
        let surviving_handle = pool.lane(0).unwrap();
        drop(pool);
        assert_eq!(retired.lock().unwrap().len(), 2);
        assert!(surviving_handle.submit(|_| {}).is_err());

        let (failure_tx, failure_rx) = crossbeam_channel::unbounded();
        let failure_retired = Arc::clone(&retired);
        let failed_pool = ThreadAffinePool::new(
            1,
            move || LocalContext {
                owner: std::thread::current().id(),
                calls: Rc::new(Cell::new(0)),
                retired: Arc::clone(&failure_retired),
            },
            move |error| {
                failure_tx.send(error).unwrap();
            },
        )
        .unwrap();
        let failed_lane = failed_pool.lane(0).unwrap();
        failed_lane
            .submit(|_| panic!("affinity-negative-owner-fault"))
            .unwrap();
        let error = failure_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(error.contains("affinity-negative-owner-fault"));
        drop(failed_pool);
        assert_eq!(retired.lock().unwrap().len(), 3);
        assert!(failed_lane.submit(|_| {}).is_err());
    }
}
