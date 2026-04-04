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
use lunco_core::{Vessel, Avatar, ActiveAction, ActionStatus, CommandMessage, CelestialBody, Spacecraft};
use lunco_celestial::{CelestialClock, TrajectoryView};

mod intents;
pub use intents::*;

#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub struct MouseSensitivity {
    pub sensitivity: f32,
}

impl Default for MouseSensitivity {
    fn default() -> Self {
        Self { sensitivity: 0.45 }
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
           .register_type::<TransitionBehavior>()
           .register_type::<AdaptiveNearPlane>()
           .register_type::<MouseSensitivity>();

        app.add_systems(Update, (
            avatar_init_system,
            capture_avatar_intent,
            avatar_behavior_input_system,
            (
                avatar_orbital_system,
                avatar_surface_system,
                avatar_transition_system,
                avatar_altitude_transition_system,
                avatar_universal_locomotion_system,
                avatar_drag_lifecycle,
                avatar_raycast_possession,
                avatar_escape_possession,
                avatar_toggle_detached_mode,
                avatar_global_hotkeys,
                update_avatar_clip_planes_system,
            ).chain(),
        ));
        app.add_systems(PostUpdate, avatar_unified_orientation_system);
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

#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct TransitionBehavior {
    pub target: Entity,
    pub start_pos: bevy::math::DVec3,
    pub start_rot: Quat,
    pub end_dist: f64,
    pub end_pitch: f32,
    pub end_yaw: f32,
    pub duration: f32,
    pub elapsed: f32,
}

impl Default for TransitionBehavior {
    fn default() -> Self {
        Self {
            target: Entity::PLACEHOLDER,
            start_pos: bevy::math::DVec3::ZERO,
            start_rot: Quat::IDENTITY,
            end_dist: 10.0,
            end_pitch: 0.0,
            end_yaw: 0.0,
            duration: 1.0,
            elapsed: 0.0,
        }
    }
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
fn capture_avatar_intent(
    mut q_avatar: Query<(Entity, &IntentState, &mut IntentAnalogState), With<Avatar>>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    mut last_mouse_pos: Local<Option<Vec2>>,
    clock: Res<lunco_celestial::CelestialClock>,
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    scroll_res: ResMut<CameraScroll>,
) {
    let window = windows.iter().next();
    let current_mouse_pos = window.and_then(|w| w.cursor_position());
    let mut delta = Vec2::ZERO;
    let mut mouse_moved = false;
    
    *last_mouse_pos = current_mouse_pos;

    for (entity, intent_state, mut analog) in q_avatar.iter_mut() {
        if mouse.pressed(MouseButton::Right) {
            let d = intent_state.axis_pair(&UserIntent::Look);
            if d.length_squared() > 0.00001 {
                delta = d * 10.0;
                mouse_moved = true;
            }
        }

        let mut forward = 0.0;
        let mut side = 0.0;
        let mut elevation = 0.0;
        
        if intent_state.pressed(&UserIntent::MoveForward) { forward += 1.0; }
        if intent_state.pressed(&UserIntent::MoveBackward) { forward -= 1.0; }
        if intent_state.pressed(&UserIntent::MoveRight) { side += 1.0; }
        if intent_state.pressed(&UserIntent::MoveLeft) { side -= 1.0; }
        if intent_state.pressed(&UserIntent::MoveUp) { elevation += 1.0; }
        if intent_state.pressed(&UserIntent::MoveDown) { elevation -= 1.0; }

        let boost = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) { 10.0 } else { 1.0 };
        
        analog.forward = forward * boost;
        analog.side = side * boost;
        analog.elevation = elevation * boost;
        analog.look_delta = delta;
        analog.timestamp = clock.epoch;
        
        commands.entity(entity).trigger(|e| {
            let mut a = (*analog).clone();
            a.entity = e;
            a
        });

        if forward.abs() > 0.1 || side.abs() > 0.1 || elevation.abs() > 0.1 || mouse_moved || scroll_res.delta.abs() > 0.001 {
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

/// Unified orientation system that handles gimballing and target-frame linking for all behaviors.
fn avatar_unified_orientation_system(
    mut q_avatar: Query<(&mut Transform, Option<&FlybyBehavior>, Option<&OrbitalBehavior>, Option<&SurfaceBehavior>, Has<DetachedCamera>), With<Avatar>>,
    q_spatial: Query<&Transform, Without<Avatar>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    
    for (mut tf, flyby_opt, orbital_opt, surface_opt, is_detached) in q_avatar.iter_mut() {
        let (yaw, pitch, use_target_frame, target_ent) = if let Some(flyby) = flyby_opt {
            (flyby.yaw, flyby.pitch, false, None)
        } else if let Some(orbital) = orbital_opt {
            (orbital.yaw, orbital.pitch, orbital.use_target_frame, orbital.target)
        } else if let Some(surface) = surface_opt {
            (surface.yaw, surface.pitch, surface.use_target_frame, surface.target)
        } else { continue; };

        let mut rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
        
        if use_target_frame && !is_detached && !ctrl_pressed {
            if let Some(target) = target_ent {
                if let Ok(t_tf) = q_spatial.get(target) {
                    rotation = t_tf.rotation * rotation;
                }
            }
        }
        tf.rotation = rotation;
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
        }
    }
}

/// Updates the Avatar's transform and cell based on OrbitalBehavior.
fn avatar_orbital_system(
    time: Res<Time>,
    mut q_avatar: Query<(Entity, &mut Transform, &mut CellCoord, &mut OrbitalBehavior, &ChildOf, Has<DetachedCamera>), With<Avatar>>,
    q_spatial: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    mut scroll_res: ResMut<CameraScroll>,
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let dt = time.delta_secs();
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    for (avatar_ent, mut tf, mut cell, mut orbital, child_of, is_detached) in q_avatar.iter_mut() {
        if scroll_res.delta != 0.0 {
            let scroll = scroll_res.delta as f64 * -0.01;
            orbital.distance = (orbital.distance - (scroll * (orbital.distance * 0.1))).clamp(1.0, 1.0e11);
            scroll_res.delta = 0.0;
        }

        if is_detached || ctrl_pressed { continue; }

        let Some(target_ent) = orbital.target else { continue; };
        let Ok((t_cell, t_tf)) = q_spatial.get(target_ent) else { continue; };
        let Ok(target_child_of) = q_parents.get(target_ent) else { continue; };
        let target_grid_ent = target_child_of.parent();

        if child_of.parent() != target_grid_ent {
            let target_abs_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(target_ent, t_cell, t_tf, &q_parents, &q_grids, &q_spatial);
            let Ok(target_grid) = q_grids.get(target_grid_ent) else { continue; };
            let (new_cell, new_tf_translation) = target_grid.translation_to_grid(target_abs_pos);
            *cell = new_cell;
            tf.translation = new_tf_translation;
            commands.entity(avatar_ent).set_parent_in_place(target_grid_ent);
            continue; 
        }

        let Ok(grid) = q_grids.get(child_of.parent()) else { continue; };
        let mut rotation = Quat::from_euler(EulerRot::YXZ, orbital.yaw, orbital.pitch, 0.0);
        if orbital.use_target_frame {
            rotation = t_tf.rotation * rotation;
        }

        let target_pos = grid.grid_position_double(t_cell, t_tf);
        let offset = rotation.mul_vec3(Vec3::Z).as_dvec3() * orbital.distance;
        let desired_pos = target_pos + offset + Vec3::Y.as_dvec3() * orbital.vertical_offset as f64;
        
        let lerp_factor = (dt * 30.0 * (1.0 - orbital.damping)).min(1.0) as f64;
        let current_pos = grid.grid_position_double(&cell, &tf);
        let next_pos = current_pos.lerp(desired_pos, lerp_factor);

        let (new_cell, new_tf) = grid.translation_to_grid(next_pos);
        *cell = new_cell;
        tf.translation = new_tf;

        let forward = (target_pos - next_pos).normalize().as_vec3();
        if forward.length_squared() > 0.01 {
            let target_point = tf.translation + forward;
            tf.look_at(target_point, Vec3::Y);
        }
    }
}

/// Updates the Avatar's transform and cell based on SurfaceBehavior.
fn avatar_surface_system(
    time: Res<Time>,
    mut q_avatar: Query<(&mut Transform, &mut CellCoord, &mut SurfaceBehavior, &ChildOf, Has<DetachedCamera>), With<Avatar>>,
    q_spatial: Query<(&CellCoord, &Transform), (Without<Avatar>, With<Vessel>)>,
    q_grids: Query<&Grid>,
    keys: Res<ButtonInput<KeyCode>>,
    mut scroll_res: ResMut<CameraScroll>,
) {
    let dt = time.delta_secs();
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    for (mut tf, mut cell, mut surface, child_of, is_detached) in q_avatar.iter_mut() {
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };
        
        if scroll_res.delta != 0.0 {
            surface.height = (surface.height - scroll_res.delta * 2.0).clamp(1.0, 1000.0);
            scroll_res.delta = 0.0;
        }

        if is_detached || ctrl_pressed { continue; }

        let Some(target_ent) = surface.target else { continue; };
        let Ok((t_cell, t_tf)) = q_spatial.get(target_ent) else { continue; };

        let target_pos = grid.grid_position_double(t_cell, t_tf);
        let up: Dir3 = if surface.lock_up { t_tf.up() } else { Dir3::Y };
        let mut rotation = Quat::from_euler(EulerRot::YXZ, surface.yaw, surface.pitch, 0.0);
        if surface.use_target_frame {
            rotation = t_tf.rotation * rotation;
        }
        let offset = rotation.mul_vec3(Vec3::Z) * 10.0; 
        
        let desired_pos = target_pos + offset.as_dvec3() + up.as_dvec3() * surface.height as f64;
        let lerp_factor = (dt * 5.0 * (1.0 - surface.damping)).min(1.0) as f64;
        let current_pos = grid.grid_position_double(&cell, &tf);
        let next_pos = current_pos.lerp(desired_pos, lerp_factor);

        let (new_cell, new_tf) = grid.translation_to_grid(next_pos);
        *cell = new_cell;
        tf.translation = new_tf;
    }
}

/// Smoothly interpolates the avatar toward a new focus target.
fn avatar_transition_system(
    time: Res<Time>,
    mut q_avatar: Query<(Entity, &mut Transform, &mut CellCoord, &mut TransitionBehavior, &ChildOf), With<Avatar>>,
    q_spatial: Query<(&CellCoord, &Transform, &ChildOf), Without<Avatar>>,
    q_spatial_abs: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();

    for (avatar_ent, mut tf, mut cell, mut trans, child_of) in q_avatar.iter_mut() {
        trans.elapsed += dt;
        let t = (trans.elapsed / trans.duration).clamp(0.0, 1.0) as f64;
        let ease_t = t * t * (3.0 - 2.0 * t); // Quadratic ease in-out

        let Ok((t_cell, t_tf, _t_child_of)) = q_spatial.get(trans.target) else {
             commands.entity(avatar_ent).remove::<TransitionBehavior>();
             continue;
        };
        let target_abs_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(trans.target, t_cell, t_tf, &q_parents, &q_grids, &q_spatial_abs);

        let end_rot = Quat::from_euler(EulerRot::YXZ, trans.end_yaw, trans.end_pitch, 0.0);
        let end_offset = end_rot.mul_vec3(Vec3::Z).as_dvec3() * trans.end_dist;
        let end_abs_pos = target_abs_pos + end_offset;

        let current_abs_pos = trans.start_pos.lerp(end_abs_pos, ease_t);
        let current_rot = trans.start_rot.slerp(end_rot, ease_t as f32);

        let Ok(cam_grid) = q_grids.get(child_of.0) else { continue; };
        // Determine the absolute position of the camera's current grid origin
        let cam_grid_abs_pos = if child_of.0 == Entity::PLACEHOLDER { bevy::math::DVec3::ZERO } else {
            // Use the absolute position of the entity the camera is currently parented to
            lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(child_of.0, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial_abs)
        };

        let (new_cell, new_tf_trans) = cam_grid.translation_to_grid(current_abs_pos - cam_grid_abs_pos);
        *cell = new_cell;
        tf.translation = new_tf_trans;
        tf.rotation = current_rot;

        if t >= 1.0 {
            commands.entity(avatar_ent).remove::<TransitionBehavior>();
            commands.entity(avatar_ent).insert(OrbitalBehavior {
                target: Some(trans.target),
                distance: trans.end_dist,
                pitch: trans.end_pitch,
                yaw: trans.end_yaw,
                vertical_offset: 0.0,
                damping: 0.1,
                use_target_frame: true,
            });
        }
    }
}

/// Automatically switches between behaviors based on altitude/distance to target.
fn avatar_altitude_transition_system(
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf, Option<&OrbitalBehavior>, Option<&FlybyBehavior>), (With<Avatar>, Without<TransitionBehavior>)>,
    q_spatial: Query<(&CellCoord, &Transform, &ChildOf, Option<&CelestialBody>, Option<&Spacecraft>), Without<Avatar>>,
    q_grids: Query<&Grid>,
) {
    for (avatar_ent, tf, cell, child_of, orbital_opt, flyby_opt) in q_avatar.iter() {
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };
        let current_pos = grid.grid_position_double(cell, tf);

        // Determine target and radius from active behavior
        let (target_ent, is_orbital) = if let Some(orbital) = orbital_opt {
            (orbital.target, true)
        } else if let Some(flyby) = flyby_opt {
            (flyby.target, false)
        } else { continue; };

        let Some(target) = target_ent else { continue; };
        
        if let Ok((t_cell, t_tf, t_child_of, t_body, _t_sc)) = q_spatial.get(target) {
            let Ok(t_grid) = q_grids.get(t_child_of.0) else { continue; };
            let t_pos = t_grid.grid_position_double(t_cell, t_tf);
            
            // Only CelestialBodies (Planets/Moons) support auto-transition to Flyby.
            // Spacecraft stay in Orbital mode by default.
            let Some(body) = t_body else { continue; };
            let target_radius = body.radius_m;
            
            let dist = current_pos.distance(t_pos);
            let threshold = target_radius * 1.5; 

            if is_orbital && dist < threshold {
                // Switch Orbital -> Flyby
                let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                commands.entity(avatar_ent).remove::<OrbitalBehavior>();
                commands.entity(avatar_ent).insert(FlybyBehavior {
                    target: Some(target),
                    offset: current_pos,
                    yaw,
                    pitch,
                });
                info!("Auto-transition to FLYBY mode.");
            } else if !is_orbital && dist > threshold * 1.5 {
                // Switch Flyby -> Orbital
                let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                commands.entity(avatar_ent).remove::<FlybyBehavior>();
                commands.entity(avatar_ent).insert(OrbitalBehavior {
                    target: Some(target),
                    distance: dist,
                    yaw,
                    pitch,
                    vertical_offset: 0.0,
                    damping: 0.1,
                    use_target_frame: true,
                });
                info!("Auto-transition to ORBITAL mode.");
            }
        }
    }
}

