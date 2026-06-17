//! Uniform port table — the single surface for **discovering** and
//! **reading/writing** every exposed simulation value, whichever engine owns
//! it.
//!
//! A *port* is a named scalar (`f64`) on a participant entity. Modelica
//! variables, Avian forces/state, joint angles, environment signals, and
//! custom USD attributes all present as ports, so that wires
//! ([`crate::SimConnection`]), the API (`ListPorts` / `GetPort` / `SetPort`),
//! and the UI inspector all treat them uniformly — the FMI/SSP contract.
//!
//! ## Auto-exposure
//!
//! A backend exposes its vars with **no USD authoring**: it populates the
//! `inputs` / `outputs` maps of its marker component (`SimComponent`,
//! [`crate::AvianSim`], the future `JointSim` / env source). A key in
//! `outputs` is an [`PortDirection::Out`] port; a key in `inputs` is an
//! [`PortDirection::In`] port. The maps *are* the live value table; this
//! module is the read side over them.
//!
//! ## TODO(ports): tag-driven exposure
//!
//! Today [`entity_ports`] / [`read_port`] / [`write_port`] hand-enumerate the
//! known port-bearing component types. This does not scale to arbitrary
//! components. Replace it with a `#[derive(SimExposed)]` + `#[port(in|out)]`
//! field attribute (or a `ReflectSimPort` type-data registration) so any
//! component field becomes a port purely by tagging — the ontology's
//! "Attribute exposed via Bevy Reflection" (see `docs/architecture/01-ontology.md`).
//! Keep this module's function signatures stable so that lands as a drop-in:
//! the resolver gains a reflect tier, callers are unchanged.

use bevy::prelude::*;

use crate::connection::{PortDirection, PortType};
use crate::{AvianSim, SimComponent};

/// A discovered port: identity, causality, physical domain, current value.
///
/// Returned by [`entity_ports`] for listing/introspection. The `value` is a
/// snapshot read at call time; live consumers read through the maps directly.
#[derive(Debug, Clone)]
pub struct PortRef {
    /// Port name — the key in the owning component's `inputs`/`outputs` map.
    pub name: String,
    /// Causality (key in `outputs` → `Out`, key in `inputs` → `In`).
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
/// of guessing from the name. Type is used only for UI grouping and
/// connection validation, so a wrong guess is cosmetic, never load-bearing.
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
fn push_map(out: &mut Vec<PortRef>, map: &std::collections::HashMap<String, f64>, dir: PortDirection) {
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
/// The backbone of `ListPorts`. TODO(ports): generalize beyond the
/// hand-written `SimComponent` + `AvianSim` set — joint/env backends append
/// their components here for now; the reflect-harvest replaces the whole body.
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
    out
}

/// Read the current value of port `name` on `entity`, searching outputs then
/// inputs across every port-bearing backend. `None` if no such port.
///
/// The backbone of `GetPort`. TODO(ports): same generalization as
/// [`entity_ports`].
pub fn read_port(world: &World, entity: Entity, name: &str) -> Option<f64> {
    if let Some(c) = world.get::<SimComponent>(entity) {
        if let Some(v) = c.outputs.get(name).or_else(|| c.inputs.get(name)) {
            return Some(*v);
        }
    }
    if let Some(a) = world.get::<AvianSim>(entity) {
        if let Some(v) = a.outputs.get(name).or_else(|| a.inputs.get(name)) {
            return Some(*v);
        }
    }
    None
}

/// Write a setpoint to input port `name` on `entity`. Returns `true` if the
/// port existed and was written.
///
/// The backbone of `SetPort`. Only `In` ports are writable (an `Out` is
/// engine-produced); writing one is rejected (`false`).
///
/// TODO(ports): this writes the input slot **once**, but `propagate_connections`
/// zeroes all `SimComponent.inputs` every `FixedUpdate`, so a wired port reverts
/// next tick. Per the locked design (decision 2), route `SetPort` through a
/// **ControlStream hold**: latest-wins, `hold_last(timeout)` fallback, overriding
/// any live wire until released (Manual beats Auto, then reverts). That hold is
/// re-applied after propagate each tick. Implement the hold as the next step;
/// this direct write is the placeholder so the resolver API is complete.
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
    false
}
