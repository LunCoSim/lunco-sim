//! # Surface Mobility & Traction Physics
//!
//! This crate implements the core physics models for planetary rovers and 
//! surface exploration vehicles. 
//!
//! ## The "Why": Raycast-Based Ground Interaction
//! Traditional mesh-to-mesh collision for wheels is computationally expensive 
//! and prone to "snagging" on terrain geometry. We use a **Raycast Wheel** 
//! model to provide a stable, high-performance alternative:
//! 1. **Suspension Logic**: An emulated spring-damper system computes normal 
//!    forces based on ray length, preventing high-frequency jitter.
//! 2. **Traction Physics**: Lateral and longitudinal friction are applied 
//!    at the ray's contact point, allowing for complex skid and slip behaviors 
//!    without the overhead of continuous contact manifolds.
//! 3. **Numeric Stability**: By projecting a single ray, we ensure the wheel 
//!    always "floats" at the correct elevation, even on highly irregular 
//!    procedural terrain.
//!
//! ## Control Mixing Models
//! The crate supports hotswappable steering architectures:
//! - **Differential (Skid) Drive**: Common for heavy loaders and excavators; 
//!   turns by varying velocity between left and right tracks.
//! - **Ackermann Steering**: Standard for high-speed mobility; pivots leading 
//!   wheels to maintain a common center of rotation, reducing tire scrub.

use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::*;
use lunco_core::architecture::{DigitalPort, CommandMessage, CommandResponse, CommandStatus};
use lunco_fsw::FlightSoftware;
use lunco_core::RoverVessel;

/// Manages the integration of mobility physics and control observers.
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

/// A high-performance wheel model using emulated suspension rays.
///
/// **Theory**: Instead of a physical collider, this component projects a ray 
/// downwards. The resulting distance is used to solve the spring-damper 
/// equation, simulating the behavior of a physical tire and strut.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct WheelRaycast {
    /// Port mapping for suspension telemetry.
    pub suspension_port: Entity,
    /// Port mapping for drive torque actuation.
    pub drive_port: Entity,
    /// Port mapping for steering angle actuation.
    pub steer_port: Entity,
    /// Length of the suspension at rest in meters.
    pub rest_length: f64,
    /// Hooke's Law spring constant (Stiffness in N/m).
    pub spring_k: f64,
    /// Damping coefficient to suppress oscillations (Ns/m).
    pub damping_c: f64,
    /// Radius of the tire (effectively the minimum offset from ground).
    pub wheel_radius: f64,
    /// Entity for the visual mesh to be transformed.
    pub visual_entity: Option<Entity>,
    /// Resultant normal force from the last physics tick, used for friction calculations.
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

/// System solving the vertical suspension dynamics.
///
/// **Logic**: Performs a ray-world intersection check. If a hit is detected 
/// within the suspension travel range, it applies an upward force to the 
/// parent chassis based on the compression distance and relative velocity.
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
                    // Compression = (RestLen + Radius) - HitDistance
                    let compression = ((wheel.rest_length + wheel.wheel_radius) - distance).max(0.0);
                    let spring_force_mag = compression * wheel.spring_k;
                    
                    // Damping calculation based on relative normal velocity
                    let lin_vel = forces.linear_velocity();
                    let ang_vel = forces.angular_velocity();
                    let velocity_at_wheel = lin_vel + ang_vel.cross(world_pos - forces.position().0);
                    let relative_vel = velocity_at_wheel.dot(ray_dir_world); 
                    
                    let damping_force_mag = (relative_vel * wheel.damping_c).max(0.0);
                    let total_force_mag = (spring_force_mag + damping_force_mag).max(0.0);
                    
                    // Apply counter-force to the chassis to push it away from the ground
                    let force_vec = hit.normal * total_force_mag;
                    forces.apply_force_at_point(force_vec, world_pos);
                    wheel.last_normal_force = total_force_mag;
                } else {
                    wheel.last_normal_force = 0.0;
                }
            } else {
                wheel.last_normal_force = 0.0;
            }

            // Sync visual position to make the wheel appear to move with the suspension
            if let Some(visual_entity) = wheel.visual_entity {
                if let Ok(mut visual_tf) = q_visual.get_mut(visual_entity) {
                    let wheel_center_rel_y = (wheel_tf.translation.y as f64 + 0.5 - closest_hit_dist) + wheel.wheel_radius;
                    visual_tf.translation.y = wheel_center_rel_y as f32;
                }
            }
        }
    }
}