/// Shared locomotion system.
fn avatar_universal_locomotion_system(
    time: Res<Time>,
    mut q_avatar: Query<(&mut Transform, &mut CellCoord, &ChildOf, Option<&ControllerLink>, Has<DetachedCamera>, &IntentAnalogState, Option<&mut FlybyBehavior>, Option<&OrbitalBehavior>, Option<&SurfaceBehavior>), With<Avatar>>,
    q_grids: Query<&Grid>,
    keys: Res<ButtonInput<KeyCode>>,
    mut scroll_res: ResMut<CameraScroll>,
    mut spatial: ParamSet<(
        Query<(&CellCoord, &Transform, &ChildOf, Option<&CelestialBody>, Option<&Spacecraft>), Without<Avatar>>,
    )>,
) {
    let dt = time.delta_secs() as f64;
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    
    for (mut tf, mut cell, child_of, possessed, is_detached, analog, mut flyby_opt, orbital_opt, surface_opt) in q_avatar.iter_mut() {
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };
        let is_unlocked = (possessed.is_none() && flyby_opt.is_some()) || is_detached || ctrl_pressed;
        if !is_unlocked { continue; }

        let current_pos = grid.grid_position_double(&cell, &tf);
        let mut speed = 100.0;

        let target_ent = flyby_opt.as_ref().and_then(|f| f.target)
            .or_else(|| orbital_opt.and_then(|o| o.target))
            .or_else(|| surface_opt.and_then(|s| s.target));

        if let Some(target) = target_ent {
            if let Ok((t_cell, t_tf, t_child_of, t_body, t_sc)) = spatial.p0().get(target) {
                let mut target_radius = 0.0;
                let mut should_scale = false;
                if let Some(body) = t_body { target_radius = body.radius_m; should_scale = true; }
                else if let Some(sc) = t_sc { target_radius = sc.hit_radius_m as f64; should_scale = true; }

                if should_scale {
                    if let Ok(t_grid) = q_grids.get(t_child_of.0) {
                        let t_pos = t_grid.grid_position_double(t_cell, t_tf);
                        speed = (current_pos.distance(t_pos) - target_radius).max(10.0) * 0.5;
                    }
                }
            }
        }

        let mut move_vec = Vec3::ZERO;
        move_vec += *tf.forward() * analog.forward;
        move_vec += *tf.right() * analog.side;
        move_vec += Vec3::Y * analog.elevation;

        if (flyby_opt.is_some() || orbital_opt.is_some()) && scroll_res.delta != 0.0 {
            move_vec += *tf.forward() * scroll_res.delta * 0.1 * (speed as f32 / dt as f32); 
            scroll_res.delta = 0.0;
        }

        if move_vec.length_squared() < 0.00001 { continue; }
        let next_pos = current_pos + move_vec.as_dvec3() * speed * dt;
        let (new_cell, new_tf) = grid.translation_to_grid(next_pos);
        *cell = new_cell; tf.translation = new_tf;
        if let Some(ref mut flyby) = flyby_opt { flyby.offset = next_pos; }
    }
}

