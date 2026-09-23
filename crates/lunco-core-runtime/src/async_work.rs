//! Shared bounded admission for immutable background preparation.
//!
//! Owners keep their typed jobs and result handling. This module orders queued
//! work before it reaches Bevy's existing async-compute pool; it never commits
//! a result into simulation state.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashSet;

use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use bevy::tasks::AsyncComputeTaskPool;

/// Semantic priority for a queued preparation request.
///
/// Priority only selects which queued job starts first. It never chooses a
/// simulation tick or result commit order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AsyncWorkPriority {
    /// Required to open or continue an explicitly held simulation boundary.
    SimulationRequired,
    /// Serves the active Twin, document, or viewport.
    Interactive,
    /// Optional preparation for inactive or background work.
    Background,
}

impl AsyncWorkPriority {
    const fn index(self) -> usize {
        match self {
            Self::SimulationRequired => 0,
            Self::Interactive => 1,
            Self::Background => 2,
        }
    }
}

/// Kind of immutable preparation submitted to the shared work admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AsyncWorkKind {
    /// Parse a Modelica document source revision.
    ModelicaSourceParse,
    /// Prepare a Modelica source library.
    ModelicaLibraryPreparation,
    /// Compile a Rhai scenario source revision.
    RhaiCompilation,
    /// Analyze a SysML source revision.
    SysmlAnalysis,
    /// Prepare a USD source revision.
    UsdPreparation,
    /// Prepare presentation-only derived work.
    VisualizationPreparation,
}

/// Stable identity for one preparation operation.
///
/// `scope_generation` identifies the mounted Twin or application generation;
/// `identity` identifies the owner; `source_revision` and `operation` separate
/// immutable inputs and repeated work for the same owner. Owners must use
/// stable values, not worker arrival order or ECS iteration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AsyncWorkKey {
    kind: AsyncWorkKind,
    scope_generation: u64,
    identity: u128,
    source_revision: u64,
    operation: u64,
}

impl AsyncWorkKey {
    /// Construct the stable identity for one preparation operation.
    pub const fn new(
        kind: AsyncWorkKind,
        scope_generation: u64,
        identity: u128,
        source_revision: u64,
        operation: u64,
    ) -> Self {
        Self {
            kind,
            scope_generation,
            identity,
            source_revision,
            operation,
        }
    }
}

/// Why a work request was not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncWorkRejection {
    /// The bounded waiting queue is full; the owner may retry after capacity changes.
    QueueFull,
    /// The same stable operation identity is queued or already running.
    DuplicateKey,
    /// This host needs an explicit Web Worker transport for CPU preparation.
    NativeDispatcherUnavailable,
}

/// Why new shared admission limits were rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncWorkLimitError {
    /// Neither queueing nor worker capacity can be zero.
    ZeroLimit,
    /// The requested queue limit is below requests already accepted.
    BelowQueuedCount { queued: usize, requested: usize },
}

/// Queue and worker status for the shared async admission policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsyncWorkSnapshot {
    /// Number of queued jobs in priority order: required, interactive, background.
    pub queued: [usize; 3],
    /// Number of admitted jobs in priority order: required, interactive, background.
    pub in_flight: [usize; 3],
    /// Total accepted jobs since this resource was created.
    pub submitted: u64,
    /// Total jobs that finished since this resource was created.
    pub finished: u64,
    /// Total requests rejected by capacity, duplicate-key, or platform checks.
    pub rejected: u64,
    /// Changes when queued work, limits, or worker counts change.
    pub revision: u64,
    /// Changes only when queue or worker capacity becomes available.
    pub capacity_revision: u64,
}

type AsyncWork = Box<dyn FnOnce() + Send + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct QueueKey {
    priority: AsyncWorkPriority,
    key: AsyncWorkKey,
}

#[derive(Default)]
struct SharedWorkState {
    #[cfg(not(target_arch = "wasm32"))]
    active_keys: Mutex<HashSet<AsyncWorkKey>>,
    in_flight: [AtomicUsize; 3],
    submitted: AtomicU64,
    finished: AtomicU64,
    rejected: AtomicU64,
    revision: AtomicU64,
    capacity_revision: AtomicU64,
}

