//! # Flight Software (FSW)
//!
//! [`FlightSoftware`] is a vessel's command surface plus the index from actuator
//! name to the [`Port`](lunco_core::Port) entity that carries the value. The
//! command vocabulary is data — see [`FlightSoftware::inputs`].

use bevy::prelude::*;
use std::collections::HashMap;

/// The primary Flight Software container for a spacecraft or rover.
///
/// **Logic**: Acts as the "Entity Manager" for the vessel, mapping 
/// human-readable Port names (SysML mnemonics) to the physical/digital 
/// registers they control.
#[derive(Component, Default)]
pub struct FlightSoftware {
    /// Maps mnemonic strings (e.g., "thruster_main") to their ECS entity ID.
    pub port_map: HashMap<String, Entity>,
    /// Commanded logical **input** ports — the vessel's command surface. A rover
    /// seeds `throttle`/`steer`/`brake`, an avatar `forward`/`side`/`up`, a lander
    /// `throttle`/`pitch`/`roll`/`yaw`. Written through the shared port substrate
    /// (`SetPorts` → the FSW command backend) and consumed by the vehicle's
    /// actuator (`apply_drive_mix`, `apply_fly`, a Modelica bridge, …).
    ///
    /// The command *vocabulary is data*: the keys seeded here declare exactly which
    /// command ports this vehicle accepts, so the backend stays strict (an
    /// undeclared name is rejected → still reported as a dangling wire). This
    /// replaces the old bespoke `DriveCommand{throttle,steer,brake}` component —
    /// there is no per-vehicle-class command type any more.
    pub inputs: HashMap<String, f64>,
    /// Derived brake state, cached from `inputs["brake"] > 0.5` by the actuator so
    /// the per-tick physics systems read a bool without a map lookup.
    pub brake_active: bool,
}

impl FlightSoftware {
    /// Build with a `port_map` and a seeded command vocabulary: the input-port
    /// names this vehicle accepts, each initialised to `0.0`. The seeded keys ARE
    /// the vehicle's command surface (see [`FlightSoftware::inputs`]).
    pub fn new(port_map: HashMap<String, Entity>, command_ports: &[&str]) -> Self {
        Self {
            port_map,
            inputs: command_ports.iter().map(|n| (n.to_string(), 0.0)).collect(),
            brake_active: false,
        }
    }

    /// Current value of command input `name` (`0.0` if this vehicle doesn't
    /// accept it). The read side of the FSW command surface for actuators.
    #[inline]
    pub fn cmd(&self, name: &str) -> f64 {
        self.inputs.get(name).copied().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_surface_seeds_declared_ports_only() {
        let fsw = FlightSoftware::new(HashMap::new(), &["throttle", "steer", "brake"]);
        // Seeded keys exist and default to 0.0; undeclared keys read as 0.0 too.
        assert_eq!(fsw.cmd("throttle"), 0.0);
        assert_eq!(fsw.cmd("steer"), 0.0);
        assert_eq!(fsw.cmd("brake"), 0.0);
        assert_eq!(fsw.cmd("nonexistent"), 0.0);
        // Only the declared command vocabulary is present in the map — this is what
        // keeps the FSW command backend strict (undeclared writes are rejected).
        assert_eq!(fsw.inputs.len(), 3);
        assert!(fsw.inputs.contains_key("throttle"));
        assert!(!fsw.inputs.contains_key("nonexistent"));
    }

    #[test]
    fn writing_a_command_input_reads_back() {
        let mut fsw = FlightSoftware::new(HashMap::new(), &["forward", "side", "up"]);
        // An avatar's command vocabulary — same mechanism, different keys.
        *fsw.inputs.get_mut("forward").unwrap() = 1.0;
        assert_eq!(fsw.cmd("forward"), 1.0);
        assert_eq!(fsw.cmd("side"), 0.0);
    }
}
