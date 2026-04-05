//! Implementation of the user's presence and interaction within the simulation.
//!
//! This crate defines the [Avatar] entity, which handles camera logic,
//! focus transitions, and vessel possession.

use bevy::prelude::*;
use bevy::math::DVec3;
use leafwing_input_manager::prelude::*;
use big_space::prelude::{Grid, CellCoord};

use lunco_controller::ControllerLink;
use lunco_core::{Vessel, Avatar, ActiveAction, ActionStatus, CommandMessage, CelestialBody, Spacecraft};
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
        
        app.register_type::<ObserverBehavior>()
           .register_type::<ObserverMode>()
           .register_type::<AdaptiveNearPlane>()
           .register_type::<MouseSensitivity>();

        app.add_systems(Update, (
            avatar_init_system,
            capture_avatar_intent,
            avatar_behavior_input_system,
            (
                avatar_observer_system,
                avatar_transition_system,
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

#[derive(Reflect, Clone, Debug, PartialEq)]
pub enum ObserverMode {
    Orbital,
    Flyby,
    Surface,
}

/// Unified camera behavior state machine.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct ObserverBehavior {
    pub target: Option<Entity>,
    pub mode: ObserverMode,
    pub distance: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub damping: f32,
    pub vertical_offset: f32,
    pub use_target_frame: bool,
    /// Offset used specifically during Flyby mode.
    pub flyby_offset: DVec3,
}

impl Default for ObserverBehavior {
    fn default() -> Self {
        Self {
            target: None,
            mode: ObserverMode::Flyby,
            distance: 100.0,
            yaw: 0.0,
            pitch: -0.5,
            damping: 0.1,
            vertical_offset: 0.0,
            use_target_frame: true,
            flyby_offset: DVec3::ZERO,
        }
    }
}

/// Smooth focus transition state.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct TransitionBehavior {
    pub target: Entity,
    pub start_pos_solar: DVec3,
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
            start_pos_solar: DVec3::ZERO,
            start_rot: Quat::IDENTITY,
            end_dist: 10.0,
            end_pitch: 0.0,
            end_yaw: 0.0,
            duration: 1.0,
            elapsed: 0.0,
        }
    }
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

/// Marker component for an avatar that is currently in a "detached" free-look mode.
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
            if is_detached { commands.entity(entity).remove::<DetachedCamera>(); }
            else { commands.entity(entity).insert(DetachedCamera); }
        }
    }
}

