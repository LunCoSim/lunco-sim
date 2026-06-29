//! Co-simulation **port substrate** — the FMI/SSP scalar-exchange surface shared
//! by every participant so they all read/write exposed values through ONE path.
//!
//! A *port* is a named scalar (`f64`) on a participant entity. Modelica variables,
//! avian rigid-body state, joint angles, the SysML/hardware "nervous system"
//! ports, and (in future) an imported FMU all present as ports, so that wires,
//! the API (`ListPorts` / `GetPort` / `SetPort`), the UI inspector, and every
//! scripting runtime (rhai/python) treat them uniformly — the FMI/SSP contract.
//!
//! ## Why this lives in `lunco-core`
//!
//! Ports are co-sim *substrate*, not an engine or API concern: the wire engine
//! (`lunco-cosim`) runs ON them, the API and scripts merely consume them. Putting
//! the registry here — below every participant — lets each crate **register** its
//! backends downward and **consume** the registry, with nobody depending "up".
//! This is what lets `lunco-scripting` reach ports even though `lunco-cosim`
//! (which owns the avian/joint/Modelica backends) depends ON scripting: both
//! depend down on this module. A future FMU-import or script-defined component is
//! just one more registered backend the wire engine then honours.
//!
//! ## Value model
//!
//! The wire currency is `f64` (continuous Real — what FMI-CS exchanges almost
//! everywhere). Typed backends convert at their own boundary (a `DigitalPort`
//! `i16` register saturates on write, like a real DAC/ADC). We deliberately do
//! **not** model `Bool`/`Enum`/`String` ports until a concrete need appears.
//!
//! ## One registry, four thin operations
//!
//! Every port-bearing backend is one [`PortBackend`] entry (list / read-output /
//! read-input / write-input), registered into the [`PortRegistry`] resource. The
//! four query methods just fold over the registered backends in order, so a new
//! backend is added by **registering** it — no consumer changes. Registration
//! order *is* resolution precedence (first match wins).

use bevy::prelude::*;
use std::collections::HashMap;

/// Direction (causality) of a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect)]
pub enum PortDirection {
    /// Port receives values from connections.
    In,
    /// Port provides values to connections.
    Out,
    /// Port can both receive and provide values.
    InOut,
}

/// Physical domain of a port. Used for UI grouping and connection validation;
/// a wrong guess is cosmetic, never load-bearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect)]
pub enum PortType {
    /// Mechanical force/torque.
    Force,
    /// Position, velocity, acceleration, attitude.
    Kinematic,
    /// Voltage, current.
    Electrical,
    /// Temperature, heat flow.
    Thermal,
    /// Dimensionless or mixed-domain signal.
    Signal,
}

/// A discovered port: identity, causality, physical domain, current value.
///
/// Returned by [`PortRegistry::entity_ports`] for listing/introspection. The
/// `value` is a snapshot read at call time; live consumers read through the
/// registry directly.
#[derive(Debug, Clone)]
pub struct PortRef {
    /// Port name — the key in the owning backend, or the canonical name for a
    /// single-value backend.
    pub name: String,
    /// Causality.
    pub direction: PortDirection,
    /// Physical domain (best-effort classification — see [`classify`]).
    pub port_type: PortType,
    /// Snapshot of the current value.
    pub value: f64,
}

/// Best-effort physical-domain classification from a port name.
///
/// A heuristic for UI grouping / connection validation only. Backends that know
/// their type better should set [`PortRef::port_type`] directly.
pub fn classify(name: &str) -> PortType {
    if name.starts_with("force") || name.starts_with("torque") {
        PortType::Force
    } else if name.starts_with("position")
        || name.starts_with("velocity")
        || name.starts_with("angvel")
        || name.starts_with("quat")
        || name == "height"
        || name == "angle"
        || name == "yaw"
        || name == "pitch"
        || name == "roll"
    {
        PortType::Kinematic
    } else if name.contains("temp") || name.contains("heat") {
        PortType::Thermal
    } else {
        PortType::Signal
    }
}

