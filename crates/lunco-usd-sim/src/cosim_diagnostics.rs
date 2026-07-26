//! `GetBrokenConnections` API query — the read side of [`lunco_cosim::CosimDiagnostics`].
//!
//! Lives here, not in `lunco-cosim` or `lunco-api`, because this is the crate that
//! already sees BOTH: `lunco-api` (the query trait + registry) and `lunco-cosim`
//! (the diagnostics resource). Keeping the provider here means the simulation
//! master never has to depend on the transport surface, and the transport never
//! has to depend on the cosim engine — the same layering the port registry uses.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::schema::ApiResponse;
use lunco_cosim::CosimDiagnostics;

/// `GetBrokenConnections` — backs `GET /api/diagnostics`. Reports the co-sim
/// connection targets that dropped their write on the most recent propagation
/// tick, so a caller can poll wiring health instead of scraping the log.
///
/// Truthful by construction: it reports exactly the targets propagation could not
/// write and whether each exposes a port surface (`fault: true` = has ports but
/// not this one, the actionable case; `fault: false` = structural/still-loading
/// endpoint). When the cosim engine isn't installed it degrades to
/// `cosim_tracked: false` rather than a hopeful empty "all clear".
///
/// params: none · returns:
/// `{ cosim_tracked, broken_count, fault_count, broken: [{port, entity_bits, global_id, has_port_surface, fault, dropped_value}] }`
pub struct BrokenConnectionsProvider;

impl ApiQueryProvider for BrokenConnectionsProvider {
    fn name(&self) -> &'static str {
        "GetBrokenConnections"
    }

    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> ApiResponse {
        let diag = world.get_resource::<CosimDiagnostics>();
        let broken: Vec<serde_json::Value> = diag
            .map(|d| {
                d.broken
                    .iter()
                    .map(|b| {
                        serde_json::json!({
                            "port": b.port,
                            "entity_bits": b.entity.to_bits(),
                            "global_id": b.global_id.map(|g| g.get()),
                            "has_port_surface": b.has_port_surface,
                            // A genuine wiring fault (declare/fix the wire) vs a
                            // structural or still-loading endpoint (expected).
                            "fault": b.has_port_surface,
                            "dropped_value": b.dropped_value,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let fault_count = broken
            .iter()
            .filter(|b| b["fault"].as_bool().unwrap_or(false))
            .count();
        ApiResponse::ok(serde_json::json!({
            "cosim_tracked": diag.is_some(),
            "broken_count": broken.len(),
            "fault_count": fault_count,
            "broken": broken,
        }))
    }
}

/// Registers [`BrokenConnectionsProvider`]. Idempotent: `init_resource` no-ops if
/// the registry already exists (it's owned by `ApiQueryRegistryPlugin`).
pub fn register(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(BrokenConnectionsProvider);
}
