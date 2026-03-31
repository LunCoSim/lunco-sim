use bevy::prelude::*;
use bevy::math::{DVec3, DQuat};
use avian3d::prelude::*;
use lunco_sim_core::architecture::{PhysicalPort, DigitalPort, Wire};
use lunco_sim_core::{Vessel, RoverVessel};
use lunco_sim_fsw::FlightSoftware;
use std::collections::HashMap;

pub struct LunCoSimPhysicsPlugin;

impl Plugin for LunCoSimPhysicsPlugin {
    fn build(&self, app: &mut App) {
        println!("LunCoSimPhysicsPlugin Build Called");
        app.add_systems(FixedUpdate, (
            apply_motor_torques, 
            apply_brakes, 
            update_physics_sensors,
            suspension_system,
        ).chain());
    }
}

#[derive(Component)]
pub struct Suspension {
    pub rest_length: f64,
    pub spring_k: f64,
    pub damping_c: f64,
    pub local_axis: DVec3,
}

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
            
            // Only push apart when compressed
            let compression: f64 = (susp.rest_length - current_length).max(0.0);
            let spring_force_mag: f64 = compression * susp.spring_k;
            
            let closing_speed: f64 = rel_vel;
            let damping_force_mag: f64 = (closing_speed * susp.damping_c).max(0.0);
            
            let total_force_mag: f64 = spring_force_mag + damping_force_mag;
            
            static mut LOG_COUNT: i32 = 0;
            unsafe {
                if LOG_COUNT < 20 {
                    println!("TICK {:?} - Susp Force: {} | Compression: {} | Length: {} | Pos1 Y: {} | Pos2 Y: {} | Anch1_Loc_None: {}", LOG_COUNT, total_force_mag, compression, current_length, pos1.y, pos2.y, joint.local_anchor1().is_none());
                    LOG_COUNT += 1;
                }
            }

            if !total_force_mag.is_finite() { continue; }
            
            // Safety: Cap force to prevent numerical explosion
            let total_force_mag = total_force_mag.clamp(0.0, 100_000.0);
            
            let force_vec: DVec3 = world_axis * total_force_mag;
            
            forces1.apply_force_at_point(force_vec, anchor1_world);
            forces2.apply_force_at_point(-force_vec, anchor2_world);
        }
    }
}

#[derive(Component)]
pub struct MotorActuator {
    pub port_entity: Entity,
    pub axis: DVec3,
}

fn apply_motor_torques(
    q_ports: Query<&PhysicalPort>,
    mut q_motors: Query<(&MotorActuator, Forces)>,
) {
    static mut ONCE: bool = false;
    unsafe { if !ONCE { println!("Physics Heartbeat Running"); ONCE = true; } }
    for (motor, mut forces) in q_motors.iter_mut() {
        if let Ok(port) = q_ports.get(motor.port_entity) {
            let torque_mag = port.value as f64;
            forces.apply_local_torque(motor.axis * torque_mag);
        }
    }
}

#[derive(Component)]
pub struct BrakeActuator {
    pub port_entity: Entity,
    pub max_force: f64,
}

fn apply_brakes(
    q_ports: Query<&PhysicalPort>,
    mut q_brakes: Query<(&BrakeActuator, &mut AngularVelocity, &mut LinearVelocity)>,
) {
    for (brake, mut ang_vel, mut lin_vel) in q_brakes.iter_mut() {
        if let Ok(port) = q_ports.get(brake.port_entity) {
            let brake_factor = (1.0 - (port.value as f64 / brake.max_force).clamp(0.0, 1.0)).powf(2.0);
            ang_vel.0 *= brake_factor;
            lin_vel.0 *= brake_factor;
        }
    }
}

#[derive(Component)]
pub struct AngularVelocitySensor {
    pub port_entity: Entity,
    pub axis: DVec3,
}

fn update_physics_sensors(
    q_sensors: Query<(&AngularVelocitySensor, &AngularVelocity)>,
    mut q_ports: Query<&mut PhysicalPort>,
) {
    for (sensor, velocity) in q_sensors.iter() {
        if let Ok(mut port) = q_ports.get_mut(sensor.port_entity) {
            port.value = velocity.0.dot(sensor.axis) as f32;
        }
    }
}

#[derive(PhysicsLayer, Default)]
pub enum Layer {
    #[default]
    Default,
    RoverChassis,
    RoverWheel,
}

