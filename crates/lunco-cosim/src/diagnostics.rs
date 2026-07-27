//! Co-sim connection diagnostics — the machine-readable form of the dangling-wire
//! log lines that [`crate::systems::propagate::propagate_connections`] emits.
//!
//! The interaction report asked for `GET /api/diagnostics` so a caller can *poll*
//! unresolved connections instead of scraping the terminal. The log line and this
//! resource are the same fact in two forms; this one is refreshed every
//! propagation tick so a poller always sees the current fabric, not a stale
//! snapshot.
//!
//! It reports exactly what the propagation step already knows and no more — the
//! target that didn't take a write, and whether that target exposes any port
//! surface at all:
//!
//! * `has_port_surface = true` — the entity has ports but not *this* one: a
//!   genuine fault (typo'd or stale wire, or a model that never published the
//!   output). This is the actionable case.
//! * `has_port_surface = false` — the entity exposes no ports yet: a structural
//!   endpoint (folded into `WheelParams` at parse) or a model still loading.
//!   Expected transiently; informative, not a fault.
//!
//! It does NOT invent a finer "pending vs structural vs type-mismatch"
//! classification the substrate can't yet vouch for — that needs the typed
//! causality/unit metadata a later stage adds. Reporting only what is known keeps
//! the endpoint truthful, the property the report's "queued ≠ succeeded" critique
//! is about.

use bevy::prelude::*;
use lunco_core::GlobalEntityId;

/// One connection target that did not accept its write on the last propagation
/// tick. Rebuilt every tick by [`crate::systems::propagate::propagate_connections`].
#[derive(Debug, Clone)]
pub struct BrokenConnection {
    /// The target entity whose input port could not be written.
    pub entity: Entity,
    /// Its stable id, when assigned — the identity an API caller can address.
    pub global_id: Option<GlobalEntityId>,
    /// The input port name the wire targeted.
    pub port: String,
    /// Whether the entity exposes any port surface. `true` = genuine fault (has
    /// ports, not this one); `false` = structural/still-loading endpoint.
    pub has_port_surface: bool,
    /// The accumulated value that was dropped (what the source(s) resolved to).
    pub dropped_value: f64,
}

/// The live set of unresolved connection targets, refreshed every propagation
/// tick. Empty when every wire resolves. Read by the API's `GetBrokenConnections`
/// query (registered in `lunco-usd-sim`, which sees both this crate and the API).
///
/// **Two questions, two fields.** A poller asks *"what is broken right now"* and
/// wants [`broken`](Self::broken), which clears itself when a wire resolves. A
/// gate asks *"did anything ever fail to land"* and cannot use that: propagation
/// is CHANGE-DRIVEN, so a wire that dropped its write at load is not re-attempted
/// on a quiet tick and the live set reads empty a second later. A scene test that
/// sampled `broken` at verdict time therefore passed a run whose rover was never
/// actuated — the failure had happened, been reported, and been overwritten.
///
/// [`faults`](Self::faults) is the record of what happened, so the answer does
/// not depend on when it is asked.
#[derive(Resource, Debug, Default)]
pub struct CosimDiagnostics {
    /// Targets that dropped their write on the most recent tick.
    pub broken: Vec<BrokenConnection>,
    /// Wires that have NEVER successfully written, keyed by `(entity, port)` so a
    /// wire that drops on a thousand ticks is one entry.
    ///
    /// Only `has_port_surface` targets are recorded: an endpoint that exposes no
    /// ports at all is a structural or still-loading target, and admitting those
    /// would make this a load-order report instead of a fault log — the same
    /// distinction the `warn!`/`debug!` split at the write site already draws.
    ///
    /// **A wire that later lands RETRACTS its entry**, and once landed it can
    /// never be re-reported (see [`landed`](Self::landed)). Dropping a write
    /// before the endpoint is ready is not an authoring error, it is load order:
    /// a joint's `angle` port exists only once avian has admitted both its bodies
    /// into the island graph, which is a documented multi-frame window every
    /// jointed mechanism passes through. Recording those permanently made every
    /// antenna in the project look broken while `rocker_bogie`'s own scenario
    /// measured the joint working.
    ///
    /// What survives is the wire that never landed at all — the Modelica drive
    /// law writing a port no rover declares, the antenna joint that never
    /// attaches. That is the authoring error, and it is what a gate must fail on.
    ///
    /// Two more entry sources share this ledger:
    /// * `SetPorts` writes to a name the target's port surface doesn't declare
    ///   (M12) — same `(entity, port)` key and landed-retraction rules as wires.
    /// * Algebraic loops in the wiring (M6), keyed under a synthetic port name
    ///   prefixed [`crate::systems::propagate::ALGEBRAIC_LOOP_PORT`]; these are
    ///   facts about the current fabric, retracted and re-derived on rewire.
    pub faults: std::collections::HashMap<(Entity, String), BrokenConnection>,
    /// `(entity, port)` pairs proven wired by at least one successful write.
    ///
    /// Needed because propagation is CHANGE-DRIVEN and cannot be re-asked: a
    /// quiet tick writes nothing, so "is this wire fine *now*" has no answer.
    /// "Has this wire ever carried a value" does, and it only ever ratchets one
    /// way, which is what makes the gate order-independent.
    pub landed: std::collections::HashSet<(Entity, String)>,
}