/// Captures high-level [UserIntent] signals.
fn capture_avatar_intent(
    mut q_avatar: Query<(Entity, &IntentState, &mut IntentAnalogState), With<Avatar>>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    mut last_mouse_pos: Local<Option<Vec2>>,
    clock: Res<CelestialClock>,
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
            if d.length_squared() > 0.00001 { delta = d * 10.0; mouse_moved = true; }
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

/// Applies look deltas to the active behavior.
fn avatar_behavior_input_system(
    mut q_avatar: Query<(&IntentAnalogState, Option<&mut ObserverBehavior>), With<Avatar>>,
    sensitivity: Res<MouseSensitivity>,
) {
    for (analog, mut obs_opt) in q_avatar.iter_mut() {
        if analog.look_delta.length_squared() < 0.0001 { continue; }
        let delta_yaw = -analog.look_delta.x * sensitivity.sensitivity * 0.01;
        let delta_pitch = -analog.look_delta.y * sensitivity.sensitivity * 0.01;

        if let Some(ref mut obs) = obs_opt {
            obs.yaw += delta_yaw;
            obs.pitch = (obs.pitch + delta_pitch).clamp(-1.5, 1.5);
        }
    }
}

/// Handles gimbals and frame-linking.
fn avatar_unified_orientation_system(
    mut q_avatar: Query<(&mut Transform, Option<&ObserverBehavior>, Has<DetachedCamera>), With<Avatar>>,
    q_spatial: Query<&Transform, Without<Avatar>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    for (mut tf, obs_opt, is_detached) in q_avatar.iter_mut() {
        let Some(obs) = obs_opt else { continue; };
        let mut rotation = Quat::from_euler(EulerRot::YXZ, obs.yaw, obs.pitch, 0.0);
        
        if obs.use_target_frame && !is_detached && !ctrl_pressed {
            if let Some(target) = obs.target {
                if let Ok(t_tf) = q_spatial.get(target) {
                    rotation = t_tf.rotation * rotation;
                }
            }
        }
        tf.rotation = rotation;
    }
}

fn avatar_global_hotkeys(q_avatar: Query<&IntentState, With<Avatar>>, mut clock: ResMut<CelestialClock>) {
    for intent_state in q_avatar.iter() {
        if intent_state.just_pressed(&UserIntent::Pause) { clock.paused = !clock.paused; }
    }
}

/// Core system: Unified Observer Logic.
fn avatar_observer_system(
    time: Res<Time>,
    mut q_avatar: Query<(Entity, &mut Transform, &mut CellCoord, &mut ObserverBehavior, &ChildOf, Has<DetachedCamera>), (With<Avatar>, Without<TransitionBehavior>)>,
    q_spatial: Query<(&CellCoord, &Transform, Option<&CelestialBody>, Option<&Spacecraft>), Without<Avatar>>,
    q_spatial_abs: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    mut scroll_res: ResMut<CameraScroll>,
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let dt = time.delta_secs();
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    for (avatar_ent, mut tf, mut cell, mut obs, child_of, is_detached) in q_avatar.iter_mut() {
        let Some(target_ent) = obs.target else { continue; };
        let Ok((t_cell, t_tf, t_body, t_sc)) = q_spatial.get(target_ent) else { continue; };
        let target_radius = t_body.map(|b| b.radius_m).or(t_sc.map(|s| s.hit_radius_m as f64)).unwrap_or(1.0);
        let min_dist = target_radius * 1.5;

        // 1. Unified Scroll Zoom
        if scroll_res.delta != 0.0 {
            let scroll = scroll_res.delta as f64 * -0.01;
            obs.distance = (obs.distance - (scroll * (obs.distance * 0.1))).clamp(min_dist, 1.0e11);
            scroll_res.delta = 0.0;
        }

        // 2. Mode State Machine
        if let Some(body) = t_body {
            let current_pos = q_grids.get(child_of.0).unwrap().grid_position_double(&cell, &tf);
            let target_pos = q_grids.get(child_of.0).unwrap().grid_position_double(t_cell, t_tf);
            let altitude = current_pos.distance(target_pos) - body.radius_m;
            
            if obs.mode == ObserverMode::Orbital && altitude < body.radius_m * 0.5 {
                obs.mode = ObserverMode::Flyby;
                obs.flyby_offset = current_pos;
                info!("Observer: Switched to FLYBY");
            } else if obs.mode == ObserverMode::Flyby && altitude > body.radius_m * 0.7 {
                obs.mode = ObserverMode::Orbital;
                info!("Observer: Switched to ORBITAL");
            }
        }

        if is_detached || ctrl_pressed { continue; }

        // 3. Coordinate Sync (Grid Migration)
        let Ok(target_child_of) = q_parents.get(target_ent) else { continue; };
        let target_grid_ent = target_child_of.parent();
        if child_of.parent() != target_grid_ent {
            let abs_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(target_ent, t_cell, t_tf, &q_parents, &q_grids, &q_spatial_abs);
            let Ok(grid) = q_grids.get(target_grid_ent) else { continue; };
            let (new_cell, new_tf_trans) = grid.translation_to_grid(abs_pos);
            *cell = new_cell; tf.translation = new_tf_trans;
            commands.entity(avatar_ent).set_parent_in_place(target_grid_ent);
            continue; 
        }

        // 4. Movement Logic
        let Ok(grid) = q_grids.get(child_of.parent()) else { continue; };
        let target_pos = grid.grid_position_double(t_cell, t_tf);

        if obs.mode == ObserverMode::Orbital {
            let mut rotation = Quat::from_euler(EulerRot::YXZ, obs.yaw, obs.pitch, 0.0);
            if obs.use_target_frame { rotation = t_tf.rotation * rotation; }
            let offset = rotation.mul_vec3(Vec3::Z).as_dvec3() * obs.distance;
            let desired_pos = target_pos + offset + Vec3::Y.as_dvec3() * obs.vertical_offset as f64;
            
            let lerp_factor = (dt * 30.0 * (1.0 - obs.damping)).min(1.0) as f64;
            let current_pos = grid.grid_position_double(&cell, &tf);
            let next_pos = current_pos.lerp(desired_pos, lerp_factor);

            let (new_cell, new_tf) = grid.translation_to_grid(next_pos);
            *cell = new_cell; tf.translation = new_tf;

            let forward = (target_pos - next_pos).normalize().as_vec3();
            if forward.length_squared() > 0.01 { 
                let target_point = tf.translation + forward;
                tf.look_at(target_point, Vec3::Y); 
            }
        } else {
            let (new_cell, new_tf) = grid.translation_to_grid(obs.flyby_offset);
            *cell = new_cell; tf.translation = new_tf;
        }
    }
}

/// Absolute focus transition system.
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
        let raw_t = (trans.elapsed / trans.duration).clamp(0.0, 1.0);
        let t = if raw_t < 0.5 { 2.0 * raw_t * raw_t } else { 1.0 - (-2.0 * raw_t + 2.0).powi(2) / 2.0 };
        let ease_t = t as f64;

        let Ok((t_cell, t_tf, _t_child_of)) = q_spatial.get(trans.target) else {
             commands.entity(avatar_ent).remove::<TransitionBehavior>(); continue;
        };
        let target_abs_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(trans.target, t_cell, t_tf, &q_parents, &q_grids, &q_spatial_abs);

        let end_rot = Quat::from_euler(EulerRot::YXZ, trans.end_yaw, trans.end_pitch, 0.0);
        let end_offset_solar = end_rot.mul_vec3(Vec3::Z).as_dvec3() * trans.end_dist;
        let end_pos_solar = target_abs_pos + end_offset_solar;

        let current_pos_solar = trans.start_pos_solar.lerp(end_pos_solar, ease_t);
        let current_rot = trans.start_rot.slerp(end_rot, ease_t as f32);

        let Ok(cam_grid) = q_grids.get(child_of.0) else { continue; };
        let cam_grid_abs_pos = if child_of.0 == Entity::PLACEHOLDER { DVec3::ZERO } else {
            lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(child_of.0, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial_abs)
        };

        let (new_cell, new_tf_trans) = cam_grid.translation_to_grid(current_pos_solar - cam_grid_abs_pos);
        *cell = new_cell; tf.translation = new_tf_trans; tf.rotation = current_rot;

        if raw_t >= 1.0 {
            commands.entity(avatar_ent).remove::<TransitionBehavior>();
            commands.entity(avatar_ent).insert(ObserverBehavior {
                target: Some(trans.target), distance: trans.end_dist,
                pitch: trans.end_pitch, yaw: trans.end_yaw,
                mode: ObserverMode::Orbital, ..default()
            });
            info!("Transition Completed");
        }
    }
}

