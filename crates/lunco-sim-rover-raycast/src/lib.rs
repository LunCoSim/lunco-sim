use avian3d::prelude::*;
use avian3d::math::{Vector, Dir};
use bevy::prelude::*;
use bevy::ecs::relationship::Relationship;
use std::collections::HashMap;

use lunco_sim_core::architecture::{PhysicalPort, DigitalPort, Wire};
use lunco_sim_physics::{Layer, MotorActuator};
use lunco_sim_fsw::FlightSoftware;

pub struct LunCoSimRoverRaycastPlugin;

impl Plugin for LunCoSimRoverRaycastPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (
                apply_wheel_suspension,
                apply_wheel_drive,
                apply_wheel_steering,
            )
                .chain()
                .before(PhysicsSystems::Prepare),
        );
    }
}

#[derive(Component)]
pub struct WheelRaycast {
    pub suspension_port: Entity,
    pub drive_port: Entity,
    pub steer_port: Entity,
    pub rest_length: f32,
    pub spring_k: f32,
    pub damping_c: f32,
    pub wheel_radius: f32,
    pub visual_entity: Option<Entity>,
}

#[derive(Component)]
pub struct RoverVessel;

fn apply_wheel_suspension(
    q_wheels: Query<(
        &WheelRaycast,
        &RayHits,
        &Transform,
        &ChildOf,
    )>,
    mut q_chassis: Query<(Forces, &Position, &Rotation), With<RoverVessel>>,
    mut q_visual: Query<&mut Transform, (Without<WheelRaycast>, Without<RoverVessel>)>,
) {
    for (wheel, hits, wheel_tf, parent) in q_wheels.iter() {
        let parent_entity = Relationship::get(parent);
        if let Ok((mut forces, chassis_pos, chassis_rot)) = q_chassis.get_mut(parent_entity) {
            let mut closest_hit_dist = wheel.rest_length + wheel.wheel_radius;
            
            // Critical: Calculate world position from latest physics state to avoid GlobalTransform lag
            let world_pos = chassis_pos.0 + chassis_rot.0 * wheel_tf.translation.as_dvec3();
            let ray_dir_world = chassis_rot.0 * Vec3::NEG_Y.as_dvec3();
            
            if let Some(hit) = hits.iter_sorted().next() {
                let distance = hit.distance as f32;
                if distance < wheels_limit(wheel) {
                    closest_hit_dist = distance;
                    let compression = ((wheel.rest_length + wheel.wheel_radius) - distance).max(0.0);
                    let spring_force_mag = (compression * wheel.spring_k) as f64;
                    
                    let lin_vel = forces.linear_velocity();
                    let ang_vel = forces.angular_velocity();
                    
                    let velocity_at_wheel = lin_vel + ang_vel.cross(world_pos - chassis_pos.0);
                    
                    let relative_vel = velocity_at_wheel.dot(ray_dir_world); // Positive when compressing (moving down)
                    
                    // One-way damping: resist compression only
                    let damping_force_mag = (relative_vel * wheel.damping_c as f64).max(0.0);
                    let total_force_mag = (spring_force_mag + damping_force_mag).max(0.0);
                    
                    // Apply force at the hub's world position
                    let force_vec = hit.normal * total_force_mag;
                    forces.apply_force_at_point(force_vec, world_pos);
                }
            }

            if let Some(visual_entity) = wheel.visual_entity {
                if let Ok(mut visual_tf) = q_visual.get_mut(visual_entity) {
                    // ray caster origin is wheel_tf.translation + (0, 0.5, 0)
                    // relative Ground Y = (wheel_tf.y + 0.5) - closest_hit_dist
                    // We want the wheel center to be Ground Y + radius
                    let wheel_center_rel_y = (wheel_tf.translation.y + 0.5 - closest_hit_dist) + wheel.wheel_radius;
                    visual_tf.translation.y = wheel_center_rel_y;
                }
            }
        }
    }
}

fn wheels_limit(wheel: &WheelRaycast) -> f32 {
    wheel.rest_length + wheel.wheel_radius
}

fn apply_wheel_drive(
    q_wheels: Query<(
        &WheelRaycast,
        &GlobalTransform,
        &RayHits,
        &ChildOf,
    )>,
    q_ports: Query<&PhysicalPort>,
    mut q_chassis: Query<Forces, With<RoverVessel>>,
) {
    for (wheel, wheel_tf, hits, parent) in q_wheels.iter() {
        let parent_entity = Relationship::get(parent);
        if let Ok(mut forces) = q_chassis.get_mut(parent_entity) {
            if let Ok(port) = q_ports.get(wheel.drive_port) {
                if hits.iter().next().is_some() {
                    let forward = wheel_tf.forward().as_dvec3();
                    let drive_force_mag = (port.value * 1000.0) as f64;
                    let force_vec = forward * drive_force_mag;
                    forces.apply_force_at_point(force_vec, wheel_tf.translation().as_dvec3());
                }
            }
        }
    }
}

