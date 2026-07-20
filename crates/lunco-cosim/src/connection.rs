//! Simulation connections and ports.
//!
//! Follows the FMI/SSP ontology:
//! - [`SimPort`] — a named interface point on a [`crate::SimComponent`] (SSP: Connector)
//! - [`crate::SimConnection`] — a link between two ports (SSP: Connection)
//!
//! `startElement.startConnector → endElement.endConnector`

use bevy::prelude::*;

// Port causality/domain enums live in the neutral substrate so every participant
// (engine, API, scripting) shares one definition; re-exported here because this
// crate's `SimPort` and the avian backends address them as `connection::Port*`.
pub use lunco_core::ports::PortDirection;

/// A named interface point on a simulation entity.
///
/// Ports declare what an entity can connect to. The UI uses them to show
/// available connection points; the USD loader uses them to validate
/// connections defined in scene files.
///
/// Ports are metadata — the actual values flow through [`crate::SimComponent`]
/// inputs/outputs hash maps. A port just declares that a named slot exists
/// and what kind of value it carries.
#[derive(Debug, Clone, Reflect)]
pub struct SimPort {
    /// Port name (must match a key in `SimComponent.inputs` or `.outputs`).
    pub name: String,
    /// Whether this port receives or provides values.
    pub direction: PortDirection,
}

/// Collection of ports on a simulation entity.
///
/// Attach this alongside a [`crate::SimComponent`] to declare the entity's
/// connectable interface. Systems like `setup_balloon_wires` can build
/// this from the Modelica model's input/output declarations.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct SimPorts {
    pub ports: Vec<SimPort>,
}

/// A connection between two simulation ports.
///
/// Copies the output value of `start_element.start_connector` to
/// the input of `end_element.end_connector` every simulation step.
///
/// ## Port Resolution
///
/// Connector names are resolved by [`propagate_connections`](crate::systems::propagate::propagate_connections):
///
/// - `"netForce"`, `"volume"`, etc. → [`crate::SimComponent`](crate::SimComponent) outputs
/// - `"height"`, `"force_y"`, etc. → [`crate::AvianSim`](crate::AvianSim) outputs/inputs
///
/// ## Example
///
/// ```rust,ignore
/// commands.spawn(SimConnection {
///     start_element: balloon_entity,
///     start_connector: "netForce".into(),
///     end_element: balloon_entity,
///     end_connector: "force_y".into(),
///     scale: 1.0,
///     offset: 0.0,
/// });
/// ```
///
/// ## Affine transform (SSP `LinearTransformation`)
///
/// The propagated value is `source * scale + offset`. `scale` is the SSP
/// connection *factor* and `offset` the SSP *offset* — together they express
/// unit conversions (Celsius↔Kelvin), sensor zero-points, and actuator gains
/// (e.g. a normalized command port → physical units). `offset` defaults to
/// `0.0` so pure-gain wires need not name it.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct SimConnection {
    /// Entity owning the source port.
    pub start_element: Entity,
    /// Name of the source port (must be an output).
    pub start_connector: String,
    /// Entity owning the target port.
    pub end_element: Entity,
    /// Name of the target port (must be an input).
    pub end_connector: String,
    /// Multiplicative factor applied during propagation (SSP factor).
    pub scale: f64,
    /// Additive offset applied after scaling (SSP offset). `value = src*scale + offset`.
    pub offset: f64,
}

impl Default for SimConnection {
    fn default() -> Self {
        Self {
            start_element: Entity::PLACEHOLDER,
            start_connector: String::new(),
            end_element: Entity::PLACEHOLDER,
            end_connector: String::new(),
            scale: 1.0,
            offset: 0.0,
        }
    }
}

/// **A program's promise that it is fast enough to be trusted with a force** —
/// `docs/architecture/28-modelica-realtime-physics.md` §2.
///
/// Declared in USD as `lunco:program:realtimeSafe = true`, **never inferred**.
/// Only a program carrying it may drive an avian `force_*` / `torque_*` port on a
/// client-**predicted** `Dynamic` body: that requires a deterministic,
/// bounded-cost step — the same stop-times and the same work on every peer, every
/// tick. A model that takes 40ms to step, wired into a predicted body, diverges
/// from the server every frame it is late.
///
/// Absent is the default and means "not promised", which the wiring pass refuses a
/// force port (`lunco-usd-sim`'s `rewire_usd_connections`). Programs that never
/// touch physics — a supervisory script, a battery model — simply never declare it;
/// they are free to be stiff, adaptive, and slow, because state coupling cannot
/// desync a predicted body.
///
/// It is not a quality rating, and there is nothing below it: whether a program is
/// stepped in the live loop at all is decided by whether a live scene references it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Reflect)]
#[reflect(Component)]
pub struct RealtimeSafe;

/// Is `port` an avian force/torque input — i.e. does writing it push a rigid
/// body around? These are the ONLY ports whose writer can desync a
/// client-predicted body, so they are what the [`RealtimeSafe`] gate guards.
///
/// The set is [`crate::avian::BODY_FORCE_PORTS`], declared beside the port table
/// that implements it — NOT matched by spelling here.
pub fn is_physics_force_port(port: &str) -> bool {
    crate::avian::BODY_FORCE_PORTS.contains(&port)
}

#[cfg(test)]
mod realtime_gate_tests {
    use super::*;

    #[test]
    fn force_ports_are_the_gated_ones() {
        assert!(is_physics_force_port("force_y"));
        assert!(is_physics_force_port("torque_z"));
        // Body-frame thrust pushes a body just as hard as world-frame thrust.
        assert!(is_physics_force_port("force_local_x"));
        assert!(!is_physics_force_port("throttle"));
        assert!(!is_physics_force_port("angle"));
        // A gearbox's MECHANICAL shaft torque is not a body force: it drives a
        // reduction, not a rigid body, so it must not demand a realtime promise.
        assert!(!is_physics_force_port("torque"));
    }

    /// Tripwire: a body-force port added to the avian table but not declared in
    /// [`crate::avian::BODY_FORCE_PORTS`] would go UNGATED and silently. This
    /// cannot see through the write closures, so it uses the naming convention
    /// as a heuristic alarm — if you add a conventionally-named force port,
    /// declare it (or, if it genuinely does not touch a body, rename it).
    #[test]
    fn conventionally_named_force_ports_are_all_declared() {
        for group in crate::ports::AVIAN {
            for p in group.ports {
                let looks_like_force =
                    p.name.starts_with("force_") || p.name.starts_with("torque_");
                if looks_like_force {
                    assert!(
                        is_physics_force_port(p.name),
                        "avian port `{}` looks like a body-force port but is not in \
                         BODY_FORCE_PORTS — it would bypass the RealtimeSafe gate",
                        p.name
                    );
                }
            }
        }
    }
}