/// Shared locomotion system.
fn avatar_universal_locomotion_system(
    time: Res<Time>,
    mut q_avatar: Query<(&mut Transform, &mut CellCoord, &ChildOf, Option<&ControllerLink>, Has<DetachedCamera>, &IntentAnalogState, Option<&mut ObserverBehavior>), With<Avatar>>,
    q_grids: Query<&Grid>,
    keys: Res<ButtonInput<KeyCode>>,
    mut scroll_res: ResMut<CameraScroll>,
    mut spatial: ParamSet<(Query<(&CellCoord, &Transform, &ChildOf, Option<&CelestialBody>, Option<&Spacecraft>), Without<Avatar>>, )>,
) {
    let dt = time.delta_secs() as f64;
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    for (mut tf, mut cell, child_of, possessed, is_detached, analog, mut obs_opt) in q_avatar.iter_mut() {
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };
        let current_pos = grid.grid_position_double(&cell, &tf);
        let mut speed = 100.0;

        if let Some(ref obs) = obs_opt {
            if let Some(target) = obs.target {
                if let Ok((t_cell, t_tf, t_child_of, t_body, t_sc)) = spatial.p0().get(target) {
                    let mut radius = 0.0; let mut scale = false;
                    if let Some(body) = t_body { radius = body.radius_m; scale = true; }
                    else if let Some(sc) = t_sc { radius = sc.hit_radius_m as f64; scale = true; }
                    if scale { if let Ok(t_grid) = q_grids.get(t_child_of.0) { speed = (current_pos.distance(t_grid.grid_position_double(t_cell, t_tf)) - radius).max(10.0) * 0.5; } }
                }
            }
        }

        let mut move_vec = Vec3::ZERO;
        if obs_opt.is_some() && scroll_res.delta != 0.0 {
            move_vec += *tf.forward() * scroll_res.delta * 0.1 * (speed as f32 / dt as f32); 
            scroll_res.delta = 0.0;
        }

        let is_unlocked = is_detached || ctrl_pressed || (possessed.is_none() && obs_opt.as_ref().map(|o| o.mode == ObserverMode::Flyby).unwrap_or(true));
        if is_unlocked {
            move_vec += *tf.forward() * analog.forward;
            move_vec += *tf.right() * analog.side;
            move_vec += Vec3::Y * analog.elevation;
        }

        if move_vec.length_squared() < 0.00001 { continue; }
        let next_pos = current_pos + move_vec.as_dvec3() * speed * dt;
        let (new_cell, new_tf) = grid.translation_to_grid(next_pos);
        *cell = new_cell; tf.translation = new_tf;
        if let Some(ref mut obs) = obs_opt { if obs.mode == ObserverMode::Flyby { obs.flyby_offset = next_pos; } }
    }
}

