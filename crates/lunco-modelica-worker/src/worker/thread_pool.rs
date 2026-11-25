//! Bounded worker threads with explicit affinity for thread-local solver state.

use crossbeam_channel::{Sender, unbounded};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;

type Job<C> = Box<dyn FnOnce(&mut C) + Send + 'static>;

enum Request<C> {
    Run(Job<C>),
    Stop,
}

/// A mailbox for one persistent owner; its context never crosses threads.
pub(super) struct Lane<C> {
    tx: Sender<Request<C>>,
    index: usize,
    reservation: Option<Arc<LaneReservation>>,
}

/// A live session's affinity load follows its handle, including construction
/// in flight. Failed construction and retired sessions release the same lease.
struct LaneReservation {
    load: Arc<AtomicUsize>,
    weight: usize,
}

impl Drop for LaneReservation {
    fn drop(&mut self) {
        self.load.fetch_sub(self.weight, Ordering::Relaxed);
    }
}

impl<C> Clone for Lane<C> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            index: self.index,
            reservation: self.reservation.clone(),
        }
    }
}

impl<C> Lane<C> {
    pub(super) fn index(&self) -> usize {
        self.index
    }

    pub(super) fn submit(&self, job: impl FnOnce(&mut C) + Send + 'static) -> Result<(), String> {
        self.tx
            .send(Request::Run(Box::new(job)))
            .map_err(|_| "Modelica solver owner is unavailable".to_owned())
    }
}

/// A bounded pool for one execution domain with thread-local owner state.
/// Shutdown is explicit, so retained mailbox handles cannot keep workers alive.
pub(super) struct ThreadAffinePool<C> {
    lanes: Vec<Lane<C>>,
    reserved_loads: Vec<Arc<AtomicUsize>>,
    threads: Vec<JoinHandle<()>>,
    next_lane: usize,
    failure: Arc<dyn Fn(String) + Send + Sync>,
}

impl<C: 'static> ThreadAffinePool<C> {
    pub(super) fn new(
        name: &'static str,
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
            reserved_loads: Vec::with_capacity(count),
            threads: Vec::with_capacity(count),
            next_lane: 0,
            failure,
        };
        for index in 0..count {
            let (tx, rx) = unbounded();
            let factory = Arc::clone(&factory);
            let failure = Arc::clone(&pool.failure);
            let thread = std::thread::Builder::new()
                .name(format!("{name}-{index}"))
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
                        failure(format!("{name} owner {index} panicked: {message}"));
                    }
                })
                .map_err(|error| format!("cannot start {name} owner {index}: {error}"))?;
            pool.lanes.push(Lane {
                tx,
                index,
                reservation: None,
            });
            pool.reserved_loads.push(Arc::new(AtomicUsize::new(0)));
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

    /// Choose the least-loaded persistent owner using admitted model size.
    /// Resolve ties by lane index; timing and OS scheduling do not change the
    /// authoritative receipt order or migrate an existing solver session.
    pub(super) fn reserve_lane(&mut self, weight: usize) -> Lane<C> {
        let weight = weight.max(1);
        let index = self
            .reserved_loads
            .iter()
            .enumerate()
            .min_by_key(|(index, load)| (load.load(Ordering::Relaxed), *index))
            .expect("a constructed worker pool has at least one lane")
            .0;
        let load = Arc::clone(&self.reserved_loads[index]);
        load.fetch_add(weight, Ordering::Relaxed);
        let mut lane = self.lanes[index].clone();
        lane.reservation = Some(Arc::new(LaneReservation { load, weight }));
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
    fn affinity_load_is_balanced_and_owned_by_the_complete_lease() {
        let mut pool = ThreadAffinePool::new("weighted", 2, || (), |e| panic!("{e}")).unwrap();
        let heavy = pool.reserve_lane(10);
        let retained = heavy.clone();
        let small = pool.reserve_lane(1);
        let next = pool.reserve_lane(2);
        assert_eq!((heavy.index(), small.index(), next.index()), (0, 1, 1));
        let read_loads = |pool: &ThreadAffinePool<()>| {
            pool.reserved_loads
                .iter()
                .map(|load| load.load(Ordering::Relaxed))
                .collect::<Vec<_>>()
        };
        assert_eq!(read_loads(&pool), vec![10, 3]);
        drop(heavy);
        assert_eq!(read_loads(&pool), vec![10, 3]);
        drop(retained);
        assert_eq!(read_loads(&pool), vec![0, 3]);
        let replacement = pool.reserve_lane(2);
        assert_eq!(replacement.index(), 0);
        assert_eq!(read_loads(&pool), vec![2, 3]);
        drop((small, next, replacement));
        assert_eq!(read_loads(&pool), vec![0, 0]);
    }

    #[test]
    fn preparation_wait_does_not_block_a_live_owner() {
        let (started_tx, started_rx) = crossbeam_channel::bounded(1);
        let (release_tx, release_rx) = crossbeam_channel::bounded(1);
        let mut preparation =
            ThreadAffinePool::new("isolation-prepare", 1, || (), |e| panic!("{e}")).unwrap();
        let mut live = ThreadAffinePool::new(
            "isolation-live",
            1,
            || Rc::new(Cell::new(0)),
            |e| panic!("{e}"),
        )
        .unwrap();
        preparation.spawn(move || {
            started_tx.send(()).unwrap();
            release_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let (tx, rx) = crossbeam_channel::bounded(1);
        live.next_lane()
            .submit(move |context| {
                context.set(context.get() + 1);
                tx.send(context.get()).unwrap();
            })
            .unwrap();
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
            1
        );
        release_tx.send(()).unwrap();
        drop(preparation);
        drop(live);
    }

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
            "affinity-test",
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
            "affinity-failure",
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
