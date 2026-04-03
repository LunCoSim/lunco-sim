//! Implementation of the user's presence and interaction within the simulation.
//!
//! This crate defines the [Avatar] entity, which can be in several states:
//! - **Free-cam**: Flying freely through the scene for observation.
//! - **Possessed**: Linked to a vessel via [ControllerLink], allowing direct 
//!   piloting of rovers or spacecraft.
//! - **Orbital**: Following a celestial body or vessel in a third-person view.

use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use avian3d::prelude::*;

use lunco_controller::ControllerLink;
use lunco_core::{Vessel, Avatar, OrbitState};
use lunco_celestial::CelestialClock;

mod intents;
pub use intents::*;

/// Plugin for managing user avatar logic, input processing, and possession.
pub struct LunCoAvatarPlugin;

impl Plugin for LunCoAvatarPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(InputManagerPlugin::<UserIntent>::default());
        app.add_observer(on_user_intent);
        app.add_observer(on_possess_command);
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
    }
}

/// Marker component for an avatar that is currently in a "detached" free-look mode
/// even if linked to a vessel.
#[derive(Component)]
pub struct DetachedCamera;

/// Toggles between fixed vessel-follow cameras and a detached free-look camera.
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

/// Captures high-level [UserIntent] from the InputManager and populates [IntentAnalogState].
///
/// This system acts as a bridge between discrete input events and continuous 
/// analog control signals used by simulation subsystems.
fn capture_avatar_intent(
    mut q_avatar: Query<(Entity, &IntentState, &mut IntentAnalogState), With<Avatar>>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    mut last_mouse_pos: Local<Option<Vec2>>,
    clock: Res<lunco_celestial::CelestialClock>,
    mut commands: Commands,
) {
    let window = windows.iter().next();
    let current_mouse_pos = window.and_then(|w| w.cursor_position());
    let mut mouse_moved = false;
    if let (Some(curr), Some(last)) = (current_mouse_pos, *last_mouse_pos) {
        if mouse.pressed(MouseButton::Right) && curr.distance(last) > 2.0 {
            mouse_moved = true;
        }
    }
    *last_mouse_pos = current_mouse_pos;

    for (entity, intent_state, mut analog) in q_avatar.iter_mut() {
        let mut forward = 0.0;
        let mut side = 0.0;
        let mut elevation = 0.0;
        
        if intent_state.pressed(&UserIntent::MoveForward) { forward += 1.0; }
        if intent_state.pressed(&UserIntent::MoveBackward) { forward -= 1.0; }
        if intent_state.pressed(&UserIntent::MoveRight) { side += 1.0; }
        if intent_state.pressed(&UserIntent::MoveLeft) { side -= 1.0; }
        if intent_state.pressed(&UserIntent::MoveUp) { elevation += 1.0; }
        if intent_state.pressed(&UserIntent::MoveDown) { elevation -= 1.0; }

        // Update the analog snapshot used for high-frequency control loops.
        analog.forward = forward;
        analog.side = side;
        analog.elevation = elevation;
        analog.timestamp = clock.epoch;
        
        // Trigger a global EntityEvent for other systems to react to the new intent state.
        commands.entity(entity).trigger(|e| {
            let mut a = analog.clone();
            a.entity = e;
            a
        });

        // Preemption logic: Any manual movement input cancels active automated 
        // actions (e.g., stopping a camera transition if the user moves the mouse).
        if forward.abs() > 0.1 || side.abs() > 0.1 || elevation.abs() > 0.1 || mouse_moved {
             commands.entity(entity).remove::<lunco_core::ActiveAction>();
        }
    }
}

/// Handles global UI-level hotkeys captured through the Avatar's input mapping.
fn avatar_global_hotkeys(
    q_avatar: Query<&IntentState, With<Avatar>>,
    mut clock: ResMut<CelestialClock>,
) {
    for intent_state in q_avatar.iter() {
        if intent_state.just_pressed(&UserIntent::Pause) {
            clock.paused = !clock.paused;
            info!("Toggled simulation pause via UserIntent. Paused: {}", clock.paused);
        }
    }
}

/// Updates the translation of the avatar in free-cam or detached mode.
fn avatar_freecam_translation(
    time: Res<Time>,
    mut q_avatar: Query<(&mut Transform, &IntentState, Has<DetachedCamera>), (With<Avatar>, Or<(Without<ControllerLink>, With<DetachedCamera>)>)>,
) {
    let speed = 20.0 * time.delta_secs();
    
    for (mut transform, intent_state, _is_detached) in q_avatar.iter_mut() {
        let mut velocity = Vec3::ZERO;
        let forward = *transform.forward();
        let right = *transform.right();
        
        if intent_state.pressed(&UserIntent::MoveForward) { velocity += forward; }
        if intent_state.pressed(&UserIntent::MoveBackward) { velocity -= forward; }
        if intent_state.pressed(&UserIntent::MoveRight) { velocity += right; }
        if intent_state.pressed(&UserIntent::MoveLeft) { velocity -= right; }
        if intent_state.pressed(&UserIntent::MoveUp) { velocity += Vec3::Y; }
        if intent_state.pressed(&UserIntent::MoveDown) { velocity -= Vec3::Y; }
        
        transform.translation += velocity.normalize_or_zero() * speed;
    }
}