fn avatar_drag_lifecycle(q_avatar: Query<(&Transform, &DragActivity, &ActiveAction), With<Avatar>>, mut q_targets: Query<(&mut Transform, &mut CellCoord, &ChildOf), Without<Avatar>>, q_grids: Query<&Grid>) {
    for (avatar_tf, drag, action) in q_avatar.iter() {
        if action.status == ActionStatus::Running && action.name == "Dragging" {
            if let Ok((mut t_tf, mut t_cell, t_child_of)) = q_targets.get_mut(drag.target) {
                let Ok(grid) = q_grids.get(t_child_of.0) else { continue; };
                let (new_cell, new_tf) = grid.translation_to_grid(avatar_tf.translation.as_dvec3() + avatar_tf.forward().as_dvec3() * drag.distance as f64);
                *t_cell = new_cell; t_tf.translation = new_tf;
            }
        }
    }
}

fn on_drag_commands(trigger: On<CommandMessage>, mut commands: Commands, q_avatar: Query<(Entity, &Transform), With<Avatar>>) {
    let msg = trigger.event();
    let (avatar_ent, _) = q_avatar.get(msg.source).unwrap_or((q_avatar.iter().next().unwrap().0, q_avatar.iter().next().unwrap().1));
    match msg.name.as_str() {
        "START_DRAG" => { commands.entity(avatar_ent).insert((DragActivity { target: Entity::from_bits(msg.args[0] as u64), distance: msg.args[1] as f32 }, ActiveAction { name: "Dragging".to_string(), status: ActionStatus::Running, progress: 0.0 })); },
        "STOP_DRAG" => { commands.entity(avatar_ent).remove::<DragActivity>().remove::<ActiveAction>(); }, _ => {}
    }
}

fn update_avatar_clip_planes_system(mut q_camera: Query<(&mut Projection, &Transform, &CellCoord, &ChildOf), (With<Camera>, With<AdaptiveNearPlane>)>, q_bodies: Query<(&CelestialBody, &Transform, &CellCoord, &ChildOf)>, q_grids: Query<&Grid>) {
    for (mut projection, cam_tf, cam_cell, cam_child_of) in q_camera.iter_mut() {
        let Ok(grid) = q_grids.get(cam_child_of.0) else { continue; };
        let cam_pos = grid.grid_position_double(cam_cell, cam_tf);
        if let Projection::Perspective(ref mut perspective) = *projection {
            perspective.far = 1.0e15; let mut min_dist = 1.0e15;
            for (body, b_tf, b_cell, b_child_of) in q_bodies.iter() {
                if let Ok(b_grid) = q_grids.get(b_child_of.0) {
                    let d = cam_pos.distance(b_grid.grid_position_double(b_cell, b_tf)) - body.radius_m;
                    if d < min_dist { min_dist = d; }
                }
            }
            perspective.near = (min_dist as f32 * 0.01).clamp(0.1, 100.0);
        }
    }
}

