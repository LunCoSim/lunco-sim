//! Cross-cutting status bus for the workbench.
//!
//! Every subsystem (MSL load, compile, sim, save, API, …) publishes
//! `StatusEvent`s into the [`StatusBus`] resource. A single set of
//! renderers fans events out to:
//!
//! - **Status bar** at the bottom of the viewport — latest live event
//!   per source, clickable to open a history popover.
//! - **Console panel** — every event, chronological audit trail.
//! - **Diagnostics panel** — only error/warning events tied to the
//!   active document.
//!
//! Subsystems don't know about any of those views; they just `push` and
//! the right surfaces light up. New views (egui native status bar,
//! external API stream) just subscribe to the bus.
//!
//! ## Two flavours of event
//!
//! - **Discrete** events ([`StatusBus::push`]) are appended to `history`
//!   and shown in Console / Diagnostics. Use for "MSL ready",
//!   "compile started", "save failed".
//! - **Progress** events ([`StatusBus::set_progress`]) replace the
//!   most recent progress entry from the same source instead of being
//!   appended — they would otherwise spam the history during a long
//!   download. Each `(done, total)` tick *replaces* the prior tick from
//!   that source. Once `done == total`, callers typically follow with
//!   a discrete `Info` event (e.g. "MSL ready") to terminate.
//!
//! ## Change detection
//!
//! `seq()` increments on every push. Renderers cache the last seq they
//! saw and skip the DOM/UI update when nothing moved.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;

use bevy::prelude::*;
use web_time::Instant;

/// Scope of a busy/progress entry. Determines which surfaces render the
/// indicator (per-tab overlay, per-node tree row, global status bar) and
/// allows aggregate queries via [`StatusBus::is_busy`].
///
/// IDs are opaque `u64` newtypes so this enum stays decoupled from the
/// concrete document/tab/node types in upstream crates. Convert at the
/// call site: `BusyScope::Tab(tab_id.0)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BusyScope {
    /// Whole application — appears in the bottom status bar.
    Global,
    /// Tied to a specific document; multi-tab views of that document
    /// can all reflect the same busy state.
    Document(u64),
    /// Tied to a specific tab/pane; only that tab's overlay renders it.
    Tab(u64),
    /// Tied to a tree node (e.g. a package-browser row).
    Node(u64),
}

impl BusyScope {
    /// Returns `true` when `self` is `other` or contained by `other` in
    /// the scope hierarchy. Used by [`StatusBus::is_busy`] to answer
    /// "is anything in this scope busy?" queries.
    ///
    /// Hierarchy (today): `Tab` ⊂ `Global`; `Node` ⊂ `Global`; `Document`
    /// ⊂ `Global`. Tab/Document linkage is resolved by the caller (the
    /// bus does not know which tab belongs to which document).
    pub fn is_within(self, other: BusyScope) -> bool {
        if self == other {
            return true;
        }
        matches!(other, BusyScope::Global)
    }
}

/// Opaque identifier for a single in-flight busy entry. Issued by
/// [`StatusBus::begin`] and carried by [`BusyHandle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BusyId(u64);

/// Terminal state of a unit of work, recorded on the bus when the
/// owning [`BusyHandle`] drops. Lets panels distinguish "no content
/// because the task succeeded with an empty result" from "no content
/// because the task failed" — or was cancelled — without each panel
/// keeping its own per-error state.
///
/// Default on plain `Drop` is [`BusyOutcome::Succeeded`]; failure
/// paths call [`BusyHandle::set_outcome`] with [`BusyOutcome::Failed`].
/// [`spawn_tracked_cancellable`](crate::tracked_task::spawn_tracked_cancellable)
/// records [`BusyOutcome::Cancelled`] automatically when the
/// cooperative cancel flag was set at the time the future finished.
#[derive(Debug, Clone)]
pub enum BusyOutcome {
    /// Work ran to completion. Empty results are still `Succeeded` —
    /// "no nodes to draw" is a successful empty diagram.
    Succeeded,
    /// Work terminated with a user-visible error.
    Failed(String),
    /// Work short-circuited because the caller (or the user via the
    /// cancel button) flipped the cancel token. Distinct from
    /// `Succeeded` so panels can choose a neutral affordance
    /// instead of an "empty result" overlay.
    Cancelled,
}

/// RAII guard for an in-flight busy entry. Move into the task / per-tab
/// state whose lifetime defines the work; on `Drop` the bus removes the
/// entry on the next frame (via a drained mpsc channel) so callers
/// cannot leak progress state by forgetting to remove the owning handle.
///
/// Send-safe: may be moved into `AsyncComputeTaskPool` futures.
pub struct BusyHandle {
    id: BusyId,
    drop_tx: Sender<(BusyId, BusyOutcome)>,
    outcome: Option<BusyOutcome>,
}

impl BusyHandle {
    /// Identifier for this handle. Stable for the handle's lifetime.
    pub fn id(&self) -> BusyId {
        self.id
    }

    /// Record a terminal outcome for this handle. Replaces any prior
    /// outcome; the value set last before `Drop` is what the bus
    /// stores in `last_outcome`. Use [`BusyOutcome::Failed`] to mark
    /// a user-visible failure so panels can render an error overlay
    /// without per-panel error plumbing.
    pub fn set_outcome(&mut self, outcome: BusyOutcome) {
        self.outcome = Some(outcome);
    }
}

