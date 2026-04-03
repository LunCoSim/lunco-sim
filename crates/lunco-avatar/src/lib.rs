use bevy::prelude::*;
use bevy::ecs::relationship::Relationship;
use avian3d::prelude::*;

use lunco_controller::ControllerLink;
use lunco_core::{Vessel, Avatar, OrbitState};
use lunco_celestial::CelestialClock;
use lunco_camera::{ObserverCamera, ObserverMode};

pub struct LunCoAvatarPlugin;

impl Plugin for LunCoAvatarPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<UserIntent>();
        app.add_observer(on_user_intent);
        app.add_systems(Update, (
            capture_avatar_intent,
            avatar_freecam_translation,
            avatar_freecam_rotation,
            avatar_raycast_possession,
            avatar_orbit_input,
            avatar_escape_possession,
            avatar_toggle_detached_mode,
            avatar_global_hotkeys,
        ).chain());

        // Modular camera system handles follow via ObserverCamera in lunco-camera
    }
}

/// High-level semantic actions intended by the user.
/// Decouples raw input (WASD/Keys) from simulation results.
#[derive(Component, Event, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct UserIntent {
    /// Semantic forward/backward movement (-1.0 to 1.0)
    pub forward: f64,
    /// Semantic side movement (-1.0 to 1.0)
    pub side: f64,
    /// Semantic rotation/yaw (-1.0 to 1.0)
    pub turn: f64,
    /// Semantic elevation Change (-1.0 to 1.0)
    pub elevation: f64,
    /// The avatar entity that generated this intent
    pub source: Entity,
    /// Timestamp of when this intent was formed
    pub timestamp: f64,
}

impl Default for UserIntent {
    fn default() -> Self {
        Self {
            forward: 0.0,
            side: 0.0,
            turn: 0.0,
            elevation: 0.0,
            source: Entity::PLACEHOLDER,
            timestamp: 0.0,
        }
    }
}

#[derive(Component)]
pub struct DetachedCamera;

fn avatar_toggle_detached_mode(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    q_avatar: Query<(Entity, Has<DetachedCamera>), (With<Avatar>, With<ControllerLink>)>,
) {
    if keys.just_pressed(KeyCode::KeyV) {
        for (entity, is_detached) in q_avatar.iter() {
            if is_detached {
                commands.entity(entity).remove::<DetachedCamera>();
            } else {
                commands.entity(entity).insert(DetachedCamera);
            }
        }
    }
}

fn capture_avatar_intent(
    keys: Res<ButtonInput<KeyCode>>,
    clock: Res<lunco_celestial::CelestialClock>,
    mut q_avatar: Query<(Entity, &mut UserIntent), With<Avatar>>,
    mut commands: Commands,
) {
    for (entity, mut intent) in q_avatar.iter_mut() {
        let mut forward = 0.0;
        let mut side = 0.0;
        let mut elevation = 0.0;
        
        if keys.pressed(KeyCode::KeyW) { forward += 1.0; }
        if keys.pressed(KeyCode::KeyS) { forward -= 1.0; }
        if keys.pressed(KeyCode::KeyA) { side -= 1.0; }
        if keys.pressed(KeyCode::KeyD) { side += 1.0; }
        if keys.pressed(KeyCode::KeyE) { elevation += 1.0; }
        if keys.pressed(KeyCode::KeyQ) { elevation -= 1.0; }

        // Capture intent
        intent.forward = forward;
        intent.side = side;
        intent.elevation = elevation;
        intent.source = entity;
        intent.timestamp = clock.epoch;
        
        // Trigger the intent globally for the translator to pick up
        commands.trigger(intent.clone());

        // Phase 5: Preemption - Manual input cancels active automated actions (ViewPoint transitions, etc)
        if forward.abs() > 0.1 || side.abs() > 0.1 || elevation.abs() > 0.1 {
             commands.entity(entity).remove::<lunco_core::ActiveAction>();
        }
    }
}

fn avatar_global_hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    mut clock: ResMut<CelestialClock>,
    q_avatar: Query<&ObserverCamera, With<Avatar>>,
) {
    if keys.just_pressed(KeyCode::Space) {
        for obs in q_avatar.iter() {
            if obs.mode == ObserverMode::Orbital || obs.mode == ObserverMode::Flyby {
                clock.paused = !clock.paused;
                info!("Toggled simulation pause via avatar-space interaction. Paused: {}", clock.paused);
                break;
            }
        }
    }
}

fn avatar_freecam_translation(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut q_avatar: Query<(&mut Transform, Has<DetachedCamera>), (With<Avatar>, Or<(Without<ControllerLink>, With<DetachedCamera>)>)>,
) {
    let mut speed = 20.0 * time.delta_secs();
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) { speed *= 2.0; }
    let ctrl_pressed = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    
    for (mut transform, is_detached) in q_avatar.iter_mut() {
        if is_detached && !ctrl_pressed { continue; }
        let mut velocity = Vec3::ZERO;
        let forward = *transform.forward();
        let right = *transform.right();
        if keys.pressed(KeyCode::KeyW) { velocity += forward; }
        if keys.pressed(KeyCode::KeyS) { velocity -= forward; }
        if keys.pressed(KeyCode::KeyD) { velocity += right; }
        if keys.pressed(KeyCode::KeyA) { velocity -= right; }
        if keys.pressed(KeyCode::KeyE) { velocity += Vec3::Y; }
        if keys.pressed(KeyCode::KeyQ) { velocity -= Vec3::Y; }
        transform.translation += velocity.normalize_or_zero() * speed;
    }
}

