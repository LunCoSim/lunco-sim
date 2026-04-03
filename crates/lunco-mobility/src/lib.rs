//! Physics-based mobility systems for surface exploration rovers.
//!
//! This crate implements various drive and steering models, including:
//! - **Raycast Wheels**: High-performance wheel simulation using raycasts for 
//!   suspension and friction.
//! - **Drive Models**: Differential (Skid) drive and Ackermann steering logic.
//! - **Suspension**: Physics-based spring-damper systems for joint-based 
//!   vehicle chassis.

use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::*;
use lunco_core::architecture::{DigitalPort, CommandMessage, CommandResponse, CommandStatus};
use lunco_fsw::FlightSoftware;
use lunco_core::RoverVessel;

/// Plugin for ground mobility physics and control systems.
pub struct LunCoMobilityPlugin;

impl Plugin for LunCoMobilityPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Suspension>()
           .register_type::<DifferentialDrive>()
           .register_type::<AckermannSteer>()
           .register_type::<WheelRaycast>()
           .add_observer(on_mobility_command)
           .add_systems(FixedUpdate, (
               suspension_system,
               apply_wheel_suspension,
               apply_wheel_drive,
               apply_wheel_steering,
           ).chain().run_if(|tw: Res<lunco_core::TimeWarpState>| tw.physics_enabled));
    }
}

/// A simplified wheel model that uses raycasting for ground interaction.
///
/// This avoids the complexity of simulating a full cylinder collider while
/// providing believable suspension and traction behavior.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct WheelRaycast {
    /// Entity index of the suspension physical port.
    pub suspension_port: Entity,
    /// Entity index of the drive physical port.
    pub drive_port: Entity,
    /// Entity index of the steering physical port.
    pub steer_port: Entity,
    /// Length of the suspension at rest (meters).
    pub rest_length: f64,
    /// Spring stiffness constant (N/m).
    pub spring_k: f64,
    /// Damping coefficient (Ns/m).
    pub damping_c: f64,
    /// Visual radius of the wheel (meters).
    pub wheel_radius: f64,
    /// Optional reference to the visual wheel entity for animation.
    pub visual_entity: Option<Entity>,
    /// Cached normal force from the last physics step, used for friction.
    pub last_normal_force: f64,
}

impl Default for WheelRaycast {
    fn default() -> Self {
        Self {
            suspension_port: Entity::PLACEHOLDER,
            drive_port: Entity::PLACEHOLDER,
            steer_port: Entity::PLACEHOLDER,
            rest_length: 0.4,
            spring_k: 8000.0,
            damping_c: 2800.0,
            wheel_radius: 0.4,
            visual_entity: None,
            last_normal_force: 0.0,
        }
    }
}

/// Calculates and applies suspension forces for [WheelRaycast] components.
///
/// This system uses [RayHits] to find the distance to the ground and 
/// applies a countering spring/damper force to the vehicle chassis.
fn apply_wheel_suspension(
    mut q_wheels: Query<(
        &mut WheelRaycast,
        &RayHits,
        &Transform,
        &ChildOf,
    )>,
    mut q_chassis: Query<Forces, With<RoverVessel>>,
    mut q_visual: Query<&mut Transform, (Without<WheelRaycast>, Without<RoverVessel>)>,
) {
    for (mut wheel, hits, wheel_tf, parent) in q_wheels.iter_mut() {
        let parent_entity = parent.parent();
        if let Ok(mut forces) = q_chassis.get_mut(parent_entity) {
            let mut closest_hit_dist = wheel.rest_length + wheel.wheel_radius;
            
            let world_pos = forces.position().0 + forces.rotation().0 * wheel_tf.translation.as_dvec3();
            let ray_dir_world = forces.rotation().0 * Vec3::NEG_Y.as_dvec3();
            
            if let Some(hit) = hits.iter_sorted().next() {
                let distance = hit.distance;
                if distance < (wheel.rest_length + wheel.wheel_radius) {
                    closest_hit_dist = distance;
                    let compression = ((wheel.rest_length + wheel.wheel_radius) - distance).max(0.0);
                    let spring_force_mag = compression * wheel.spring_k;
                    
                    let lin_vel = forces.linear_velocity();
                    let ang_vel = forces.angular_velocity();
                    let velocity_at_wheel = lin_vel + ang_vel.cross(world_pos - forces.position().0);
                    let relative_vel = velocity_at_wheel.dot(ray_dir_world); 
                    
                    let damping_force_mag = (relative_vel * wheel.damping_c).max(0.0);
                    let total_force_mag = (spring_force_mag + damping_force_mag).max(0.0);
                    
                    let force_vec = hit.normal * total_force_mag;
                    forces.apply_force_at_point(force_vec, world_pos);
                    wheel.last_normal_force = total_force_mag;
                } else {
                    wheel.last_normal_force = 0.0;
                }
            } else {
                wheel.last_normal_force = 0.0;
            }

            // Sync visual position of the wheel mesh.
            if let Some(visual_entity) = wheel.visual_entity {
                if let Ok(mut visual_tf) = q_visual.get_mut(visual_entity) {
                    let wheel_center_rel_y = (wheel_tf.translation.y as f64 + 0.5 - closest_hit_dist) + wheel.wheel_radius;
                    visual_tf.translation.y = wheel_center_rel_y as f32;
                }
            }
        }
    }
}

