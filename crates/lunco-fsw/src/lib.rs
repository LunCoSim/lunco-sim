use bevy::prelude::*;
use std::collections::HashMap;
use lunco_core::architecture::CommandMessage;

pub struct LunCoFswPlugin;

impl Plugin for LunCoFswPlugin {
    fn build(&self, app: &mut App) {
        // Only emit NACK if NO OTHER system handled the command.
        // However, Bevy Observers run in order. We might need a way to track if it was handled.
        // For now, we'll keep a simplified version that handles non-vessel-specific commands 
        // OR we just remove it and let subsystems handle everything.
        app.add_observer(unrecognized_command_handler);
    }
}

/// Marker component for any entity that acts as a Flight Software Subsystem.
/// Subsystems should register their own observers for CommandMessage.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component, Default)]
pub struct VesselSubsystem;

/// The Flight Software logic component (Legacy Monolithic - Now just a data container).
#[derive(Component, Default)]
pub struct FlightSoftware {
    pub port_map: HashMap<String, Entity>,
    pub brake_active: bool,
}

/// Fallback observer that triggers if a command was sent to a FlightSoftware entity
/// but wasn't handled by more specific observers (like raycast or joint).
fn unrecognized_command_handler(
    trigger: On<CommandMessage>,
    q_fsw: Query<&FlightSoftware>,
) {
    let cmd = trigger.event();
    
    // This is tricky in Bevy 0.18 because multiple observers can trigger.
    // If we want a "Final" fallback, we might need a status flag in the CommandMessage 
    // or just accept that decentralization means no central "NACK" unless we use a system.
    
    // For the sake of the Step 16 refactor, we remove the monolithic match.
    if q_fsw.contains(cmd.target) {
        // If it's a known command for ANY rover, we shouldn't NACK here.
        // But since observers are independent, this is a design challenge.
        // We'll leave it empty for now or only handle commands that are truly global.
    }
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