impl Drop for BusyHandle {
    fn drop(&mut self) {
        // Default to `Succeeded` — most tasks complete normally and
        // never call `set_outcome`. Failure paths must opt in.
        let outcome = self.outcome.take().unwrap_or(BusyOutcome::Succeeded);
        // Best-effort: receiver is held by the bus; if the bus has been
        // dropped (e.g. app shutdown) the send simply fails.
        let _ = self.drop_tx.send((self.id, outcome));
    }
}

/// Single state machine derived from the bus + a panel's content
/// predicate. Replaces per-panel OR-of-booleans like
/// `loading || parse_pending || projecting`.
#[derive(Debug, Clone)]
pub enum LifecycleState {
    /// Work is in flight for this scope. Render a loading indicator.
    Loading,
    /// No work in flight and the panel has content. Render content,
    /// no overlay.
    Content,
    /// No work in flight, no content, last outcome (if any) was
    /// success. Render an "empty" affordance.
    Empty,
    /// No work in flight, no content, last outcome was failure.
    /// Carries the error message for the overlay.
    Failed(String),
}

/// Maximum number of *discrete* events kept in `history`. Progress
/// events don't count against this — they're stored separately.
pub const STATUS_HISTORY_CAPACITY: usize = 200;

/// Severity / classification of a status event. Drives:
/// - Status bar dot colour.
/// - Console log level.
/// - Diagnostics inclusion (Error / Warn surface there; Info / Progress don't).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusLevel {
    /// Routine notification — green dot, console `info`.
    Info,
    /// Recoverable issue — yellow dot, console `warn`, surfaces in Diagnostics.
    Warn,
    /// Failure — red dot, console `error`, surfaces in Diagnostics.
    Error,
    /// Actionable notification — red dot and text, console `info`.
    ///
    /// The status bar emits [`StatusBarAction`] when the user clicks an
    /// attention event. The owning feature decides what that action means.
    Attention,
    /// In-flight progress tick. Replaces the last `Progress` from the
    /// same source instead of appending. Use [`StatusBus::set_progress`]
    /// for state mirrors or [`StatusBus::begin`] for task-owned work.
    Progress,
}

/// Typed intent emitted when the user clicks an actionable status-bar event.
///
/// The workbench does not know what a source-specific action should do. The
/// owning feature observes this event and queues its own typed view-model
/// action, keeping the status bar generic and preventing it from reaching into
/// domain state.
#[derive(Event, Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusBarAction {
    /// Source key of the clicked status event.
    pub source: &'static str,
}

