//! `GetBrokenConnections` API query — the read side of co-simulation and
//! scene-scoped runtime diagnostics.
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
/// `{ cosim_tracked, broken_count, pending_count, algebraic_loop_count,
///    fault_count, broken: [...], pending: [...], algebraic_loops: [...],
///    runtime_fault: ... }`
pub struct BrokenConnectionsProvider;

impl ApiQueryProvider for BrokenConnectionsProvider {
    fn name(&self) -> &'static str {
        "GetBrokenConnections"
    }

    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> ApiResponse {
        let diag = world.get_resource::<CosimDiagnostics>();
        let runtime_fault = world
            .get_resource::<lunco_core::RuntimeFaults>()
            .and_then(|faults| faults.first.as_ref())
            .map(|fault| {
                serde_json::json!({
                    "kind": fault.kind,
                    "entity_bits": fault.entity.map(|entity| entity.to_bits()),
                    "subject": fault.subject,
                    "detail": fault.detail,
                })
            });
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
        let algebraic_loops = diag
            .map(|d| {
                d.algebraic_loops
                    .iter()
                    .map(|loop_diag| {
                        serde_json::json!({
                            "entity_bits": loop_diag.entity.to_bits(),
                            "global_id": loop_diag.global_id.map(|g| g.get()),
                            "detail": loop_diag.detail,
                            "force_producing": loop_diag.force_producing,
                            "rejected": loop_diag.rejected,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        ApiResponse::ok(serde_json::json!({
            "cosim_tracked": diag.is_some(),
            "broken_count": broken.len(),
            "pending_count": pending.len(),
            "algebraic_loop_count": algebraic_loops.len(),
            // Keep the historical count semantics: one entry per terminal
            // connection/runtime fault. A rejected algebraic loop is exposed
            // in `algebraic_loops` and its terminal effect in `runtime_fault`;
            // counting both would report the same failure twice.
            "fault_count": broken.len() + usize::from(runtime_fault.is_some()),
            "broken": broken,
            "pending": pending,
            "algebraic_loops": algebraic_loops,
            "runtime_fault": runtime_fault,
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
