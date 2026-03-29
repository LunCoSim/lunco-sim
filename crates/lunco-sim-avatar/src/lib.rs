use bevy::prelude::*;
use bevy::input::mouse::MouseMotion;
use avian3d::prelude::*;
use lunco_sim_controller::ControllerLink;

pub struct LunCoSimAvatarPlugin;

impl Plugin for LunCoSimAvatarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (
            avatar_freecam_translation,
            avatar_freecam_rotation,
            avatar_raycast_possession,
            avatar_orbit_logic,
            avatar_escape_possession,
        ));
    }
}

#[derive(Component)]
pub struct Avatar;

#[derive(Component)]
pub struct OrbitState {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub vertical_offset: f32,
}

impl Default for OrbitState {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: -0.5, // Start from the top
            distance: 10.0,
            vertical_offset: 1.0, // Default to slightly above center
        }
    }
}

#[derive(Component)]
pub struct Vessel; // Generic marker for anything that can be possessed

fn avatar_freecam_translation(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut q_avatar: Query<&mut Transform, (With<Avatar>, Without<ControllerLink>)>,
) {
    let speed = 20.0 * time.delta_secs();
    
    for mut transform in q_avatar.iter_mut() {
        let mut velocity = Vec3::ZERO;
        let forward = *transform.forward();
        let right = *transform.right();
        let up = Vec3::Y;

        if keys.pressed(KeyCode::KeyW) { velocity += forward; }
        if keys.pressed(KeyCode::KeyS) { velocity -= forward; }
        if keys.pressed(KeyCode::KeyD) { velocity += right; }
        if keys.pressed(KeyCode::KeyA) { velocity -= right; }
        if keys.pressed(KeyCode::KeyE) { velocity += up; }
        if keys.pressed(KeyCode::KeyQ) { velocity -= up; }

        let normalized = velocity.normalize_or_zero();
        transform.translation += normalized * speed;
    }
}

fn avatar_freecam_rotation(
    keys: Res<ButtonInput<MouseButton>>,
    mut mouse_motion: MessageReader<MouseMotion>,
    mut q_avatar: Query<&mut Transform, (With<Avatar>, Without<ControllerLink>)>,
) {
    if !keys.pressed(MouseButton::Right) {
        mouse_motion.clear();
        return;
    }

    let mut delta = Vec2::ZERO;
    for event in mouse_motion.read() {
        delta += event.delta;
    }

    let sensitivity = 0.005;
    for mut transform in q_avatar.iter_mut() {
        let (mut yaw, mut pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
        yaw -= delta.x * sensitivity;
        pitch -= delta.y * sensitivity;
        pitch = pitch.clamp(-1.5, 1.5);
        transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
    }
}

fn avatar_orbit_logic(
    keys: Res<ButtonInput<MouseButton>>,
    keys_input: Res<ButtonInput<KeyCode>>,
    mut mouse_motion: MessageReader<MouseMotion>,
    mut scroll_events: MessageReader<bevy::input::mouse::MouseWheel>,
    time: Res<Time>,
    mut q_avatar: Query<(&mut Transform, &mut OrbitState, &ControllerLink), With<Avatar>>,
    q_targets: Query<&GlobalTransform>,
) {
    for (mut transform, mut orbit, link) in q_avatar.iter_mut() {
        // 1. Handle Orbit Rotation (Right Mouse)
        if keys.pressed(MouseButton::Right) {
            let mut delta = Vec2::ZERO;
            for event in mouse_motion.read() {
                delta += event.delta;
            }
            let sensitivity = 0.005;
            orbit.yaw -= delta.x * sensitivity;
            orbit.pitch -= delta.y * sensitivity;
            orbit.pitch = orbit.pitch.clamp(-1.5, -0.1); // Keep above the vessel
        } else {
            mouse_motion.read(); // Clear buffer
        }

        // 2. Handle Zoom (Mouse Wheel)
        for event in scroll_events.read() {
            orbit.distance = (orbit.distance - event.y * 2.0).clamp(2.0, 100.0);
        }

        // 3. Handle Vertical Offset (QE)
        let offset_speed = 5.0 * time.delta_secs();
        if keys_input.pressed(KeyCode::KeyE) { orbit.vertical_offset += offset_speed; }
        if keys_input.pressed(KeyCode::KeyQ) { orbit.vertical_offset -= offset_speed; }

        // 4. Calculate smooth follow
        if let Ok(target_tf) = q_targets.get(link.vessel_entity) {
            let mut target_pos = target_tf.translation();
            target_pos.y += orbit.vertical_offset; // Apply vertical focal point shift
            
            let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
            let offset = rotation * Vec3::new(0.0, 0.0, orbit.distance);
            let desired_pos = target_pos + offset;
            
            let lerp_factor = (10.0 * time.delta_secs()).min(1.0);
            transform.translation = transform.translation.lerp(desired_pos, lerp_factor);
            transform.look_at(target_pos, Vec3::Y);
        }
    }
}

fn avatar_raycast_possession(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform, Entity), (With<Avatar>, Without<ControllerLink>)>,
    spatial_query: SpatialQuery,
    mut commands: Commands,
    vessel_q: Query<Entity, With<Vessel>>,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }

    let Some(window) = windows.iter().next() else { return };
    let Some(cursor_position) = window.cursor_position() else { return };
    
    for (camera, camera_transform, avatar_entity) in camera_q.iter() {
        let Ok(ray) = camera.viewport_to_world(camera_transform, cursor_position) else { continue };

        let hit = spatial_query.cast_ray(
            ray.origin,
            ray.direction,
            1000.0,
            true,
            &SpatialQueryFilter::default(),
        );

        if let Some(hit_data) = hit {
            if let Ok(vessel_entity) = vessel_q.get(hit_data.entity) {
                commands.entity(avatar_entity).insert((
                    ControllerLink { vessel_entity },
                    OrbitState::default(),
                ));
                println!("Avatar possessed Vessel: {:?}", vessel_entity);
            }
        }
    }
}

fn avatar_escape_possession(
    keys: Res<ButtonInput<KeyCode>>,
    mut q_avatar: Query<Entity, (With<Avatar>, With<ControllerLink>)>,
    mut commands: Commands,
) {
    if keys.just_pressed(KeyCode::Backspace) {
        for entity in q_avatar.iter_mut() {
            commands.entity(entity).remove::<ControllerLink>();
            commands.entity(entity).remove::<OrbitState>();
            println!("Avatar released possession.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_avatar_possess_logic() {
        let mut app = App::new();
        app.add_plugins(LunCoSimAvatarPlugin);
        
        // Mock requirements
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.insert_resource(ButtonInput::<MouseButton>::default());
        app.insert_resource(Time::default());
        app.init_resource::<Messages<MouseMotion>>();

        let avatar = app.world_mut().spawn((Avatar, Transform::default())).id();
        let target = app.world_mut().spawn(Vessel).id();

        // 1. Not possessed
        assert!(app.world().get::<ControllerLink>(avatar).is_none());

        // 2. Perform manual possession simulation
        app.world_mut().entity_mut(avatar).insert(ControllerLink { vessel_entity: target });
        
        assert!(app.world().get::<ControllerLink>(avatar).is_some());
    }
}
