//! Shared bounded admission for immutable background preparation.
//!
//! Owners keep their typed jobs and result handling. This module orders queued
//! work on a bounded Bevy task pool isolated from native asset preparation; it
//! never commits a result into simulation state.

use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc, Mutex, PoisonError, Weak,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use bevy::tasks::{AsyncComputeTaskPool, TaskPool, TaskPoolBuilder};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;

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
    /// Prepare an external FMI archive and native instance on its owning thread.
    FmiPreparation,
    /// Synthesize and validate a Modelica network from an immutable USD snapshot.
    ModelicaNetworkSynthesis,
    /// Compile a Rhai scenario source revision.
    RhaiCompilation,
    /// Read and parse an indexed Twin policy source set.
    TwinPolicyPreparation,
    /// Analyze a SysML source revision.
    SysmlAnalysis,
    /// Prepare a USD source, recipe, or immutable projection plan revision.
    UsdPreparation,
    /// Resolve dependencies introduced by an atomic USD document command.
    UsdDocumentAuthoring,
    /// Prepare presentation-only derived work.
    VisualizationPreparation,
    /// Encode an immutable session-input capture for durable export.
    SessionInputArchiveExport,
    /// Encode, validate, or read optional completed experiment artifacts.
    ExperimentArtifact,
    /// One-shot asset validation through an existing async source reader.
    ValidationPreparation,
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
    /// The bounded waiting queue or external-task capacity is full.
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
    /// Queued requests withdrawn by their owner before dispatch.
    pub cancelled: u64,
    /// Changes when queued work, limits, or worker counts change.
    pub revision: u64,
    /// Changes only when queue or worker capacity becomes available.
    pub capacity_revision: u64,
}

type AsyncWork = Box<dyn FnOnce(ExternalWorkPermit) + Send + 'static>;

struct QueuedWork {
    job: AsyncWork,
    request: Option<Arc<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct QueueKey {
    priority: AsyncWorkPriority,
    /// Owner-selected order within one semantic priority class.
    order: u64,
    key: AsyncWorkKey,
}

#[derive(Default)]
struct SharedWorkState {
    queued: Weak<Mutex<QueuedWorkState>>,
    active_keys: Mutex<HashSet<AsyncWorkKey>>,
    in_flight: [AtomicUsize; 3],
    submitted: AtomicU64,
    finished: AtomicU64,
    rejected: AtomicU64,
    cancelled: AtomicU64,
    revision: AtomicU64,
    capacity_revision: AtomicU64,
}

struct QueuedWorkState {
    max_queued: usize,
    max_in_flight: usize,
    stopped: bool,
    jobs: BTreeMap<QueueKey, QueuedWork>,
    keys: HashSet<AsyncWorkKey>,
    #[cfg(not(target_arch = "wasm32"))]
    required_streak: u8,
    #[cfg(not(target_arch = "wasm32"))]
    admissions_since_background: u8,
}

impl Default for QueuedWorkState {
    fn default() -> Self {
        Self {
            jobs: BTreeMap::new(),
            keys: HashSet::new(),
            max_queued: 256,
            max_in_flight: 4,
            stopped: false,
            #[cfg(not(target_arch = "wasm32"))]
            required_streak: 0,
            #[cfg(not(target_arch = "wasm32"))]
            admissions_since_background: 0,
        }
    }
}

impl QueuedWorkState {
    fn has_capacity(&self, priority: AsyncWorkPriority, in_flight: [usize; 3]) -> bool {
        let optional = in_flight[1] + in_flight[2];
        in_flight[0] + optional < self.max_in_flight
            && (priority == AsyncWorkPriority::SimulationRequired
                || self.max_in_flight == 1
                || optional < self.max_in_flight - 1)
    }
}

/// Shared bounded admission for preparation and asynchronous transports.
///
/// Submissions are bounded and sorted by semantic priority, owner-selected
/// order, then stable operation identity. A reserved lower-priority share
/// prevents background work from starvation. With multiple in-flight slots,
/// optional work leaves one available for required preparation. Domain owners
/// retain result types, stale-result checks, and commit boundaries.
#[derive(Resource)]
pub struct AsyncWorkAdmission {
    queued: Arc<Mutex<QueuedWorkState>>,
    shared: Arc<SharedWorkState>,
}

impl Default for AsyncWorkAdmission {
    fn default() -> Self {
        let queued = Arc::new(Mutex::new(QueuedWorkState::default()));
        let shared = Arc::new(SharedWorkState {
            queued: Arc::downgrade(&queued),
            ..Default::default()
        });
        Self { queued, shared }
    }
}

impl Drop for AsyncWorkAdmission {
    fn drop(&mut self) {
        let abandoned = {
            let mut queued = self.queued.lock().unwrap_or_else(PoisonError::into_inner);
            queued.stopped = true;
            queued.keys.clear();
            std::mem::take(&mut queued.jobs)
        };
        // Captured external permits can release capacity when jobs retire.
        // Their drop must happen outside the queue lock and cannot restart it.
        drop(abandoned);
    }
}

impl AsyncWorkAdmission {
    /// Reserve shared capacity for an existing async reader or task transport.
    /// The owner retains this permit through preparation and unconsumed results.
    /// On wasm this admits cooperative page-executor work, not a Web Worker.
    pub fn admit_external(
        &self,
        priority: AsyncWorkPriority,
        key: AsyncWorkKey,
    ) -> Result<ExternalWorkPermit, AsyncWorkRejection> {
        let queued = self.queued.lock().unwrap_or_else(PoisonError::into_inner);
        let mut active = self
            .shared
            .active_keys
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let rejection = if queued.keys.contains(&key) || active.contains(&key) {
            Some(AsyncWorkRejection::DuplicateKey)
        } else if !queued.has_capacity(
            priority,
            self.shared
                .in_flight
                .each_ref()
                .map(|count| count.load(Ordering::Acquire)),
        ) {
            Some(AsyncWorkRejection::QueueFull)
        } else {
            None
        };
        if let Some(error) = rejection {
            self.shared.rejected.fetch_add(1, Ordering::Relaxed);
            self.shared.revision.fetch_add(1, Ordering::Release);
            return Err(error);
        }
        active.insert(key);
        self.shared.in_flight[priority.index()].fetch_add(1, Ordering::AcqRel);
        self.shared.submitted.fetch_add(1, Ordering::Relaxed);
        self.shared.revision.fetch_add(1, Ordering::Release);
        Ok(ExternalWorkPermit {
            _completion: WorkCompletionGuard {
                key,
                priority,
                shared: Arc::clone(&self.shared),
            },
        })
    }