fn avatar_raycast_possession(mouse: Res<ButtonInput<MouseButton>>, windows: Query<&Window>, camera_q: Query<(&Camera, &GlobalTransform, Entity), With<Avatar>>, mut commands: Commands, q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>, q_spacecraft: Query<(Entity, &GlobalTransform, &Spacecraft)>, q_rovers: Query<(Entity, &GlobalTransform), With<Vessel>>) {
    if !mouse.just_pressed(MouseButton::Left) { return; }
    let Some(pos) = windows.iter().next().and_then(|w| w.cursor_position()) else { return };
    let Some((camera, cam_gtf, avatar_entity)) = camera_q.iter().next() else { return };
    let Ok(ray) = camera.viewport_to_world(cam_gtf, pos) else { return };
    let mut nearest = None; let mut min_t = f32::INFINITY; let mut is_possessable = false;
    for (entity, gtf, body) in q_bodies.iter() {
        let oc = ray.origin - gtf.translation(); let b = oc.dot(ray.direction.as_vec3()); let c = oc.dot(oc) - (body.radius_m as f32).powi(2);
        let discr = b * b - c; if discr >= 0.0 { let t = -b - discr.sqrt(); if t > 0.0 && t < min_t { min_t = t; nearest = Some(entity); is_possessable = false; } }
    }
    for (entity, gtf, sc) in q_spacecraft.iter() {
        let oc = ray.origin - gtf.translation(); let b = oc.dot(ray.direction.as_vec3()); let c = oc.dot(oc) - sc.hit_radius_m.powi(2);
        let discr = b * b - c; if discr >= 0.0 { let t = -b - discr.sqrt(); if t > 0.0 && t < min_t { min_t = t; nearest = Some(entity); is_possessable = true; } }
    }
    if let Some(target) = nearest { if is_possessable { commands.trigger(CommandMessage { id: 0, target, name: "POSSESS".to_string(), args: smallvec::smallvec![], source: avatar_entity }); } else { commands.trigger(CommandMessage { id: 0, target, name: "FOCUS".to_string(), args: smallvec::smallvec![], source: avatar_entity }); } }
}

fn avatar_escape_possession(keys: Res<ButtonInput<KeyCode>>, mut q_avatar: Query<Entity, (With<Avatar>, With<ControllerLink>)>, mut commands: Commands) {
    if keys.just_pressed(KeyCode::Backspace) { for entity in q_avatar.iter_mut() { commands.trigger(CommandMessage { id: 0, target: entity, name: "RELEASE".to_string(), args: smallvec::smallvec![], source: entity }); } }
}

fn on_release_command(trigger: On<CommandMessage>, mut commands: Commands, q_avatar: Query<(&Transform, &CellCoord, &ChildOf), With<Avatar>>, q_grids: Query<&Grid>) {
    let msg = trigger.event();
    if msg.name == "RELEASE" {
        let avatar_ent = msg.target;
        let (pos, yaw, pitch) = if let Ok((tf, cell, child_of)) = q_avatar.get(avatar_ent) {
             let Ok(grid) = q_grids.get(child_of.0) else { return; };
             let (y, p, _) = tf.rotation.to_euler(EulerRot::YXZ); (grid.grid_position_double(cell, tf), y, p)
        } else { (DVec3::ZERO, 0.0, 0.0) };
        commands.entity(avatar_ent).remove::<ControllerLink>().remove::<DetachedCamera>().remove::<ObserverBehavior>().insert(ObserverBehavior { target: None, mode: ObserverMode::Flyby, flyby_offset: pos, yaw, pitch, ..default() });
    }
}

fn on_user_intent(trigger: On<IntentAnalogState>, q_avatar: Query<&ControllerLink, With<Avatar>>, mut commands: Commands, keys: Res<ButtonInput<KeyCode>>, mut last_state: Local<(bool, bool)>) {
    let analog = trigger.event(); let avatar_entity = trigger.entity; let is_ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let is_moving = analog.forward.abs() > 0.01 || analog.side.abs() > 0.01; let (was_ctrl, was_moving) = *last_state;
    if let Ok(link) = q_avatar.get(avatar_entity) {
        let needs_stop = (is_ctrl && !was_ctrl) || (!is_ctrl && was_moving && !is_moving);
        if is_ctrl { if needs_stop { commands.trigger(CommandMessage { id: analog.timestamp as u64, target: link.vessel_entity, name: "DRIVE_ROVER".to_string(), args: smallvec::smallvec![0.0, 0.0], source: avatar_entity }); } *last_state = (is_ctrl, is_moving); return; }
        if is_moving || needs_stop { commands.trigger(CommandMessage { id: analog.timestamp as u64, target: link.vessel_entity, name: "DRIVE_ROVER".to_string(), args: smallvec::smallvec![analog.forward as f64, analog.side as f64], source: avatar_entity }); }
    }
    *last_state = (is_ctrl, is_moving);
}

