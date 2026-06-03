//! Command envelope — the shape every mutation flows through.
//!
//! Today, every interactive change in LunCo (drag a Modelica node,
//! steer a rover, edit USD) ends up as a Bevy `Reflect` event handled
//! by an observer. The envelope adds a small header in front of the
//! event so the same code path works whether the mutation originated
//! locally, came in over the network, or replays from a recorded
//! session.
//!
//! Single-user runs use the envelope but never actually serialise it
//! — the dispatcher generates a fresh [`OpId`] on each call, hands the
//! mutation to the observer, and discards the [`Ack`]. The networked
//! runtime (future) intercepts at the same boundary: the wire envelope
//! becomes the input to the dispatcher; [`Ack`] / [`Reject`] become
//! the response back to the originating client.
//!
//! Why types live in `lunco-core` and not `lunco-networking`: domain
//! crates (`lunco-doc`, `lunco-modelica`, `lunco-mobility`) need to
//! talk about mutations even when the networking crate isn't in the
//! build. `lunco-networking` will add transport on top, not the
//! envelope itself.
//!
//! Conventions:
//! - [`Mutation`] is the wire-shape carrier; payload is generic.
//! - [`Ack`] reports the post-apply state plus any server-assigned
//!   values (e.g. an auto-allocated Modelica instance name).
//! - [`Reject`] gives the client enough to decide whether to revert,
//!   retry, or surface to the user.

use crate::ids::make_id_53;
use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fmt;

/// Identity of a single command invocation. Same 53-bit time-sorted
/// shape as [`crate::GlobalEntityId`], minted from the same generator,
/// but newtype-distinct so events and entities can't be confused.
///
/// Used by the document layer for idempotent replay (a duplicate
/// `OpId` is silently dropped) and by the future network layer for
/// dedupe / ack correlation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpId(pub u64);

impl OpId {
    /// Mint a fresh op-id. Local dispatchers call this; the wire layer
    /// reads it from the envelope instead.
    pub fn new() -> Self {
        Self(make_id_53())
    }
}

impl Default for OpId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for OpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OpId({})", self.0)
    }
}

impl fmt::Display for OpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Identity of the originating session — local user, remote client,
/// agent, replay driver. [`SessionId::LOCAL`] is the implicit value
/// for everything that hasn't crossed a network boundary.
///
/// Networking will assign per-connection ids. Domain code only reads
/// this for attribution (edit history, conflict messages); no domain
/// behaviour branches on session.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub u64);

impl SessionId {
    /// The single-process session — every locally-originated mutation
    /// carries this until the networking layer replaces it with a
    /// per-connection id at boundary crossings.
    pub const LOCAL: SessionId = SessionId(0);
}

impl Default for SessionId {
    fn default() -> Self {
        Self::LOCAL
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SessionId({})", self.0)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Which Twin write-channel a command's networked form rides. Set at
/// registration time (see `lunco-api::declare_channel`); the dispatcher
/// reads it to decide whether to keep a command in-process, ship it on
/// the reliable Command Bus, or fan it out best-effort on a ControlStream.
///
/// These are the ontology's three write surfaces (`docs/architecture/
/// 01-ontology.md` §4) — the ROS 2 Service / Action / Topic trichotomy —
/// **not** generic netcode adjectives. The variant names ARE the channel
/// names so a domain author who's read the ontology picks the right one
/// without translation. Note this axis is orthogonal to *authority* (who
/// may issue a command against an entity): authority is a runtime gate on
/// the target, not a property of the channel — see `crates/lunco-networking/
/// AUTHORITY.md`.
///
/// Single-user runs treat all three identically (everything is applied
/// locally, `IsServer = true`). The network layer (future) consults this
/// to route correctly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WireChannel {
    /// In-process only; never serialized. Camera, view toggles, editor
    /// focus/selection, debug overlays. Not on any bus.
    Local,
    /// **Command Bus** — reliable, ordered, ack'd (XTCE Telecommand /
    /// ROS Service / F′ Command). Client applies optimistically, server
    /// reconciles + acks; stale `parent_gen` is rejected. Examples:
    /// `PossessVessel`, `AddComponent`, `SetPlacement`, USD prim edits,
    /// spawn. Possession/authority arbitration rides here (the ontology's
    /// `AcquireStream` pattern).
    CommandBus,
    /// **ControlStream** — best-effort, latest-sample-wins, no ack, no
    /// replay (ROS 2 Topic / `cmd_vel` / F′ setpoint+rate-group). Examples:
    /// rover throttle, manual joystick input, live parameter scrubs,
    /// presence cursors. This is AGENTS.md §4.2 made declarative.
    ControlStream,
}

/// Wire-shape carrier for a mutation. The payload is whatever the
/// dispatcher / observer expects — a `ModelicaOp`, a Reflect event,
/// or anything else that round-trips through serde.
///
/// In single-user mode this is effectively transparent: the
/// dispatcher mints a fresh [`OpId`], wraps the payload, hands it to
/// the observer, and drops the envelope. In network mode the
/// envelope is the wire format.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mutation<P> {
    pub id: OpId,
    pub origin: SessionId,
    /// Domain-specific generation the client based this mutation on
    /// (e.g. `Document::generation()` for modelica edits). `None` for
    /// commands that don't have a meaningful causal predecessor —
    /// rover throttle, camera moves.
    pub parent_gen: Option<u64>,
    pub payload: P,
}

