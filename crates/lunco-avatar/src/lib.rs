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
use big_space::prelude::{Grid, CellCoord};

use lunco_controller::ControllerLink;
use lunco_core::{Vessel, Avatar, ActiveAction, ActionStatus, CommandMessage, CelestialBody};
use lunco_celestial::CelestialClock;

mod intents;
pub use intents::*;

#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub struct MouseSensitivity {
    pub sensitivity: f32,
}

impl Default for MouseSensitivity {
    fn default() -> Self {
        Self { sensitivity: 0.15 }
    }
}

/// Resource for tracking cumulative mouse scroll delta for zoom control.
#[derive(Resource, Default)]
pub struct CameraScroll {
    pub delta: f32,
}

/// Plugin for managing user avatar logic, input processing, and possession.
pub struct LunCoAvatarPlugin;

impl Plugin for LunCoAvatarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraScroll>()
           .init_resource::<MouseSensitivity>();
        app.add_plugins(InputManagerPlugin::<UserIntent>::default());
        app.add_observer(on_user_intent);
        app.add_observer(on_possess_command);
        app.add_observer(on_release_command);
        app.add_observer(on_focus_command);
        app.add_observer(on_drag_commands);
        
        app.register_type::<OrbitalBehavior>()
           .register_type::<SurfaceBehavior>()
           .register_type::<FlybyBehavior>()
           .register_type::<AdaptiveNearPlane>()
           .register_type::<MouseSensitivity>();

        app.add_systems(Update, (
            capture_avatar_intent,
            avatar_behavior_input_system,
            (
                avatar_orbital_system,
                avatar_surface_system,
                avatar_flyby_system,
                avatar_drag_lifecycle,
                avatar_raycast_possession,
                avatar_escape_possession,
                avatar_toggle_detached_mode,
                avatar_global_hotkeys,
                update_avatar_clip_planes_system,
            ).chain(),
        ));
    }
}

/// Parameters for scale-adaptive 3rd-person orbital observation.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component)]
pub struct OrbitalBehavior {
    pub target: Option<Entity>,
    pub distance: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub vertical_offset: f32,
    pub damping: f32,
    pub use_target_frame: bool,
}

/// Parameters for terrain-locked terrestrial following.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component)]
pub struct SurfaceBehavior {
    pub target: Option<Entity>,
    pub height: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub damping: f32,
    pub lock_up: bool,
    pub use_target_frame: bool,
}

/// Parameters for 3D translation-centric exploration.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component)]
pub struct FlybyBehavior {
    pub target: Option<Entity>,
    pub offset: bevy::math::DVec3,
    pub yaw: f32,
    pub pitch: f32,
}

/// Ensures optical stability by adjusting near plane based on surface proximity.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component)]
pub struct AdaptiveNearPlane;

