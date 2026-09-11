//! Tool-call vocabulary shared between task/program emitters and tool handlers.
//!
//! ## Why these types live here
//!
//! [`ToolFired`] is produced by a generic task/program action and consumed by
//! arbitrary tool-handler crates (`lunco-avatar`'s `take_photo`, a future
//! `lunco-science`, …). Forcing every handler crate to depend on the producer
//! just to read the event would
//! invert the dependency: instruments would depend on the driver. Keeping the
//! *vocabulary* (this module) in `lunco-core` — which every crate already
//! depends on — breaks that cycle. The *registry* of handlers (the mechanism)
//! lives in `lunco-tools`; this module is deliberately just data + the event.
//!
//! See `docs/architecture/12-api.md` §"When NOT to use Command" —
//! [`ToolFired`] is a notification, not a user intent, so it is a hand-rolled
//! `Event`, not a `#[Command]`.

use bevy::prelude::*;

/// A single tool invocation queued by a task/program action. The
/// `tool` names the action (convention `family::verb`, e.g.
/// `"science::take_photo"`); `args` is an opaque payload (typically JSON) the
/// tool's handler interprets — the core stays tool-agnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolInvocation {
    /// Tool name (convention `family::verb`, e.g. `"science::take_photo"`).
    pub tool: String,
    /// Opaque args string forwarded verbatim to the tool's handler.
    pub args: String,
}

/// Notification that a task/program tool action fired. Emitted by the owning
/// runtime after it drains the per-tick tool-call queue. Tool-handler crates
/// observe this to run the named tool.
///
/// Read via an observer (`fn on_tool_fired(_: On<ToolFired>)`) or — when the
/// `lunco-tools-bevy` dispatch plugin is installed — its observer fans each
/// fired event out to the registered handler's `execute()`.
#[derive(Event, Clone, Debug)]
pub struct ToolFired {
    /// Entity whose task/program fired the tool.
    pub vessel: Entity,
    /// The vessel's [`GlobalEntityId`] (the api_id rhai/HTTP clients address it
    /// by) — the value a handler passes in a command's Entity field so the
    /// reflect-dispatch resolver maps it back to `vessel`. `0` when the vessel
    /// has no registered gid (a handler should treat that as "no vessel").
    pub vessel_gid: u64,
    /// Tool name (matches [`ToolInvocation::tool`]).
    pub tool: String,
    /// Opaque args (matches [`ToolInvocation::args`]).
    pub args: String,
}