/// Moves the dragged entity.
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
                *t_cell = new_cell; t_tf.translation = new_tf;
            }
        }
    }
}

/// Handles DRAG commands.
fn on_drag_commands(trigger: On<CommandMessage>, mut commands: Commands, q_avatar: Query<(Entity, &Transform), With<Avatar>>) {
    let msg = trigger.event();
    let (avatar_ent, _avatar_tf) = q_avatar.get(msg.source).unwrap_or((q_avatar.iter().next().unwrap().0, q_avatar.iter().next().unwrap().1));
    match msg.name.as_str() {
        "START_DRAG" => {
            let target = Entity::from_bits(msg.args[0] as u64);
            let distance = msg.args[1] as f32;
            commands.entity(avatar_ent).insert((DragActivity { target, distance }, ActiveAction { name: "Dragging".to_string(), status: ActionStatus::Running, progress: 0.0 }));
        },
        "STOP_DRAG" => {
            commands.entity(avatar_ent).remove::<DragActivity>();
            commands.entity(avatar_ent).remove::<ActiveAction>();
        },
        _ => {}
    }
}

/// Adaptive clipping logic.
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
                let b_pos = b_grid.grid_position_double(b_cell, b_tf);
                let d = cam_pos.distance(b_pos) - body.radius_m;
                if d < min_dist { min_dist = d; }
            }
            perspective.near = (min_dist as f32 * 0.01).clamp(0.1, 100.0);
        }
    }
}

