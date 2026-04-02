use bevy::prelude::*;
use std::collections::HashMap;
use lunco_core::architecture::{CommandMessage, DigitalPort};

pub struct LunCoFswPlugin;

impl Plugin for LunCoFswPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(process_commands);
    }
}

/// The Flight Software logic component.
#[derive(Component, Default)]
pub struct FlightSoftware {
    pub port_map: HashMap<String, Entity>,
    pub brake_active: bool,
}

fn process_commands(
    trigger: On<CommandMessage>,
    mut q_fsw: Query<&mut FlightSoftware>,
    mut q_digital_ports: Query<&mut DigitalPort>,
) {
    let cmd = trigger.event();
    if let Ok(mut fsw) = q_fsw.get_mut(cmd.target) {
        match cmd.name.as_str() {
            "DRIVE_ROVER" => {
                if cmd.args.len() >= 1 {
                    let mut drive_power = (cmd.args[0] * 32767.0).clamp(-32767.0, 32767.0) as f32;
                    let mut steer_power = if cmd.args.len() >= 2 {
                        (cmd.args[1] * 32767.0).clamp(-32767.0, 32767.0) as f32
                    } else { 0.0 };

                    if fsw.brake_active {
                        drive_power = 0.0;
                        steer_power = 0.0;
                    }

                    // Differential Drive Mixing
                    let left_mix = (drive_power + steer_power).clamp(-32767.0, 32767.0) as i16;
                    let right_mix = (drive_power - steer_power).clamp(-32767.0, 32767.0) as i16;

                    if let Some(&port_l) = fsw.port_map.get("drive_left") {
                        if let Ok(mut p) = q_digital_ports.get_mut(port_l) { p.raw_value = left_mix; }
                    }
                    if let Some(&port_r) = fsw.port_map.get("drive_right") {
                        if let Ok(mut p) = q_digital_ports.get_mut(port_r) { p.raw_value = right_mix; }
                    }
                    if let Some(&port_s) = fsw.port_map.get("steering") {
                        if let Ok(mut p) = q_digital_ports.get_mut(port_s) { p.raw_value = steer_power as i16; }
                    }
                }
            }
            "BRAKE_ROVER" => {
                let brake_val = if cmd.args.len() >= 1 { cmd.args[0] } else { 1.0 };
                fsw.brake_active = brake_val > 0.5;

                let port_val = if fsw.brake_active { 32767 } else { 0 };
                
                if let Some(&port_b) = fsw.port_map.get("brake") {
                    if let Ok(mut p) = q_digital_ports.get_mut(port_b) { p.raw_value = port_val; }
                }

                if fsw.brake_active {
                    for name in ["drive_left", "drive_right"] {
                        if let Some(&port_id) = fsw.port_map.get(name) {
                            if let Ok(mut port) = q_digital_ports.get_mut(port_id) {
                                port.raw_value = 0;
                            }
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

    fn setup_test_app() -> (App, Entity, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(LunCoFswPlugin);
        let p_l = app.world_mut().spawn(DigitalPort::default()).id();
        let p_r = app.world_mut().spawn(DigitalPort::default()).id();
        let mut map = HashMap::new();
        map.insert("drive_left".to_string(), p_l);
        map.insert("drive_right".to_string(), p_r);
        let fsw_entity = app.world_mut().spawn(FlightSoftware { port_map: map, brake_active: false }).id();
        (app, fsw_entity, p_l, p_r)
    }

    #[test]
    fn test_rover_differential_turning_left() {
        let (mut app, fsw_entity, p_l, p_r) = setup_test_app();

        // Forward (1.0) + Steer Left (-1.0)
        app.world_mut().trigger(CommandMessage {
            target: fsw_entity,
            name: "DRIVE_ROVER".to_string(),
            args: vec![1.0, -1.0], 
            source: Entity::PLACEHOLDER,
        });

        // Left = 1.0 + (-1.0) = 0
        // Right = 1.0 - (-1.0) = 2.0 -> 255
        assert_eq!(app.world().get::<DigitalPort>(p_l).unwrap().raw_value, 0);
        assert_eq!(app.world().get::<DigitalPort>(p_r).unwrap().raw_value, 32767);
    }

    #[test]
    fn test_rover_differential_turning_right() {
        let (mut app, fsw_entity, p_l, p_r) = setup_test_app();

        // Forward (1.0) + Steer Right (1.0)
        app.world_mut().trigger(CommandMessage {
            target: fsw_entity,
            name: "DRIVE_ROVER".to_string(),
            args: vec![1.0, 1.0], 
            source: Entity::PLACEHOLDER,
        });

        // Left = 1.0 + 1.0 = 2.0 -> 255
        // Right = 1.0 - 1.0 = 0
        assert_eq!(app.world().get::<DigitalPort>(p_l).unwrap().raw_value, 32767);
        assert_eq!(app.world().get::<DigitalPort>(p_r).unwrap().raw_value, 0);
    }

    #[test]
    fn test_rover_braking() {
        let (mut app, fsw_entity, p_l, p_r) = setup_test_app();

        // Start Moving
        app.world_mut().trigger(CommandMessage {
            target: fsw_entity,
            name: "DRIVE_ROVER".to_string(),
            args: vec![1.0, 0.0], 
            source: Entity::PLACEHOLDER,
        });
        assert_eq!(app.world().get::<DigitalPort>(p_l).unwrap().raw_value, 32767);

        // Apply Brake
        app.world_mut().trigger(CommandMessage {
            target: fsw_entity,
            name: "BRAKE_ROVER".to_string(),
            args: vec![],
            source: Entity::PLACEHOLDER,
        });

        assert_eq!(app.world().get::<DigitalPort>(p_l).unwrap().raw_value, 0);
        assert_eq!(app.world().get::<DigitalPort>(p_r).unwrap().raw_value, 0);
    }
}
