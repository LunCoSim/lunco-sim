//! Co-sim connection diagnostics — the machine-readable form of the dangling-wire
//! log lines that [`crate::systems::propagate::propagate_connections`] emits.
//!
//! The interaction report asked for `GET /api/diagnostics` so a caller can *poll*
//! unresolved connections instead of scraping the terminal. The log line and this
//! resource are the same fact in two forms; this one is refreshed every
//! propagation tick so a poller always sees the current fabric, not a stale
//! snapshot.
//!
//! It distinguishes work which is still waiting for an endpoint contract from a
//! terminal wiring failure. A generated Modelica island deliberately exists while
//! it is compiling; its port interface is not final until that lifecycle stage
//! completes. Treating an early partial surface as a typo made load order look
//! like an authoring fault.
//!
//! * [`CosimDiagnostics::pending`] holds structural endpoints and endpoints
//!   whose [`crate::SimComponent`] is still compiling. It is an observation, not
//!   a warning or a test failure.
//! * [`CosimDiagnostics::broken`] holds only terminal failures: a ready (or
//!   failed) endpoint that still cannot accept the named input.
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
    /// Targets waiting for their endpoint contract. Rebuilt each propagation
    /// tick; never logged as a wiring fault.
    pub pending: Vec<BrokenConnection>,
    /// Targets that dropped their write after their endpoint contract became
    /// terminal. Rebuilt each propagation tick.
    pub broken: Vec<BrokenConnection>,
    /// Wires that have NEVER successfully written, keyed by `(entity, port)` so a
    /// wire that drops on a thousand ticks is one entry.
    ///
    /// Only terminal targets are recorded. A compiling model, or an endpoint
    /// with no runtime port surface, remains pending rather than manufacturing a
    /// failure during scene assembly.
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
    /// Another entry source shares this ledger: `SetPorts` writes to a name the
    /// target's port surface doesn't declare (M12) — the same `(entity, port)`
    /// key and landed-retraction rules as wires. Causal feedback cycles are not
    /// ledger entries: they are valid explicit co-simulation topology, while
    /// acausal islands require a typed backend partition before stepping.
    pub faults: std::collections::HashMap<(Entity, String), BrokenConnection>,
    /// `(entity, port)` pairs proven wired by at least one successful write.
    ///
    /// Needed because propagation is CHANGE-DRIVEN and cannot be re-asked: a
    /// quiet tick writes nothing, so "is this wire fine *now*" has no answer.
    /// "Has this wire ever carried a value" does, and it only ever ratchets one
    /// way, which is what makes the gate order-independent.
    pub landed: std::collections::HashSet<(Entity, String)>,
}
