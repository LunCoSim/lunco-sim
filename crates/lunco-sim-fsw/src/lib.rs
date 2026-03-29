use bevy::prelude::*;
use std::collections::HashMap;
use lunco_sim_core::architecture::{CommandMessage, DigitalPort};

pub struct LunCoSimFswPlugin;

impl Plugin for LunCoSimFswPlugin {
    fn build(&self, app: &mut App) {
        // Bevy 0.18 changed to Observers - no more Update systems for Events that target entities!
        app.add_observer(process_commands);
    }
}

/// The Flight Software logic component.
/// Holds a mapping of semantic port names (e.g. "motor_l", "steering") 
/// to the actual `DigitalPort` Entity IDs.
#[derive(Component, Default)]
pub struct FlightSoftware {
    pub port_map: HashMap<String, Entity>,
}

fn process_commands(
    trigger: On<CommandMessage>,
    q_fsw: Query<&FlightSoftware>,
    mut q_digital_ports: Query<&mut DigitalPort>,
) {
    let cmd = trigger.event();
    // If the command targets an entity with FSW logic
    if let Ok(fsw) = q_fsw.get(cmd.target) {
            match cmd.name.as_str() {
                "SET_PORT" => {
                    // Expects args: [port_index_as_f32, value_to_set_as_f32]
                    // In a real system we'd use robust serialization, 
                    // but for Stage 1 f32 array is enough.
                    if cmd.args.len() >= 2 {
                        // Very naive mapping for generic tests
                        let val = cmd.args[1] as i16;
                        // Let's assume there is a generic way to find port by index 
                        // For demonstration, we just do string-based lookup if we had it.
                    }
                }
                "DRIVE_ROVER" => {
                    // Extracted MVP logic for Rover Drive mixing
                    // Arg 0: Drive (-1.0 to 1.0)
                    // Arg 1: Steer (-1.0 to 1.0)
                    if cmd.args.len() >= 1 {
                        let drive_power = (cmd.args[0] * 255.0).clamp(-255.0, 255.0) as i16;
                        let steer_power = if cmd.args.len() >= 2 {
                            (cmd.args[1] * 255.0).clamp(-255.0, 255.0) as i16
                        } else { 0 };

                        if let Some(&drive_port_left) = fsw.port_map.get("drive_left") {
                            if let Ok(mut port) = q_digital_ports.get_mut(drive_port_left) {
                                port.raw_value = drive_power;
                            }
                        }
                        if let Some(&drive_port_right) = fsw.port_map.get("drive_right") {
                            if let Ok(mut port) = q_digital_ports.get_mut(drive_port_right) {
                                port.raw_value = drive_power;
                            }
                        }
                        if let Some(&steer) = fsw.port_map.get("steer") {
                            if let Ok(mut port) = q_digital_ports.get_mut(steer) {
                                port.raw_value = steer_power;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fsw_drive_command() {
        let mut app = App::new();
        app.add_plugins(LunCoSimFswPlugin);

        let p_drive_l = app.world_mut().spawn(DigitalPort::default()).id();
        let p_drive_r = app.world_mut().spawn(DigitalPort::default()).id();
        let p_steer = app.world_mut().spawn(DigitalPort::default()).id();

        let mut map = HashMap::new();
        map.insert("drive_left".to_string(), p_drive_l);
        map.insert("drive_right".to_string(), p_drive_r);
        map.insert("steer".to_string(), p_steer);

        let fsw_entity = app.world_mut().spawn(FlightSoftware { port_map: map }).id();

        // Triggering the event globally (Bevy 0.18)
        app.world_mut().trigger(
            CommandMessage {
                target: fsw_entity,
                name: "DRIVE_ROVER".to_string(),
                args: vec![1.0, -0.5], // Full forward, half left
                source: Entity::PLACEHOLDER,
            }
        );

        app.update();

        assert_eq!(app.world().get::<DigitalPort>(p_drive_l).unwrap().raw_value, 255);
        assert_eq!(app.world().get::<DigitalPort>(p_drive_r).unwrap().raw_value, 255);
        assert_eq!(app.world().get::<DigitalPort>(p_steer).unwrap().raw_value, -127);
    }
}
