//! Wire propagation system.
//!
//! Copies output values to input values through [`SimWire`] connections.
//! This is the core of the co-simulation master algorithm — follows the
//! FMI pattern of "read outputs, write inputs."

use bevy::prelude::*;

use crate::{AvianSim, SimComponent, SimWire};

/// System sets for co-simulation propagation.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CosimSet {
    /// Propagate wires: read outputs → write inputs.
    Propagate,
}

/// Propagates values through all [`SimWire`] connections.
pub fn propagate_wires(
    q_wires: Query<&SimWire>,
    mut set: ParamSet<(
        Query<&SimComponent>,
        Query<&AvianSim>,
        Query<&mut SimComponent>,
        Query<&mut AvianSim>,
    )>,
) {
    // Reset all SimComponent inputs to 0 before propagation.
    // Wire writes use `+=` to accumulate multiple sources (e.g. two force wires),
    // but without this reset the values would grow unboundedly across frames.
    // AvianSim inputs are cleared by take_inputs() in apply_sim_forces instead.
    for mut comp in set.p2().iter_mut() {
        for val in comp.inputs.values_mut() {
            *val = 0.0;
        }
    }

    let mut writes: Vec<(Entity, String, f64)> = Vec::new();

    // First pass: Read outputs
    for wire in &q_wires {
        if wire.start_element == Entity::PLACEHOLDER || wire.end_element == Entity::PLACEHOLDER {
            continue;
        }

        // Handle __gravity__ specially
        if wire.start_connector == "__gravity__" {
            writes.push((wire.end_element, wire.end_connector.clone(), 9.81));
            continue;
        }

        // Read output. Must check BOTH SimComponent and AvianSim because an entity
        // might have both, and we need to find which one has the connector.
        let value = set.p0().get(wire.start_element)
            .ok()
            .and_then(|c| c.outputs.get(&wire.start_connector).copied())
            .or_else(|| {
                set.p1().get(wire.start_element)
                    .ok()
                    .and_then(|a| a.outputs.get(&wire.start_connector).copied())
            });

        if let Some(val) = value {
            writes.push((wire.end_element, wire.end_connector.clone(), val * wire.scale));
        }
    }

    // Second pass: apply all writes
    for (end_element, end_connector, value) in writes {
        if let Ok(mut comp) = set.p2().get_mut(end_element) {
            let entry = comp.inputs.entry(end_connector.clone()).or_insert(0.0);
            *entry += value;
        } 
        
        // Also check AvianSim for the same entity (or different entity)
        // Note: use a separate block to allow writing to both if needed, 
        // though usually a connector is unique to one model.
        if let Ok(mut avian) = set.p3().get_mut(end_element) {
            let entry = avian.inputs.entry(end_connector).or_insert(0.0);
            *entry += value;
        }
    }
}