/// System applying longitudinal drive torque and lateral friction.
///
/// **Theory**: Drive force is applied along the wheel's forward vector. 
/// Lateral friction is emulated by countering any side-velocity at the 
/// contact hub, preventing rovers from sliding like they are on ice.
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
                // Traction only exists when the ray is hitting the ground
                if hits.iter().next().is_some() {
                    let forward = wheel_tf.forward().as_dvec3();
                    let drive_force_mag = port.value as f64 * 12000.0; // Scaled to vessel mass
                    let force_vec = forward * drive_force_mag;
                    
                    forces.apply_force_at_point(force_vec, wheel_tf.translation().as_dvec3());

                    let chassis_vel = forces.linear_velocity();
                    let chassis_ang_vel = forces.angular_velocity();
                    let hub_pos_world = wheel_tf.translation().as_dvec3();
                    let hub_vel = chassis_vel + chassis_ang_vel.cross(hub_pos_world - forces.position().0);

                    // Coulomb-like friction approximation
                    let normal_force = wheel.last_normal_force;
                    let friction_k = 1.1; // Baseline static friction coefficient
                    
                    let right = wheel_tf.right().as_dvec3();
                    let lateral_vel = hub_vel.dot(right);
                    
                    let lateral_friction_force = -lateral_vel * friction_k * normal_force * right;
                    forces.apply_force_at_point(lateral_friction_force, hub_pos_world);
                }
            }
        }
    }
}

/// Updates steering angle based on physical port state.
fn apply_wheel_steering(
    mut q_wheels: Query<(&WheelRaycast, &mut Transform)>,
    q_ports: Query<&lunco_core::architecture::PhysicalPort>,
    mut q_visual: Query<&mut Transform, Without<WheelRaycast>>,
) {
    for (wheel, mut transform) in q_wheels.iter_mut() {
        if let Ok(port) = q_ports.get(wheel.steer_port) {
            let target_angle = (port.value * 0.5) as f32; // Limit to +/- 30 degrees roughly
            transform.rotation = Quat::from_rotation_y(target_angle);
            
            if let Some(visual_entity) = wheel.visual_entity {
                if let Ok(mut visual_tf) = q_visual.get_mut(visual_entity) {
                    visual_tf.rotation = transform.rotation * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
                }
            }
        }
    }
}

/// Control Logic: Differential (Skid) mixing.
/// 
/// **Math**: `Left = Forward + Steer`, `Right = Forward - Steer`. 
/// Creates a torque differential that rotates the chassis.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component, Default)]
pub struct DifferentialDrive {
    /// Digital port identifier for left-side motors.
    pub left_port: String,
    /// Digital port identifier for right-side motors.
    pub right_port: String,
}

/// Control Logic: Ackermann Steering.
/// 
/// **Math**: `Drive = Forward`, `SteerAngle = Input * MaxAngle`. 
/// High-stability steering for vehicles with articulating front/rear axles.
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

/// Suspension configuration for joint-based (non-raycast) chassis.
///
/// **Why**: Some vehicles use physical collision wheels for higher fidelity, 
/// but still require emulated spring-damper logic for PrismaticJoints.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct Suspension {
    /// target static length of the strut.
    pub rest_length: f64,
    /// Stiffness (N/m).
    pub spring_k: f64,
    /// Dampening (Ns/m).
    pub damping_c: f64,
    /// Direction of extension.
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

/// Solves linear suspension equations forentities linked by joints.
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

/// Global command observer for all mobility-equipped vessels.
///
/// **Responsibility**: Translates high-level [CommandMessage]s into low-level 
/// [DigitalPort] signals. It handles the "Mixing" logic appropriate for the 
/// vessel's specific hardware configuration (Skid vs. Ackermann).
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

                    // Signal Multiplexing based on drive model
                    if fsw.brake_active {
                        // Hard brake override
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

        // Broadcast telemetry feedback for ground correlation
        commands.trigger(CommandResponse {
            command_id: cmd.id,
            status,
        });
    }
}


