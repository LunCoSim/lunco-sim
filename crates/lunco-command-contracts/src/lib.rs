//! Pure mutation and command-envelope contracts shared by documents,
//! transports, networking, and runtime adapters.
//
//! This crate deliberately contains no ECS or Bevy runtime. It owns the
//! serializable envelope and result types; command reflection and ECS result
//! storage remain in the runtime core. Acknowledgement data uses the shared
//! typed `HookValue` ABI rather than a transport-specific JSON value.

use lunco_id::make_id_53;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Identity of a single command invocation. Same 53-bit time-sorted
/// shape as the stable entity identity, minted from the same generator,
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
/// locally; `NetworkRole::Standalone` is authoritative). The network layer
/// (future) consults this to route correctly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SyncChannel {
    /// In-process only; never serialized. Camera, view toggles, editor
    /// focus/selection, debug overlays. Not on any bus.
    Local,
    /// **Command Bus** — reliable, ordered, ack'd (XTCE Telecommand /
    /// ROS Service / F′ Command). Client applies optimistically, server
    /// reconciles + acks; stale `parent_gen` is rejected. Examples:
    /// `AcquireControl`, `AddComponent`, `SetPlacement`, USD prim edits,
    /// spawn. Possession/authority arbitration rides here (the ontology's
    /// `AcquireStream` pattern).
    CommandBus,
    /// **ControlStream** — best-effort, latest-sample-wins, no ack, no
    /// replay (ROS 2 Topic / `cmd_vel` / F′ setpoint+rate-group). Examples:
    /// rover throttle, manual joystick input, live parameter scrubs,
    /// presence cursors. This is AGENTS.md §4.2 made declarative.
    ControlStream,
    /// **BulkData** — reliable + ordered like [`Self::CommandBus`], but a
    /// *separate* lane for large, non-latency-critical payloads: the scenario
    /// manifest and (future) content-addressed asset transfers. Kept off the
    /// CommandBus so a big manifest / multi-MB asset stream can't
    /// head-of-line-block join-critical traffic (Handshake, Ownership,
    /// possession, spawn). File-transfer analogue to the interactive command bus.
    BulkData,
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

/// Successful apply. Reports the new domain generation and optional
/// command-specific data the client needs to continue (the canonical
/// example: an `AddComponent` whose name was allocated by the document layer).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ack {
    pub op_id: OpId,
    /// New domain generation after the apply, when the receiving
    /// document has one. `None` for stateless / ephemeral commands.
    pub new_gen: Option<u64>,
    /// Optional typed command-specific result data. The command owns the shape of
    /// this value; common examples are an allocated entity id, queued status,
    /// generated source, or captured stdout. API transports convert it only at
    /// their explicit response boundary. Continuous simulation values do not
    /// belong here — they remain authored `outputs:*` ports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<lunco_hooks::HookValue>,
}

impl Ack {
    pub fn new(op_id: OpId) -> Self {
        Self {
            op_id,
            new_gen: None,
            data: None,
        }
    }

    /// Build a successful acknowledgement carrying typed command-specific result data.
    pub fn with_data(op_id: OpId, data: lunco_hooks::HookValue) -> Self {
        Self {
            op_id,
            new_gen: None,
            data: Some(data),
        }
    }
}

/// Apply failure. Carries enough information for the client to
/// decide whether to revert its optimistic mutation, retry against a
/// fresher generation, or surface a banner to the user.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Reject {
    /// Document is read-only (e.g. a source-library tab). Optimistic
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
