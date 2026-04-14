//! # Flight Software (FSW) & Command Fabric
//!
//! This crate implements the simulation's "Cerebellum"—the decentralized 
//! control architecture responsible for coordinating vessel subsystems.
//!
//! ## The "Why": Decentralized vs. Monolithic
//! Traditional simulators often use a single "Vessel Manager" script. 
//! LunCoSim follows a **Decentralized Subsystem** pattern, mirroring 
//! real aerospace hardware:
//! 1. **Autonomous Entities**: Subsystems (e.g., GNC, Power, Mobility) are 
//!    independent ECS entities. 
//! 2. **Asynchronous Messages**: Communication occurs via [CommandMessage]s 
//!    broadcast over the ECS event bus, allowing modules to be 
//!    hotswapped or re-tasked in real-time.
//! 3. **Hardware Abstraction**: The [FlightSoftware] component uses a 
//!    [port_map] to decouple semantic software logic (e.g., "DEPLOY_PANEL") 
//!    from the underlying physical Port entity, facilitating digital twin 
//!    mirroring where the same code can run against different vehicle manifests.

use bevy::prelude::*;
use std::collections::HashMap;

/// Dummy event type for unrecognized command fallback.
#[derive(Event)]
pub struct UnrecognizedCommand;

/// Plugin managing the asynchronous command fabric and FSW lifecycle.
pub struct LunCoFswPlugin;

impl Plugin for LunCoFswPlugin {
    fn build(&self, app: &mut App) {
        // Fallback handler captures orphaned commands for NACK telemetry.
        app.add_observer(unrecognized_command_handler);
    }
}

/// Marker component for an autonomous functional unit.
///
/// **Theory**: Represents a distinct piece of flight hardware (or emulated 
/// process) that registers its own listeners for [CommandMessage].
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component, Default)]
pub struct VesselSubsystem;

/// The primary Flight Software container for a spacecraft or rover.
///
/// **Logic**: Acts as the "Entity Manager" for the vessel, mapping 
/// human-readable Port names (SysML mnemonics) to the physical/digital 
/// registers they control.
#[derive(Component, Default)]
pub struct FlightSoftware {
    /// Maps mnemonic strings (e.g., "thruster_main") to their ECS entity ID.
    pub port_map: HashMap<String, Entity>,
    /// Global state flag for overriding drive commands.
    pub brake_active: bool,
}

/// Fallback observer that manages commands sent to a [FlightSoftware] entity
/// that were not handled by any other more specific subsystem observers.
fn unrecognized_command_handler(
    _trigger: On<UnrecognizedCommand>,
    _q_fsw: Query<&FlightSoftware>,
) {
    // Current design uses decentralized observers. If a command reaches this 
    // fallback, it signifies a command that was not understood by any 
    // installed module. 
    // TODO: Implement centralized NACK logging.
}


// TODO: Migrate tests to typed commands (DriveRover, BrakeRover)
// #[cfg(test)]
// mod tests {
//     use super::*;
// 
//     fn setup_test_app() -> (App, Entity, Entity, Entity) {
//         let mut app = App::new();
//         app.add_plugins(LunCoFswPlugin);
//         let p_l = app.world_mut().spawn(DigitalPort::default()).id();
//         let p_r = app.world_mut().spawn(DigitalPort::default()).id();
//         let mut map = HashMap::new();
//         map.insert("drive_left".to_string(), p_l);
//         map.insert("drive_right".to_string(), p_r);
//         let fsw_entity = app.world_mut().spawn(FlightSoftware { port_map: map, brake_active: false }).id();
//         (app, fsw_entity, p_l, p_r)
//     }
// 
//     #[test]
//     fn test_rover_differential_turning_left() {
//         let (mut app, fsw_entity, p_l, p_r) = setup_test_app();
//         app.world_mut().trigger(lunco_mobility::DriveRover {
//             target: fsw_entity, forward: 1.0, steer: -1.0,
//         });
//         assert_eq!(app.world().get::<DigitalPort>(p_l).unwrap().raw_value, 0);
//         assert_eq!(app.world().get::<DigitalPort>(p_r).unwrap().raw_value, 32767);
//     }
// }