    /// Admit one immutable job without waiting for queue capacity.
    pub fn submit(
        &mut self,
        priority: AsyncWorkPriority,
        key: AsyncWorkKey,
        job: impl FnOnce() + Send + 'static,
    ) -> Result<(), AsyncWorkRejection> {
        self.submit_ordered(priority, key, 0, job)
    }

    /// Admit immutable work with an owner-defined stable order within its
    /// semantic priority class. Lower values start first; equal values use the
    /// stable operation key. The order affects worker admission only, never
    /// result validity or the owner's commit order.
    pub fn submit_ordered(
        &mut self,
        priority: AsyncWorkPriority,
        key: AsyncWorkKey,
        order: u64,
        job: impl FnOnce() + Send + 'static,
    ) -> Result<(), AsyncWorkRejection> {
        self.enqueue(priority, key, order, None, move |permit| {
            let _permit = permit;
            job();
        })
    }

    /// Queue a native transport whose capacity remains owned after its start
    /// callback returns. Keep the request until consumption or retirement, and
    /// transfer the permit through running work and its unconsumed result.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn submit_transport(
        &self,
        priority: AsyncWorkPriority,
        key: AsyncWorkKey,
        start: impl FnOnce(ExternalWorkPermit) + Send + 'static,
    ) -> Result<QueuedWorkRequest, AsyncWorkRejection> {
        let request = Arc::new(());
        self.enqueue(priority, key, 0, Some(Arc::clone(&request)), start)?;
        Ok(QueuedWorkRequest {
            key,
            request,
            queued: Arc::downgrade(&self.queued),
            shared: Arc::clone(&self.shared),
        })
    }

    fn enqueue(
        &self,
        priority: AsyncWorkPriority,
        key: AsyncWorkKey,
        order: u64,
        request: Option<Arc<()>>,
        job: impl FnOnce(ExternalWorkPermit) + Send + 'static,
    ) -> Result<(), AsyncWorkRejection> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (priority, key, order, request, job);
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

            if queued.jobs.len() >= queued.max_queued {
                self.shared.rejected.fetch_add(1, Ordering::Relaxed);
                self.shared.revision.fetch_add(1, Ordering::Release);
                return Err(AsyncWorkRejection::QueueFull);
            }