#[derive(Default)]
struct QueuedWorkState {
    jobs: BTreeMap<QueueKey, AsyncWork>,
    #[cfg(not(target_arch = "wasm32"))]
    keys: HashSet<AsyncWorkKey>,
    #[cfg(not(target_arch = "wasm32"))]
    required_streak: u8,
    #[cfg(not(target_arch = "wasm32"))]
    admissions_since_background: u8,
}

/// Shared admission queue for CPU preparation running on Bevy's async pool.
///
/// Submissions are bounded and sorted by semantic priority, then by stable
/// owner identity. A reserved lower-priority share prevents background work
/// from starvation. Domain owners retain result types, stale-result checks,
/// and commit boundaries.
#[derive(Resource)]
pub struct AsyncWorkAdmission {
    queued: Mutex<QueuedWorkState>,
    shared: Arc<SharedWorkState>,
    /// Maximum number of requests waiting for a worker slot.
    max_queued: usize,
    /// Maximum number of admitted requests running or waiting on the task pool.
    max_in_flight: usize,
}

impl Default for AsyncWorkAdmission {
    fn default() -> Self {
        Self {
            queued: Mutex::new(QueuedWorkState::default()),
            shared: Arc::new(SharedWorkState::default()),
            max_queued: 256,
            max_in_flight: 4,
        }
    }
}

impl AsyncWorkAdmission {
    /// Admit one immutable job without waiting for queue capacity.
    pub fn submit(
        &mut self,
        priority: AsyncWorkPriority,
        key: AsyncWorkKey,
        job: impl FnOnce() + Send + 'static,
    ) -> Result<(), AsyncWorkRejection> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (priority, key, job);
            self.shared.rejected.fetch_add(1, Ordering::Relaxed);
            self.shared.revision.fetch_add(1, Ordering::Release);
            return Err(AsyncWorkRejection::NativeDispatcherUnavailable);
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut queued = self.queued.lock().unwrap_or_else(PoisonError::into_inner);
            let active = self
                .shared
                .active_keys
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if queued.keys.contains(&key) || active.contains(&key) {
                self.shared.rejected.fetch_add(1, Ordering::Relaxed);
                self.shared.revision.fetch_add(1, Ordering::Release);
                return Err(AsyncWorkRejection::DuplicateKey);
            }
            drop(active);

            if queued.jobs.len() >= self.max_queued {
                self.shared.rejected.fetch_add(1, Ordering::Relaxed);
                self.shared.revision.fetch_add(1, Ordering::Release);
                return Err(AsyncWorkRejection::QueueFull);
            }