fn avatar_freecam_rotation(
    keys: Res<ButtonInput<MouseButton>>,
    mut q_avatar: Query<&mut Transform, (With<Avatar>, Or<(Without<ControllerLink>, With<DetachedCamera>)>)>,
    windows: Query<&Window>,
    mut last_mouse_pos: Local<Option<Vec2>>,
) {
    let window = windows.iter().next();
    let curr = window.and_then(|w| w.cursor_position());
    let mut delta = Vec2::ZERO;
    if let (Some(c), Some(l)) = (curr, *last_mouse_pos) {
        if keys.pressed(MouseButton::Right) { delta = c - l; }
    }
    *last_mouse_pos = curr;

    let sensitivity = 0.005;
    for mut transform in q_avatar.iter_mut() {
        let (mut yaw, mut pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
        yaw -= delta.x * sensitivity;
        pitch -= delta.y * sensitivity;
        pitch = pitch.clamp(-1.5, 1.5);
        transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
    }
}

fn avatar_orbit_input(
    keys: Res<ButtonInput<MouseButton>>,
    keys_input: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    windows: Query<&Window>,
    mut last_mouse_pos: Local<Option<Vec2>>,
    mut q_avatar: Query<&mut OrbitState, (With<Avatar>, Without<DetachedCamera>)>,
) {
    let window = windows.iter().next();
    let curr = window.and_then(|w| w.cursor_position());
    let mut delta = Vec2::ZERO;
    if let (Some(c), Some(l)) = (curr, *last_mouse_pos) {
        if keys.pressed(MouseButton::Right) { delta = c - l; }
    }
    *last_mouse_pos = curr;

    let mut scroll = 0.0;
    if keys_input.pressed(KeyCode::Equal) { scroll += 1.0; }
    if keys_input.pressed(KeyCode::Minus) { scroll -= 1.0; }

    for mut orbit in q_avatar.iter_mut() {
        if keys.pressed(MouseButton::Right) {
            let sensitivity = 0.005;
            orbit.yaw -= delta.x * sensitivity;
            orbit.pitch -= delta.y * sensitivity;
            orbit.pitch = orbit.pitch.clamp(-1.5, -0.1);
        }
        orbit.distance = (orbit.distance - scroll * 2.0).clamp(2.0, 100.0);
        let offset_speed = 5.0 * time.delta_secs();
        if keys_input.pressed(KeyCode::KeyE) { orbit.vertical_offset += offset_speed; }
        if keys_input.pressed(KeyCode::KeyQ) { orbit.vertical_offset -= offset_speed; }
    }
}

// avatar_camera_follow removed - functionality moved to lunco-camera::update_observer_camera_system

fn avatar_raycast_possession(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform, Entity), (With<Avatar>, Without<ControllerLink>)>,
    spatial_query: Option<SpatialQuery>,
    mut commands: Commands,
    vessel_q: Query<Entity, With<Vessel>>,
) {
    let Some(spatial_query) = spatial_query else { return; };
    if !mouse.just_pressed(MouseButton::Left) { return; }
    let Some(window) = windows.iter().next() else { return };
    if let Some(cursor_position) = window.cursor_position() {
        for (camera, camera_transform, avatar_entity) in camera_q.iter() {
            if let Ok(f32_ray) = camera.viewport_to_world(camera_transform, cursor_position) {
                if let Some(hit) = spatial_query.cast_ray(f32_ray.origin.as_dvec3(), f32_ray.direction, 1000.0, true, &SpatialQueryFilter::default()) {
                    if let Ok(vessel_entity) = vessel_q.get(hit.entity) {
                        commands.entity(avatar_entity).insert((
                            ControllerLink { vessel_entity }, 
                            OrbitState::default(),
                            UserIntent::default()
                        ));
                    }
                }
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
            commands.entity(entity).remove::<DetachedCamera>();
            commands.entity(entity).remove::<UserIntent>();
        }
    }
}

/// Observer that translates high-level UserIntent into physical CommandMessages 
/// specifically for the vessel linked to the avatar.
fn on_user_intent(
    trigger: On<UserIntent>,
    q_avatar: Query<&ControllerLink, With<Avatar>>,
    mut commands: Commands,
) {
    let intent = trigger.event();
    let avatar_entity = intent.source;
    
    if let Ok(link) = q_avatar.get(avatar_entity) {
        if intent.forward.abs() > 0.01 || intent.side.abs() > 0.01 {
            commands.trigger(lunco_core::architecture::CommandMessage {
                id: intent.timestamp as u64, 
                target: link.vessel_entity,
                name: "DRIVE_ROVER".to_string(),
                args: smallvec::smallvec![intent.forward, intent.side],
                source: avatar_entity,
            });
        }
    }
}
