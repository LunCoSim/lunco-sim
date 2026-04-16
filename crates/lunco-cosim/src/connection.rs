//! Simulation connections and ports.
//!
//! Follows the FMI/SSP ontology:
//! - [`SimPort`] — a named interface point on a [`SimComponent`] (SSP: Connector)
//! - [`SimConnection`] — a link between two ports (SSP: Connection)
//!
//! `startElement.startConnector → endElement.endConnector`

use bevy::prelude::*;

/// Direction of a simulation port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect)]
pub enum PortDirection {
    /// Port receives values from connections.
    In,
    /// Port provides values to connections.
    Out,
    /// Port can both receive and provide values.
    InOut,
}

/// Physical domain of a simulation port.
///
/// Used for validation: connections should only link ports of the same type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect)]
pub enum PortType {
    /// Mechanical force/torque.
    Force,
    /// Position, velocity, acceleration.
    Kinematic,
    /// Voltage, current.
    Electrical,
    /// Temperature, heat flow.
    Thermal,
    /// Dimensionless or mixed-domain signal.
    Signal,
}

/// A named interface point on a simulation entity.
///
/// Ports declare what an entity can connect to. The UI uses them to show
/// available connection points; the USD loader uses them to validate
/// connections defined in scene files.
///
/// Ports are metadata — the actual values flow through [`SimComponent`]
/// inputs/outputs hash maps. A port just declares that a named slot exists
/// and what kind of value it carries.
#[derive(Debug, Clone, Reflect)]
pub struct SimPort {
    /// Port name (must match a key in `SimComponent.inputs` or `.outputs`).
    pub name: String,
    /// Whether this port receives or provides values.
    pub direction: PortDirection,
    /// Physical domain for connection validation.
    pub port_type: PortType,
}

/// Collection of ports on a simulation entity.
///
/// Attach this alongside a [`SimComponent`] to declare the entity's
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
/// - `"netForce"`, `"volume"`, etc. → [`SimComponent`](crate::SimComponent) outputs
/// - `"height"`, `"force_y"`, etc. → [`AvianSim`](crate::AvianSim) outputs/inputs
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
/// });
/// ```
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
    /// Scaling factor applied during propagation.
    pub scale: f64,
}

impl Default for SimConnection {
    fn default() -> Self {
        Self {
            start_element: Entity::PLACEHOLDER,
            start_connector: String::new(),
            end_element: Entity::PLACEHOLDER,
            end_connector: String::new(),
            scale: 1.0,
        }
    }
}
