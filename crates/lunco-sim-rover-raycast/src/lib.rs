use avian3d::prelude::*;
use avian3d::math::{Vector, Dir};
use bevy::prelude::*;
use bevy::ecs::relationship::Relationship;

use lunco_sim_core::architecture::PhysicalPort;
use lunco_sim_physics::Layer;

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
}

#[derive(Component)]
pub struct RoverVessel;

fn apply_wheel_suspension(
    q_wheels: Query<(
        &WheelRaycast,
        &RayHits,
        &GlobalTransform,
        &ChildOf,
    )>,
    mut q_chassis: Query<Forces, With<RoverVessel>>,
) {
    for (wheel, hits, wheel_tf, parent) in q_wheels.iter() {
        let parent_entity = Relationship::get(parent);
        if let Ok(mut forces) = q_chassis.get_mut(parent_entity) {
            for hit in hits.iter() {
                let distance = hit.distance as f32;
                if distance < wheel.rest_length + wheel.wheel_radius {
                    let compression = (wheel.rest_length + wheel.wheel_radius) - distance;
                    let spring_force_mag = (compression * wheel.spring_k) as f64;
                    let damping_force_mag = (wheel.damping_c * 0.1) as f64;

                    let total_force_mag = spring_force_mag + damping_force_mag;
                    let force_vec = hit.normal * total_force_mag;

                    let ray_origin = wheel_tf.translation().as_dvec3();
                    let ray_dir_world = (wheel_tf.rotation() * Vec3::NEG_Y).as_dvec3();
                    let hit_point = ray_origin + (ray_dir_world * hit.distance);
                    
                    forces.apply_force_at_point(force_vec, hit_point);
                }
            }
        }
    }
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
                    let drive_force_mag = (port.value * 5000.0) as f64;
                    let force_vec = forward * drive_force_mag;

                    forces.apply_force_at_point(
                        force_vec, 
                        wheel_tf.translation().as_dvec3()
                    );
                }
            }
        }
    }
}

fn apply_wheel_steering(
    mut q_wheels: Query<(&WheelRaycast, &mut Transform)>,
    q_ports: Query<&PhysicalPort>,
) {
    for (wheel, mut transform) in q_wheels.iter_mut() {
        if let Ok(port) = q_ports.get(wheel.steer_port) {
            let target_angle = (port.value * 0.5) as f32;
            transform.rotation = Quat::from_rotation_y(target_angle);
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
        ))
        .id();

    let wheel_configs = [
        ("FR", Vec3::new(chassis_width * 0.6, -0.2, chassis_length * 0.4)),
        ("FL", Vec3::new(-chassis_width * 0.6, -0.2, chassis_length * 0.4)),
        ("RR", Vec3::new(chassis_width * 0.6, -0.2, -chassis_length * 0.4)),
        ("RL", Vec3::new(-chassis_width * 0.6, -0.2, -chassis_length * 0.4)),
    ];

    for (label, rel_pos) in wheel_configs {
        let drive_port = commands.spawn(PhysicalPort { value: 0.0 }).id();
        let steer_port = commands.spawn(PhysicalPort { value: 0.0 }).id();
        let susp_port = commands.spawn(PhysicalPort { value: 0.0 }).id();

        commands.spawn((
            Name::new(format!("{}_{}", name, label)),
            WheelRaycast {
                suspension_port: susp_port,
                drive_port,
                steer_port: if is_ackermann && label.starts_with('F') { steer_port } else { Entity::PLACEHOLDER },
                rest_length: 0.6,
                spring_k: 30000.0,
                damping_c: 2000.0,
                wheel_radius: 0.4,
            },
            RayCaster::new(Vector::ZERO, Dir::NEG_Y)
                .with_max_distance(1.5)
                .with_solidness(true),
            Transform::from_translation(rel_pos),
            ChildOf(rover_entity),
        ));

        commands.spawn((
            Mesh3d(wheel_mesh.clone()),
            MeshMaterial3d(materials.add(Color::BLACK)),
            Transform::from_translation(rel_pos + Vec3::new(0.0, -0.6, 0.0)),
            ChildOf(rover_entity),
        ));
    }

    rover_entity
}