            let queued_at = bevy::log::tracing::enabled!(bevy::log::Level::DEBUG)
                .then(bevy::platform::time::Instant::now);
            queued.jobs.insert(
                QueueKey {
                    priority,
                    order,
                    key,
                },
                QueuedWork {
                    request,
                    job: Box::new(move |permit| {
                        let _span = bevy::log::debug_span!(
                            "async_preparation_job",
                            kind = ?key.kind,
                            priority = ?priority
                        )
                        .entered();
                        if let Some(queued_at) = queued_at {
                            bevy::log::debug!(
                                kind = ?key.kind,
                                priority = ?priority,
                                key = ?key,
                                queue_wait_ms = queued_at.elapsed().as_secs_f64() * 1000.0,
                                "[async-work] preparation started"
                            );
                        }
                        job(permit);
                    }),
                },
            );
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
        let mut queued = self.queued.lock().unwrap_or_else(PoisonError::into_inner);
        if max_queued == 0 || max_in_flight == 0 {
            return Err(AsyncWorkLimitError::ZeroLimit);
        }
        if max_queued < queued.jobs.len() {
            return Err(AsyncWorkLimitError::BelowQueuedCount {
                queued: queued.jobs.len(),
                requested: max_queued,
            });
        }
        queued.max_queued = max_queued;
        queued.max_in_flight = max_in_flight;
        self.shared.revision.fetch_add(1, Ordering::Release);
        self.shared
            .capacity_revision
            .fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Withdraw a queued request before dispatch. A request already dispatched
    /// to a worker remains owned by that worker; its owner must reject a stale
    /// result at the commit boundary.
    pub fn cancel_queued(&mut self, key: AsyncWorkKey) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = key;
            false
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            cancel_queued_work(&self.queued, &self.shared, key, None)
        }
    }

    /// Withdraw every queued request for one owner identity. Work already
    /// dispatched to a worker remains owner-fenced at its result boundary.
    pub fn cancel_owner(
        &mut self,
        kind: AsyncWorkKind,
        scope_generation: u64,
        identity: u128,
    ) -> usize {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (kind, scope_generation, identity);
            0
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut queued = self.queued.lock().unwrap_or_else(PoisonError::into_inner);
            let keys = queued
                .jobs
                .keys()
                .filter(|queue_key| {
                    queue_key.key.kind == kind
                        && queue_key.key.scope_generation == scope_generation
                        && queue_key.key.identity == identity
                })
                .copied()
                .collect::<Vec<_>>();
            let mut abandoned = Vec::with_capacity(keys.len());
            for queue_key in &keys {
                abandoned.extend(queued.jobs.remove(queue_key));
                queued.keys.remove(&queue_key.key);
            }
            if !keys.is_empty() {
                self.shared
                    .cancelled
                    .fetch_add(keys.len() as u64, Ordering::Relaxed);
                self.shared.revision.fetch_add(1, Ordering::Release);
                self.shared
                    .capacity_revision
                    .fetch_add(1, Ordering::Release);
            }
            drop(queued);
            drop(abandoned);
            keys.len()
        }
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
            cancelled: self.shared.cancelled.load(Ordering::Relaxed),
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

    #[cfg(all(test, not(target_arch = "wasm32")))]
    fn pop_next(&self) -> Option<(AsyncWorkPriority, AsyncWorkKey, AsyncWork)> {
        let mut queued = self.queued.lock().unwrap_or_else(PoisonError::into_inner);
        pop_next_queued(&self.shared, &mut queued)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn pop_next_queued(
    shared: &SharedWorkState,
    queued: &mut QueuedWorkState,
) -> Option<(AsyncWorkPriority, AsyncWorkKey, AsyncWork)> {
    const REQUIRED_STREAK_LIMIT: u8 = 8;
    const BACKGROUND_SHARE_INTERVAL: u8 = 16;

    let in_flight = shared
        .in_flight
        .each_ref()
        .map(|count| count.load(Ordering::Acquire));
    let eligible = |key: &QueueKey| queued.has_capacity(key.priority, in_flight);

    let interactive = queued
        .jobs
        .keys()
        .any(|key| eligible(key) && key.priority == AsyncWorkPriority::Interactive);
    let background = queued
        .jobs
        .keys()
        .any(|key| eligible(key) && key.priority == AsyncWorkPriority::Background);
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
                .find(|key| eligible(key) && key.priority == priority)
                .copied()
        })
        .or_else(|| queued.jobs.keys().find(|key| eligible(key)).copied())?;

    let job = queued.jobs.remove(&queue_key)?;
    queued.keys.remove(&queue_key.key);
    shared.revision.fetch_add(1, Ordering::Release);
    shared.capacity_revision.fetch_add(1, Ordering::Release);
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
    Some((queue_key.priority, queue_key.key, job.job))
}

