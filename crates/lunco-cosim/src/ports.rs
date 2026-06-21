//! Uniform port table — the single surface for **discovering** and
//! **reading/writing** every exposed simulation value, whichever backend owns
//! it.
//!
//! A *port* is a named scalar (`f64`) on a participant entity. Modelica
//! variables ([`SimComponent`]), Avian forces/state ([`crate::AvianSim`]),
//! joint angles ([`crate::JointSim`]), and the SysML/hardware "nervous system"
//! ports ([`PhysicalPort`] / [`DigitalPort`]) all present as ports, so that
//! wires ([`crate::SimConnection`]), the API (`ListPorts` / `GetPort` /
//! `SetPort`), and the UI inspector treat them uniformly — the FMI/SSP contract.
//!
//! This module is **the** place that knows how each backend stores its values.
//! [`crate::systems::propagate::propagate_connections`] addresses every endpoint
//! through here, so a new port-bearing backend is wired into the whole fabric
//! by extending this module alone — `propagate` never changes again.
//!
//! ## Value model
//!
//! The wire currency is `f64` (continuous Real — what FMI-CS exchanges almost
//! everywhere). Typed backends convert at their own boundary: a [`DigitalPort`]
//! `i16` register reads as `f64` and saturates on write, exactly like a real
//! DAC/ADC. The connection's affine transform (`scale`/`offset`) carries the
//! gain. We deliberately do **not** model `Bool`/`Enum`/`String` ports until a
//! concrete need appears — physics co-simulation is all-Real.
//!
//! ## Canonical port names for single-value backends
//!
//! [`PhysicalPort`] and [`DigitalPort`] each hold one scalar, exposed under a
//! fixed name so wires can reference it: [`PHYSICAL_PORT_NAME`] (`"value"`) and
//! [`DIGITAL_PORT_NAME`] (`"raw"`).
//!
//! ## TODO(ports): tag-driven exposure
//!
//! The backends are still hand-enumerated below. Replace with a
//! `#[derive(SimExposed)]` + `#[port(in|out)]` reflect-harvest so any component
//! field becomes a port by tagging (the ontology's "Attribute exposed via Bevy
//! Reflection"). Keep these function signatures stable so that lands as a
//! drop-in: callers (`propagate`, the API, the inspector) are unchanged.

use bevy::prelude::*;

use lunco_core::architecture::{DigitalPort, PhysicalPort};

use crate::connection::{PortDirection, PortType};
use crate::{AvianSim, JointSim, SimComponent};

/// The fixed port name a [`PhysicalPort`] exposes (its `f32` `value`).
pub const PHYSICAL_PORT_NAME: &str = "value";
/// The fixed port name a [`DigitalPort`] exposes (its `i16` `raw_value`).
pub const DIGITAL_PORT_NAME: &str = "raw";

/// A discovered port: identity, causality, physical domain, current value.
///
/// Returned by [`entity_ports`] for listing/introspection. The `value` is a
/// snapshot read at call time; live consumers read through the maps directly.
#[derive(Debug, Clone)]
pub struct PortRef {
    /// Port name — the key in the owning component's `inputs`/`outputs` map,
    /// or the canonical name for a single-value backend.
    pub name: String,
    /// Causality (key in `outputs` → `Out`, key in `inputs` → `In`, a
    /// single-value port → `InOut`).
    pub direction: PortDirection,
    /// Physical domain (best-effort classification — see [`classify`]).
    pub port_type: PortType,
    /// Snapshot of the current value.
    pub value: f64,
}

/// Best-effort physical-domain classification from a port name.
///
/// TODO(ports): drop this heuristic once backends publish `SimPort` metadata
/// (name → `PortType`) alongside their values — read the declared type instead
/// of guessing from the name. Type is used only for UI grouping and connection
/// validation, so a wrong guess is cosmetic, never load-bearing.
fn classify(name: &str) -> PortType {
    if name.starts_with("force") || name.starts_with("torque") {
        PortType::Force
    } else if name.starts_with("position")
        || name.starts_with("velocity")
        || name == "height"
        || name == "angle"
    {
        PortType::Kinematic
    } else if name.contains("temp") || name.contains("heat") {
        PortType::Thermal
    } else {
        PortType::Signal
    }
}

#[inline]
fn push_map(
    out: &mut Vec<PortRef>,
    map: &std::collections::HashMap<String, f64>,
    dir: PortDirection,
) {
    for (name, value) in map {
        out.push(PortRef {
            name: name.clone(),
            direction: dir,
            port_type: classify(name),
            value: *value,
        });
    }
}