/// Append every `(name, value)` in `map` as a [`PortRef`] of direction `dir`.
/// Helper for map-backed backends (e.g. Modelica `inputs`/`outputs`).
#[inline]
pub fn push_map(out: &mut Vec<PortRef>, map: &HashMap<String, f64>, dir: PortDirection) {
    for (name, value) in map {
        out.push(PortRef {
            name: name.clone(),
            direction: dir,
            port_type: classify(name),
            value: *value,
        });
    }
}

/// One port-bearing backend, expressed as four operations over `(World, Entity)`.
///
/// Ops are plain `fn` pointers (non-capturing closures), so a backend is `Copy`
/// and the registry is cheap to clone out of the world for `&mut World` access.
/// Each op is causality-correct: `read_output`/`read_input` see only the matching
/// direction; `write_input` accepts only an existing input slot (the strictness
/// that lets `propagate` report dangling wires). A single-value backend is
/// bidirectional — its one scalar *is* both its output and input.
#[derive(Clone, Copy)]
pub struct PortBackend {
    /// Append this backend's ports on `entity` (outputs then inputs) to `out`.
    pub list: fn(&World, Entity, &mut Vec<PortRef>),
    /// Read the **output** named `name`, or `None`.
    pub read_output: fn(&World, Entity, &str) -> Option<f64>,
    /// Read the **input** named `name`, or `None`.
    pub read_input: fn(&World, Entity, &str) -> Option<f64>,
    /// Write `value` to **input** `name`; `true` iff the port existed here.
    pub write_input: fn(&mut World, Entity, &str, f64) -> bool,
}

/// The single registry of port-bearing backends — **the** read/write/list surface
/// for every exposed simulation value, whichever backend owns it.
///
/// Backends are registered (in dependency-correct order) by their owning crate's
/// plugin; the four query methods fold over them. Registration order is
/// resolution precedence (first match wins). `Clone` is cheap (a `Vec` of `Copy`
/// `fn` pointers) so a `&mut World` caller clones it out before writing.
#[derive(Resource, Default, Clone)]
pub struct PortRegistry {
    backends: Vec<PortBackend>,
}

impl PortRegistry {
    /// Register a backend. Later registrations have lower precedence on name
    /// collisions. Call from a plugin `build`.
    pub fn register(&mut self, backend: PortBackend) {
        self.backends.push(backend);
    }

    /// Enumerate every exposed port on `entity`, across all backends.
    /// The backbone of `ListPorts`.
    pub fn entity_ports(&self, world: &World, entity: Entity) -> Vec<PortRef> {
        let mut out = Vec::new();
        for backend in &self.backends {
            (backend.list)(world, entity, &mut out);
        }
        out
    }

    /// Read the **output** named `name` on `entity` — the value a connection reads
    /// from its *source*. Searches outputs only (plus bidirectional single-value
    /// ports). Critical when a name exists as both input and output on one entity.
    pub fn read_output_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        self.backends
            .iter()
            .find_map(|b| (b.read_output)(world, entity, name))
    }

    /// Read the current value of port `name`, preferring an **output**, then
    /// falling back to an **input**. The backbone of `GetPort`.
    pub fn read_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        if let Some(v) = self.read_output_port(world, entity, name) {
            return Some(v);
        }
        self.backends
            .iter()
            .find_map(|b| (b.read_input)(world, entity, name))
    }

    /// Read the **input** value of port `name` — the commanded side, skipping
    /// outputs. Use where the input specifically is wanted (e.g. a joint's
    /// commanded motor setpoint vs its measured angle, both named `angle`).
    pub fn read_input_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        self.backends
            .iter()
            .find_map(|b| (b.read_input)(world, entity, name))
    }

    /// Write `value` to **input** port `name`. Returns `true` if such an input
    /// existed and was written. Strict: an undeclared name is rejected (never
    /// silently created) — what lets the API and propagation master report
    /// dangling wires. First backend that owns the port wins.
    pub fn write_port(&self, world: &mut World, entity: Entity, name: &str, value: f64) -> bool {
        for backend in &self.backends {
            if (backend.write_input)(world, entity, name, value) {
                return true;
            }
        }
        false
    }
}
