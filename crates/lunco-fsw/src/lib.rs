//! Flight Software (FSW) architecture and subsystem management.
//!
//! This crate defines the core data structures for a decentralized Flight 
//! Software system. Unlike traditional monolithic simulators, LunCoSim treats 
//! FSW as a collection of hotswappable components and [VesselSubsystem]s 
//! that communicate via an asynchronous message fabric.

use bevy::prelude::*;
use std::collections::HashMap;
use lunco_core::architecture::CommandMessage;

/// Plugin for managing the Flight Software infrastructure and global command handling.
pub struct LunCoFswPlugin;

impl Plugin for LunCoFswPlugin {
    fn build(&self, app: &mut App) {
        // Registers a fallback handler to manage commands not captured by 
        // specialized subsystem observers.
        app.add_observer(unrecognized_command_handler);
    }
}

/// Marker component for any entity that acts as a Flight Software Subsystem.
///
/// Subsystems (e.g., Guidance, Navigation, Power Management) are independent 
/// ECS entities that register their own observers for [CommandMessage] to 
/// handle specific tasks.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component, Default)]
pub struct VesselSubsystem;

/// The primary data container for a vessel's Flight Software state.
///
/// It maintains a mapping of semantic port names (e.g., "drive_left") to 
/// specific ECS entities (usually [DigitalPort]s), allowing the software 
/// to address hardware without knowing its exact entity ID or location.
#[derive(Component, Default)]
pub struct FlightSoftware {
    /// Maps human-readable hardware descriptors to their digital register entities.
    pub port_map: HashMap<String, Entity>,
    /// Global state flag for the braking system.
    pub brake_active: bool,
}

/// Fallback observer that manages commands sent to a [FlightSoftware] entity 
/// that were not handled by any other more specific subsystem observers.
fn unrecognized_command_handler(
    _trigger: On<CommandMessage>,
    _q_fsw: Query<&FlightSoftware>,
) {
    // Current design uses decentralized observers. If a command reaches this 
    // fallback, it signifies a command that was not understood by any 
    // installed module. 
    // TODO: Implement centralized NACK logging.
}


#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_app() -> (App, Entity, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(LunCoFswPlugin);
        // We need one of the rover plugins to actually handle the commands now!
        // We'll use a mock observer for the test instead of full physics.
        let p_l = app.world_mut().spawn(DigitalPort::default()).id();
        let p_r = app.world_mut().spawn(DigitalPort::default()).id();
        let mut map = HashMap::new();
        map.insert("drive_left".to_string(), p_l);
        map.insert("drive_right".to_string(), p_r);
        let fsw_entity = app.world_mut().spawn(FlightSoftware { port_map: map, brake_active: false }).id();
        
        // Mock observer to simulate a modular subsystem in tests
        app.world_mut().add_observer(move |
            trigger: On<CommandMessage>,
            mut q_fsw: Query<&mut FlightSoftware>,
            mut q_ports: Query<&mut DigitalPort>,
        | {
            let cmd = trigger.event();
            if cmd.target != fsw_entity { return; }
            if let Ok(mut fsw) = q_fsw.get_mut(cmd.target) {
                if cmd.name == "DRIVE_ROVER" {
                    let drive = cmd.args[0] as f32;
                    let steer = if cmd.args.len() > 1 { cmd.args[1] as f32 } else { 0.0 };
                    if let Ok(mut p) = q_ports.get_mut(*fsw.port_map.get("drive_left").unwrap()) {
                        p.raw_value = ((drive + steer) * 32767.0) as i16;
                    }
                    if let Ok(mut p) = q_ports.get_mut(*fsw.port_map.get("drive_right").unwrap()) {
                        p.raw_value = ((drive - steer) * 32767.0) as i16;
                    }
                } else if cmd.name == "BRAKE_ROVER" {
                    fsw.brake_active = true;
                    if let Ok(mut p) = q_ports.get_mut(*fsw.port_map.get("drive_left").unwrap()) { p.raw_value = 0; }
                    if let Ok(mut p) = q_ports.get_mut(*fsw.port_map.get("drive_right").unwrap()) { p.raw_value = 0; }
                }
            }
        });

        (app, fsw_entity, p_l, p_r)
    }

    #[test]
    fn test_rover_differential_turning_left() {
        let (mut app, fsw_entity, p_l, p_r) = setup_test_app();
        app.world_mut().trigger(CommandMessage {
            id: 1,
            target: fsw_entity,
            name: "DRIVE_ROVER".to_string(),
            args: smallvec::smallvec![1.0, -1.0], 
            source: Entity::PLACEHOLDER,
        });
        assert_eq!(app.world().get::<DigitalPort>(p_l).unwrap().raw_value, 0);
        assert_eq!(app.world().get::<DigitalPort>(p_r).unwrap().raw_value, 32767);
    }

    #[test]
    fn test_rover_differential_turning_right() {
        let (mut app, fsw_entity, p_l, p_r) = setup_test_app();
        app.world_mut().trigger(CommandMessage {
            id: 2,
            target: fsw_entity,
            name: "DRIVE_ROVER".to_string(),
            args: smallvec::smallvec![1.0, 1.0], 
            source: Entity::PLACEHOLDER,
        });
        assert_eq!(app.world().get::<DigitalPort>(p_l).unwrap().raw_value, 32767);
        assert_eq!(app.world().get::<DigitalPort>(p_r).unwrap().raw_value, 0);
    }

    #[test]
    fn test_rover_braking() {
        let (mut app, fsw_entity, p_l, p_r) = setup_test_app();
        app.world_mut().trigger(CommandMessage {
            id: 3,
            target: fsw_entity,
            name: "DRIVE_ROVER".to_string(),
            args: smallvec::smallvec![1.0, 0.0], 
            source: Entity::PLACEHOLDER,
        });
        app.world_mut().trigger(CommandMessage {
            id: 4,
            target: fsw_entity,
            name: "BRAKE_ROVER".to_string(),
            args: smallvec::smallvec![],
            source: Entity::PLACEHOLDER,
        });
        assert_eq!(app.world().get::<DigitalPort>(p_l).unwrap().raw_value, 0);
        assert_eq!(app.world().get::<DigitalPort>(p_r).unwrap().raw_value, 0);
    }
}
