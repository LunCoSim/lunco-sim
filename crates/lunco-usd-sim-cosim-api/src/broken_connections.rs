//! `GetBrokenConnections` API query — the read side of co-simulation and
//! scene-scoped runtime diagnostics.
//!
//! This optional package depends on both the API registry and the co-simulation
//! diagnostics resource; the runtime remains independent from the transport
//! surface.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryResult};
use lunco_api_core::{ApiValue, IntoApiValue, api_value};
use lunco_cosim_core::CosimDiagnostics;

/// JSON cannot carry an IEEE non-finite scalar. Preserve the rejected numeric
/// class at this API boundary, while the simulation diagnostic retains its f64.
enum RejectedScalar {
    Finite(f64),
    NotANumber,
    PositiveInfinity,
    NegativeInfinity,
}

impl From<f64> for RejectedScalar {
    fn from(value: f64) -> Self {
        if value.is_nan() {
            Self::NotANumber
        } else if value == f64::INFINITY {
            Self::PositiveInfinity
        } else if value == f64::NEG_INFINITY {
            Self::NegativeInfinity
        } else {
            Self::Finite(value)
        }
    }
}

impl IntoApiValue for RejectedScalar {
    fn into_api_value(self) -> ApiValue {
        let (kind, value) = match self {
            Self::Finite(value) => ("finite", Some(value)),
            Self::NotANumber => ("not_a_number", None),
            Self::PositiveInfinity => ("positive_infinity", None),
            Self::NegativeInfinity => ("negative_infinity", None),
        };
        api_value!({ "kind": kind, "value": value })
    }
}

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
pub(crate) struct BrokenConnectionsProvider;

impl ApiQueryProvider for BrokenConnectionsProvider {
    fn name(&self) -> &'static str {
        "GetBrokenConnections"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let diag = world.get_resource::<CosimDiagnostics>();
        let runtime_fault = world
            .get_resource::<lunco_core::RuntimeFaults>()
            .and_then(|faults| faults.first.as_ref())
            .map(|fault| {
                api_value!({
                    "kind": fault.kind,
                    "entity_bits": fault.entity.map(|entity| entity.to_bits()),
                    "subject": fault.subject.clone(),
                    "detail": fault.detail.clone(),
                })
            });
        let encode = |items: &[lunco_cosim_core::BrokenConnection]| {
            items
                .iter()
                .map(|b| {
                    api_value!({
                        "port": b.port.as_ref(),
                        "entity_bits": b.entity.to_bits(),
                        "global_id": b.global_id.map(|g| g.get()),
                        "has_port_surface": b.has_port_surface,
                        "dropped_value": b.dropped_value.map(RejectedScalar::from),
                        "failure": b.failure.as_deref(),
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
                        api_value!({
                            "entity_bits": loop_diag.entity.to_bits(),
                            "global_id": loop_diag.global_id.map(|g| g.get()),
                            "detail": loop_diag.detail.clone(),
                            "force_producing": loop_diag.force_producing,
                            "rejected": loop_diag.rejected,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(Some(api_value!({
            "cosim_tracked": diag.is_some(),
            "broken_count": broken.len(),
            "pending_count": pending.len(),
            "algebraic_loop_count": algebraic_loops.len(),
            // Count one entry per terminal connection/runtime fault. A
            // rejected algebraic loop is exposed
            // in `algebraic_loops` and its terminal effect in `runtime_fault`;
            // counting both would report the same failure twice.
            "fault_count": broken.len() + usize::from(runtime_fault.is_some()),
            "broken": broken,
            "pending": pending,
            "algebraic_loops": algebraic_loops,
            "runtime_fault": runtime_fault,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_scalar_encoding_is_transport_safe_and_preserves_numeric_class() {
        for (input, kind) in [
            (-0.0, "finite"),
            (f64::MAX, "finite"),
            (f64::NAN, "not_a_number"),
            (f64::INFINITY, "positive_infinity"),
            (f64::NEG_INFINITY, "negative_infinity"),
        ] {
            let encoded = RejectedScalar::from(input).into_api_value();
            lunco_api_core::validate_reflection_value(&encoded).unwrap();
            assert_eq!(encoded.get("kind").and_then(ApiValue::as_str), Some(kind));
            if input.is_finite() {
                assert_eq!(
                    encoded
                        .get("value")
                        .and_then(ApiValue::as_f64)
                        .unwrap()
                        .to_bits(),
                    input.to_bits()
                );
            } else {
                assert_eq!(encoded.get("value"), Some(&ApiValue::Unit));
            }
        }
    }
}
