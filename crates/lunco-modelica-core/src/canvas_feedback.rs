//! Core data for API-driven canvas feedback (focus + connection pulses).
//!
//! When an API/agent caller adds a component or connection, it enqueues a
//! request here; the canvas UI drains these queues to play a focus glow / edge
//! flash so the user notices the change. The *queue types* are plain data (no
//! egui), so the core API handlers can name them; the *resources* are inserted
//! only by the UI plugin, so a headless server simply skips the push (the
//! `get_resource_mut` returns `None`) — no queue, no leak, no UI dependency.
//!
//! The animation layers + drivers that consume these live in
//! `crate::ui::panels::canvas_diagram::pulse`.

use bevy::prelude::*;

/// Default pulse-glow duration when the API caller doesn't override it.
/// Per-call override lives on `AddModelicaComponent.animation_ms`; `0` disables
/// the highlight entirely.
pub const DEFAULT_PULSE_MS: u32 = 2000;
/// Default edge-flash duration for `ConnectComponents`; `0` disables it.
pub const DEFAULT_EDGE_FLASH_MS: u32 = 1500;

/// One pending camera focus, queued by an API caller, drained by the canvas's
/// per-frame system once the projection settles.
#[derive(Debug, Clone)]
pub struct PendingApiFocus {
    /// Document the new component lives in.
    pub doc: lunco_doc::DocumentId,
    /// Component instance name (matches `Node.origin` after projection).
    pub name: String,
    /// When the API caller queued this. Used both for batch debounce
    /// and timeout GC.
    pub queued_at: web_time::Instant,
    /// Per-call pulse glow duration (ms). `0` disables the glow for this
    /// entry. Defaults to [`DEFAULT_PULSE_MS`] when the API caller didn't
    /// supply `animation_ms`.
    pub animation_ms: u32,
}

/// FIFO queue of pending API-driven focuses. The API `AddModelicaComponent`
/// handler pushes; the canvas's `drive_pending_api_focus` system drains.
///
/// Kept as a `Vec` not a `HashMap` so order is preserved — batch debounce
/// needs to know whether the *latest* push is recent enough to coalesce.
#[derive(Resource, Default)]
pub struct PendingApiFocusQueue(pub Vec<PendingApiFocus>);

impl PendingApiFocusQueue {
    pub fn push(&mut self, focus: PendingApiFocus) {
        self.0.push(focus);
    }
}

/// Connection-add queue (mirror of [`PendingApiFocusQueue`] but for
/// `ConnectComponents`). The driver matches each entry against the scene's
/// edge list (by from/to component+port) and pushes a brief flash.
#[derive(Resource, Default)]
pub struct PendingApiConnectionQueue(pub Vec<PendingApiConnection>);

#[derive(Debug, Clone)]
pub struct PendingApiConnection {
    pub doc: lunco_doc::DocumentId,
    pub from_component: String,
    pub from_port: String,
    pub to_component: String,
    pub to_port: String,
    pub queued_at: web_time::Instant,
    /// Per-call edge-flash duration (ms). `0` = no flash. Defaults to
    /// [`DEFAULT_EDGE_FLASH_MS`] when not supplied.
    pub animation_ms: u32,
}

impl PendingApiConnectionQueue {
    pub fn push(&mut self, entry: PendingApiConnection) {
        self.0.push(entry);
    }
}