/// Conversion shortcut: any payload can be wrapped in a default
/// local envelope by relying on `Into<Mutation<P>>`. This lets call
/// sites write `host.apply(op)` and have the dispatcher stamp the
/// envelope automatically — no clutter at the boundary.
///
/// Doesn't conflict with the reflexive `impl<T> From<T> for T`
/// because the target type here is `Mutation<P>`, not `P`.
impl<P> From<P> for Mutation<P> {
    fn from(payload: P) -> Self {
        Self::local(payload)
    }
}

impl<P> Mutation<P> {
    /// Build an envelope for a locally-originated mutation. Mints a
    /// fresh [`OpId`], stamps [`SessionId::LOCAL`]. Same as
    /// `payload.into()` — explicit form is preferred when the call
    /// site needs to set additional fields (`parent_gen`, etc.) or
    /// when the type inference around `Into` gets in the reader's
    /// way.
    pub fn local(payload: P) -> Self {
        Self {
            id: OpId::new(),
            origin: SessionId::LOCAL,
            parent_gen: None,
            payload,
        }
    }

    /// Build an envelope for a locally-originated mutation that
    /// expects causal ordering against a known generation.
    pub fn local_against(parent_gen: u64, payload: P) -> Self {
        Self {
            id: OpId::new(),
            origin: SessionId::LOCAL,
            parent_gen: Some(parent_gen),
            payload,
        }
    }
}

/// Successful apply. Reports the new domain generation and any
/// server-assigned values the client needs to learn about (the
/// canonical example: an `AddComponent` whose name was allocated by
/// the document layer).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ack {
    pub op_id: OpId,
    /// New domain generation after the apply, when the receiving
    /// document has one. `None` for stateless / ephemeral commands.
    pub new_gen: Option<u64>,
    /// Loose key/value bag for server-assigned outputs. Domains agree
    /// on the keys: e.g. modelica's name allocator writes
    /// `{"assigned_name": "R3"}`.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub assigned: serde_json::Value,
}

impl Ack {
    pub fn new(op_id: OpId) -> Self {
        Self {
            op_id,
            new_gen: None,
            assigned: serde_json::Value::Null,
        }
    }
}

/// Apply failure. Carries enough information for the client to
/// decide whether to revert its optimistic mutation, retry against a
/// fresher generation, or surface a banner to the user.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Reject {
    /// Document is read-only (e.g. an MSL library tab). Optimistic
    /// mutation should be reverted; UI surfaces the "Duplicate to
    /// Workspace" hint.
    ReadOnly,
    /// `parent_gen` doesn't match the current document generation —
    /// the client is behind. Either rebase and retry or, for
    /// commutative ops, retry blindly.
    StaleParent { current_gen: u64 },
    /// Op-level invariant violated (e.g. RemoveComponent of an
    /// unknown name). Carries a human-readable message.
    InvalidOp(String),
    /// `OpId` matches a recently-applied op — silently absorbed; the
    /// client can treat this as success. Surfaces as a `Reject` so
    /// the type system forces callers to acknowledge dedupe vs apply.
    Duplicate,
}