            queued
                .jobs
                .insert(QueueKey { priority, key }, Box::new(job));
            queued.keys.insert(key);
            self.shared.submitted.fetch_add(1, Ordering::Relaxed);
            self.shared.revision.fetch_add(1, Ordering::Release);
            Ok(())
        }
    }

    /// Set queue and worker limits. Zero limits and shrinking below queued work
    /// are rejected so accepted requests are never silently discarded.
    pub fn set_limits(
        &mut self,
        max_queued: usize,
        max_in_flight: usize,
    ) -> Result<(), AsyncWorkLimitError> {
        let queued = self.queued.lock().unwrap_or_else(PoisonError::into_inner);
        if max_queued == 0 || max_in_flight == 0 {
            return Err(AsyncWorkLimitError::ZeroLimit);
        }
        if max_queued < queued.jobs.len() {
            return Err(AsyncWorkLimitError::BelowQueuedCount {
                queued: queued.jobs.len(),
                requested: max_queued,
            });
        }
        self.max_queued = max_queued;
        self.max_in_flight = max_in_flight;
        self.shared.revision.fetch_add(1, Ordering::Release);
        self.shared
            .capacity_revision
            .fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Current queue, worker, rejection, and revision counters.
    pub fn snapshot(&self) -> AsyncWorkSnapshot {
        let queued_state = self.queued.lock().unwrap_or_else(PoisonError::into_inner);
        let mut queued = [0; 3];
        for key in queued_state.jobs.keys() {
            queued[key.priority.index()] += 1;
        }
        AsyncWorkSnapshot {
            queued,
            in_flight: std::array::from_fn(|index| {
                self.shared.in_flight[index].load(Ordering::Acquire)
            }),
            submitted: self.shared.submitted.load(Ordering::Relaxed),
            finished: self.shared.finished.load(Ordering::Relaxed),
            rejected: self.shared.rejected.load(Ordering::Relaxed),
            revision: self.shared.revision.load(Ordering::Acquire),
            capacity_revision: self.shared.capacity_revision.load(Ordering::Acquire),
        }
    }

    /// Revision used by event-driven owners to retry after queue capacity changes.
    /// Capacity revision used by owners that retry requests rejected by a full queue.
    pub fn capacity_revision(&self) -> u64 {
        self.shared.capacity_revision.load(Ordering::Acquire)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn has_queued(&self) -> bool {
        !self
            .queued
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .jobs
            .is_empty()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn active_count(&self) -> usize {
        self.shared
            .in_flight
            .iter()
            .map(|count| count.load(Ordering::Acquire))
            .sum()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn pop_next(&self) -> Option<(AsyncWorkPriority, AsyncWorkKey, AsyncWork)> {
        const REQUIRED_STREAK_LIMIT: u8 = 8;
        const BACKGROUND_SHARE_INTERVAL: u8 = 16;

        let mut queued = self.queued.lock().unwrap_or_else(PoisonError::into_inner);
        let interactive = queued
            .jobs
            .keys()
            .any(|key| key.priority == AsyncWorkPriority::Interactive);
        let background = queued
            .jobs
            .keys()
            .any(|key| key.priority == AsyncWorkPriority::Background);
        let forced_priority =
            if background && queued.admissions_since_background >= BACKGROUND_SHARE_INTERVAL {
                Some(AsyncWorkPriority::Background)
            } else if interactive && queued.required_streak >= REQUIRED_STREAK_LIMIT {
                Some(AsyncWorkPriority::Interactive)
            } else {
                None
            };
        let queue_key = forced_priority
            .and_then(|priority| {
                queued
                    .jobs
                    .keys()
                    .find(|key| key.priority == priority)
                    .copied()
            })
            .or_else(|| queued.jobs.keys().next().copied())?;

        let job = queued.jobs.remove(&queue_key)?;
        queued.keys.remove(&queue_key.key);
        self.shared.revision.fetch_add(1, Ordering::Release);
        self.shared
            .capacity_revision
            .fetch_add(1, Ordering::Release);
        match queue_key.priority {
            AsyncWorkPriority::SimulationRequired => {
                queued.required_streak = queued.required_streak.saturating_add(1);
                queued.admissions_since_background =
                    queued.admissions_since_background.saturating_add(1);
            }
            AsyncWorkPriority::Interactive => {
                queued.required_streak = 0;
                queued.admissions_since_background =
                    queued.admissions_since_background.saturating_add(1);
            }
            AsyncWorkPriority::Background => {
                queued.required_streak = 0;
                queued.admissions_since_background = 0;
            }
        }
        Some((queue_key.priority, queue_key.key, job))
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct WorkCompletionGuard {
    key: AsyncWorkKey,
    priority: AsyncWorkPriority,
    shared: Arc<SharedWorkState>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for WorkCompletionGuard {
    fn drop(&mut self) {
        let mut active = self
            .shared
            .active_keys
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        active.remove(&self.key);
        self.shared.in_flight[self.priority.index()].fetch_sub(1, Ordering::AcqRel);
        self.shared.finished.fetch_add(1, Ordering::Relaxed);
        self.shared.revision.fetch_add(1, Ordering::Release);
        self.shared
            .capacity_revision
            .fetch_add(1, Ordering::Release);
    }
}

/// Plugin installing the shared queue and its bounded PostUpdate dispatcher.
pub struct AsyncWorkAdmissionPlugin;

impl Plugin for AsyncWorkAdmissionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AsyncWorkAdmission>();
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(PostUpdate, dispatch_async_work.run_if(async_work_is_queued));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn async_work_is_queued(admission: Res<AsyncWorkAdmission>) -> bool {
    admission.has_queued()
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_async_work(admission: ResMut<AsyncWorkAdmission>) {
    let Some(pool) = AsyncComputeTaskPool::try_get() else {
        bevy::log::warn_once!(
            "[async-work] queued preparation cannot start before AsyncComputeTaskPool is initialized"
        );
        return;
    };
    while admission.active_count() < admission.max_in_flight {
        let Some((priority, key, job)) = admission.pop_next() else {
            break;
        };
        let mut active = admission
            .shared
            .active_keys
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !active.insert(key) {
            bevy::log::error!("[async-work] duplicate key reached dispatch: {priority:?} {key:?}");
            continue;
        }
        drop(active);

        admission.shared.in_flight[priority.index()].fetch_add(1, Ordering::AcqRel);
        admission.shared.revision.fetch_add(1, Ordering::Release);
        let completion = WorkCompletionGuard {
            key,
            priority,
            shared: Arc::clone(&admission.shared),
        };
        pool.spawn(async move {
            let _completion = completion;
            job();
        })
        .detach();
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn key(identity: u64) -> AsyncWorkKey {
        AsyncWorkKey::new(AsyncWorkKind::RhaiCompilation, 4, identity as u128, 12, 0)
    }

    #[test]
    fn requests_are_bounded_and_duplicate_identity_is_rejected() {
        let mut admission = AsyncWorkAdmission::default();
        admission.set_limits(1, 1).unwrap();
        admission
            .submit(AsyncWorkPriority::Interactive, key(1), || {})
            .unwrap();

        assert_eq!(
            admission.submit(AsyncWorkPriority::Background, key(1), || {}),
            Err(AsyncWorkRejection::DuplicateKey)
        );
        assert_eq!(
            admission.submit(AsyncWorkPriority::Background, key(2), || {}),
            Err(AsyncWorkRejection::QueueFull)
        );
        assert_eq!(admission.snapshot().queued, [0, 1, 0]);
        assert_eq!(admission.snapshot().rejected, 2);
    }

    #[test]
    fn class_order_and_owner_identity_make_dispatch_stable() {
        let mut admission = AsyncWorkAdmission::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        for (priority, identity) in [
            (AsyncWorkPriority::Background, 1),
            (AsyncWorkPriority::Interactive, 8),
            (AsyncWorkPriority::Interactive, 2),
            (AsyncWorkPriority::SimulationRequired, 9),
        ] {
            let seen = Arc::clone(&seen);
            admission
                .submit(priority, key(identity), move || {
                    seen.lock().unwrap().push(identity);
                })
                .unwrap();
        }

        for _ in 0..4 {
            let (_, _, job) = admission.pop_next().unwrap();
            job();
        }
        assert_eq!(*seen.lock().unwrap(), [9, 2, 8, 1]);
    }

    #[test]
    fn background_gets_a_reserved_admission_share_under_required_load() {
        let mut admission = AsyncWorkAdmission::default();
        for identity in 0..24 {
            admission
                .submit(AsyncWorkPriority::SimulationRequired, key(identity), || {})
                .unwrap();
        }
        admission
            .submit(AsyncWorkPriority::Background, key(100), || {})
            .unwrap();

        for _ in 0..16 {
            assert_eq!(
                admission.pop_next().unwrap().0,
                AsyncWorkPriority::SimulationRequired
            );
        }
        assert_eq!(
            admission.pop_next().unwrap().0,
            AsyncWorkPriority::Background
        );
    }

    #[test]
    fn completion_releases_the_shared_slot_and_operation_identity() {
        let mut admission = AsyncWorkAdmission::default();
        let key = key(1);
        admission
            .submit(AsyncWorkPriority::Interactive, key, || {})
            .unwrap();
        let (priority, key, _) = admission.pop_next().unwrap();
        admission.shared.active_keys.lock().unwrap().insert(key);
        admission.shared.in_flight[priority.index()].fetch_add(1, Ordering::AcqRel);
        let completion = WorkCompletionGuard {
            key,
            priority,
            shared: Arc::clone(&admission.shared),
        };
        assert_eq!(admission.snapshot().in_flight, [0, 1, 0]);
        assert_eq!(
            admission.submit(AsyncWorkPriority::Background, key, || {}),
            Err(AsyncWorkRejection::DuplicateKey)
        );
        drop(completion);
        assert_eq!(admission.snapshot().in_flight, [0, 0, 0]);
        assert_eq!(admission.snapshot().finished, 1);
        assert!(
            admission
                .submit(AsyncWorkPriority::Background, key, || {})
                .is_ok()
        );
    }

    #[test]
    fn capacity_revision_changes_only_when_a_waiting_slot_opens_or_limits_change() {
        let mut admission = AsyncWorkAdmission::default();
        let initial_capacity_revision = admission.capacity_revision();
        admission
            .submit(AsyncWorkPriority::Background, key(1), || {})
            .unwrap();
        assert_eq!(admission.capacity_revision(), initial_capacity_revision);

        let (_, _, job) = admission.pop_next().unwrap();
        job();
        assert_eq!(admission.capacity_revision(), initial_capacity_revision + 1);
        admission.set_limits(128, 3).unwrap();
        assert_eq!(admission.capacity_revision(), initial_capacity_revision + 2);
    }

    #[test]
    fn invalid_limits_do_not_discard_accepted_requests() {
        let mut admission = AsyncWorkAdmission::default();
        admission
            .submit(AsyncWorkPriority::Background, key(1), || {})
            .unwrap();
        admission
            .submit(AsyncWorkPriority::Background, key(2), || {})
            .unwrap();
        assert_eq!(
            admission.set_limits(0, 1),
            Err(AsyncWorkLimitError::ZeroLimit)
        );
        assert_eq!(
            admission.set_limits(0, 0),
            Err(AsyncWorkLimitError::ZeroLimit)
        );
        assert_eq!(
            admission.set_limits(1, 1),
            Err(AsyncWorkLimitError::BelowQueuedCount {
                queued: 2,
                requested: 1
            })
        );
        assert_eq!(admission.snapshot().queued, [0, 0, 2]);
    }

    #[test]
    fn post_update_dispatch_respects_priority_and_global_in_flight_limit() {
        use std::{
            sync::mpsc,
            thread,
            time::{Duration, Instant},
        };

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AsyncWorkAdmissionPlugin));
        let (required_started_tx, required_started_rx) = mpsc::channel();
        let (required_release_tx, required_release_rx) = mpsc::channel();
        let (background_started_tx, background_started_rx) = mpsc::channel();
        let (background_release_tx, background_release_rx) = mpsc::channel();

        {
            let mut admission = app.world_mut().resource_mut::<AsyncWorkAdmission>();
            admission.set_limits(4, 1).unwrap();
            admission
                .submit(AsyncWorkPriority::Background, key(1), move || {
                    background_started_tx.send(()).unwrap();
                    background_release_rx.recv().unwrap();
                })
                .unwrap();
            admission
                .submit(AsyncWorkPriority::SimulationRequired, key(2), move || {
                    required_started_tx.send(()).unwrap();
                    required_release_rx.recv().unwrap();
                })
                .unwrap();
        }

        app.world_mut().run_schedule(PostUpdate);
        required_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("required work starts before background work");
        app.world_mut().run_schedule(PostUpdate);
        assert!(background_started_rx.try_recv().is_err());
        assert_eq!(
            app.world()
                .resource::<AsyncWorkAdmission>()
                .snapshot()
                .in_flight,
            [1, 0, 0]
        );

        required_release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.world_mut().run_schedule(PostUpdate);
            if background_started_rx.try_recv().is_ok() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "background job was not dispatched"
            );
            thread::yield_now();
        }
        background_release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if app
                .world()
                .resource::<AsyncWorkAdmission>()
                .snapshot()
                .finished
                == 2
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "worker completion was not recorded"
            );
            thread::yield_now();
        }
        assert_eq!(
            app.world()
                .resource::<AsyncWorkAdmission>()
                .snapshot()
                .in_flight,
            [0, 0, 0]
        );
    }
}