/// Enumerate every exposed port on `entity`, across all port-bearing backends.
///
/// The backbone of `ListPorts`. TODO(ports): replace the hand-written backend
/// set with the reflect-harvest (see module docs).
pub fn entity_ports(world: &World, entity: Entity) -> Vec<PortRef> {
    let mut out = Vec::new();
    if let Some(c) = world.get::<SimComponent>(entity) {
        push_map(&mut out, &c.outputs, PortDirection::Out);
        push_map(&mut out, &c.inputs, PortDirection::In);
    }
    if let Some(a) = world.get::<AvianSim>(entity) {
        push_map(&mut out, &a.outputs, PortDirection::Out);
        push_map(&mut out, &a.inputs, PortDirection::In);
    }
    if let Some(j) = world.get::<JointSim>(entity) {
        push_map(&mut out, &j.outputs, PortDirection::Out);
        push_map(&mut out, &j.inputs, PortDirection::In);
    }
    // Single-value SysML/hardware ports: one bidirectional scalar each.
    if let Some(p) = world.get::<PhysicalPort>(entity) {
        out.push(PortRef {
            name: PHYSICAL_PORT_NAME.to_string(),
            direction: PortDirection::InOut,
            port_type: PortType::Signal,
            value: p.value as f64,
        });
    }
    if let Some(d) = world.get::<DigitalPort>(entity) {
        out.push(PortRef {
            name: DIGITAL_PORT_NAME.to_string(),
            direction: PortDirection::InOut,
            port_type: PortType::Signal,
            value: d.raw_value as f64,
        });
    }
    out
}

/// Read the **output** named `name` on `entity` — the value a connection reads
/// from its *source*.
///
/// A connection source must be an output, so this searches `outputs` maps only
/// (plus the bidirectional single-value ports, whose one value *is* their
/// output). This is critical when a name exists as both an input and an output
/// on one entity — e.g. a balloon's `height` is a `SimComponent` *input* and an
/// `AvianSim` *output*; the wire must read the AvianSim output, not the stale
/// SimComponent input. The read side of [`crate::systems::propagate`].
pub fn read_output_port(world: &World, entity: Entity, name: &str) -> Option<f64> {
    if let Some(c) = world.get::<SimComponent>(entity) {
        if let Some(v) = c.outputs.get(name) {
            return Some(*v);
        }
    }
    if let Some(a) = world.get::<AvianSim>(entity) {
        if let Some(v) = a.outputs.get(name) {
            return Some(*v);
        }
    }
    if let Some(j) = world.get::<JointSim>(entity) {
        if let Some(v) = j.outputs.get(name) {
            return Some(*v);
        }
    }
    if name == PHYSICAL_PORT_NAME {
        if let Some(p) = world.get::<PhysicalPort>(entity) {
            return Some(p.value as f64);
        }
    }
    if name == DIGITAL_PORT_NAME {
        if let Some(d) = world.get::<DigitalPort>(entity) {
            return Some(d.raw_value as f64);
        }
    }
    None
}

/// Read the current value of port `name` on `entity`, preferring an **output**
/// across all backends, then falling back to an **input**. `None` if no such
/// port. The backbone of `GetPort` (introspection wants the meaningful value
/// regardless of causality).
pub fn read_port(world: &World, entity: Entity, name: &str) -> Option<f64> {
    if let Some(v) = read_output_port(world, entity, name) {
        return Some(v);
    }
    if let Some(c) = world.get::<SimComponent>(entity) {
        if let Some(v) = c.inputs.get(name) {
            return Some(*v);
        }
    }
    if let Some(a) = world.get::<AvianSim>(entity) {
        if let Some(v) = a.inputs.get(name) {
            return Some(*v);
        }
    }
    if let Some(j) = world.get::<JointSim>(entity) {
        if let Some(v) = j.inputs.get(name) {
            return Some(*v);
        }
    }
    None
}

/// Write `value` to **input** port `name` on `entity`. Returns `true` if such an
/// input port existed and was written.
///
/// Strict: only writes a port that already exists (an undeclared name is
/// rejected with `false`, never silently created) — this is what lets the API
/// and the propagation master report dangling wires. `Out`-only ports are not
/// writable; the single-value ports ([`PhysicalPort`]/[`DigitalPort`]) are
/// bidirectional and always accept a write (with saturation for the `i16`
/// register).
///
/// Shared by `SetPort` and [`crate::systems::propagate::propagate_connections`].
///
/// TODO(ports): route a `SetPort` through a **ControlStream hold** so a manual
/// write survives the next propagate tick (latest-wins, overrides a live wire
/// until released — locked design decision 2). Today a wired input reverts next
/// tick; an unwired input holds.
pub fn write_port(world: &mut World, entity: Entity, name: &str, value: f64) -> bool {
    if let Some(mut c) = world.get_mut::<SimComponent>(entity) {
        if c.inputs.contains_key(name) {
            c.inputs.insert(name.to_string(), value);
            return true;
        }
    }
    if let Some(mut a) = world.get_mut::<AvianSim>(entity) {
        if a.inputs.contains_key(name) {
            a.inputs.insert(name.to_string(), value);
            return true;
        }
    }
    if let Some(mut j) = world.get_mut::<JointSim>(entity) {
        if j.inputs.contains_key(name) {
            j.inputs.insert(name.to_string(), value);
            return true;
        }
    }
    if name == PHYSICAL_PORT_NAME {
        if let Some(mut p) = world.get_mut::<PhysicalPort>(entity) {
            p.value = value as f32;
            return true;
        }
    }
    if name == DIGITAL_PORT_NAME {
        if let Some(mut d) = world.get_mut::<DigitalPort>(entity) {
            // DAC quantization: saturate the continuous value into the i16
            // register, exactly as real hardware clamps out-of-range commands.
            d.raw_value = value.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16;
            return true;
        }
    }
    false
}
