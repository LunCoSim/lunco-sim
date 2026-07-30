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
/// connection targets after their interface lifecycle has reached a terminal
/// state, plus endpoints still waiting for their runtime contract. A caller can
/// therefore distinguish assembly progress from a wiring failure without
/// scraping logs.
///
/// `broken` contains only terminal failures. `pending` contains structural or
/// compiling endpoints, which are expected during assembly and never emit a
/// warning. When the cosim engine isn't installed it degrades to
/// `cosim_tracked: false` rather than a hopeful empty "all clear".
///
/// params: none · returns:
/// `{ cosim_tracked, broken_count, pending_count, fault_count, broken: [...], pending: [...] }`
pub struct BrokenConnectionsProvider;

impl ApiQueryProvider for BrokenConnectionsProvider {
    fn name(&self) -> &'static str {
        "GetBrokenConnections"
    }

    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> ApiResponse {
        let diag = world.get_resource::<CosimDiagnostics>();
        let encode = |items: &[lunco_cosim::BrokenConnection]| {
            items
                .iter()
                .map(|b| {
                    serde_json::json!({
                        "port": b.port,
                        "entity_bits": b.entity.to_bits(),
                        "global_id": b.global_id.map(|g| g.get()),
                        "has_port_surface": b.has_port_surface,
                        "dropped_value": b.dropped_value,
                    })
                })
                .collect::<Vec<_>>()
        };
        let broken = diag.map(|d| encode(&d.broken)).unwrap_or_default();
        let pending = diag.map(|d| encode(&d.pending)).unwrap_or_default();
        ApiResponse::ok(serde_json::json!({
            "cosim_tracked": diag.is_some(),
            "broken_count": broken.len(),
            "pending_count": pending.len(),
            "fault_count": broken.len(),
            "broken": broken,
            "pending": pending,
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