/// One status event. Carries everything the renderers need without
/// extra resource look-ups.
#[derive(Debug, Clone)]
pub struct StatusEvent {
    /// Scope of this event. Discrete events default to [`BusyScope::Global`];
    /// scoped progress originates from [`StatusBus::begin`].
    pub scope: BusyScope,
    /// Short subsystem identifier shown to the user (`"MSL"`, `"Compile"`).
    pub source: &'static str,
    /// Severity classification — drives icon, log level, and Diagnostics inclusion.
    pub level: StatusLevel,
    /// Human-readable message body.
    pub message: String,
    /// `(done, total)` for progress events; `None` for discrete events.
    /// `total == 0` means indeterminate progress (show shimmer / spinner).
    pub progress: Option<(u64, u64)>,
    /// Wall-clock time the event was pushed, used for ordering and decay.
    pub at: Instant,
    /// Opaque id when this event is the active progress for a [`BusyHandle`].
    /// `None` for discrete events and direct global progress events.
    pub busy_id: Option<BusyId>,
    /// Optional cancellation flag shared with the originating task.
    /// Indicators rendered for an entry whose `cancel` is `Some` show
    /// a `[✕]` button that flips the inner `AtomicBool` to `true`;
    /// the task's cooperative-cancel checkpoints (e.g. projection's
    /// `should_stop`) then short-circuit. Dropping the handle alone
    /// is *not* enough for tasks that run to completion in the
    /// pool — the flag is what stops the work.
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl StatusEvent {
    /// Percentage `0.0..=100.0` derived from `progress`. Returns `None`
    /// when the event has no progress or `total == 0`.
    pub fn progress_pct(&self) -> Option<f64> {
        let (done, total) = self.progress?;
        if total == 0 {
            return None;
        }
        Some((done as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
    }
}

/// The bus source name terrain streaming publishes under.
///
/// Shared rather than spelled twice. The publisher (`lunco-luncosim`'s
/// `report_terrain_stream_status`) and the consumer (the screenshot readiness
/// gate's `VISUAL_BUSY_SOURCES`) were previously coupled by two independent
/// `"terrain"` literals: renaming one silently stopped the gate from gating, with
/// no compile error and no log, so recordings quietly started over streaming
/// terrain again. One `const`, one compile error if it ever moves.
pub const TERRAIN_SOURCE: &str = "terrain";

/// The status source for optional derived terrain visual products. It is kept
/// separate from [`TERRAIN_SOURCE`] because refinement must not replace the
/// tile-streaming progress entry or make the terrain stream appear complete.
/// The optional source is intentionally not part of the screenshot readiness
/// allowlist: ground and resident tiles are presentable before refinement.
pub const TERRAIN_DERIVED_SOURCE: &str = "terrain-derived";

/// The status source for native/web DEM generation. It is separate from
/// [`TERRAIN_SOURCE`], which reports streamed tile residency, so one terrain
/// lifecycle cannot clear the other's progress entry.
pub const TERRAIN_BUILD_SOURCE: &str = "terrain-build";

/// The status source for an explicit presentation camera supplied by the
/// windowed host when a scene has no authored viewport camera.
pub const CAMERA_SOURCE: &str = "camera";

/// The bus source name USD scene spawning publishes under.
///
/// Shared for the same reason as [`TERRAIN_SOURCE`]: the publisher
/// (`lunco-luncosim`'s `report_scene_spawn_status`, mirroring
/// `lunco_usd_sim::cosim::SceneLoadInFlight` + `UsdAwaitingStage`) and the
/// screenshot readiness gate must agree on the spelling, and a silent
/// disagreement degrades into recordings that open on a half-spawned scene.
pub const SCENE_SOURCE: &str = "scene";

/// The status source for a textured USD DomeLight's image projection. The
/// projection is render-visible work, so offline recording must wait for it in
/// the same way it waits for scene spawning and terrain streaming.
pub const DOME_SOURCE: &str = "dome";

/// The status source for USD-driven Modelica participants. The luncosim
/// composition root mirrors pending source loads and compiler states here so
/// the offline recorder waits on the same lifecycle that holds a not-yet-
/// runnable vehicle out of physics.
pub const MODELICA_SOURCE: &str = "modelica";

/// The status source for the Modelica editor's active document/compiler.
/// Kept separate from [`MODELICA_SOURCE`], which reports USD-driven runtime
/// participants, so editor transitions cannot clear or replace scene status.
pub const MODELICA_EDITOR_SOURCE: &str = "modelica-editor";

/// Runtime-authored HTML surfaces that are part of the recorded presentation.
/// The offline recorder waits for this source so a required HUD cannot appear
/// one render frame after the shot's first frame.
pub const RUNTIME_UI_SOURCE: &str = "runtime-ui";

/// Workbench-wide status bus. Insert via [`StatusBusPlugin`].
///
/// Carries two flavours of state — discrete history events (info / warn /
/// error) and active progress entries keyed by `(scope, source)`. Active
/// progress is what indicators read; history is what the status-bar
/// popup and toast renderer consume.
#[derive(Resource)]
pub struct StatusBus {
    /// Append-only history of *discrete* events, capped at
    /// [`STATUS_HISTORY_CAPACITY`]. Older entries fall off the front.
    history: VecDeque<StatusEvent>,
    /// Latest in-flight progress per `(scope, source)`. Replaced on every
    /// `set_progress` from the same key; cleared by `remove_progress`,
    /// `end`, or [`BusyHandle`] drop.
    active_progress: HashMap<(BusyScope, &'static str), StatusEvent>,
    /// Reverse index: which `(scope, source)` does a given `BusyId` own?
    /// Lets [`BusyHandle::Drop`] clear the right entry without the caller
    /// remembering its own scope/source.
    by_id: HashMap<BusyId, (BusyScope, &'static str)>,
    /// Handles retained by level-triggered state mirrors. Task-owned work
    /// keeps its handle in the task or domain state instead.
    mirror_handles: HashMap<(BusyScope, &'static str), BusyHandle>,
    /// Bumped on every push (discrete or progress). Renderers cache the
    /// last seq they saw to skip work when the bus hasn't changed.
    seq: u64,
    /// Total discrete events ever appended to `history`, including ones
    /// since dropped off the front of the capped ring. Unlike
    /// `history().count()` (which plateaus at capacity), this keeps
    /// growing, so a consumer mirroring history can use it as a
    /// never-stalling watermark (CQ-523).
    history_total: u64,
    /// Monotonic counter for new [`BusyId`]s.
    next_id: u64,
    /// Sender cloned into every [`BusyHandle`]. The matching receiver
    /// lives in `drop_rx`; `drain_busy_drops` walks it each frame.
    drop_tx: Sender<(BusyId, BusyOutcome)>,
    /// Receiver for handle-drop notifications. `Mutex` because Bevy
    /// requires `Resource: Send + Sync` and `Receiver` is `!Sync`.
    drop_rx: Mutex<Receiver<(BusyId, BusyOutcome)>>,
    /// Latest terminal outcome per `(scope, source)`. Written by
    /// `drain_busy_drops` when a [`BusyHandle`] drops; consulted by
    /// [`Self::lifecycle`] so panels can distinguish "successfully
    /// empty" from "failed" without per-panel error state.
    last_outcome: HashMap<(BusyScope, &'static str), (BusyOutcome, Instant)>,
}

impl Default for StatusBus {
    fn default() -> Self {
        let (drop_tx, drop_rx) = channel();
        Self {
            history: VecDeque::new(),
            active_progress: HashMap::new(),
            by_id: HashMap::new(),
            mirror_handles: HashMap::new(),
            seq: 0,
            history_total: 0,
            next_id: 0,
            drop_tx,
            drop_rx: Mutex::new(drop_rx),
            last_outcome: HashMap::new(),
        }
    }
}

impl StatusBus {
    /// Append a discrete event to history.
    pub fn push(&mut self, source: &'static str, level: StatusLevel, message: impl Into<String>) {
        debug_assert!(
            level != StatusLevel::Progress,
            "use set_progress or begin for Progress events"
        );
        let ev = StatusEvent {
            scope: BusyScope::Global,
            source,
            level,
            message: message.into(),
            progress: None,
            at: Instant::now(),
            busy_id: None,
            cancel: None,
        };
        if self.history.len() >= STATUS_HISTORY_CAPACITY {
            self.history.pop_front();
        }
        self.history.push_back(ev);
        self.history_total = self.history_total.wrapping_add(1);
        self.seq = self.seq.wrapping_add(1);
    }

    /// Set the active progress projection for a state source. The bus owns
    /// the handle because a state mirror has no task object whose lifetime
    /// can hold one. Repeated calls update the same entry; `remove_progress`
    /// ends it. This does not append to `history` — call `push(..., Info, ...)`
    /// separately to mark phase transitions you want preserved.
    ///
    /// The entry's `at` is the *start* time: when a tick replaces an
    /// existing entry at the same `(Global, source)` key, the original
    /// `at` is preserved rather than reset to now. This keeps
    /// [`Self::display_latest`]'s "longest-running stays pinned" contract
    /// intact for sources that tick continuously — e.g. the MSL download
    /// (which starts at boot and re-pushes every frame) must stay the
    /// pinned status-bar entry even when a later, shorter task (a queued
    /// compile) begins. Resetting `at` each tick made MSL perennially the
    /// *youngest* entry, so the bar flipped to "Compiling…" and the
    /// download progress disappeared.
    pub fn set_progress(
        &mut self,
        source: &'static str,
        message: impl Into<String>,
        done: u64,
        total: u64,
    ) {
        let message = message.into();
        let key = (BusyScope::Global, source);
        if !self.mirror_handles.contains_key(&key) {
            let handle = self.begin(BusyScope::Global, source, message.clone());
            self.mirror_handles.insert(key, handle);
        }
        let Some(id) = self.mirror_handles.get(&key).map(BusyHandle::id) else {
            return;
        };
        self.update_progress_by_id(id, done, total);
        self.update_label_by_id(id, message);
    }

    /// Remove the active progress projection for `source`.
    pub fn remove_progress(&mut self, source: &'static str) {
        let key = (BusyScope::Global, source);
        if let Some(handle) = self.mirror_handles.remove(&key) {
            self.clear_by_id(handle.id, BusyOutcome::Succeeded);
        }
    }

    /// Begin tracking an in-flight unit of work. Returns a [`BusyHandle`]
    /// whose `Drop` removes the entry on the next frame (via
    /// `drain_busy_drops`). Move the handle into the future / per-tab
    /// state whose lifetime defines the work.
    ///
    /// Replaces any existing entry at `(scope, source)` — this is the
    /// same dedup behaviour as [`Self::set_progress`], extended to scopes.
    pub fn begin(
        &mut self,
        scope: BusyScope,
        source: &'static str,
        label: impl Into<String>,
    ) -> BusyHandle {
        self.mirror_handles.remove(&(scope, source));
        let id = BusyId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        // Evict any prior entry at the same (scope, source) — keep the
        // by_id index in sync so the prior handle's drop is a no-op.
        if let Some(prev) = self.active_progress.get(&(scope, source)) {
            if let Some(prev_id) = prev.busy_id {
                self.by_id.remove(&prev_id);
            }
        }
        let ev = StatusEvent {
            scope,
            source,
            level: StatusLevel::Progress,
            message: label.into(),
            progress: None,
            at: Instant::now(),
            busy_id: Some(id),
            cancel: None,
        };
        self.active_progress.insert((scope, source), ev);
        self.by_id.insert(id, (scope, source));
        self.seq = self.seq.wrapping_add(1);
        BusyHandle {
            id,
            drop_tx: self.drop_tx.clone(),
            outcome: None,
        }
    }

    /// [`Self::begin`] variant that records a cancellation flag the
    /// indicator widget can flip from a `[✕]` button. The originating
    /// task is responsible for honouring the flag at its cooperative
    /// checkpoints — flipping it does not abort a `bevy::tasks::Task`
    /// in the pool, only signals the body to short-circuit.
    pub fn begin_cancellable(
        &mut self,
        scope: BusyScope,
        source: &'static str,
        label: impl Into<String>,
        token: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> BusyHandle {
        let handle = self.begin(scope, source, label);
        // `begin` just inserted the entry — we know the slot exists.
        if let Some(ev) = self.active_progress.get_mut(&(scope, source)) {
            ev.cancel = Some(token);
        }
        handle
    }

    /// Update the progress tick for an outstanding [`BusyHandle`].
    /// `total == 0` means indeterminate.
    pub fn with_progress(&mut self, handle: &BusyHandle, done: u64, total: u64) {
        self.update_progress_by_id(handle.id, done, total);
    }

    /// Update the human-readable label for an outstanding [`BusyHandle`].
    pub fn with_label(&mut self, handle: &BusyHandle, label: impl Into<String>) {
        self.update_label_by_id(handle.id, label);
    }

    fn update_progress_by_id(&mut self, id: BusyId, done: u64, total: u64) {
        let Some(&(scope, source)) = self.by_id.get(&id) else {
            return;
        };
        let Some(ev) = self.active_progress.get_mut(&(scope, source)) else {
            return;
        };
        if ev.progress == Some((done, total)) {
            return;
        }
        ev.progress = Some((done, total));
        self.seq = self.seq.wrapping_add(1);
    }

    fn update_label_by_id(&mut self, id: BusyId, label: impl Into<String>) {
        let Some(&(scope, source)) = self.by_id.get(&id) else {
            return;
        };
        let Some(ev) = self.active_progress.get_mut(&(scope, source)) else {
            return;
        };
        let label = label.into();
        if ev.message == label {
            return;
        }
        ev.message = label;
        self.seq = self.seq.wrapping_add(1);
    }

    /// Internal: clear the entry owned by `id` and record its
    /// terminal `outcome` in `last_outcome` for the corresponding
    /// `(scope, source)`. Called by `drain_busy_drops` when a
    /// [`BusyHandle`] is dropped.
    fn clear_by_id(&mut self, id: BusyId, outcome: BusyOutcome) {
        let Some(key) = self.by_id.remove(&id) else {
            return;
        };
        // Only clear if this id still owns the slot — a re-`begin` at
        // the same key would have already evicted us via `by_id`.
        // The outcome is recorded regardless: even when the handle's
        // entry was superseded, the terminal state for that source
        // is still useful as the "most recent outcome".
        self.last_outcome.insert(key, (outcome, Instant::now()));
        if let Some(ev) = self.active_progress.get(&key) {
            if ev.busy_id == Some(id) {
                self.active_progress.remove(&key);
                self.seq = self.seq.wrapping_add(1);
            }
        }
    }

    /// Drop every recorded outcome whose `(scope, _)` matches
    /// `target` exactly. Use on resource teardown (document close,
    /// tab close, panel close) so `last_outcome` doesn't grow
    /// unboundedly across a long-running session. Scope hierarchy
    /// is NOT walked here — `Global` only clears `Global` entries,
    /// not all descendants — because callers know the precise scope
    /// they're tearing down.
    pub fn clear_outcomes_for(&mut self, target: BusyScope) {
        let before = self.last_outcome.len();
        self.last_outcome.retain(|(s, _), _| *s != target);
        if self.last_outcome.len() != before {
            self.seq = self.seq.wrapping_add(1);
        }
    }

    /// Wind down all live status state owned by the active Twin.
    ///
    /// The bus deliberately keeps its bounded history: that is an
    /// application-level audit trail, not an active Twin resource. In-flight
    /// progress, terminal outcomes, and mirror handles are live state and
    /// cannot survive a Twin replacement. Clearing `by_id` also makes drops
    /// from work that finishes after teardown harmless.
    pub fn wind_down_twin(&mut self) {
        let changed = !self.active_progress.is_empty()
            || !self.by_id.is_empty()
            || !self.mirror_handles.is_empty()
            || !self.last_outcome.is_empty();
        self.active_progress.clear();
        self.by_id.clear();
        self.mirror_handles.clear();
        self.last_outcome.clear();
        if changed {
            self.seq = self.seq.wrapping_add(1);
        }
    }

    /// Most recent terminal outcome within `scope`, if any. Picks the
    /// latest by wall-clock timestamp. Returns the source key so
    /// callers can filter (e.g. "only show the projection error,
    /// not the parse outcome").
    pub fn last_outcome(&self, scope: BusyScope) -> Option<(&BusyOutcome, &'static str)> {
        self.last_outcome
            .iter()
            .filter(|((s, _), _)| s.is_within(scope))
            .max_by_key(|(_, (_, at))| *at)
            .map(|((_, src), (out, _))| (out, *src))
    }

    /// Derive a [`LifecycleState`] for a panel scoped to `scope`,
    /// given the panel's own "has content" predicate. Single
    /// derivation path — replaces hand-coded ORs of `loading`,
    /// `parse_pending`, `projecting`, etc.
    ///
    /// **Content priority.** When the panel already has content,
    /// the state is always [`LifecycleState::Content`] — even if
    /// work is in flight for `scope`. This means:
    /// - The loading card is never painted over already-loaded
    ///   content (no 1-frame "indicator covers scene" flash on
    ///   projection completion).
    /// - Background refresh (e.g. AST reparse while a model is
    ///   visible) doesn't cover the canvas.
    /// - Initial loads — where no content exists yet — still show
    ///   `Loading` until the first paint.
    ///
    /// Panels that want a discreet "refreshing" affordance can
    /// read `is_busy(scope) && has_content` separately; it isn't
    /// baked into the lifecycle state machine.
    pub fn lifecycle(&self, scope: BusyScope, has_content: bool) -> LifecycleState {
        if has_content {
            return LifecycleState::Content;
        }
        if self.is_busy(scope) {
            return LifecycleState::Loading;
        }
        match self.last_outcome(scope) {
            Some((BusyOutcome::Failed(msg), _)) => LifecycleState::Failed(msg.clone()),
            _ => LifecycleState::Empty,
        }
    }

    /// `true` if any active progress entry's scope is within `scope`.
    /// Walks the parent-of relation — `is_busy(BusyScope::Global)` is
    /// `true` whenever anything is busy.
    pub fn is_busy(&self, scope: BusyScope) -> bool {
        self.active_progress.keys().any(|(s, _)| s.is_within(scope))
    }

    /// Iterator over active entries whose scope is within `scope`.
    pub fn entries_in(&self, scope: BusyScope) -> impl Iterator<Item = &StatusEvent> {
        self.active_progress
            .iter()
            .filter_map(move |((s, _), ev)| s.is_within(scope).then_some(ev))
    }

    /// Longest-running active entry within `scope`, useful for the
    /// "show one indicator" rendering case.
    pub fn longest_in(&self, scope: BusyScope) -> Option<&StatusEvent> {
        self.entries_in(scope).min_by_key(|ev| ev.at)
    }

    /// Iterator over the discrete history, oldest first.
    /// Double-ended so callers can render newest-first via `.rev()`.
    pub fn history(&self) -> std::collections::vec_deque::Iter<'_, StatusEvent> {
        self.history.iter()
    }

    /// Total discrete events ever pushed to history (a monotonic
    /// watermark that, unlike `history().count()`, never plateaus when
    /// the ring is at capacity). Consumers that forward "what's new
    /// since last seen" should diff against this, not the live length.
    pub fn history_total(&self) -> u64 {
        self.history_total
    }

    /// Iterator over the active progress events. Order is unspecified;
    /// callers that show only one are expected to pick by recency.
    pub fn active_progress(&self) -> impl Iterator<Item = &StatusEvent> {
        self.active_progress.values()
    }

    /// Event the status bar should show in its single-line strip.
    /// When work is in flight, picks the *longest-running* active
    /// entry rather than the most recently started — the latter
    /// flickered as concurrent drill-in / duplicate / projection
    /// tasks rotated through `at`. A newer actionable discrete event
    /// (warning, error, or attention) takes precedence so a failure
    /// cannot be hidden behind an older long-running operation. The
    /// longest-running entry stays pinned until it completes when no
    /// newer actionable event exists. Falls back to the most recent
    /// discrete history event when no progress is active.
    pub fn display_latest(&self) -> Option<&StatusEvent> {
        let progress = self.active_progress.values().min_by_key(|e| e.at);
        let latest = self.history.back();
        match (progress, latest) {
            (Some(progress), Some(latest))
                if latest.at >= progress.at
                    && matches!(
                        latest.level,
                        StatusLevel::Warn | StatusLevel::Error | StatusLevel::Attention
                    ) =>
            {
                Some(latest)
            }
            (Some(progress), _) => Some(progress),
            (None, latest) => latest,
        }
    }

    /// Sequence number bumped on each push. Renderers use this for
    /// cheap change-detection — store it in a `Local`, compare next
    /// frame, skip work if unchanged.
    pub fn seq(&self) -> u64 {
        self.seq
    }
}

/// Dev-only knob that delays the cleanup of busy entries — every
/// `BusyHandle::Drop` lands in the channel as usual, but
/// [`drain_busy_drops`] holds the entry alive in `active_progress`
/// until `entry.at + min_duration` has passed. Set to a nonzero
/// `Duration` to exercise indicator render paths that fast tasks
/// would otherwise skip (the [`SHOW_AFTER`] / [`ELAPSED_AFTER`]
/// thresholds, the elapsed-time read-out, the cancel button).
///
/// Default is zero — production runs are unaffected.
///
/// [`SHOW_AFTER`]: crate::status_bus::StatusBus
/// [`ELAPSED_AFTER`]: crate::status_bus::StatusBus
#[derive(Resource, Default, Clone, Copy)]
pub struct BusyDebug {
    /// Minimum wall-clock lifetime for a busy entry. An entry is held in
    /// `active_progress` until `entry.at + min_duration` even after its handle
    /// drops. Zero (the default) disables the delay entirely.
    pub min_duration: std::time::Duration,
}

/// Drains [`BusyHandle`] drop notifications and clears the corresponding
/// active-progress entries. Runs every frame as part of [`StatusBusPlugin`].
pub fn drain_busy_drops(mut bus: ResMut<StatusBus>, debug: Option<Res<BusyDebug>>) {
    let min = debug.map(|d| d.min_duration).unwrap_or_default();
    // Pull all pending drops out under the mutex first so we can release
    // it before mutating self via `clear_by_id`.
    let mut drops: Vec<(BusyId, BusyOutcome)> = Vec::new();
    if let Ok(rx) = bus.drop_rx.lock() {
        while let Ok(d) = rx.try_recv() {
            drops.push(d);
        }
    }
    if min.is_zero() {
        for (id, outcome) in drops {
            bus.clear_by_id(id, outcome);
        }
        return;
    }
    // Slow-path harness: only clear entries whose `started_at` is
    // older than `min`. Re-queue younger drops by sending the id
    // back into the channel so a future frame retries.
    let now = Instant::now();
    let tx = bus.drop_tx.clone();
    for (id, outcome) in drops {
        let still_young = bus
            .by_id
            .get(&id)
            .and_then(|key| bus.active_progress.get(key))
            .map(|ev| now.saturating_duration_since(ev.at) < min)
            .unwrap_or(false);
        if still_young {
            let _ = tx.send((id, outcome));
        } else {
            bus.clear_by_id(id, outcome);
        }
    }
}

/// Drop a closed document's terminal-outcome cache so `last_outcome`
/// doesn't accumulate dead entries across long sessions. One shared
/// observer here instead of per-domain copies: every document type
/// (Modelica, script, USD) closes through the same `CloseDocument`
/// command, and the bus is the workbench's own resource.
pub fn clear_outcomes_on_close_document(
    trigger: On<lunco_doc_bevy::CloseDocument>,
    mut bus: ResMut<StatusBus>,
) {
    bus.clear_outcomes_for(BusyScope::Document(trigger.event().doc.0));
}

/// Active Twin replacement is the lifetime boundary for live workbench
/// status. A non-active Twin can close without interrupting the current
/// session's progress.
pub fn wind_down_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    mut bus: ResMut<StatusBus>,
) {
    if trigger.event().was_active {
        bus.wind_down_twin();
    }
}

/// Source label for anything that arrives as a domain `TelemetryEvent`
/// rather than through a workbench-native `push`. The event's own
/// mnemonic leads the message, so the bar reads
/// `Telemetry — DEM_BUILD_FAILED: Terrain 'apollo15' …`.
pub const TELEMETRY_SOURCE: &str = "Telemetry";

/// Fan Error/Critical domain telemetry onto the status bus.
///
/// **This is the bridge every emitting crate assumes exists.** Domain
/// crates (`lunco-terrain-surface`, `lunco-assets`, `lunco-celestial`,
/// `lunco-tutorial`, `lunco-workspace`, `lunco-usd-bevy`) deliberately do
/// not depend on the workbench — they raise a `lunco_core::TelemetryEvent`
/// and document that "the status bar surfaces it". Nothing did: before
/// this observer the only `StatusLevel::Error` pushes in the workspace
/// were Modelica-specific, so every one of those failures reached the log
/// and stopped there. One observer here lights all of them up at once,
/// and keeps the dependency arrow pointing the right way — UI consumes
/// domain events, domains never learn about the UI.
///
/// Only `Error` and `Critical` are forwarded. `TelemetryEvent` is also the
/// carrier for ordinary simulation events (zone entries, script `emit`,
/// sensor faults) at `Debug`/`Info`, and those would swamp the history and
/// the bar — they remain available to the Console and API subscribers via
/// the telemetry stream itself. `Warning` is forwarded too, since
/// Diagnostics is meant to show warnings tied to a document.
pub fn surface_error_telemetry(
    trigger: On<lunco_core::TelemetryEvent>,
    mut bus: ResMut<StatusBus>,
) {
    use lunco_core::{Severity, TelemetryValue};

    let ev = trigger.event();
    let level = match ev.severity {
        Severity::Critical | Severity::Error => StatusLevel::Error,
        Severity::Warning => StatusLevel::Warn,
        // Debug/Info are the high-volume simulation path — not status-bar material.
        Severity::Debug | Severity::Info => return,
    };

    // The payload is usually the human-readable reason (emitters build it
    // with `TelemetryValue::String`). Numeric payloads still say something
    // useful, so render them rather than dropping the event.
    let detail = match &ev.data {
        TelemetryValue::String(s) if !s.is_empty() => s.clone(),
        TelemetryValue::String(_) => String::new(),
        TelemetryValue::F64(v) => v.to_string(),
        TelemetryValue::I64(v) => v.to_string(),
        TelemetryValue::Bool(v) => v.to_string(),
    };

    let message = if detail.is_empty() {
        format!("{} [source={}]", ev.name, ev.source)
    } else {
        format!("{} [source={}]: {}", ev.name, ev.source, detail)
    };

    bus.push(TELEMETRY_SOURCE, level, message);
}

/// Adds the [`StatusBus`] resource and the per-frame `drain_busy_drops`
/// system. Renderers and fan-out systems are added by their owning
/// plugins (each can opt in independently).
pub struct StatusBusPlugin;

impl Plugin for StatusBusPlugin {
    fn build(&self, app: &mut App) {
        // `PreUpdate` runs before any panel render in `Update`, so
        // handles dropped in the previous frame's render (e.g. the
        // `BusyHandle` inside a `spawn_tracked` future that just
        // completed) clear from `active_progress` before any panel
        // reads `is_busy` / `lifecycle`. Keeps the rendered bus
        // state in lock-step with the underlying work.
        app.init_resource::<StatusBus>()
            .add_systems(PreUpdate, drain_busy_drops)
            .add_observer(clear_outcomes_on_close_document)
            .add_observer(wind_down_on_twin_closed)
            .add_observer(surface_error_telemetry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bridge exists and fires. Five domain crates raise Error
    /// telemetry and document that the status bar shows it; for a long
    /// time nothing connected the two, and a silent failure is exactly
    /// what those events were added to prevent. If this test ever goes
    /// away, so does the guarantee.
    #[test]
    fn error_telemetry_reaches_the_status_bus() {
        use lunco_core::{Severity, TelemetryEvent, TelemetryValue};

        let mut app = App::new();
        app.add_plugins(StatusBusPlugin);
        app.world_mut().trigger(TelemetryEvent {
            name: "DEM_BUILD_FAILED".to_string(),
            source: 42,
            severity: Severity::Error,
            data: TelemetryValue::String(
                "/twins/apollo15/terrain: no ground was created".to_string(),
            ),
            timestamp: 0.0,
        });

        let bus = app.world().resource::<StatusBus>();
        let event = bus
            .history()
            .find(|e| e.level == StatusLevel::Error)
            .expect("Error telemetry must land on the bus");
        assert!(
            event.message.contains("DEM_BUILD_FAILED [source=42]")
                && event.message.contains("/twins/apollo15/terrain")
        );
    }

    #[test]
    fn newer_actionable_event_supersedes_older_progress() {
        let mut bus = StatusBus::default();
        bus.set_progress("scene", "loading", 1, 2);
        bus.push(
            "runtime",
            StatusLevel::Error,
            "scene failed [source=42]: missing prim /World/Terrain",
        );

        let shown = bus.display_latest().expect("status bar event");
        assert_eq!(shown.level, StatusLevel::Error);
        assert!(shown.message.contains("missing prim /World/Terrain"));
    }

    /// `TelemetryEvent` also carries ordinary simulation traffic (zone
    /// entries, script `emit`, sensor samples) at Debug/Info. Forwarding
    /// those would bury real failures in the bar and the history.
    #[test]
    fn routine_telemetry_does_not_reach_the_status_bus() {
        use lunco_core::{Severity, TelemetryEvent, TelemetryValue};

        let mut app = App::new();
        app.add_plugins(StatusBusPlugin);
        for severity in [Severity::Debug, Severity::Info] {
            app.world_mut().trigger(TelemetryEvent {
                name: "ZONE_ENTER".to_string(),
                source: 7,
                severity,
                data: TelemetryValue::I64(42),
                timestamp: 0.0,
            });
        }

        let bus = app.world().resource::<StatusBus>();
        assert_eq!(
            bus.history().count(),
            0,
            "routine telemetry must not spam the bus: {:?}",
            bus.history().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn mirrored_progress_targets_global_scope() {
        let mut bus = StatusBus::default();
        bus.set_progress("MSL", "loading", 1, 10);
        assert!(bus.is_busy(BusyScope::Global));
        assert_eq!(bus.entries_in(BusyScope::Global).count(), 1);
        bus.remove_progress("MSL");
        assert!(!bus.is_busy(BusyScope::Global));
    }

    #[test]
    fn begin_and_drop_clears_via_drain() {
        let mut bus = StatusBus::default();
        let handle = bus.begin(BusyScope::Tab(7), "drill-in", "Loading…");
        assert!(bus.is_busy(BusyScope::Tab(7)));
        assert!(bus.is_busy(BusyScope::Global));
        drop(handle);
        // Simulate a frame.
        let drops: Vec<(BusyId, BusyOutcome)> = bus.drop_rx.lock().unwrap().try_iter().collect();
        for (id, outcome) in drops {
            bus.clear_by_id(id, outcome);
        }
        assert!(!bus.is_busy(BusyScope::Tab(7)));
        assert!(!bus.is_busy(BusyScope::Global));
    }

    #[test]
    fn re_begin_evicts_prior_handle_silently() {
        let mut bus = StatusBus::default();
        let h1 = bus.begin(BusyScope::Tab(1), "drill-in", "first");
        let id1 = h1.id();
        let _h2 = bus.begin(BusyScope::Tab(1), "drill-in", "second");
        // h1's id no longer owns the slot; dropping it must not clear
        // the entry now belonging to h2.
        drop(h1);
        bus.clear_by_id(id1, BusyOutcome::Succeeded);
        assert!(bus.is_busy(BusyScope::Tab(1)));
    }

    #[test]
    fn distinct_scopes_with_same_source_dont_trample() {
        let mut bus = StatusBus::default();
        let _a = bus.begin(BusyScope::Tab(1), "drill-in", "tab 1");
        let _b = bus.begin(BusyScope::Tab(2), "drill-in", "tab 2");
        assert_eq!(bus.entries_in(BusyScope::Global).count(), 2);
        assert!(bus.is_busy(BusyScope::Tab(1)));
        assert!(bus.is_busy(BusyScope::Tab(2)));
    }

    #[test]
    fn mirrored_progress_preserves_start_time_so_display_latest_pins_oldest() {
        // The status bar's `display_latest` shows the longest-running
        // active entry. A continuously-ticking source (MSL download) must
        // stay pinned even when a later, shorter task (compile) begins —
        // updating progress must NOT reset the entry's `at` to now.
        let mut bus = StatusBus::default();
        bus.set_progress("MSL", "downloading", 1, 100);
        let msl_at = bus.display_latest().expect("msl entry").at;
        // A later task starts after MSL.
        let _compile = bus.begin(BusyScope::Document(1), "compile", "Compiling…");
        // MSL keeps ticking (every frame).
        bus.set_progress("MSL", "downloading", 50, 100);
        // The pinned entry is still MSL (oldest start), not the compile.
        let shown = bus.display_latest().expect("an entry");
        assert_eq!(shown.source, "MSL");
        assert_eq!(shown.at, msl_at, "set_progress must preserve start time");
    }

    #[test]
    fn with_progress_updates_existing_entry() {
        let mut bus = StatusBus::default();
        let h = bus.begin(BusyScope::Global, "compile", "compiling");
        bus.with_progress(&h, 3, 10);
        let ev = bus.longest_in(BusyScope::Global).expect("entry");
        assert_eq!(ev.progress, Some((3, 10)));
    }

    #[test]
    fn identical_mirrored_progress_is_idempotent() {
        let mut bus = StatusBus::default();
        bus.set_progress("terrain", "streaming", 3, 10);
        let seq = bus.seq;

        bus.set_progress("terrain", "streaming", 3, 10);
        assert_eq!(
            bus.seq, seq,
            "unchanged progress must not invalidate renderers"
        );

        bus.remove_progress("missing");
        assert_eq!(bus.seq, seq, "removing absent progress must be a no-op");
    }

    #[test]
    fn active_twin_close_winds_down_live_status_without_erasing_history() {
        let mut app = App::new();
        app.add_plugins(StatusBusPlugin);
        {
            let mut bus = app.world_mut().resource_mut::<StatusBus>();
            bus.push("session", StatusLevel::Info, "kept audit event");
            bus.set_progress("scene", "loading", 1, 2);
        }

        app.world_mut().trigger(lunco_workspace::TwinClosed {
            twin: lunco_workspace::TwinId::new(1),
            root: std::path::PathBuf::from("/outgoing"),
            was_active: true,
        });

        let bus = app.world().resource::<StatusBus>();
        assert!(!bus.is_busy(BusyScope::Global));
        assert!(bus.last_outcome(BusyScope::Global).is_none());
        assert_eq!(bus.history().count(), 1);
    }
}