/// Allows the user to "possess" a vessel or focus on a body by clicking.
fn avatar_raycast_possession(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform, Entity), With<Avatar>>,
    mut commands: Commands,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>,
    q_spacecraft: Query<(Entity, &GlobalTransform, &Spacecraft)>,
    q_rovers: Query<(Entity, &GlobalTransform), With<Vessel>>,
) {
    if !mouse.just_pressed(MouseButton::Left) { return; }
    let Some(window) = windows.iter().next() else { return };
    let Some(cursor_position) = window.cursor_position() else { return };
    
    let Some((camera, cam_gtf, avatar_entity)) = camera_q.iter().next() else { return };
    let Ok(ray) = camera.viewport_to_world(cam_gtf, cursor_position) else { return };

    let mut nearest_target = None;
    let mut min_t = f32::INFINITY;
    let mut is_possessable = false;

    // 1. Check Celestial Bodies
    for (entity, body_gtf, body) in q_bodies.iter() {
        let center = body_gtf.translation();
        let radius = body.radius_m as f32;
        let oc = ray.origin - center;
        let b = oc.dot(ray.direction.as_vec3());
        let c = oc.dot(oc) - radius * radius;
        let discr = b * b - c;
        if discr >= 0.0 {
            let t = -b - discr.sqrt();
            if t > 0.0 && t < min_t { 
                min_t = t; 
                nearest_target = Some(entity); 
                is_possessable = false; 
            }
        }
    }

    // 2. Check Spacecraft
    for (entity, sc_gtf, sc) in q_spacecraft.iter() {
        let center = sc_gtf.translation();
        let radius = sc.hit_radius_m;
        let oc = ray.origin - center;
        let b = oc.dot(ray.direction.as_vec3());
        let c = oc.dot(oc) - radius * radius;
        let discr = b * b - c;
        if discr >= 0.0 {
            let t = -b - discr.sqrt();
            if t > 0.0 && t < min_t { 
                min_t = t; 
                nearest_target = Some(entity); 
                is_possessable = true; 
            }
        }
    }

    // 3. Check Rovers (using a fixed 10m hit radius for now)
    for (entity, vessel_gtf) in q_rovers.iter() {
        if q_spacecraft.contains(entity) { continue; } // Already checked
        let center = vessel_gtf.translation();
        let radius = 10.0;
        let oc = ray.origin - center;
        let b = oc.dot(ray.direction.as_vec3());
        let c = oc.dot(oc) - radius * radius;
        let discr = b * b - c;
        if discr >= 0.0 {
            let t = -b - discr.sqrt();
            if t > 0.0 && t < min_t { 
                min_t = t; 
                nearest_target = Some(entity); 
                is_possessable = true; 
            }
        }
    }

    if let Some(target) = nearest_target {
        if is_possessable {
            commands.trigger(lunco_core::architecture::CommandMessage { id: 0, target, name: "POSSESS".to_string(), args: smallvec::smallvec![], source: avatar_entity });
        } else {
            commands.trigger(lunco_core::architecture::CommandMessage { id: 0, target, name: "FOCUS".to_string(), args: smallvec::smallvec![], source: avatar_entity });
        }
    }
}

