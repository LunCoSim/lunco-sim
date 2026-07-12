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
/// unit conversions (Celsius↔Kelvin), sensor zero-points, and DAC/ADC gains
/// (e.g. a `DigitalPort` raw register → physical units). `offset` defaults to
/// `0.0` so existing pure-gain wires are unchanged.
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

/// **The coupling tier of a co-simulated model** —
/// `docs/architecture/28-modelica-realtime-physics.md` §2 (A4).
///
/// Declared in USD as `lunco:cosim:tier = "A" | "B" | "C"`, **never inferred**.
/// It states what the model is allowed to do to the physics loop:
///
/// * **A — realtime-safe.** May drive an avian force/torque port on a
///   client-predicted `Dynamic` body. Requires a deterministic, bounded-cost
///   step: same stop-times and same work on every peer, every tick.
/// * **B — slow-domain.** Thermal, power, ECLSS, battery: coupled by *state*
///   (a signal read by scripts/UI/telemetry), never by force on a predicted
///   body. Free to be stiff and adaptive; its cost may vary.
/// * **C — offline.** Batch/experiment only; not stepped in the live loop.
///
/// The tier is a property of the MODEL (the entity carrying the
/// `SimComponent`), and it gates the wires OUT of it. An unset tier is
/// "undeclared", which is not the same as A: the wiring pass warns when an
/// undeclared or non-A model reaches a predicted body's force port
/// (`lunco-usd-sim`'s `rewire_usd_connections`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Reflect)]
#[reflect(Component)]
pub enum CosimTier {
    /// Realtime-safe: may drive predicted physics.
    A,
    /// Slow-domain: state coupling only.
    B,
    /// Offline / batch only.
    C,
}

impl CosimTier {
    /// Parse the USD `lunco:cosim:tier` attribute. Case-insensitive; anything
    /// else is a *declaration error*, not a silent default — the caller warns.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "A" => Some(Self::A),
            "B" => Some(Self::B),
            "C" => Some(Self::C),
            _ => None,
        }
    }

    /// May a model of this tier drive a force/torque port on a client-predicted
    /// `Dynamic` body? Only Tier A. (An UNSET tier is `None` at the call site
    /// and is likewise not permitted — undeclared ≠ A.)
    pub fn may_drive_predicted_physics(self) -> bool {
        matches!(self, Self::A)
    }
}

/// Is `port` an avian force/torque input — i.e. does writing it push a
/// rigid body around? These are the port names the avian backend exposes
/// (`force_x/y/z`, `torque_x/y/z`), and they are the ONLY ports whose writer
/// can desync a client-predicted body. Used by the tier gate (A4).
pub fn is_physics_force_port(port: &str) -> bool {
    port.starts_with("force") || port.starts_with("torque")
}

#[cfg(test)]
mod tier_tests {
    use super::*;

    #[test]
    fn tier_parses_and_gates() {
        assert_eq!(CosimTier::parse("a"), Some(CosimTier::A));
        assert_eq!(CosimTier::parse(" B "), Some(CosimTier::B));
        assert_eq!(CosimTier::parse("realtime"), None);
        assert!(CosimTier::A.may_drive_predicted_physics());
        assert!(!CosimTier::B.may_drive_predicted_physics());
        assert!(!CosimTier::C.may_drive_predicted_physics());
    }

    #[test]
    fn force_ports_are_the_gated_ones() {
        assert!(is_physics_force_port("force_y"));
        assert!(is_physics_force_port("torque_z"));
        assert!(!is_physics_force_port("throttle"));
        assert!(!is_physics_force_port("angle"));
    }
}
