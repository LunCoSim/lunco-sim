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
#[derive(Resource, Debug, Default)]
pub struct CosimDiagnostics {
    /// Targets that dropped their write on the most recent tick.
    pub broken: Vec<BrokenConnection>,
}
