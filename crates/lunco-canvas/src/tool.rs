//! Interaction trait — what turns mouse/keyboard events into
//! scene mutations.
//!
//! Exactly one tool is **active** at a time; the canvas dispatches
//! each [`InputEvent`](crate::event::InputEvent) to the active tool,
//! which decides whether to consume it (start a drag, begin an edge
//! connection) or let built-in navigation handle it (pan, zoom).
//!
//! # Shipping today: only [`DefaultTool`]
//!
//! B1 ships the trait and one implementation that covers the Modelica
//! use case — select, drag, connect, delete, rubber-band. Custom
//! tools (annotation brush, schematic autoroute, typed-port-validate)
//! become additional impls when the real requirement lands; no
//! change to the canvas is needed.
//!
//! # `CanvasOps` façade
//!
//! Tools don't get a `&mut Canvas`. They get a narrow `CanvasOps`
//! facade that lets them mutate the scene / selection / viewport
//! and emit [`SceneEvent`](crate::event::SceneEvent)s, but not reach
//! for layers/overlays/other tools. Makes tool impls independently
//! testable and protects the canvas from tool-side invariant breaks.
//!
//! # Outcome
//!
//! `handle` returns [`ToolOutcome`] — whether the event was
//! *consumed* (built-in navigation should skip it) or *passed
//! through* (navigation handles it). This is how tool authors let
//! the canvas deal with "boring" events without reimplementing
//! pan/zoom themselves.

use crate::event::{InputEvent, SceneEvent};
use crate::scene::Scene;
use crate::selection::Selection;
use crate::viewport::Viewport;

/// Result of a tool handling one event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutcome {
    /// Tool consumed the event — built-in navigation should skip it.
    Consumed,
    /// Tool didn't care — navigation (pan/zoom/etc.) handles it.
    Passthrough,
}

/// Narrow mutable façade passed to tools. Deliberately does not
/// expose layers/overlays/other-tools — a tool's job is to mutate
/// authored state, emit events, and optionally change selection; it
/// has no business touching render plugins.
pub struct CanvasOps<'a> {
    pub scene: &'a mut Scene,
    pub selection: &'a mut Selection,
    pub viewport: &'a mut Viewport,
    pub events: &'a mut Vec<SceneEvent>,
}

/// What drives the canvas's interactive behaviour. See module docs.
pub trait Tool: Send + Sync {
    /// Called once per input event while this tool is active.
    fn handle(&mut self, event: &InputEvent, ops: &mut CanvasOps) -> ToolOutcome;

    /// Per-frame hook (even when no input). Default: noop. Used by
    /// tools with in-flight state — e.g. a rubber-band that needs
    /// the active pointer position to draw its preview, or an
    /// autoroute tool that animates a path preview.
    fn tick(&mut self, _ops: &mut CanvasOps, _dt: f32) {}

    /// Name used in debug output and (eventually) toolbar buttons.
    fn name(&self) -> &'static str;
}

/// The one default tool shipped with the crate. A placeholder in B1
/// — the real implementation (drag, connect, select, rubber-band)
/// lands in B2 wired to `input.rs`. Kept here so the *slot* is in
/// place and the `Tool` trait has at least one caller from day one.
pub struct DefaultTool;

impl Tool for DefaultTool {
    fn handle(&mut self, _event: &InputEvent, _ops: &mut CanvasOps) -> ToolOutcome {
        ToolOutcome::Passthrough
    }
    fn name(&self) -> &'static str {
        "default"
    }
}
