use avian3d::prelude::*;
use avian3d::math::Vector;
use bevy::prelude::*;
use bevy::ecs::relationship::Relationship;
use big_space::prelude::CellCoord;
use std::collections::HashMap;

use lunco_core::architecture::{PhysicalPort, DigitalPort, Wire};
use lunco_physics::Layer;
use lunco_fsw::FlightSoftware;

pub struct LunCoRoverRaycastPlugin;

impl Plugin for LunCoRoverRaycastPlugin {
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
    pub rest_length: f64,
    pub spring_k: f64,
    pub damping_c: f64,
    pub wheel_radius: f64,
    pub visual_entity: Option<Entity>,
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

#[derive(Component)]
pub struct RoverVessel;

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
        let parent_entity = Relationship::get(parent);
        if let Ok(mut forces) = q_chassis.get_mut(parent_entity) {
            let mut closest_hit_dist = wheel.rest_length + wheel.wheel_radius;
            
            // Critical: Calculate world position from latest physics state to avoid GlobalTransform lag
            let world_pos = forces.position().0 + forces.rotation().0 * wheel_tf.translation.as_dvec3();
            let ray_dir_world = forces.rotation().0 * Vec3::NEG_Y.as_dvec3();
            
            if let Some(hit) = hits.iter_sorted().next() {
                let distance = hit.distance;
                if distance < wheels_limit(&wheel) {
                    closest_hit_dist = distance;
                    let compression = ((wheel.rest_length + wheel.wheel_radius) - distance).max(0.0);
                    let spring_force_mag = compression * wheel.spring_k;
                    
                    let lin_vel = forces.linear_velocity();
                    let ang_vel = forces.angular_velocity();
                    
                    let velocity_at_wheel = lin_vel + ang_vel.cross(world_pos - forces.position().0);
                    
                    let relative_vel = velocity_at_wheel.dot(ray_dir_world); // Positive when compressing (moving down)
                    
                    // One-way damping: resist compression only
                    let damping_force_mag = (relative_vel * wheel.damping_c).max(0.0);
                    let total_force_mag = (spring_force_mag + damping_force_mag).max(0.0);
                    
                    // Apply force at the hub's world position
                    let force_vec = hit.normal * total_force_mag;
                    forces.apply_force_at_point(force_vec, world_pos);
                    
                    // Store normal force for friction calculation
                    wheel.last_normal_force = total_force_mag;
                } else {
                    wheel.last_normal_force = 0.0;
                }
            } else {
                wheel.last_normal_force = 0.0;
            }

            if let Some(visual_entity) = wheel.visual_entity {
                if let Ok(mut visual_tf) = q_visual.get_mut(visual_entity) {
                    // ray caster origin is wheel_tf.translation + (0, 0.5, 0)
                    // relative Ground Y = (wheel_tf.y + 0.5) - closest_hit_dist
                    // We want the wheel center to be Ground Y + radius
                    let wheel_center_rel_y = (wheel_tf.translation.y as f64 + 0.5 - closest_hit_dist) + wheel.wheel_radius;
                    visual_tf.translation.y = wheel_center_rel_y as f32;
                }
            }
        }
    }
}

fn wheels_limit(wheel: &WheelRaycast) -> f64 {
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
                    let drive_force_mag = port.value as f64 * 12000.0; // Significant boost for 1k kg
                    let force_vec = forward * drive_force_mag;
                    
                    // Main drive force
                    forces.apply_force_at_point(force_vec, wheel_tf.translation().as_dvec3());

                    // --- LATERAL FRICTION (The "Grip" from bevy_car) ---
                    let chassis_vel = forces.linear_velocity();
                    let chassis_ang_vel = forces.angular_velocity();
                    let hub_pos_world = wheel_tf.translation().as_dvec3();
                    let hub_vel = chassis_vel + chassis_ang_vel.cross(hub_pos_world - forces.position().0);

                    // Friction is proportional to how hard the tire is pressing into ground (Normal Force)
                    let normal_force = wheel.last_normal_force;
                    let friction_k = 1.1; // "Stickiness" coefficient
                    
                    let right = wheel_tf.right().as_dvec3();
                    let lateral_vel = hub_vel.dot(right);
                    
                    let lateral_friction_force = -lateral_vel * friction_k * normal_force * right;
                    forces.apply_force_at_point(lateral_friction_force, hub_pos_world);
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
    wheel_mesh: Handle<Mesh>,
    spawn_pos: Vec3,
    name: &str,
    color: Color,
) -> Entity {
    spawn_raycast_rover_internal(commands, wheel_mesh, spawn_pos, name, color, false)
}

