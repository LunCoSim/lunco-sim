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
//! - **Progress** events ([`StatusBus::push_progress`]) replace the
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

use bevy::prelude::*;
use web_time::Instant;

/// Maximum number of *discrete* events kept in `history`. Progress
/// events don't count against this — they're stored separately.
pub const STATUS_HISTORY_CAPACITY: usize = 200;

/// Severity / classification of a status event. Drives:
/// - Status bar dot colour.
/// - Console log level.
/// - Diagnostics inclusion (Error / Warn surface there; Info / Progress don't).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusLevel {
    Info,
    Warn,
    Error,
    /// In-flight progress tick. Replaces the last `Progress` from the
    /// same source instead of appending. Use [`StatusBus::push_progress`].
    Progress,
}

/// One status event. Carries everything the renderers need without
/// extra resource look-ups.
#[derive(Debug, Clone)]
pub struct StatusEvent {
    /// Short subsystem identifier shown to the user (`"MSL"`, `"Compile"`).
    pub source: &'static str,
    pub level: StatusLevel,
    pub message: String,
    /// `(done, total)` for progress events; `None` for discrete events.
    /// `total == 0` means indeterminate progress (show shimmer / spinner).
    pub progress: Option<(u64, u64)>,
    pub at: Instant,
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

/// Workbench-wide status bus. Insert via [`StatusBusPlugin`].
#[derive(Resource, Default)]
pub struct StatusBus {
    /// Append-only history of *discrete* events, capped at
    /// [`STATUS_HISTORY_CAPACITY`]. Older entries fall off the front.
    history: VecDeque<StatusEvent>,
    /// Latest in-flight progress per `source`. Replaced on every
    /// `push_progress` from the same source; cleared by `clear_progress`.
    active_progress: HashMap<&'static str, StatusEvent>,
    /// Bumped on every push (discrete or progress). Renderers cache the
    /// last seq they saw to skip work when the bus hasn't changed.
    seq: u64,
}

impl StatusBus {
    /// Append a discrete event to history.
    pub fn push(
        &mut self,
        source: &'static str,
        level: StatusLevel,
        message: impl Into<String>,
    ) {
        debug_assert!(
            level != StatusLevel::Progress,
            "use push_progress for Progress events"
        );
        let ev = StatusEvent {
            source,
            level,
            message: message.into(),
            progress: None,
            at: Instant::now(),
        };
        if self.history.len() >= STATUS_HISTORY_CAPACITY {
            self.history.pop_front();
        }
        self.history.push_back(ev);
        self.seq = self.seq.wrapping_add(1);
    }

    /// Update / install the active progress tick for `source`. Does
    /// not append to `history` — call `push(..., Info, ...)` separately
    /// to mark phase transitions you want preserved.
    pub fn push_progress(
        &mut self,
        source: &'static str,
        message: impl Into<String>,
        done: u64,
        total: u64,
    ) {
        let ev = StatusEvent {
            source,
            level: StatusLevel::Progress,
            message: message.into(),
            progress: Some((done, total)),
            at: Instant::now(),
        };
        self.active_progress.insert(source, ev);
        self.seq = self.seq.wrapping_add(1);
    }

    /// Drop the active progress tick for `source` (e.g. when the
    /// task it tracked completes). The latest discrete event for that
    /// source remains in history.
    pub fn clear_progress(&mut self, source: &'static str) {
        if self.active_progress.remove(source).is_some() {
            self.seq = self.seq.wrapping_add(1);
        }
    }

    /// Iterator over the discrete history, oldest first.
    /// Double-ended so callers can render newest-first via `.rev()`.
    pub fn history(&self) -> std::collections::vec_deque::Iter<'_, StatusEvent> {
        self.history.iter()
    }

    /// Iterator over the active progress events. Order is unspecified;
    /// callers that show only one are expected to pick by recency.
    pub fn active_progress(&self) -> impl Iterator<Item = &StatusEvent> {
        self.active_progress.values()
    }

    /// Latest event to *display* — the most recent active progress
    /// entry if any, else the most recent discrete history entry.
    /// What renderers should show in a single-line status strip.
    pub fn display_latest(&self) -> Option<&StatusEvent> {
        self.active_progress
            .values()
            .max_by_key(|e| e.at)
            .or_else(|| self.history.back())
    }

    /// Sequence number bumped on each push. Renderers use this for
    /// cheap change-detection — store it in a `Local`, compare next
    /// frame, skip work if unchanged.
    pub fn seq(&self) -> u64 {
        self.seq
    }
}

/// Adds the [`StatusBus`] resource. Renderers and fan-out systems are
/// added by their owning plugins (each can opt in independently).
pub struct StatusBusPlugin;

impl Plugin for StatusBusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StatusBus>();
    }
}