fn apply_wheel_steering(
    mut q_wheels: Query<(&WheelRaycast, &mut Transform)>,
    q_ports: Query<&PhysicalPort>,
    mut q_visual: Query<&mut Transform, Without<WheelRaycast>>,
) {
    for (wheel, mut transform) in q_wheels.iter_mut() {
        if let Ok(port) = q_ports.get(wheel.steer_port) {
            let target_angle = (port.value * 0.5) as f32;
            transform.rotation = Quat::from_rotation_y(target_angle);
            
            if let Some(visual_entity) = wheel.visual_entity {
                if let Ok(mut visual_tf) = q_visual.get_mut(visual_entity) {
                    visual_tf.rotation = transform.rotation * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
                }
            }
        }
    }
}

pub fn spawn_raycast_skid_rover(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    wheel_mesh: Handle<Mesh>,
    spawn_pos: Vec3,
    name: &str,
    color: Color,
) -> Entity {
    spawn_raycast_rover_internal(commands, meshes, materials, wheel_mesh, spawn_pos, name, color, false)
}

pub fn spawn_raycast_ackermann_rover(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    wheel_mesh: Handle<Mesh>,
    spawn_pos: Vec3,
    name: &str,
    color: Color,
) -> Entity {
    spawn_raycast_rover_internal(commands, meshes, materials, wheel_mesh, spawn_pos, name, color, true)
}

fn spawn_raycast_rover_internal(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    wheel_mesh: Handle<Mesh>,
    spawn_pos: Vec3,
    name: &str,
    color: Color,
    is_ackermann: bool,
) -> Entity {
    let chassis_width = 2.0_f32;
    let chassis_height = 0.5_f32;
    let chassis_length = 3.5_f32;

    let red_material = materials.add(StandardMaterial { base_color: Color::from(Srgba::RED), perceptual_roughness: 0.5, ..default() });
    let blue_material = materials.add(StandardMaterial { base_color: Color::from(Srgba::BLUE), perceptual_roughness: 0.5, ..default() });

    let rover_entity = commands
        .spawn((
            Name::new(name.to_string()),
            RoverVessel,
            lunco_sim_core::Vessel,
            Mesh3d(meshes.add(Cuboid::new(chassis_width, chassis_height, chassis_length))),
            MeshMaterial3d(materials.add(color)),
            Transform::from_translation(spawn_pos),
            RigidBody::Dynamic,
            Collider::cuboid(chassis_width as f64, chassis_height as f64, chassis_length as f64),
            CollisionLayers::new(Layer::RoverChassis, [Layer::Default]),
            Mass(1000.0),
        ))
        .id();

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

    let wheel_configs = [
        ("FR", Vec3::new(chassis_width * 0.6, -0.4, chassis_length * 0.4), false, true),
        ("FL", Vec3::new(-chassis_width * 0.6, -0.4, chassis_length * 0.4), true, true),
        ("RR", Vec3::new(chassis_width * 0.6, -0.4, -chassis_length * 0.4), false, false),
        ("RL", Vec3::new(-chassis_width * 0.6, -0.4, -chassis_length * 0.4), true, false),
    ];

    let wheel_rot = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);

    for (label, rel_pos, is_left, is_front) in wheel_configs {
        let drive_port = commands.spawn(PhysicalPort { value: 0.0 }).id();
        let steer_port = commands.spawn(PhysicalPort { value: 0.0 }).id();
        let susp_port = commands.spawn(PhysicalPort { value: 0.0 }).id();

        let digital_source = if is_left { drive_l_digital } else { drive_r_digital };
        commands.spawn(Wire { source: digital_source, target: drive_port, scale: 1.0 });
        if is_front && is_ackermann {
            commands.spawn(Wire { source: steer_digital, target: steer_port, scale: 1.0 });
        }

        let wheel_material = if is_front { red_material.clone() } else { blue_material.clone() };

        let visual_wheel = commands.spawn((
            Name::new(format!("{}_visual", label)),
            Mesh3d(wheel_mesh.clone()),
            MeshMaterial3d(wheel_material),
            Transform::from_translation(rel_pos).with_rotation(wheel_rot), 
            ChildOf(rover_entity),
        )).id();

        commands.spawn((
            Name::new(format!("{}_{}", name, label)),
            WheelRaycast {
                suspension_port: susp_port,
                drive_port,
                steer_port: if is_front { steer_port } else { Entity::PLACEHOLDER },
                rest_length: 0.4,
                spring_k: 30000.0,
                damping_c: 10000.0, 
                wheel_radius: 0.4,
                visual_entity: Some(visual_wheel),
            },
            RayCaster::new(Vector::new(0.0, 0.5, 0.0), Dir3::NEG_Y)
                .with_max_distance(1.2)
                .with_solidness(true)
                .with_query_filter(SpatialQueryFilter::from_excluded_entities([rover_entity])),
            Transform::from_translation(rel_pos),
            ChildOf(rover_entity),
        ));
    }

    rover_entity
}
