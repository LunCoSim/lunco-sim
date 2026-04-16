//! Simulation wire — explicit connection between two connectors.
//!
//! Follows the FMI/SSP `<Connection>` pattern:
//! `startElement.startConnector → endElement.endConnector`

use bevy::prelude::*;

/// A wire connecting two connectors.
///
/// Copies the output value of `start_element.start_connector` to
/// the input of `end_element.end_connector` every simulation step.
///
/// ## Wire Resolution
///
/// Connector names are resolved by the [`systems::propagate::propagate_wires`] system:
///
/// - `"netForce"`, `"volume"`, etc. → [`SimComponent`] outputs
/// - `"height"`, `"force_y"`, etc. → [`AvianSim`] outputs/inputs
/// - `"__gravity__"` → Global [`lunco_core::Gravity`] resource (resolved separately)
///
/// ## Example
///
/// ```rust,ignore
/// // Wire: Modelica netForce → Avian force_y
/// commands.spawn(SimWire {
///     start_element: balloon_entity,
///     start_connector: "netForce".into(),
///     end_element: balloon_entity,
///     end_connector: "force_y".into(),
///     scale: 1.0,
/// });
/// ```
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct SimWire {
    /// Entity owning the start (source) connector.
    pub start_element: Entity,
    /// Name of the start connector (must be an output).
    pub start_connector: String,
    /// Entity owning the end (target) connector.
    pub end_element: Entity,
    /// Name of the end connector (must be an input).
    pub end_connector: String,
    /// Signal gain/scaling factor applied during propagation.
    pub scale: f64,
}

impl Default for SimWire {
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