fn on_possess_command(trigger: On<CommandMessage>, mut commands: Commands, q_avatar: Query<(Entity, &Transform, Option<&ObserverBehavior>), With<Avatar>>, q_sc: Query<&Spacecraft>) {
    let msg = trigger.event();
    if msg.name == "POSSESS" {
        let (avatar_ent, cam_tf, obs_opt) = if let Ok(data) = q_avatar.get(msg.source) { data } else { let (e, tf, o) = q_avatar.iter().next().unwrap(); (e, tf, o) };
        let (yaw, pitch, _) = cam_tf.rotation.to_euler(EulerRot::YXZ);
        let radius = q_sc.get(msg.target).map(|s| s.hit_radius_m as f64).unwrap_or(10.0);
        let distance = obs_opt.map(|o| o.distance).unwrap_or(radius * 5.0).clamp(radius * 1.5, 1.0e11);
        commands.entity(avatar_ent).remove::<ObserverBehavior>().insert((ControllerLink { vessel_entity: msg.target }, ObserverBehavior { target: Some(msg.target), distance, pitch, yaw, mode: ObserverMode::Orbital, ..default() }, IntentAnalogState::default()));
        commands.entity(msg.target).insert((leafwing_input_manager::prelude::ActionState::<lunco_controller::VesselIntent>::default(), lunco_controller::get_default_input_map()));
        commands.trigger(CommandMessage { id: 0, target: msg.target, name: "FOCUS".to_string(), args: smallvec::smallvec![], source: avatar_ent });
    }
}

fn on_focus_command(
    trigger: On<CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf, Option<&ObserverBehavior>), With<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial_abs: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_bodies: Query<&CelestialBody>,
    q_sc: Query<&Spacecraft>,
) {
    let msg = trigger.event();
    if msg.name == "FOCUS" {
        let (avatar_ent, cam_tf, cam_cell, child_of, obs_opt) = if let Ok(data) = q_avatar.get(msg.source) { data } else { let Some(first) = q_avatar.iter().next() else { return; }; first };
        let (current_yaw, current_pitch) = if let Some(o) = obs_opt { (o.yaw, o.pitch) } else { let (y, p, _) = cam_tf.rotation.to_euler(EulerRot::YXZ); (y, p) };
        let mut distance = if let Some(o) = obs_opt { o.distance } else { 20.0 };
        if msg.args.len() > 0 { distance = msg.args[0]; }
        else if obs_opt.is_none() {
            if let Ok(body) = q_bodies.get(msg.target) { distance = body.radius_m * 3.0; }
            else if let Ok(sc) = q_sc.get(msg.target) { distance = (sc.hit_radius_m as f64 * 5.0).max(100.0); }
        }
        let start_pos_solar = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(avatar_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial_abs);
        commands.entity(avatar_ent).remove::<TransitionBehavior>();
        commands.entity(avatar_ent).insert(TransitionBehavior { target: msg.target, start_pos_solar, start_rot: cam_tf.rotation, end_dist: distance, end_pitch: current_pitch, end_yaw: current_yaw, duration: 1.5, elapsed: 0.0 });
    }
}

fn avatar_init_system(mut commands: Commands, q_avatar: Query<(Entity, &Transform), (With<Avatar>, Without<ObserverBehavior>, Without<TransitionBehavior>)>, q_proj: Query<Entity, (With<Avatar>, Without<AdaptiveNearPlane>, With<Projection>)>) {
    for (entity, tf) in q_avatar.iter() {
        let pos = tf.translation.as_dvec3(); let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
        commands.entity(entity).insert(ObserverBehavior { target: None, mode: ObserverMode::Flyby, flyby_offset: pos, yaw, pitch, ..default() });
    }
    for entity in q_proj.iter() { commands.entity(entity).insert(AdaptiveNearPlane); }
}
