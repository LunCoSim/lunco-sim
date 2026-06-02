//! Connection propagation system.
//!
//! Copies output values to input values through [`crate::SimConnection`]s.
//! This is the core of the co-simulation master algorithm — follows the
//! FMI pattern of "read outputs, write inputs."

use bevy::prelude::*;

use crate::{AvianSim, SimComponent, SimConnection};

/// System sets for co-simulation propagation.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CosimSet {
    /// Propagate connections: read outputs → write inputs.
    Propagate,
}

/// Propagates values through all [`crate::SimConnection`]s.
pub fn propagate_connections(
    q_connections: Query<&SimConnection>,
    mut set: ParamSet<(
        Query<&SimComponent>,
        Query<&AvianSim>,
        Query<&mut SimComponent>,
        Query<&mut AvianSim>,
    )>,
    // Reused across ticks — each FixedUpdate would otherwise allocate a
    // fresh Vec plus a fresh String per connection. Cleared before use.
    mut writes: Local<Vec<(Entity, String, f64)>>,
) {
    if q_connections.is_empty() {
        return;
    }

    // Reset all SimComponent inputs to 0 before propagation.
    // Connection writes use `+=` to accumulate multiple sources (e.g. two force
    // connections), but without this reset the values would grow unboundedly.
    // AvianSim inputs are cleared by take_inputs() in apply_sim_forces instead.
    for mut comp in set.p2().iter_mut() {
        for val in comp.inputs.values_mut() {
            *val = 0.0;
        }
    }

    writes.clear();

    // First pass: read outputs from source ports
    for conn in &q_connections {
        if conn.start_element == Entity::PLACEHOLDER || conn.end_element == Entity::PLACEHOLDER {
            continue;
        }

        // No special cases: the master algorithm is a pure output→input copy.
        // Environmental sources (e.g. gravity) are populated as ordinary
        // SimComponent outputs by domain crates BEFORE this runs — see
        // `GRAVITY_SOURCE_CONNECTOR` / `lunco-environment`'s gravity bridge —
        // so they flow through the same path with no hardcoded constants here.

        // Check BOTH SimComponent and AvianSim — an entity might have both.
        let value = set.p0().get(conn.start_element)
            .ok()
            .and_then(|c| c.outputs.get(&conn.start_connector).copied())
            .or_else(|| {
                set.p1().get(conn.start_element)
                    .ok()
                    .and_then(|a| a.outputs.get(&conn.start_connector).copied())
            });

        if let Some(val) = value {
            writes.push((conn.end_element, conn.end_connector.clone(), val * conn.scale));
        }
    }

    // Second pass: write to target ports.
    for (end_element, end_connector, value) in writes.drain(..) {
        if let Ok(mut comp) = set.p2().get_mut(end_element) {
            let entry = comp.inputs.entry(end_connector.clone()).or_insert(0.0);
            *entry += value;
        }

        if let Ok(mut avian) = set.p3().get_mut(end_element) {
            let entry = avian.inputs.entry(end_connector).or_insert(0.0);
            *entry += value;
        }
    }
}