/// Updates the rotation (yaw/pitch) of the avatar camera based on mouse movement.
fn avatar_freecam_rotation(
    mouse: Res<ButtonInput<MouseButton>>,
    mut q_avatar: Query<&mut Transform, (With<Avatar>, Or<(Without<ControllerLink>, With<DetachedCamera>)>)>,
    windows: Query<&Window>,
    mut last_mouse_pos: Local<Option<Vec2>>,
) {
    let window = windows.iter().next();
    let curr = window.and_then(|w| w.cursor_position());
    let mut delta = Vec2::ZERO;
    if let (Some(c), Some(l)) = (curr, *last_mouse_pos) {
        if mouse.pressed(MouseButton::Right) { delta = c - l; }
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

/// Processes mouse/keyboard input for the third-person [OrbitState] mode.
fn avatar_orbit_input(
    mouse: Res<ButtonInput<MouseButton>>,
    intent_q: Query<&IntentState, With<Avatar>>,
    time: Res<Time>,
    windows: Query<&Window>,
    mut last_mouse_pos: Local<Option<Vec2>>,
    mut q_avatar: Query<&mut OrbitState, (With<Avatar>, Without<DetachedCamera>)>,
) {
    let window = windows.iter().next();
    let curr = window.and_then(|w| w.cursor_position());
    let mut delta = Vec2::ZERO;
    if let (Some(c), Some(l)) = (curr, *last_mouse_pos) {
        if mouse.pressed(MouseButton::Right) { delta = c - l; }
    }
    *last_mouse_pos = curr;

    for (mut orbit, intent_state) in q_avatar.iter_mut().zip(intent_q.iter()) {
        if mouse.pressed(MouseButton::Right) {
            let sensitivity = 0.005;
            orbit.yaw -= delta.x * sensitivity;
            orbit.pitch -= delta.y * sensitivity;
            orbit.pitch = orbit.pitch.clamp(-1.5, -0.1);
        }
        
        if intent_state.pressed(&UserIntent::Zoom) {
            // Zoom logic would need to handle axis or button
        }

        let offset_speed = 5.0 * time.delta_secs();
        if intent_state.pressed(&UserIntent::MoveUp) { orbit.vertical_offset += offset_speed; }
        if intent_state.pressed(&UserIntent::MoveDown) { orbit.vertical_offset -= offset_speed; }
    }
}

/// Allows the user to "possess" a vessel by clicking on it in the scene.
///
/// Uses raycasting to identify [Vessel] entities under the cursor and 
/// inserts a [ControllerLink] to establish the pilot-vehicle relationship.
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
                        commands.trigger(lunco_core::architecture::CommandMessage {
                            id: 0,
                            target: vessel_entity,
                            name: "POSSESS".to_string(),
                            args: smallvec::smallvec![],
                            source: avatar_entity,
                        });
                    }
                }
            }
        }
    }
}

/// Releases the current vessel possession, returning the avatar to free-cam mode.
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
            commands.entity(entity).remove::<IntentAnalogState>();
        }
    }
}

/// Observer that translates high-level analog intent into physical [CommandMessage] 
/// events specifically for the vessel linked to the avatar.
fn on_user_intent(
    trigger: On<IntentAnalogState>,
    q_avatar: Query<&ControllerLink, With<Avatar>>,
    mut commands: Commands,
) {
    let analog = trigger.event();
    let avatar_entity = trigger.entity;
    
    if let Ok(link) = q_avatar.get(avatar_entity) {
        if analog.forward.abs() > 0.01 || analog.side.abs() > 0.01 {
            commands.trigger(lunco_core::architecture::CommandMessage {
                id: analog.timestamp as u64, 
                target: link.vessel_entity,
                name: "DRIVE_ROVER".to_string(),
                args: smallvec::smallvec![analog.forward as f64, analog.side as f64],
                source: avatar_entity,
            });
        }
    }
}

/// Handles global `"POSSESS"` CommandMessages to establish a link between an avatar and a vessel.
fn on_possess_command(
    trigger: On<lunco_core::architecture::CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<Entity, With<Avatar>>,
) {
    let msg = trigger.event();
    if msg.name == "POSSESS" {
        // We expect the avatar entity to be the source, but if it's missing, just take the first avatar.
        let avatar_ent = if let Ok(e) = q_avatar.get(msg.source) { e } else { q_avatar.iter().next().unwrap() };
        
        commands.entity(avatar_ent).insert((
            ControllerLink { vessel_entity: msg.target },
            OrbitState::default(),
            IntentAnalogState::default()
        ));
        
        // Ensure the vessel has input maps configured to receive commands
        commands.entity(msg.target).insert((
            leafwing_input_manager::prelude::ActionState::<lunco_controller::VesselIntent>::default(),
            lunco_controller::get_default_input_map(),
        ));

        // Trigger focus
        commands.trigger(lunco_core::architecture::CommandMessage {
            id: 0,
            target: msg.target,
            name: "FOCUS".to_string(),
            args: smallvec::smallvec![10.0],
            source: avatar_ent,
        });
    }
}