struct WorkCompletionGuard {
    key: AsyncWorkKey,
    priority: AsyncWorkPriority,
    shared: Arc<SharedWorkState>,
}

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
        drop(active);
        #[cfg(not(target_arch = "wasm32"))]
        refill_async_work(&self.shared);
    }
}

/// Shared capacity retained through preparation and unconsumed transport results.
pub struct ExternalWorkPermit {
    _completion: WorkCompletionGuard,
}

/// Owns one queued native transport request. Dropping it withdraws work that
/// has not dispatched. Running immutable work remains fenced by its domain.
#[cfg(not(target_arch = "wasm32"))]
#[must_use = "retain the request until its result is consumed or retired"]
pub struct QueuedWorkRequest {
    key: AsyncWorkKey,
    request: Arc<()>,
    queued: Weak<Mutex<QueuedWorkState>>,
    shared: Arc<SharedWorkState>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for QueuedWorkRequest {
    fn drop(&mut self) {
        if let Some(queued) = self.queued.upgrade() {
            cancel_queued_work(&queued, &self.shared, self.key, Some(&self.request));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn cancel_queued_work(
    state: &Mutex<QueuedWorkState>,
    shared: &SharedWorkState,
    key: AsyncWorkKey,
    request: Option<&Arc<()>>,
) -> bool {
    let mut queued = state.lock().unwrap_or_else(PoisonError::into_inner);
    let queue_key = queued
        .jobs
        .iter()
        .find(|(entry, work)| {
            entry.key == key
                && request.is_none_or(|expected| {
                    work.request
                        .as_ref()
                        .is_some_and(|actual| Arc::ptr_eq(expected, actual))
                })
        })
        .map(|(entry, _)| *entry);
    let Some(queue_key) = queue_key else {
        return false;
    };
    let abandoned = queued.jobs.remove(&queue_key);
    queued.keys.remove(&key);
    shared.cancelled.fetch_add(1, Ordering::Relaxed);
    shared.revision.fetch_add(1, Ordering::Release);
    shared.capacity_revision.fetch_add(1, Ordering::Release);
    drop(queued);
    drop(abandoned);
    true
}

/// Install the initial PostUpdate dispatch and completion-driven worker refill.
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
fn dispatch_async_work(admission: Res<AsyncWorkAdmission>) {
    refill_async_work(&admission.shared);
}

/// Process-owned execution keeps worker completion from joining its own pool
/// when an application retires. Queues and captured scene work keep their
/// independent owner lifetimes; the host's async width bounds this one pool.
#[cfg(not(target_arch = "wasm32"))]
fn preparation_pool() -> Option<&'static TaskPool> {
    static POOL: OnceLock<TaskPool> = OnceLock::new();
    let host_pool = AsyncComputeTaskPool::try_get()?;
    Some(POOL.get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads(host_pool.thread_num())
            .thread_name("LunCo preparation".to_owned())
            .build()
    }))
}

#[cfg(not(target_arch = "wasm32"))]
fn refill_async_work(shared: &Arc<SharedWorkState>) {
    let Some(queued_state) = shared.queued.upgrade() else {
        return;
    };
    let Some(pool) = preparation_pool() else {
        bevy::log::warn_once!(
            "[async-work] queued preparation cannot start before AsyncComputeTaskPool is initialized"
        );
        return;
    };
    loop {
        let (priority, key, job) = {
            let mut queued = queued_state.lock().unwrap_or_else(PoisonError::into_inner);
            if queued.stopped {
                break;
            }
            let Some((priority, key, job)) = pop_next_queued(shared, &mut queued) else {
                break;
            };
            let mut active = shared
                .active_keys
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !active.insert(key) {
                bevy::log::error!(
                    "[async-work] duplicate key reached dispatch: {priority:?} {key:?}"
                );
                continue;
            }
            shared.in_flight[priority.index()].fetch_add(1, Ordering::AcqRel);
            shared.revision.fetch_add(1, Ordering::Release);
            (priority, key, job)
        };
        let completion = WorkCompletionGuard {
            key,
            priority,
            shared: Arc::clone(shared),
        };
        bevy::log::debug!(
            key = ?key,
            priority = ?priority,
            in_flight = ?shared.in_flight.each_ref().map(|count| count.load(Ordering::Acquire)),
            "[async-work] preparation dispatched"
        );
        pool.spawn(async move {
            job(ExternalWorkPermit {
                _completion: completion,
            });
        })
        .detach();
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn queued_transport_waits_for_capacity_and_retains_its_result_permit() {
        use std::sync::mpsc;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AsyncWorkAdmissionPlugin));
        app.world_mut()
            .resource_mut::<AsyncWorkAdmission>()
            .set_limits(8, 1)
            .unwrap();
        let blocker = app
            .world()
            .resource::<AsyncWorkAdmission>()
            .admit_external(AsyncWorkPriority::Interactive, key(1))
            .unwrap();
        let (first_tx, first_rx) = mpsc::channel();
        let _first = app
            .world()
            .resource::<AsyncWorkAdmission>()
            .submit_transport(AsyncWorkPriority::Interactive, key(2), move |permit| {
                first_tx.send(permit).unwrap();
            })
            .unwrap();
        app.world_mut().run_schedule(PostUpdate);
        assert!(matches!(
            first_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert_eq!(
            app.world()
                .resource::<AsyncWorkAdmission>()
                .snapshot()
                .queued,
            [0, 1, 0]
        );
        drop(blocker);
        let first_result = first_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let (second_tx, second_rx) = mpsc::channel();
        let _second = app
            .world()
            .resource::<AsyncWorkAdmission>()
            .submit_transport(AsyncWorkPriority::Interactive, key(3), move |permit| {
                second_tx.send(permit).unwrap();
            })
            .unwrap();
        app.world_mut().run_schedule(PostUpdate);
        assert!(matches!(
            second_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert_eq!(
            app.world()
                .resource::<AsyncWorkAdmission>()
                .snapshot()
                .in_flight,
            [0, 1, 0]
        );
        drop(first_result);
        drop(second_rx.recv_timeout(Duration::from_secs(2)).unwrap());
        let snapshot = app.world().resource::<AsyncWorkAdmission>().snapshot();
        assert_eq!(snapshot.in_flight, [0, 0, 0]);
        assert_eq!(snapshot.finished, 3);
    }

    #[test]
    fn retired_transport_request_cannot_cancel_a_reused_operation_key() {
        let mut admission = AsyncWorkAdmission::default();
        admission.set_limits(1, 1).unwrap();
        let old = admission
            .submit_transport(AsyncWorkPriority::Interactive, key(1), |_| {})
            .unwrap();
        assert!(admission.cancel_queued(key(1)));
        let current = admission
            .submit_transport(AsyncWorkPriority::Interactive, key(1), |_| {})
            .unwrap();
        drop(old);
        assert_eq!(admission.snapshot().queued, [0, 1, 0]);
        assert!(matches!(
            admission.submit_transport(AsyncWorkPriority::Background, key(2), |_| {}),
            Err(AsyncWorkRejection::QueueFull)
        ));
        drop(current);
        assert_eq!(admission.snapshot().queued, [0, 0, 0]);
        assert_eq!(admission.snapshot().cancelled, 2);
    }

    #[test]
    fn native_asset_work_cannot_block_admitted_preparation() {
        use std::sync::mpsc;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AsyncWorkAdmissionPlugin));
        let native_pool = AsyncComputeTaskPool::get();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = crossbeam_channel::unbounded::<()>();
        for _ in 0..native_pool.thread_num() {
            let started_tx = started_tx.clone();
            let release_rx = release_rx.clone();
            native_pool
                .spawn(async move {
                    started_tx.send(()).unwrap();
                    let _ = release_rx.recv();
                })
                .detach();
        }
        for _ in 0..native_pool.thread_num() {
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        let (prepared_tx, prepared_rx) = mpsc::channel();
        app.world_mut()
            .resource_mut::<AsyncWorkAdmission>()
            .submit(AsyncWorkPriority::SimulationRequired, key(1), move || {
                let _ = prepared_tx.send(());
            })
            .unwrap();
        app.world_mut().run_schedule(PostUpdate);
        prepared_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("admitted preparation must run while native asset workers are occupied");
        drop(release_tx);
    }

    #[test]
    fn worker_completion_refills_queue_without_another_app_update() {
        use std::sync::mpsc;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AsyncWorkAdmissionPlugin));
        let (finished_tx, finished_rx) = mpsc::channel();
        {
            let mut admission = app.world_mut().resource_mut::<AsyncWorkAdmission>();
            admission.set_limits(16, 1).unwrap();
            for identity in 0..8 {
                let finished_tx = finished_tx.clone();
                admission
                    .submit(AsyncWorkPriority::Interactive, key(identity), move || {
                        finished_tx.send(identity).unwrap();
                    })
                    .unwrap();
            }
        }
        app.world_mut().run_schedule(PostUpdate);
        for identity in 0..8 {
            assert_eq!(
                finished_rx.recv_timeout(Duration::from_secs(2)).expect(
                    "accepted immutable work must refill freed capacity without a host frame"
                ),
                identity
            );
        }
    }

    #[test]
    fn retiring_admission_discards_queued_work_before_worker_completion() {
        use std::sync::mpsc;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AsyncWorkAdmissionPlugin));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (retired_tx, retired_rx) = mpsc::channel();
        {
            let mut admission = app.world_mut().resource_mut::<AsyncWorkAdmission>();
            admission.set_limits(4, 1).unwrap();
            admission
                .submit(AsyncWorkPriority::Interactive, key(1), move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                })
                .unwrap();
            admission
                .submit(AsyncWorkPriority::Interactive, key(2), move || {
                    retired_tx.send(()).unwrap();
                })
                .unwrap();
        }
        app.world_mut().run_schedule(PostUpdate);
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        app.world_mut().remove_resource::<AsyncWorkAdmission>();
        release_tx.send(()).unwrap();
        assert!(matches!(
            retired_rx.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }

    #[test]
    fn queued_cancellation_releases_captured_permits_outside_queue_lock() {
        use std::sync::mpsc;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AsyncWorkAdmissionPlugin));
        let (started_tx, started_rx) = mpsc::channel();
        {
            let mut admission = app.world_mut().resource_mut::<AsyncWorkAdmission>();
            admission.set_limits(4, 1).unwrap();
            let permit = admission
                .admit_external(AsyncWorkPriority::Interactive, key(1))
                .unwrap();
            admission
                .submit(AsyncWorkPriority::Interactive, key(2), move || drop(permit))
                .unwrap();
            admission
                .submit(AsyncWorkPriority::Interactive, key(3), move || {
                    started_tx.send(()).unwrap();
                })
                .unwrap();
            assert!(admission.cancel_queued(key(2)));
        }
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("released external capacity must refill surviving work");
    }

    fn key(identity: u64) -> AsyncWorkKey {
        AsyncWorkKey::new(AsyncWorkKind::RhaiCompilation, 4, identity as u128, 12, 0)
    }

    #[test]
    fn external_permits_bound_results_and_release_exact_identity() {
        let mut admission = AsyncWorkAdmission::default();
        admission.set_limits(2, 1).unwrap();
        let permit = Arc::new(
            admission
                .admit_external(AsyncWorkPriority::Interactive, key(91))
                .unwrap(),
        );
        assert!(matches!(
            admission.admit_external(AsyncWorkPriority::Interactive, key(91)),
            Err(AsyncWorkRejection::DuplicateKey)
        ));
        assert!(matches!(
            admission.admit_external(AsyncWorkPriority::Interactive, key(92)),
            Err(AsyncWorkRejection::QueueFull)
        ));
        let worker = Arc::clone(&permit);
        drop(permit);
        assert_eq!(admission.snapshot().in_flight, [0, 1, 0]);
        drop(worker);
        assert_eq!(admission.snapshot().in_flight, [0, 0, 0]);
        let replacement = admission
            .admit_external(AsyncWorkPriority::Interactive, key(91))
            .unwrap();
        assert_eq!(admission.snapshot().in_flight, [0, 1, 0]);
        drop(replacement);
        assert_eq!(admission.snapshot().finished, 2);
    }

    #[test]
    fn required_capacity_is_protected_from_queued_and_external_optional_work() {
        let mut admission = AsyncWorkAdmission::default();
        admission.set_limits(8, 2).unwrap();
        let optional = admission
            .admit_external(AsyncWorkPriority::Interactive, key(1))
            .unwrap();
        assert!(matches!(
            admission.admit_external(AsyncWorkPriority::Background, key(2)),
            Err(AsyncWorkRejection::QueueFull)
        ));
        admission
            .submit(AsyncWorkPriority::Background, key(2), || {})
            .unwrap();
        assert!(
            admission.pop_next().is_none(),
            "optional work must retain required capacity"
        );
        admission
            .submit(AsyncWorkPriority::SimulationRequired, key(3), || {})
            .unwrap();
        let (priority, identity, job) = admission.pop_next().unwrap();
        assert_eq!(
            (priority, identity),
            (AsyncWorkPriority::SimulationRequired, key(3))
        );
        drop(job);
        assert!(admission.pop_next().is_none());
        let required = admission
            .admit_external(AsyncWorkPriority::SimulationRequired, key(4))
            .unwrap();
        assert_eq!(admission.snapshot().in_flight, [1, 1, 0]);
        assert!(admission.cancel_queued(key(2)));
        drop(required);
        drop(optional);
        assert_eq!(admission.snapshot().in_flight, [0, 0, 0]);
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
        for (priority, identity) in [
            (AsyncWorkPriority::Background, 1),
            (AsyncWorkPriority::Interactive, 8),
            (AsyncWorkPriority::Interactive, 2),
            (AsyncWorkPriority::SimulationRequired, 9),
        ] {
            admission.submit(priority, key(identity), || {}).unwrap();
        }

        let seen: Vec<_> = (0..4).map(|_| admission.pop_next().unwrap().1).collect();
        assert_eq!(seen, [key(9), key(2), key(8), key(1)]);
    }

    #[test]
    fn owner_order_precedes_operation_identity_within_a_priority_class() {
        let mut admission = AsyncWorkAdmission::default();
        for (order, identity) in [(8, 1), (2, 9), (2, 3), (5, 4)] {
            admission
                .submit_ordered(AsyncWorkPriority::Interactive, key(identity), order, || {})
                .unwrap();
        }

        let seen: Vec<_> = (0..4).map(|_| admission.pop_next().unwrap().1).collect();
        assert_eq!(seen, [key(3), key(9), key(4), key(1)]);
    }

    #[test]
    fn owner_teardown_cancels_only_its_queued_preparation() {
        let mut admission = AsyncWorkAdmission::default();
        let kind = AsyncWorkKind::VisualizationPreparation;
        let owner_key = |identity, operation| AsyncWorkKey::new(kind, 12, identity, 8, operation);
        for (identity, operation) in [(51, 1), (51, 2), (52, 1)] {
            admission
                .submit(
                    AsyncWorkPriority::Interactive,
                    owner_key(identity, operation),
                    || {},
                )
                .unwrap();
        }

        assert_eq!(admission.cancel_owner(kind, 12, 51), 2);
        assert_eq!(admission.snapshot().cancelled, 2);
        assert_eq!(admission.pop_next().unwrap().1, owner_key(52, 1));
        assert!(admission.pop_next().is_none());
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
        drop(job);
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
    fn owner_can_cancel_queued_work_and_reuse_its_identity() {
        let mut admission = AsyncWorkAdmission::default();
        let key = key(11);
        admission
            .submit(AsyncWorkPriority::SimulationRequired, key, || {})
            .unwrap();
        let capacity_revision = admission.capacity_revision();

        assert!(admission.cancel_queued(key));
        assert_eq!(admission.snapshot().queued, [0, 0, 0]);
        assert_eq!(admission.snapshot().cancelled, 1);
        assert_eq!(admission.capacity_revision(), capacity_revision + 1);
        assert!(
            admission
                .submit(AsyncWorkPriority::SimulationRequired, key, || {})
                .is_ok()
        );
    }

    #[test]
    fn owner_cannot_cancel_work_after_dispatch() {
        let mut admission = AsyncWorkAdmission::default();
        let key = key(12);
        admission
            .submit(AsyncWorkPriority::SimulationRequired, key, || {})
            .unwrap();
        let (priority, active_key, _) = admission.pop_next().unwrap();
        admission
            .shared
            .active_keys
            .lock()
            .unwrap()
            .insert(active_key);
        admission.shared.in_flight[priority.index()].fetch_add(1, Ordering::AcqRel);

        assert!(!admission.cancel_queued(key));
        assert_eq!(admission.snapshot().in_flight, [1, 0, 0]);
        assert_eq!(admission.snapshot().cancelled, 0);
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