/// Applies drive torque and lateral friction to wheels.
///
/// Translates physical port values into actual forces applied to the 
/// chassis at the wheel's contact point.
fn apply_wheel_drive(
    q_wheels: Query<(
        &WheelRaycast,
        &GlobalTransform,
        &RayHits,
        &ChildOf,
    )>,
    q_ports: Query<&lunco_core::architecture::PhysicalPort>,
    mut q_chassis: Query<Forces, With<RoverVessel>>,
) {
    for (wheel, wheel_tf, hits, parent) in q_wheels.iter() {
        let parent_entity = parent.parent();
        if let Ok(mut forces) = q_chassis.get_mut(parent_entity) {
            if let Ok(port) = q_ports.get(wheel.drive_port) {
                if hits.iter().next().is_some() {
                    let forward = wheel_tf.forward().as_dvec3();
                    let drive_force_mag = port.value as f64 * 12000.0;
                    let force_vec = forward * drive_force_mag;
                    
                    forces.apply_force_at_point(force_vec, wheel_tf.translation().as_dvec3());

                    let chassis_vel = forces.linear_velocity();
                    let chassis_ang_vel = forces.angular_velocity();
                    let hub_pos_world = wheel_tf.translation().as_dvec3();
                    let hub_vel = chassis_vel + chassis_ang_vel.cross(hub_pos_world - forces.position().0);

                    // Simplified lateral friction to prevent sliding.
                    let normal_force = wheel.last_normal_force;
                    let friction_k = 1.1; 
                    
                    let right = wheel_tf.right().as_dvec3();
                    let lateral_vel = hub_vel.dot(right);
                    
                    let lateral_friction_force = -lateral_vel * friction_k * normal_force * right;
                    forces.apply_force_at_point(lateral_friction_force, hub_pos_world);
                }
            }
        }
    }
}

/// Updates the rotation of wheels based on steering port inputs.
fn apply_wheel_steering(
    mut q_wheels: Query<(&WheelRaycast, &mut Transform)>,
    q_ports: Query<&lunco_core::architecture::PhysicalPort>,
    mut q_visual: Query<&mut Transform, Without<WheelRaycast>>,
) {
    for (wheel, mut transform) in q_wheels.iter_mut() {
        if let Ok(port) = q_ports.get(wheel.steer_port) {
            let target_angle = (port.value * 0.5) as f32;
            transform.rotation = Quat::from_rotation_y(target_angle);
            
            // Sync visual rotation (including wheel spin representation).
            if let Some(visual_entity) = wheel.visual_entity {
                if let Ok(mut visual_tf) = q_visual.get_mut(visual_entity) {
                    visual_tf.rotation = transform.rotation * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
                }
            }
        }
    }
}

/// Hotswappable Logic: Differential Drive mixing (Skid Steering).
/// 
/// Calculated via `Left = Forward + Steer` and `Right = Forward - Steer`.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component, Default)]
pub struct DifferentialDrive {
    /// Port name for left-side motors.
    pub left_port: String,
    /// Port name for right-side motors.
    pub right_port: String,
}

/// Hotswappable Logic: Ackermann Steering.
/// 
/// Calculated via `Drive = Forward` and `Angle = Steer * MaxAngle`.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component, Default)]
pub struct AckermannSteer {
    /// Port name for drive motors (left).
    pub drive_left_port: String,
    /// Port name for drive motors (right).
    pub drive_right_port: String,
    /// Port name for the steering servo.
    pub steer_port: String,
    /// Maximum steering lock angle (radians).
    pub max_steer_angle: f32,
}

/// Suspension configuration for joint-based vehicles.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct Suspension {
    /// Target length of the spring (meters).
    pub rest_length: f64,
    /// Spring stiffness (N/m).
    pub spring_k: f64,
    /// Damping coefficient (Ns/m).
    pub damping_c: f64,
    /// Local axis of compression.
    pub local_axis: DVec3,
}

impl Default for Suspension {
    fn default() -> Self {
        Self {
            rest_length: 0.4,
            spring_k: 50000.0,
            damping_c: 2000.0,
            local_axis: DVec3::Y,
        }
    }
}