/// Releases possession.
fn avatar_escape_possession(keys: Res<ButtonInput<KeyCode>>, mut q_avatar: Query<Entity, (With<Avatar>, With<ControllerLink>)>, mut commands: Commands) {
    if keys.just_pressed(KeyCode::Backspace) {
        for entity in q_avatar.iter_mut() {
            commands.trigger(lunco_core::architecture::CommandMessage { id: 0, target: entity, name: "RELEASE".to_string(), args: smallvec::smallvec![], source: entity });
        }
    }
}

/// Handles `"RELEASE"`.
fn on_release_command(
    trigger: On<lunco_core::architecture::CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(&Transform, &CellCoord, &ChildOf), With<Avatar>>,
    q_grids: Query<&Grid>,
) {
    let msg = trigger.event();
    if msg.name == "RELEASE" {
        let avatar_ent = msg.target;
        let (pos, yaw, pitch) = if let Ok((tf, cell, child_of)) = q_avatar.get(avatar_ent) {
             let Ok(grid) = q_grids.get(child_of.0) else { return; };
             let p = grid.grid_position_double(cell, tf);
             let (y, p_pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
             (p, y, p_pitch)
        } else { (bevy::math::DVec3::ZERO, 0.0, 0.0) };
        commands.entity(avatar_ent).remove::<ControllerLink>();
        commands.entity(avatar_ent).remove::<DetachedCamera>();
        commands.entity(avatar_ent).remove::<OrbitalBehavior>();
        commands.entity(avatar_ent).remove::<SurfaceBehavior>();
        commands.entity(avatar_ent).insert(FlybyBehavior { target: None, offset: pos, yaw: yaw as f32, pitch: pitch as f32 });
    }
}

/// Observer for intent.
fn on_user_intent(trigger: On<IntentAnalogState>, q_avatar: Query<&ControllerLink, With<Avatar>>, mut commands: Commands, keys: Res<ButtonInput<KeyCode>>, mut last_state: Local<(bool, bool)>) {
    let analog = trigger.event();
    let avatar_entity = trigger.entity;
    let is_ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let is_moving = analog.forward.abs() > 0.01 || analog.side.abs() > 0.01;
    let (was_ctrl, was_moving) = *last_state;
    if let Ok(link) = q_avatar.get(avatar_entity) {
        let needs_stop = (is_ctrl && !was_ctrl) || (!is_ctrl && was_moving && !is_moving);
        if is_ctrl {
            if needs_stop { commands.trigger(lunco_core::architecture::CommandMessage { id: analog.timestamp as u64, target: link.vessel_entity, name: "DRIVE_ROVER".to_string(), args: smallvec::smallvec![0.0, 0.0], source: avatar_entity }); }
            *last_state = (is_ctrl, is_moving); return;
        }
        if is_moving || needs_stop { commands.trigger(lunco_core::architecture::CommandMessage { id: analog.timestamp as u64, target: link.vessel_entity, name: "DRIVE_ROVER".to_string(), args: smallvec::smallvec![analog.forward as f64, analog.side as f64], source: avatar_entity }); }
    }
    *last_state = (is_ctrl, is_moving);
}

/// Handles `"POSSESS"`.
fn on_possess_command(trigger: On<lunco_core::architecture::CommandMessage>, mut commands: Commands, q_avatar: Query<Entity, With<Avatar>>) {
    let msg = trigger.event();
    if msg.name == "POSSESS" {
        let avatar_ent: Entity = if let Ok(e) = q_avatar.get(msg.source) { e } else { q_avatar.iter().next().unwrap() };
        commands.entity(avatar_ent).remove::<FlybyBehavior>();
        commands.entity(avatar_ent).remove::<SurfaceBehavior>();
        commands.entity(avatar_ent).insert((ControllerLink { vessel_entity: msg.target }, OrbitalBehavior { target: Some(msg.target), distance: 10.0, pitch: -0.5, yaw: 0.0, vertical_offset: 1.0, damping: 0.1, use_target_frame: true }, IntentAnalogState::default()));
        commands.entity(msg.target).insert((leafwing_input_manager::prelude::ActionState::<lunco_controller::VesselIntent>::default(), lunco_controller::get_default_input_map()));
        commands.trigger(lunco_core::architecture::CommandMessage { id: 0, target: msg.target, name: "FOCUS".to_string(), args: smallvec::smallvec![10.0], source: avatar_ent });
    }
}

/// Handles `"FOCUS"`.
fn on_focus_command(
    trigger: On<lunco_core::architecture::CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf), With<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial_abs: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_bodies: Query<&lunco_core::CelestialBody>,
) {
    let msg = trigger.event();
    if msg.name == "FOCUS" {
        let (avatar_ent, cam_tf, cam_cell, _child_of) = if let Ok(data) = q_avatar.get(msg.source) { data } else { 
            let Some(first) = q_avatar.iter().next() else { return; }; first
        };
        let mut distance = if msg.args.len() > 0 { msg.args[0] } else { 10.0 };
        if msg.args.len() == 0 {
            if let Ok(body) = q_bodies.get(msg.target) { distance = body.radius_m * 3.0; }
        }
        
        let start_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(avatar_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial_abs);
        
        commands.entity(avatar_ent).remove::<FlybyBehavior>();
        commands.entity(avatar_ent).remove::<SurfaceBehavior>();
        commands.entity(avatar_ent).remove::<OrbitalBehavior>();
        commands.entity(avatar_ent).insert(TransitionBehavior {
            target: msg.target, start_pos, start_rot: cam_tf.rotation,
            end_dist: distance, end_pitch: -0.5, end_yaw: 0.0,
            duration: 1.5, elapsed: 0.0,
        });
    }
}

/// Initializes Avatars that have no active behavior.
fn avatar_init_system(
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform), (With<Avatar>, Without<FlybyBehavior>, Without<OrbitalBehavior>, Without<SurfaceBehavior>, Without<TransitionBehavior>)>,
    q_proj: Query<Entity, (With<Avatar>, Without<AdaptiveNearPlane>, With<Projection>)>,
) {
    for (entity, tf) in q_avatar.iter() {
        let pos = tf.translation.as_dvec3();
        let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
        commands.entity(entity).insert(FlybyBehavior { target: None, offset: pos, yaw, pitch });
    }
    for entity in q_proj.iter() { commands.entity(entity).insert(AdaptiveNearPlane); }
}