impl fmt::Display for Reject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reject::ReadOnly => write!(f, "document is read-only"),
            Reject::StaleParent { current_gen } => {
                write!(f, "stale parent generation; current is {current_gen}")
            }
            Reject::InvalidOp(msg) => write!(f, "invalid op: {msg}"),
            Reject::Duplicate => write!(f, "duplicate op id"),
        }
    }
}

impl std::error::Error for Reject {}

// ── Command outcomes (pollable results) ────────────────────────────────────
//
// A command invoked through a transport (HTTP API, MCP, future wire) gets a
// request id and, if its observer reports one, a terminal outcome the caller
// can poll for. This is the deliberately-minimal model: robotics practice
// (F′ response codes, MAVLink `COMMAND_ACK`, behaviour-tree SUCCESS/FAILURE/
// RUNNING) converges on *one result code + an in-progress state*, not XTCE's
// multi-stage ground-verification pipeline. Richer lifecycles (queued,
// progress, cancel) stay as per-domain state where they already live
// (e.g. experiments' `RunStatus`), not promoted into this substrate.
//
// Distinctions kept (and only these):
// - `Rejected` (never ran — validation/auth/dedup) vs `Failed` (ran, errored):
//   the caller reverts an optimistic edit on `Rejected`, not on `Failed`
//   (MAVLink's `DENIED` vs `FAILED`).
// - `Pending`: accepted, terminal not yet known (async/long-running). MVP
//   handlers are synchronous and never leave a result `Pending`.

/// Terminal (or in-flight) state of a command invocation, keyed by request id.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CommandOutcome {
    /// Accepted; the observer hasn't reported a terminal state yet.
    Pending,
    /// Ran successfully; carries the [`Ack`] (new generation, assigned values).
    Succeeded(Ack),
    /// Never ran — rejected before/at validation. Client should revert.
    Rejected(Reject),
    /// Ran and errored. Client should not revert.
    Failed(String),
}

/// Maximum retained outcomes; oldest are evicted FIFO. A simple cap (not a
/// wall-clock TTL) avoids `Instant`/time on wasm and keeps the store bounded.
const MAX_COMMAND_RESULTS: usize = 1024;

/// Pollable store of command outcomes, keyed by the transport's request id
/// (the `command_id` the API mints). Always-on substrate — initialised by
/// `register_core_resources` so result-reporting observers can't panic on a
/// missing resource. Transports read it via a `QueryCommandResult`-style
/// request; observers write it through the `#[on_command]`-generated wrapper.
#[derive(Resource, Default)]
pub struct CommandResults {
    map: HashMap<u64, CommandOutcome>,
    order: VecDeque<u64>,
}

impl CommandResults {
    /// Insert or overwrite an outcome, evicting the oldest entries past the cap.
    pub fn insert(&mut self, id: u64, outcome: CommandOutcome) {
        if !self.map.contains_key(&id) {
            self.order.push_back(id);
        }
        self.map.insert(id, outcome);
        while self.order.len() > MAX_COMMAND_RESULTS {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    /// Record a handler's `Result<Ack, String>` as a terminal outcome.
    /// `Ok` → [`CommandOutcome::Succeeded`], `Err` → [`CommandOutcome::Failed`]
    /// (a handler that ran and errored — not a pre-execution `Rejected`).
    pub fn record(&mut self, id: u64, result: Result<Ack, String>) {
        let outcome = match result {
            Ok(ack) => CommandOutcome::Succeeded(ack),
            Err(msg) => CommandOutcome::Failed(msg),
        };
        self.insert(id, outcome);
    }

    pub fn get(&self, id: u64) -> Option<&CommandOutcome> {
        self.map.get(&id)
    }
}

/// The request id of the command currently being dispatched, set by the
/// transport dispatcher immediately around the observer trigger so the
/// `#[on_command]` wrapper can record its outcome under the right id.
/// `None` for in-process triggers (UI `commands.trigger`) — those aren't
/// polled, so their result handlers simply don't record.
#[derive(Resource, Default)]
pub struct ActiveCommandId(Option<u64>);

impl ActiveCommandId {
    pub fn get(&self) -> Option<u64> {
        self.0
    }
    pub fn set(&mut self, id: Option<u64>) {
        self.0 = id;
    }
}