/// Solves spring-damper equations for entities connected by PrismaticJoints.
fn suspension_system(
    q_joints: Query<(&PrismaticJoint, &Suspension)>,
    mut q_bodies: Query<Forces>,
) {
    for (joint, susp) in q_joints.iter() {
        let e1 = joint.body1;
        let e2 = joint.body2;

        if let Ok([mut forces1, mut forces2]) = q_bodies.get_many_mut([e1, e2]) {
            let pos1 = forces1.position().0;
            let rot1 = forces1.rotation().0;
            let pos2 = forces2.position().0;
            let rot2 = forces2.rotation().0;
            
            let world_axis: DVec3 = rot1 * susp.local_axis;
            
            let anchor1_world: DVec3 = pos1 + rot1 * joint.local_anchor1().unwrap_or_default();
            let anchor2_world: DVec3 = pos2 + rot2 * joint.local_anchor2().unwrap_or_default();
            
            let diff_world: DVec3 = anchor2_world - anchor1_world;
            let current_length: f64 = -diff_world.dot(world_axis);
            let vel1 = forces1.velocity_at_point(anchor1_world);
            let vel2 = forces2.velocity_at_point(anchor2_world);
            let rel_vel: f64 = (vel2 - vel1).dot(world_axis);
            
            let compression: f64 = (susp.rest_length - current_length).max(0.0);
            let spring_force_mag: f64 = compression * susp.spring_k;
            
            let closing_speed: f64 = rel_vel;
            let damping_force_mag: f64 = (closing_speed * susp.damping_c).max(0.0);
            
            let total_force_mag: f64 = (spring_force_mag + damping_force_mag).clamp(0.0, 100_000.0);
            
            if !total_force_mag.is_finite() { continue; }
            
            let force_vec: DVec3 = world_axis * total_force_mag;
            
            forces1.apply_force_at_point(force_vec, anchor1_world);
            forces2.apply_force_at_point(-force_vec, anchor2_world);
        }
    }
}

/// Unified observer for all ground mobility commands.
///
/// Translates semantic instructions (DRIVE, BRAKE) into low-level digital
/// port signals based on the rover's specific drive model (Differential or Ackermann).
fn on_mobility_command(
    trigger: On<CommandMessage>,
    mut q_rovers: Query<(&mut FlightSoftware, Entity), With<RoverVessel>>,
    q_diff: Query<&DifferentialDrive>,
    q_ack: Query<&AckermannSteer>,
    mut q_digital_ports: Query<&mut DigitalPort>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    
    if let Ok((mut fsw, target_ent)) = q_rovers.get_mut(cmd.target) {
        let mut status = CommandStatus::Ack;

        match cmd.name.as_str() {
            "DRIVE_ROVER" => {
                if cmd.args.len() >= 1 {
                    let forward = cmd.args[0];
                    let steer = if cmd.args.len() >= 2 { cmd.args[1] } else { 0.0 };

                    if fsw.brake_active {
                        for name in ["drive_left", "drive_right", "steering"] {
                            if let Some(&port_id) = fsw.port_map.get(name) {
                                if let Ok(mut p) = q_digital_ports.get_mut(port_id) { p.raw_value = 0; }
                            }
                        }
                    } else if let Ok(diff) = q_diff.get(target_ent) {
                        let left_mix = ((forward + steer) * 32767.0).clamp(-32767.0, 32767.0) as i16;
                        let right_mix = ((forward - steer) * 32767.0).clamp(-32767.0, 32767.0) as i16;

                        if let Some(&port_l) = fsw.port_map.get(&diff.left_port) {
                            if let Ok(mut p) = q_digital_ports.get_mut(port_l) { p.raw_value = left_mix; }
                        }
                        if let Some(&port_r) = fsw.port_map.get(&diff.right_port) {
                            if let Ok(mut p) = q_digital_ports.get_mut(port_r) { p.raw_value = right_mix; }
                        }
                    } else if let Ok(ack) = q_ack.get(target_ent) {
                        let drive_val = (forward * 32767.0).clamp(-32767.0, 32767.0) as i16;
                        let steer_val = (steer * 32767.0).clamp(-32767.0, 32767.0) as i16;

                        if let Some(&port_l) = fsw.port_map.get(&ack.drive_left_port) {
                            if let Ok(mut p) = q_digital_ports.get_mut(port_l) { p.raw_value = drive_val; }
                        }
                        if let Some(&port_r) = fsw.port_map.get(&ack.drive_right_port) {
                            if let Ok(mut p) = q_digital_ports.get_mut(port_r) { p.raw_value = drive_val; }
                        }
                        if let Some(&port_s) = fsw.port_map.get(&ack.steer_port) {
                            if let Ok(mut p) = q_digital_ports.get_mut(port_s) { p.raw_value = steer_val; }
                        }
                    }
                } else {
                    status = CommandStatus::Failed("DRIVE_ROVER requires arguments".to_string());
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
                            if let Ok(mut port) = q_digital_ports.get_mut(port_id) { port.raw_value = 0; }
                        }
                    }
                }
            }
            _ => return,
        }

        // Send feedback regarding the command execution.
        commands.trigger(CommandResponse {
            command_id: cmd.id,
            status,
        });
    }
}

