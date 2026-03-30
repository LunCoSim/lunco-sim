use bevy::prelude::*;
use avian3d::prelude::*;
use lunco_sim_physics::MotorActuator;
use lunco_sim_core::architecture::{PhysicalPort, DigitalPort, Wire};
use lunco_sim_core::{Vessel, RoverVessel};
use lunco_sim_fsw::FlightSoftware;
use std::collections::HashMap;

pub struct LunCoSimRoverRaycastPlugin;

impl Plugin for LunCoSimRoverRaycastPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (
                update_wheel_state,
                apply_wheel_suspension,
                apply_wheel_friction,
                apply_wheel_drive,
                update_wheel_steering,
                visualize_wheel_suspension,
            )
                .chain(),
        );
    }
}

#[derive(Component)]
pub struct WheelVisual;

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct RaycastWheel {
    pub radius: f32,
    pub stiffness: f32,
    pub damping: f32,
    pub friction: f32,
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct SteeringActuator {
    pub port_entity: Entity,
    pub max_angle: f32,
}

#[derive(Component)]
pub struct BrakeActuator {
    pub port_entity: Entity,
}

impl Default for RaycastWheel {
    fn default() -> Self {
        Self {
            radius: 0.5,
            stiffness: 15000.0,
            damping: 2000.0,
            friction: 0.6,
        }
    }
}

#[derive(Component, Default, Debug, Reflect)]
#[reflect(Component)]
pub struct RaycastWheelState {
    pub compression: f32,
    pub compression_velocity: f32,
    pub surface_point: Vec3,
    pub surface_normal: Vec3,
    pub velocity: Vec3,
    pub normal_force: f32,

    pub previous_compression: Option<f32>,
    pub previous_surface_point: Option<Vec3>,
}

fn update_wheel_state(
    mut wheels: Query<(&RaycastWheel, &mut RaycastWheelState, &RayHits, &RayCaster)>,
) {
    for (wheel, mut state, ray_hits, ray) in wheels.iter_mut() {
        let ray_hit = ray_hits.get(0);
        let distance = ray_hit.map_or(wheel.radius, |hit| hit.distance);
        let hit_normal = ray_hit.map_or(Vec3::Y, |hit| hit.normal);

        let hit_point = ray.global_origin() + distance * ray.global_direction();
        let compression = (wheel.radius - distance).max(0.0);

        state.surface_point = hit_point;
        state.surface_normal = hit_normal;
        state.compression = compression;
    }
}

fn apply_wheel_suspension(
    mut wheels: Query<(&RaycastWheel, &mut RaycastWheelState, &GlobalTransform, &ChildOf)>,
    mut parents: Query<(Forces, &CenterOfMass, &Transform), With<RigidBody>>,
) {
    for (wheel, mut state, global_transform, child_of) in wheels.iter_mut() {
        if let Ok((mut parent_forces, com, parent_tf)) = parents.get_mut(child_of.0) {
            // 1. Calculate velocity of the wheel center
            // We use the parent's velocity components for stability
            let world_com = parent_tf.transform_point(com.0);
            let wheel_pos = global_transform.translation();
            let relative_pos = wheel_pos - world_com;
            
            let wheel_velocity = parent_forces.linear_velocity() + parent_forces.angular_velocity().cross(relative_pos);
            
            // 2. Compression velocity is the rate of change of compression
            // It is the component of the wheel velocity towards the ground surface normal
            let compression_velocity = -wheel_velocity.dot(state.surface_normal);
            state.compression_velocity = compression_velocity;

            // 3. Spring and Damping forces
            let spring_force = wheel.stiffness * state.compression;
            let damping_force = wheel.damping * compression_velocity;
            let total_force_mag = (spring_force + damping_force).max(0.0);

            let force = total_force_mag * state.surface_normal;

            // Apply the force to the parent at the wheel's position
            parent_forces.apply_force_at_point(force, wheel_pos);

            state.normal_force = total_force_mag;
            state.velocity = wheel_velocity;
        }
    }
}

fn apply_wheel_friction(
    wheels: Query<(Entity, &RaycastWheel, &RaycastWheelState, &GlobalTransform, &ChildOf, Option<&BrakeActuator>)>,
    mut parents: Query<(Forces, &CenterOfMass, &Transform), With<RigidBody>>,
    ports: Query<&PhysicalPort>,
) {
    for (_entity, wheel, state, global_transform, child_of, maybe_brake) in wheels.iter() {
        if let Ok((mut parent_forces, com, parent_tf)) = parents.get_mut(child_of.0) {
            // 1. Calculate velocity of the contact point on the wheel
            let world_com = parent_tf.transform_point(com.0);
            let relative_pos = state.surface_point - world_com;
            let contact_velocity = parent_forces.linear_velocity() + parent_forces.angular_velocity().cross(relative_pos);

            let max_friction = wheel.friction * state.normal_force;
            if max_friction <= 0.0 { continue; }

            // 2. Lateral friction (sideways)
            let right = global_transform.right();
            let lateral_velocity = contact_velocity.dot(right.as_vec3());
            
            // Lateral stiffness determines how quickly friction reaches its maximum
            let lateral_stiffness = 10.0; 
            let lateral_force_mag = (lateral_velocity * lateral_stiffness * state.normal_force).clamp(-max_friction, max_friction);
            let lateral_force = -lateral_force_mag * right;

            // 3. Longitudinal friction (rolling resistance / braking)
            let forward = global_transform.forward();
            let forward_velocity = contact_velocity.dot(forward.as_vec3());
            
            let rolling_resistance = 0.1;
            let longitudinal_stiffness = 10.0;
            let longitudinal_force_mag = (forward_velocity * longitudinal_stiffness * state.normal_force * rolling_resistance).clamp(-max_friction, max_friction);
            let longitudinal_force = -longitudinal_force_mag * forward;

            let resistance_torque = lateral_force + longitudinal_force;

            // 4. Braking Force
            if let Some(brake) = maybe_brake {
                if let Ok(port) = ports.get(brake.port_entity) {
                    if port.value > 0.0 {
                        let velocity_dir = contact_velocity.normalize_or_zero();
                        let brake_mag = (port.value).clamp(0.0, max_friction);
                        let brake_force = -velocity_dir * brake_mag;
                        
                        parent_forces.apply_force_at_point(brake_force, state.surface_point);
                    }
                }
            }

            parent_forces.apply_force_at_point(resistance_torque, state.surface_point);
        }
    }
}

fn update_wheel_steering(
    mut wheels: Query<(&mut Transform, &SteeringActuator)>,
    ports: Query<&PhysicalPort>,
) {
    for (mut tf, steer) in wheels.iter_mut() {
        if let Ok(port) = ports.get(steer.port_entity) {
            // port.value is roughly -1.0 to 1.0 (after scaling)
            let angle = port.value * steer.max_angle;
            tf.rotation = Quat::from_rotation_y(angle);
        }
    }
}

fn visualize_wheel_suspension(
    wheels: Query<(&RaycastWheelState, &Children)>,
    mut visuals: Query<&mut Transform, (With<WheelVisual>, Without<RaycastWheelState>)>,
) {
    for (state, children) in wheels.iter() {
        for &child in children {
            if let Ok(mut transform) = visuals.get_mut(child) {
                // Move the visual mesh up based on compression
                transform.translation = Vec3::new(0.0, state.compression, 0.0);
            }
        }
    }
}

fn apply_wheel_drive(
    wheels: Query<(&RaycastWheelState, &GlobalTransform, &ChildOf, &MotorActuator)>,
    mut parents: Query<(Forces, &CenterOfMass, &Transform), With<RigidBody>>,
    ports: Query<&PhysicalPort>,
) {
    for (state, global_transform, child_of, motor) in wheels.iter() {
        if let Ok(port) = ports.get(motor.port_entity) {
            if let Ok((mut parent_forces, _, _)) = parents.get_mut(child_of.0) {
                let forward = global_transform.forward();

                // Port value is interpreted as force magnitude for now
                let drive_force = (port.value * forward.as_vec3()).clamp_length_max(20000.0);

                if state.compression > 0.0 {
                    parent_forces.apply_force_at_point(drive_force, state.surface_point);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_build() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(PhysicsPlugins::default());
        app.add_plugins(LunCoSimRoverRaycastPlugin);
    }

    #[test]
    fn test_raycast_rover_spawn() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_resource::<Assets<Mesh>>();
        app.init_resource::<Assets<StandardMaterial>>();
        
        let wheel_mesh = {
            let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
            meshes.add(Cylinder::new(0.5, 0.4))
        };

        let rover_entity = {
            use bevy::ecs::system::SystemState;
            let mut system_state: SystemState<(
                Commands,
                ResMut<Assets<Mesh>>,
                ResMut<Assets<StandardMaterial>>,
            )> = SystemState::new(app.world_mut());
            let (mut commands, mut meshes, mut materials) = system_state.get_mut(app.world_mut());
            
            let entity = spawn_raycast_rover(
                &mut commands,
                &mut meshes,
                &mut materials,
                wheel_mesh,
                Vec3::new(0.0, 1.0, 0.0),
                "Test Rover",
                Color::WHITE,
            );
            system_state.apply(app.world_mut());
            entity
        };

        assert!(app.world().get::<Transform>(rover_entity).is_some());
        assert!(app.world().get::<RigidBody>(rover_entity).is_some());
    }
}

/// Internal shared logic for Raycast Rover variants.
fn spawn_raycast_rover_internal(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    wheel_mesh: Handle<Mesh>,
    spawn_pos: Vec3,
    name: &str,
    color: Color,
    steering_type: SteeringType,
) -> Entity {
    let chassis_width = 1.8;
    let chassis_height = 0.5;
    let chassis_length = 3.0;
    let wheel_radius = 0.5;

    let rover_entity = commands.spawn((
        Name::new(name.to_string()),
        RoverVessel,
        Vessel,
        Mesh3d(meshes.add(Cuboid::new(chassis_width, chassis_height, chassis_length))),
        MeshMaterial3d(materials.add(color)),
        Transform::from_translation(spawn_pos),
        Visibility::default(),
        RigidBody::Dynamic,
        Collider::cuboid(chassis_width, chassis_height, chassis_length),
        Friction::new(0.5),
        Mass(1000.0), 
        CenterOfMass(Vec3::new(0.0, -0.8, 0.0)), 
        LinearDamping(0.5),
        AngularDamping(1.0),
        AngularInertia::new(Vec3::new(5000.0, 5000.0, 2000.0)), 
    )).id();

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

    let f_mat = materials.add(Color::srgb(0.9, 0.1, 0.4)); 
    let r_mat = materials.add(Color::srgb(0.1, 0.4, 0.8)); 

    let wheel_configs = [
        ("fl", Vec3::new(-1.2, -0.4, -1.2), true, true), // Front Left at -Z
        ("rl", Vec3::new(-1.2, -0.4, 1.2), true, false), // Rear Left at +Z
        ("fr", Vec3::new(1.2, -0.4, -1.2), false, true), // Front Right at -Z
        ("rr", Vec3::new(1.2, -0.4, 1.2), false, false), // Rear Right at +Z
    ];

    let wheel_tilt = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let steer_port = commands.spawn((Name::new(format!("{}_port_steer", name)), PhysicalPort::default())).id();
    commands.spawn(Wire { source: steer_digital, target: steer_port, scale: 0.6 });

    for (label, rel_pos, is_left, is_front) in wheel_configs {
        let mat = if is_front { f_mat.clone() } else { r_mat.clone() };
        
        let motor_port = commands.spawn((Name::new(format!("{}_port_{}_drive", name, label)), PhysicalPort::default())).id();
        let brake_port = commands.spawn((Name::new(format!("{}_port_{}_brake", name, label)), PhysicalPort::default())).id();
        
        // Both front and rear follow their respective side channels
        let drive_source = if is_left { drive_l_digital } else { drive_r_digital };
        commands.spawn(Wire { source: drive_source, target: motor_port, scale: 12000.0 });
        commands.spawn(Wire { source: brake_digital, target: brake_port, scale: 20000.0 });

        commands.entity(rover_entity).with_children(|parent| {
            let mut wheel = parent.spawn((
                Name::new(format!("{}_wheel_{}", name, label)),
                RaycastWheel { radius: wheel_radius, stiffness: 6000.0, damping: 2500.0, friction: 0.8 },
                RaycastWheelState::default(),
                Visibility::default(),
                RayCaster::new(Vec3::ZERO, Dir3::NEG_Y)
                    .with_max_hits(1)
                    .with_max_distance(wheel_radius * 1.5)
                    .with_query_filter(SpatialQueryFilter::from_excluded_entities([rover_entity])),
                MotorActuator { port_entity: motor_port, axis: Vec3::Z },
                BrakeActuator { port_entity: brake_port },
                Transform::from_translation(rel_pos),
            ));
            
            if is_front && steering_type == SteeringType::Ackermann {
                wheel.insert(SteeringActuator { port_entity: steer_port, max_angle: 0.6 });
            }

            wheel.with_children(|p| {
                p.spawn((WheelVisual, Mesh3d(wheel_mesh.clone()), MeshMaterial3d(mat), Visibility::default(), Transform::from_rotation(wheel_tilt)));
            });
        });
    }
    rover_entity
}

#[derive(PartialEq, Eq)]
pub enum SteeringType { Skid, Ackermann }

pub fn spawn_raycast_skid_rover(commands: &mut Commands, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>, wheel_mesh: Handle<Mesh>, spawn_pos: Vec3, name: &str, color: Color) -> Entity {
    spawn_raycast_rover_internal(commands, meshes, materials, wheel_mesh, spawn_pos, name, color, SteeringType::Skid)
}

pub fn spawn_raycast_ackermann_rover(commands: &mut Commands, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>, wheel_mesh: Handle<Mesh>, spawn_pos: Vec3, name: &str, color: Color) -> Entity {
    spawn_raycast_rover_internal(commands, meshes, materials, wheel_mesh, spawn_pos, name, color, SteeringType::Ackermann)
}