/// Activity state for dragging entities in the 3D scene.
#[derive(Component, Debug, Clone)]
pub struct DragActivity {
    pub target: Entity,
    pub distance: f32,
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
    let mut delta = Vec2::ZERO;
    let mut mouse_moved = false;
    if let (Some(curr), Some(last)) = (current_mouse_pos, *last_mouse_pos) {
        if mouse.pressed(MouseButton::Right) && curr.distance(last) > 0.1 {
            delta = curr - last;
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
        analog.look_delta = delta;
        analog.timestamp = clock.epoch;
        
        // Trigger a global EntityEvent for other systems to react to the new intent state.
        commands.entity(entity).trigger(|e| {
            let mut a = (*analog).clone();
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

/// Applies [IntentAnalogState.look_delta] to the active behavior's yaw and pitch.
fn avatar_behavior_input_system(
    mut q_avatar: Query<(&IntentAnalogState, Option<&mut FlybyBehavior>, Option<&mut OrbitalBehavior>, Option<&mut SurfaceBehavior>), With<Avatar>>,
    sensitivity: Res<MouseSensitivity>,
) {
    for (analog, flyby_opt, orbital_opt, surface_opt) in q_avatar.iter_mut() {
        if analog.look_delta.length_squared() < 0.0001 { continue; }
        
        let delta_yaw = -analog.look_delta.x * sensitivity.sensitivity * 0.01;
        let delta_pitch = -analog.look_delta.y * sensitivity.sensitivity * 0.01;

        if let Some(mut flyby) = flyby_opt {
            flyby.yaw += delta_yaw;
            flyby.pitch = (flyby.pitch + delta_pitch).clamp(-1.5, 1.5);
        }
        if let Some(mut orbital) = orbital_opt {
            orbital.yaw += delta_yaw;
            orbital.pitch = (orbital.pitch + delta_pitch).clamp(-1.5, 1.5);
        }
        if let Some(mut surface) = surface_opt {
            surface.yaw += delta_yaw;
            surface.pitch = (surface.pitch + delta_pitch).clamp(-1.5, 1.5);
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

/// Updates the Avatar's transform and cell based on OrbitalBehavior.
fn avatar_orbital_system(
    time: Res<Time>,
    mut q_avatar: Query<(&mut Transform, &mut CellCoord, &mut OrbitalBehavior, &ChildOf), With<Avatar>>,
    q_spatial: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
) {
    let dt = time.delta_secs();
    for (mut tf, mut cell, orbital, child_of) in q_avatar.iter_mut() {
        let Some(target_ent) = orbital.target else { continue; };
        let Ok((t_cell, t_tf)) = q_spatial.get(target_ent) else { continue; };
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };

        let target_pos = grid.grid_position_double(t_cell, t_tf);
        let mut rotation = Quat::from_euler(EulerRot::YXZ, orbital.yaw, orbital.pitch, 0.0);
        if orbital.use_target_frame {
            rotation = t_tf.rotation * rotation;
        }
        let offset = rotation.mul_vec3(Vec3::Z).as_dvec3() * orbital.distance;
        
        let desired_pos = target_pos + offset + Vec3::Y.as_dvec3() * orbital.vertical_offset as f64;
        
        let lerp_factor = (dt * 10.0 * (1.0 - orbital.damping)).min(1.0) as f64;
        let current_pos = grid.grid_position_double(&cell, &tf);
        let next_pos = current_pos.lerp(desired_pos, lerp_factor);

        let (new_cell, new_tf) = grid.translation_to_grid(next_pos);
        *cell = new_cell;
        tf.translation = new_tf;
        tf.look_at(target_pos.as_vec3() + Vec3::Y * orbital.vertical_offset, Vec3::Y);
    }
}

/// Updates the Avatar's transform and cell based on SurfaceBehavior.
fn avatar_surface_system(
    time: Res<Time>,
    mut q_avatar: Query<(&mut Transform, &mut CellCoord, &mut SurfaceBehavior, &ChildOf), With<Avatar>>,
    q_spatial: Query<(&CellCoord, &Transform), (Without<Avatar>, With<Vessel>)>,
    q_grids: Query<&Grid>,
) {
    let dt = time.delta_secs();
    for (mut tf, mut cell, surface, child_of) in q_avatar.iter_mut() {
        let Some(target_ent) = surface.target else { continue; };
        let Ok((t_cell, t_tf)) = q_spatial.get(target_ent) else { continue; };
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };

        let target_pos = grid.grid_position_double(t_cell, t_tf);
        let up: Dir3 = if surface.lock_up { t_tf.up() } else { Dir3::Y };
        
        let mut rotation = Quat::from_euler(EulerRot::YXZ, surface.yaw, surface.pitch, 0.0);
        if surface.use_target_frame {
            rotation = t_tf.rotation * rotation;
        }
        let offset = rotation.mul_vec3(Vec3::Z) * 10.0; // Fixed distance for surface example
        
        let desired_pos = target_pos + offset.as_dvec3() + up.as_dvec3() * surface.height as f64;
        let lerp_factor = (dt * 5.0 * (1.0 - surface.damping)).min(1.0) as f64;
        let current_pos = grid.grid_position_double(&cell, &tf);
        let next_pos = current_pos.lerp(desired_pos, lerp_factor);

        let (new_cell, new_tf) = grid.translation_to_grid(next_pos);
        *cell = new_cell;
        tf.translation = new_tf;
        
        let target_vec = target_pos.as_vec3() + up * surface.height;
        tf.look_at(target_vec, up);
    }
}

/// Updates the Avatar's transform and cell based on FlybyBehavior.
fn avatar_flyby_system(
    time: Res<Time>,
    mut q_avatar: Query<(&mut Transform, &mut CellCoord, &mut FlybyBehavior, &ChildOf, &IntentState, Option<&ControllerLink>), With<Avatar>>,
    q_spatial: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
) {
    let dt = time.delta_secs();
    for (mut tf, mut cell, mut flyby, child_of, intent, possessed) in q_avatar.iter_mut() {
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };
        
        // Handle orientation
        let rotation = Quat::from_euler(EulerRot::YXZ, flyby.yaw, flyby.pitch, 0.0);
        tf.rotation = rotation;

        // Skip translation if possessed (WASD controls the vessel, not the camera)
        if possessed.is_some() { continue; }

        let mut move_vec = Vec3::ZERO;
        if intent.pressed(&UserIntent::MoveForward) { move_vec += *tf.forward(); }
        if intent.pressed(&UserIntent::MoveBackward) { move_vec -= *tf.forward(); }
        if intent.pressed(&UserIntent::MoveRight) { move_vec += *tf.right(); }
        if intent.pressed(&UserIntent::MoveLeft) { move_vec -= *tf.right(); }
        if intent.pressed(&UserIntent::MoveUp) { move_vec += Vec3::Y; }
        if intent.pressed(&UserIntent::MoveDown) { move_vec -= Vec3::Y; }

        let speed = 20.0 * dt as f64;
        let target_pos = if let Some(target) = flyby.target {
            if let Ok((t_cell, t_tf)) = q_spatial.get(target) {
                grid.grid_position_double(t_cell, t_tf)
            } else { bevy::math::DVec3::ZERO }
        } else { bevy::math::DVec3::ZERO };

        flyby.offset += move_vec.as_dvec3() * speed;
        let desired_pos = target_pos + flyby.offset;
        
        let (new_cell, new_tf) = grid.translation_to_grid(desired_pos);
        *cell = new_cell;
        tf.translation = new_tf;
    }
}

/// Moves the dragged entity relative to the Avatar's viewpoint.
fn avatar_drag_lifecycle(
    q_avatar: Query<(&Transform, &DragActivity, &ActiveAction), With<Avatar>>,
    mut q_targets: Query<(&mut Transform, &mut CellCoord, &ChildOf), Without<Avatar>>,
    q_grids: Query<&Grid>,
) {
    for (avatar_tf, drag, action) in q_avatar.iter() {
        if action.status == lunco_core::architecture::ActionStatus::Running && action.name == "Dragging" {
            if let Ok((mut t_tf, mut t_cell, t_child_of)) = q_targets.get_mut(drag.target) {
                let Ok(grid) = q_grids.get(t_child_of.0) else { continue; };
                
                let forward = avatar_tf.forward();
                let desired_world_pos = avatar_tf.translation.as_dvec3() + forward.as_dvec3() * drag.distance as f64;
                
                let (new_cell, new_tf) = grid.translation_to_grid(desired_world_pos);
                *t_cell = new_cell;
                t_tf.translation = new_tf;
            }
        }
    }
}

/// Handles DRAG commands from CLI or UI.
fn on_drag_commands(
    trigger: On<CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform), With<Avatar>>,
) {
    let msg = trigger.event();
    let (avatar_ent, _avatar_tf) = q_avatar.get(msg.source).unwrap_or((q_avatar.iter().next().unwrap().0, q_avatar.iter().next().unwrap().1));
    
    match msg.name.as_str() {
        "START_DRAG" => {
            let target = Entity::from_bits(msg.args[0] as u64);
            let distance = msg.args[1] as f32;
            commands.entity(avatar_ent).insert((
                DragActivity { target, distance },
                ActiveAction { name: "Dragging".to_string(), status: ActionStatus::Running, progress: 0.0 }
            ));
        },
        "UPDATE_DRAG" => {
            // Update logic would go here if needed
        },
        "STOP_DRAG" => {
            commands.entity(avatar_ent).remove::<DragActivity>();
            commands.entity(avatar_ent).remove::<ActiveAction>();
        },
        _ => {}
    }
}

/// Adaptive clipping logic to prevent depth buffer precision loss.
fn update_avatar_clip_planes_system(
    mut q_camera: Query<(&mut Projection, &Transform, &CellCoord, &ChildOf), (With<Camera>, With<AdaptiveNearPlane>)>,
    q_bodies: Query<(&CelestialBody, &Transform, &CellCoord, &ChildOf)>,
    q_grids: Query<&Grid>,
) {
    for (mut projection, cam_tf, cam_cell, cam_child_of) in q_camera.iter_mut() {
        let Ok(grid) = q_grids.get(cam_child_of.0) else { continue; };
        let cam_pos = grid.grid_position_double(cam_cell, cam_tf);
        
        if let Projection::Perspective(ref mut perspective) = *projection {
            perspective.far = 1.0e15;
            let mut min_dist = 1.0e15;
            
            for (body, b_tf, b_cell, b_child_of) in q_bodies.iter() {
                let Ok(b_grid) = q_grids.get(b_child_of.0) else { continue; };
                // Approximate distance for clipping
                let b_pos = b_grid.grid_position_double(b_cell, b_tf);
                let d = cam_pos.distance(b_pos) - body.radius_m;
                if d < min_dist { min_dist = d; }
            }
            perspective.near = (min_dist as f32 * 0.01).clamp(0.1, 100.0);
        }
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
            commands.trigger(lunco_core::architecture::CommandMessage {
                id: 0,
                target: entity,
                name: "RELEASE".to_string(),
                args: smallvec::smallvec![],
                source: entity,
            });
        }
    }
}

/// Handles global `"RELEASE"` CommandMessages to decouple the avatar from a vessel.
fn on_release_command(
    trigger: On<lunco_core::architecture::CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(&Transform, &CellCoord, &ChildOf), With<Avatar>>,
    q_grids: Query<&Grid>,
) {
    let msg = trigger.event();
    if msg.name == "RELEASE" {
        let avatar_ent = msg.target;
        
        // Read current state to prevent snapping
        let (pos, yaw, pitch) = if let Ok((tf, cell, child_of)) = q_avatar.get(avatar_ent) {
             let Ok(grid) = q_grids.get(child_of.0) else { return; };
             let p = grid.grid_position_double(cell, tf);
             let (y, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
             (p, y, pitch)
        } else { (bevy::math::DVec3::ZERO, 0.0, 0.0) };

        commands.entity(avatar_ent).remove::<ControllerLink>();
        commands.entity(avatar_ent).remove::<DetachedCamera>();
        // IntentAnalogState must NOT be removed, as input systems depend on it.
        commands.entity(avatar_ent).remove::<OrbitalBehavior>();
        commands.entity(avatar_ent).remove::<SurfaceBehavior>();
        
        // Return to flyby mode, preserving current state
        commands.entity(avatar_ent).insert(FlybyBehavior {
            target: None,
            offset: pos, 
            yaw: yaw,
            pitch: pitch,
        });
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
        let avatar_ent: Entity = if let Ok(e) = q_avatar.get(msg.source) { e } else { q_avatar.iter().next().unwrap() };
        
        // Transitions ensure mutual exclusivity between behaviors
        commands.entity(avatar_ent).remove::<FlybyBehavior>();
        commands.entity(avatar_ent).remove::<SurfaceBehavior>();
        commands.entity(avatar_ent).insert((
            ControllerLink { vessel_entity: msg.target },
            OrbitalBehavior { 
                target: Some(msg.target), 
                distance: 10.0, 
                pitch: -0.5, 
                yaw: 0.0, 
                vertical_offset: 1.0, 
                damping: 0.1,
                use_target_frame: true,
            },
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
/// Handles global `"FOCUS"` CommandMessages to adjust orbital camera distance or target.
fn on_focus_command(
    trigger: On<lunco_core::architecture::CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<Entity, With<Avatar>>,
) {
    let msg = trigger.event();
    if msg.name == "FOCUS" {
        let avatar_ent: Entity = if let Ok(e) = q_avatar.get(msg.source) { e } else { q_avatar.iter().next().unwrap() };
        
        // Transitions ensure mutual exclusivity between behaviors
        commands.entity(avatar_ent).remove::<FlybyBehavior>();
        commands.entity(avatar_ent).remove::<SurfaceBehavior>();
        commands.entity(avatar_ent).insert(OrbitalBehavior { 
            target: Some(msg.target), 
            distance: if msg.args.len() > 0 { msg.args[0] } else { 10.0 }, 
            pitch: -0.5, 
            yaw: 0.0, 
            vertical_offset: 1.0, 
            damping: 0.1,
            use_target_frame: true,
        });
    }
}
