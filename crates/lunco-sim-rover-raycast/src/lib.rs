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

impl Default for RaycastWheel {
    fn default() -> Self {
        Self {
            radius: 0.5,
            stiffness: 10000.0,
            damping: 500.0,
            friction: 0.8,
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
            let damping_force = (wheel.damping * compression_velocity).max(0.0);
            let total_force_mag = spring_force + damping_force;

            let force = total_force_mag * state.surface_normal;

            // Apply the force to the parent at the wheel's position
            parent_forces.apply_force_at_point(force, wheel_pos);

            state.normal_force = total_force_mag;
            state.velocity = wheel_velocity;
        }
    }
}

fn apply_wheel_friction(
    wheels: Query<(&RaycastWheel, &RaycastWheelState, &GlobalTransform, &ChildOf)>,
    mut parents: Query<(Forces, &CenterOfMass, &Transform), With<RigidBody>>,
) {
    for (wheel, state, global_transform, child_of) in wheels.iter() {
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
            let lateral_stiffness = 20.0; 
            let lateral_force_mag = (lateral_velocity * lateral_stiffness * state.normal_force).clamp(-max_friction, max_friction);
            let lateral_force = -lateral_force_mag * right;

            // 3. Longitudinal friction (rolling resistance / braking)
            let forward = global_transform.forward();
            let forward_velocity = contact_velocity.dot(forward.as_vec3());
            
            let rolling_resistance = 0.1;
            let longitudinal_stiffness = 10.0;
            let longitudinal_force_mag = (forward_velocity * longitudinal_stiffness * state.normal_force * rolling_resistance).clamp(-max_friction, max_friction);
            let longitudinal_force = -longitudinal_force_mag * forward;

            parent_forces.apply_force_at_point(lateral_force + longitudinal_force, state.surface_point);
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
                let drive_force = port.value * forward.as_vec3();

                parent_forces.apply_force_at_point(drive_force, state.surface_point);
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

/// Blueprint for assembling a Raycast-based Rover.
pub fn spawn_raycast_rover(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    wheel_mesh: Handle<Mesh>,
    spawn_pos: Vec3,
    name: &str,
    color: Color,
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
        CenterOfMass(Vec3::new(0.0, -0.4, 0.0)), 
        LinearDamping(0.2),
        AngularDamping(0.3),
        AngularInertia::new(Vec3::new(5000.0, 5000.0, 2000.0)), 
    )).id();

    let drive_l_digital = commands.spawn((Name::new(format!("{}_drive_l_reg", name)), DigitalPort::default())).id();
    let drive_r_digital = commands.spawn((Name::new(format!("{}_drive_r_reg", name)), DigitalPort::default())).id();

    let mut port_map = HashMap::new();
    port_map.insert("drive_left".to_string(), drive_l_digital);
    port_map.insert("drive_right".to_string(), drive_r_digital);
    commands.entity(rover_entity).insert(FlightSoftware { port_map });

    let f_mat = materials.add(Color::srgb(0.9, 0.1, 0.4)); 
    let r_mat = materials.add(Color::srgb(0.1, 0.4, 0.8)); 

    let wheel_configs = [
        ("fl", Vec3::new(-1.2, -0.4, 1.2), drive_l_digital, f_mat.clone()),
        ("rl", Vec3::new(-1.2, -0.4, -1.2), drive_l_digital, r_mat.clone()),
        ("fr", Vec3::new(1.2, -0.4, 1.2), drive_r_digital, f_mat.clone()),
        ("rr", Vec3::new(1.2, -0.4, -1.2), drive_r_digital, r_mat.clone()),
    ];

    let wheel_tilt = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);

    for (label, rel_pos, digital_source, mat) in wheel_configs {
        let motor_port = commands.spawn((Name::new(format!("{}_port_{}", name, label)), PhysicalPort::default())).id();
        commands.spawn(Wire { source: digital_source, target: motor_port, scale: 20000.0 });

        commands.entity(rover_entity).with_children(|parent| {
            parent.spawn((
                Name::new(format!("{}_wheel_{}", name, label)),
                RaycastWheel {
                    radius: wheel_radius,
                    stiffness: 15000.0,
                    damping: 1000.0,
                    friction: 1.0,
                },
                RaycastWheelState::default(),
                Visibility::default(),
                RayCaster::new(Vec3::ZERO, Dir3::NEG_Y)
                    .with_max_hits(1)
                    .with_max_distance(wheel_radius * 1.5)
                    .with_query_filter(SpatialQueryFilter::from_excluded_entities([rover_entity])),
                MotorActuator {
                    port_entity: motor_port,
                    axis: Vec3::Z, 
                },
                Transform::from_translation(rel_pos),
            )).with_children(|wheel| {
                wheel.spawn((
                    WheelVisual,
                    Mesh3d(wheel_mesh.clone()),
                    MeshMaterial3d(mat),
                    Visibility::default(),
                    Transform::from_rotation(wheel_tilt),
                ));
            });
        });
    }

    rover_entity
}