fn spawn_joint_rover_internal(
    commands: &mut Commands,
    _wheel_mesh: Handle<Mesh>,
    spawn_pos: Vec3,
    name: &str,
    _color: Color,
    steering_type: SteeringType,
) -> Entity {
    let chassis_width = 1.8_f64;
    let chassis_height = 0.5_f64;
    let chassis_length = 3.0_f64;
    let wheel_radius = 0.5_f64;
    let wheel_width = 0.4_f64;
    let suspension_travel = 0.3_f64; // Total vertical travel

    // No materials in tests to avoid shader panics

    let mut rover_builder = commands.spawn((
        Name::new(name.to_string()),
        RoverVessel,
        Vessel,
        Transform::from_translation(spawn_pos),
        RigidBody::Dynamic,
        Collider::cuboid(chassis_width, chassis_height, chassis_length),
        CollisionLayers::new(Layer::RoverChassis, [Layer::Default]),
        Friction::new(0.5),
        Mass(1000.0), 
        CenterOfMass(Vec3::new(0.0, -0.2, 0.0)),
        LinearDamping(0.2), 
        AngularDamping(0.5),
    ));
    let rover_entity = rover_builder.id();
    
    #[cfg(not(test))]
    {
        let color_val = _color;
        commands.queue(move |world: &mut World| {
            if world.contains_resource::<Assets<Mesh>>() && world.contains_resource::<Assets<StandardMaterial>>() {
                world.resource_scope::<Assets<Mesh>, _>(|world, mut mesh_assets| {
                    let mut material_assets = world.resource_mut::<Assets<StandardMaterial>>();
                    let mesh = mesh_assets.add(Cuboid::new(chassis_width as f32, chassis_height as f32, chassis_length as f32));
                    let material = material_assets.add(StandardMaterial::from(color_val));
                    if let Ok(mut entity) = world.get_entity_mut(rover_entity) {
                        entity.insert((Mesh3d(mesh), MeshMaterial3d(material)));
                    }
                });
            }
        });
    }

    let drive_l_digital = commands.spawn((Name::new(format!("{}_drive_l_reg", name)), DigitalPort::default())).id();
    let drive_r_digital = commands.spawn((Name::new(format!("{}_drive_r_reg", name)), DigitalPort::default())).id();
    let steer_digital = commands.spawn((Name::new(format!("{}_steer_reg", name)), DigitalPort::default())).id();
    let brake_digital = commands.spawn((Name::new(format!("{}_brake_reg", name)), DigitalPort::default())).id();

    let mut port_map = HashMap::new();
    port_map.insert("drive_left".to_string(), drive_l_digital);
    port_map.insert("drive_right".to_string(), drive_r_digital);
    port_map.insert("steering".to_string(), steer_digital);
    port_map.insert("brake".to_string(), brake_digital);
    commands.entity(rover_entity).insert(FlightSoftware { port_map, brake_active: false });

    let wheel_offset_y = -0.6; // Significantly increased from -0.1 to provide better clearance and suspension travel.

    let wheel_configs = [
        ("fl", Vec3::new(-1.2, wheel_offset_y, 1.2), true, true), 
        ("rl", Vec3::new(-1.2, wheel_offset_y, -1.2), true, false), 
        ("fr", Vec3::new(1.2, wheel_offset_y, 1.2), false, true),
        ("rr", Vec3::new(1.2, wheel_offset_y, -1.2), false, false),
    ];

    let steer_port = commands.spawn((Name::new(format!("{}_port_steer", name)), PhysicalPort::default())).id();
    commands.spawn(Wire { source: steer_digital, target: steer_port, scale: 10.0 });

    let wheel_tilt = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let wheel_tilt_d = DQuat::from_xyzw(
        wheel_tilt.x as f64,
        wheel_tilt.y as f64,
        wheel_tilt.z as f64,
        wheel_tilt.w as f64,
    );

    for (label, rel_pos, is_left, is_front) in wheel_configs {
        let digital_source = if is_left { drive_l_digital } else { drive_r_digital };
        
        let motor_port = commands.spawn((Name::new(format!("{}_port_{}_drive", name, label)), PhysicalPort::default())).id();
        let brake_port = commands.spawn((Name::new(format!("{}_port_{}_brake", name, label)), PhysicalPort::default())).id();
        commands.spawn(Wire { source: digital_source, target: motor_port, scale: 200.0 });
        commands.spawn(Wire { source: brake_digital, target: brake_port, scale: 1.0 });

        let wheel_builder = commands.spawn((
            Name::new(format!("{}_wheel_{}", name, label)),
            Transform::from_translation(spawn_pos + rel_pos).with_rotation(wheel_tilt),
            RigidBody::Dynamic,
            Collider::cylinder(wheel_radius, wheel_width),
            CollisionLayers::new(Layer::RoverWheel, [Layer::Default]),
            Friction::new(1.0), 
            Mass(20.0), 
            LinearDamping(0.5), 
            AngularDamping(2.0),
            MotorActuator { port_entity: motor_port, axis: DVec3::Y },
            BrakeActuator { port_entity: brake_port, max_force: 32767.0 },
        ));
        let wheel_entity = wheel_builder.id();

        #[cfg(not(test))]
        {
            let color_to_use = if is_front { Color::from(Srgba::RED) } else { Color::from(Srgba::BLUE) };
            let wheel_mesh_handle = _wheel_mesh.clone();
            commands.queue(move |world: &mut World| {
                if world.contains_resource::<Assets<Mesh>>() && world.contains_resource::<Assets<StandardMaterial>>() {
                    world.resource_scope::<Assets<StandardMaterial>, _>(|world, mut material_assets| {
                        let material = material_assets.add(StandardMaterial { base_color: color_to_use, perceptual_roughness: 0.5, ..default() });
                        if let Ok(mut entity) = world.get_entity_mut(wheel_entity) {
                            entity.insert((Mesh3d(wheel_mesh_handle), MeshMaterial3d(material)));
                        }
                    });
                }
            });
        }

        // Intermediate hub for steering and/or suspension
        let hub_entity = commands.spawn((
            Name::new(format!("{}_hub_{}", name, label)),
            RigidBody::Dynamic, 
            Mass(10.0), 
            Collider::sphere(0.05),
            CollisionLayers::from_bits(0, 0),
            Transform::from_translation(spawn_pos + rel_pos),
        )).id();


        // Chassis to Hub: Suspension (Prismatic)
        commands.spawn((
            PrismaticJoint::new(rover_entity, hub_entity)
                .with_local_anchor1(rel_pos.as_dvec3())
                .with_local_anchor2(DVec3::ZERO)
                .with_slider_axis(DVec3::Y)
                .with_limits(-suspension_travel, suspension_travel),
            Suspension {
                rest_length: 0.4,   // Increased to ensure compression even at rest
                spring_k: 50000.0,  // Increased to handle big mass
                damping_c: 2000.0,  // Increased for stability
                local_axis: DVec3::Y,
            }
        ));
        
        // Hub to Wheel: Drive (Revolute) + Optional Steering (Hub rotation)
        if is_front && steering_type == SteeringType::Ackermann {
            let steering_hub = commands.spawn((
                Name::new(format!("{}_steer_hub_{}", name, label)),
                RigidBody::Dynamic, 
                Mass(5.0), 
                Collider::sphere(0.04), // Small non-colliding trigger for inertia
                CollisionLayers::from_bits(0, 0),
                Transform::from_translation(spawn_pos + rel_pos),
                MotorActuator { port_entity: steer_port, axis: DVec3::Y },
            )).id();

            commands.spawn(RevoluteJoint::new(hub_entity, steering_hub)
                .with_local_anchor1(DVec3::ZERO)
                .with_local_anchor2(DVec3::ZERO)
                .with_hinge_axis(DVec3::Y)
                .with_angle_limits(-0.6, 0.6));
                
            commands.spawn(RevoluteJoint::new(steering_hub, wheel_entity)
                .with_local_anchor1(DVec3::ZERO)
                .with_local_anchor2(DVec3::ZERO)
                .with_hinge_axis(DVec3::X)
                .with_local_basis2(wheel_tilt_d.inverse()));
        } else {
            commands.spawn(RevoluteJoint::new(hub_entity, wheel_entity)
                .with_local_anchor1(DVec3::ZERO)
                .with_local_anchor2(DVec3::ZERO)
                .with_hinge_axis(DVec3::X)
                .with_local_basis2(wheel_tilt_d.inverse()));
        }
    }
    rover_entity
}

#[derive(PartialEq, Eq)]
pub enum SteeringType { Skid, Ackermann }

pub fn spawn_joint_skid_rover(commands: &mut Commands, wheel_mesh: Handle<Mesh>, spawn_pos: Vec3, name: &str, color: Color) -> Entity {
    spawn_joint_rover_internal(commands, wheel_mesh, spawn_pos, name, color, SteeringType::Skid)
}

pub fn spawn_joint_ackermann_rover(commands: &mut Commands, wheel_mesh: Handle<Mesh>, spawn_pos: Vec3, name: &str, color: Color) -> Entity {
    spawn_joint_rover_internal(commands, wheel_mesh, spawn_pos, name, color, SteeringType::Ackermann)
}