pub fn spawn_raycast_ackermann_rover(
    commands: &mut Commands,
    wheel_mesh: Handle<Mesh>,
    spawn_pos: Vec3,
    name: &str,
    color: Color,
) -> Entity {
    spawn_raycast_rover_internal(commands, wheel_mesh, spawn_pos, name, color, true)
}

fn spawn_raycast_rover_internal(
    commands: &mut Commands,
    _wheel_mesh: Handle<Mesh>,
    spawn_pos: Vec3,
    name: &str,
    _color: Color,
    is_ackermann: bool,
) -> Entity {
    let chassis_width = 2.0_f64;
    let chassis_height = 0.5_f64;
    let chassis_length = 3.5_f64;

    // No materials in tests to avoid shader panics

    let rover_entity = commands.spawn((
        Name::new(name.to_string()),
        RoverVessel,
        lunco_core::Vessel,
        Transform::from_translation(spawn_pos),
        CellCoord::default(),
        RigidBody::Dynamic,
        Collider::cuboid(chassis_width, chassis_height, chassis_length),
        CollisionLayers::new(Layer::RoverChassis, [Layer::Default]),
        Mass(1000.0),
    )).id();

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

    let wheel_configs = [
        ("FR", Vec3::new((chassis_width * 0.6) as f32, -0.4, (chassis_length * 0.4) as f32), false, true),
        ("FL", Vec3::new((-chassis_width * 0.6) as f32, -0.4, (chassis_length * 0.4) as f32), true, true),
        ("RR", Vec3::new((chassis_width * 0.6) as f32, -0.4, (-chassis_length * 0.4) as f32), false, false),
        ("RL", Vec3::new((-chassis_width * 0.6) as f32, -0.4, (-chassis_length * 0.4) as f32), true, false),
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

        let visual_wheel_builder = commands.spawn((
            Name::new(format!("{}_visual", label)),
            Transform::from_translation(rel_pos).with_rotation(wheel_rot), 
            CellCoord::default(),
            ChildOf(rover_entity),
        ));

        let wheel_entity = visual_wheel_builder.id();

        #[cfg(not(test))]
        {
            let wheel_mesh_handle = _wheel_mesh.clone();
            commands.queue(move |world: &mut World| {
                if world.contains_resource::<Assets<Mesh>>() && world.contains_resource::<Assets<StandardMaterial>>() {
                    world.resource_scope::<Assets<Mesh>, _>(|world, _mesh_assets| {
                        let mut material_assets = world.resource_mut::<Assets<StandardMaterial>>();
                        let color_to_use = if is_front { Color::from(Srgba::RED) } else { Color::from(Srgba::BLUE) };
                        let material = material_assets.add(StandardMaterial { base_color: color_to_use, perceptual_roughness: 0.5, ..default() });
                        if let Ok(mut entity) = world.get_entity_mut(wheel_entity) {
                            entity.insert((Mesh3d(wheel_mesh_handle), MeshMaterial3d(material)));
                        }
                    });
                }
            });
        }

        commands.spawn((
            Name::new(format!("{}_{}", name, label)),
            WheelRaycast {
                suspension_port: susp_port,
                drive_port,
                steer_port: if is_front { steer_port } else { Entity::PLACEHOLDER },
                rest_length: 0.4,
                spring_k: 8000.0,   // Much softer, more travel
                damping_c: 2800.0,  // Tuned for 1000kg chassis
                wheel_radius: 0.4,
                visual_entity: Some(wheel_entity),
                last_normal_force: 0.0,
            },
            RayCaster::new(Vector::new(0.0, 0.5, 0.0), Dir3::NEG_Y)
                .with_max_distance(1.2)
                .with_solidness(true)
                .with_query_filter(SpatialQueryFilter::from_excluded_entities([rover_entity])),
            Transform::from_translation(rel_pos),
            CellCoord::default(),
            ChildOf(rover_entity),
        ));
    }

    rover_entity
}
