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
use serde::{Deserialize, Serialize};
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

/// Replication policy for a command type. Set at registration time
/// (see `lunco-api::register_command`); the dispatcher reads it to
/// decide whether to forward locally only, ship to the server, or
/// fan out best-effort.
///
/// Single-user runs treat all three identically (everything is
/// applied locally). The network layer (future) consults this to
/// route correctly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Replication {
    /// Apply on the originating process only — camera, view toggles,
    /// editor focus, debug overlays. Never crosses the wire.
    Local,
    /// Server-of-record edits. Client applies optimistically, server
    /// reconciles + acks. Examples: `AddComponent`, `SetPlacement`,
    /// USD prim edits. Stale `parent_gen` is rejected.
    Authoritative,
    /// Real-time best-effort. Latest-wins on the receiver, no `Ack`
    /// expected. Examples: rover throttle, manual joystick input,
    /// camera-follow targets in shared scenes.
    Ephemeral,
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
