//! Implementation of the user's presence and interaction within the simulation.
//!
//! This crate defines the [Avatar] entity, which handles camera logic,
//! focus transitions, and vessel possession. The camera architecture uses
//! composable behavior components (`SpringArmCamera`, `OrbitCamera`, `FreeFlightCamera`) rather
//! than a monolithic state machine, enabling modular frame-aware operation
//! and smooth transitions between reference frames.
//!
//! # Architecture
//!
//! Each camera behavior is its own component with a dedicated system:
//! - **`SpringArmCamera`**: Chase camera locked to a vessel's heading (rovers, astronauts).
//! - **`OrbitCamera`**: Survey camera locked to the ecliptic/stars (planets, spacecraft).
//! - **`FreeFlightCamera`**: Free-moving camera in absolute coordinates (ghost/drone view).
//!
//! Transitions use `FrameBlend` with pre-computed endpoints for smooth "frame handoffs."

use bevy::prelude::*;
use bevy::math::DVec3;
use leafwing_input_manager::prelude::*;
use big_space::prelude::{Grid, CellCoord, FloatingOrigin};

use lunco_controller::ControllerLink;
use lunco_core::{Vessel, Avatar, ActiveAction, ActionStatus, CommandMessage, CelestialBody, Spacecraft};
use lunco_celestial::CelestialClock;

mod intents;
pub use intents::*;

// ─── Resources ───────────────────────────────────────────────────────────────

/// Mouse sensitivity for look rotation speed.
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

/// Tracks cumulative mouse scroll delta for zoom control.
///
/// This is the input bridge between egui UI systems and camera zoom logic.
/// The egui system in `lunco-client` adds to `delta`; camera systems consume it.
#[derive(Resource, Default)]
pub struct CameraScroll {
    pub delta: f32,
}

/// Camera scroll sensitivity in meters per scroll unit.
#[derive(Resource)]
pub struct CameraScrollSensitivity {
    pub value: f32,
}

impl Default for CameraScrollSensitivity {
    fn default() -> Self {
        Self { value: 0.1 }
    }
}

/// Global default values for camera behavior parameters.
///
/// Individual behavior components can override these with their own values
/// (using `Option<f32>` fields). When `None`, the system falls back to this resource.
#[derive(Resource)]
pub struct CameraDefaults {
    pub damping: f32,
    pub transition_duration: f32,
    pub default_distance: f64,
}

impl Default for CameraDefaults {
    fn default() -> Self {
        Self {
            damping: 0.1,
            transition_duration: 1.0,
            default_distance: 10.0,
        }
    }
}

// ─── Behavior Components ─────────────────────────────────────────────────────

/// Chase camera: follows a vessel with heading-locked offset.
///
/// **Reference Frame**: `Vessel` — the camera rotates with the vessel's heading,
/// providing a snappy pilot-follow perspective. Uses exponential smoothing on
/// rotation to filter out physics jitter at 60Hz.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct SpringArmCamera {
    pub target: Entity,
    pub distance: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub damping: Option<f32>,
    pub vertical_offset: f32,
}

/// Survey camera: orbits a target fixed to the stars.
///
/// **Reference Frame**: `Ecliptic` — the camera does NOT rotate with the target.
/// This keeps stars stationary while the planet rotates beneath you.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct OrbitCamera {
    pub target: Entity,
    pub distance: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub damping: Option<f32>,
    pub vertical_offset: f32,
}

/// Free-flight camera: moves independently of any target.
///
/// **Reference Frame**: `Ecliptic` — absolute solar system coordinates.
/// Used for ghost/drone observation and as the default camera state.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct FreeFlightCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub damping: Option<f32>,
}

/// Smooth focus transition with pre-computed endpoints.
///
/// Both source and target frames resolve to the same absolute solar coordinate
/// space. We compute `end_pos` and `end_rot` once at transition start (when the
/// target's pose is known), then lerp every frame. This gives a true "frame
/// handoff" without recomputing both frames per tick.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct FrameBlend {
    pub start_pos: DVec3,
    pub end_pos: DVec3,
    pub start_rot: Quat,
    pub end_rot: Quat,
    pub t: f32,
    pub duration: f32,
    pub target: Entity,
    pub possess_target: Option<Entity>,
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

// ─── Plugin ──────────────────────────────────────────────────────────────────

/// Plugin for managing user avatar logic, input processing, and possession.
pub struct LunCoAvatarPlugin;

impl Plugin for LunCoAvatarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraScroll>()
           .init_resource::<CameraScrollSensitivity>()
           .init_resource::<MouseSensitivity>()
           .init_resource::<CameraDefaults>();
        app.add_plugins(InputManagerPlugin::<UserIntent>::default());
        app.add_observer(on_user_intent);
        app.add_observer(on_possess_command);
        app.add_observer(on_release_command);
        app.add_observer(on_focus_command);
        app.add_observer(on_drag_commands);

        app.register_type::<SpringArmCamera>()
           .register_type::<OrbitCamera>()
           .register_type::<FreeFlightCamera>()
           .register_type::<FrameBlend>()
           .register_type::<AdaptiveNearPlane>()
           .register_type::<MouseSensitivity>();

        app.add_systems(Update, (
            avatar_init_system,
            capture_avatar_intent,
            avatar_behavior_input_system,
            avatar_drag_lifecycle,
            avatar_raycast_possession,
            avatar_escape_possession,
            avatar_global_hotkeys,
        ));

        // Mutual exclusion: FrameBlend runs first. Then exactly one behavior
        // system runs (whichever component is present). Locomotion only moves
        // FreeFlightCamera camera.
        app.add_systems(PostUpdate, (
            frame_blend_system,
            spring_arm_system,
            orbit_system,
            freeflight_system,
            avatar_universal_locomotion_system,
            update_avatar_clip_planes_system,
        ).chain().in_set(AvatarCameraSet));

        app.configure_sets(
            PostUpdate,
            AvatarCameraSet
                .after(avian3d::schedule::PhysicsSystems::Writeback)
                .before(bevy::transform::TransformSystems::Propagate)
        );
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct AvatarCameraSet;

// ─── Avatar Camera Factory ───────────────────────────────────────────────────

/// Spawns a fully-configured avatar camera entity.
///
/// Call this from setup code instead of manually assembling the avatar entity.
/// Ensures consistency between the main client and the sandbox binary.
///
/// # Arguments
/// * `commands` — Bevy commands for entity spawning.
/// * `grid_entity` — The big_space grid entity to parent the avatar to.
/// * `initial_offset` — Starting position offset in grid-local coordinates.
///
/// # Returns
/// The spawned entity ID.
pub fn spawn_avatar_camera(
    commands: &mut Commands,
    grid_entity: Entity,
    initial_offset: DVec3,
) -> Entity {
    let (yaw, pitch) = (std::f32::consts::PI * 0.5, -0.3);
    commands.spawn((
        Camera3d::default(),
        FreeFlightCamera { yaw, pitch, damping: None },
        AdaptiveNearPlane,
        Transform::from_translation(initial_offset.as_vec3()),
        GlobalTransform::default(),
        FloatingOrigin,
        CellCoord::default(),
        Avatar,
        IntentAnalogState::default(),
        ActionState::<lunco_core::UserIntent>::default(),
        lunco_controller::get_avatar_input_map(),
        Name::new("Avatar Camera"),
    )).set_parent_in_place(grid_entity).id()
}

// ─── Behavior Systems ────────────────────────────────────────────────────────

/// SpringArmCamera system: positions the camera behind a target vessel with
/// heading-locked offset and exponential rotation smoothing.
///
/// Only runs when `SpringArmCamera` is present AND no `FrameBlend` is active.
/// Scroll wheel adjusts `distance`. Right-click drag adjusts `yaw`/`pitch`
/// via `avatar_behavior_input_system`.
fn spring_arm_system(
    time: Res<Time>,
    mut q_avatar: Query<(Entity, &mut Transform, &mut CellCoord, &mut SpringArmCamera, &ChildOf), (With<Avatar>, Without<FrameBlend>)>,
    q_spatial: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_spatial_abs: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    defaults: Res<CameraDefaults>,
    mut scroll_res: ResMut<CameraScroll>,
    sens: Res<CameraScrollSensitivity>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    // Skip when CTRL is held — user is in momentary free-flight mode.
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) { return; }

    let dt = time.delta_secs();

    for (_avatar_ent, mut tf, mut cell, mut arm, child_of) in q_avatar.iter_mut() {
        // Safety: skip if target doesn't exist.
        let Ok((t_cell, t_tf)) = q_spatial.get(arm.target) else { continue; };
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };

        let target_abs_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
            arm.target, t_cell, t_tf, &q_parents, &q_grids, &q_spatial_abs,
        );
        let cam_grid_abs = if child_of.0 == Entity::PLACEHOLDER { DVec3::ZERO } else {
            lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                child_of.0, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial_abs,
            )
        };
        let target_pos = target_abs_pos - cam_grid_abs;

        // Scroll zoom.
        if scroll_res.delta != 0.0 {
            let zoom = scroll_res.delta as f64 * -sens.value as f64;
            arm.distance = (arm.distance + zoom).clamp(2.0, 200.0);
            scroll_res.delta = 0.0;
        }

        // Resolve target heading in f64 to eliminate quantization jitter.
        let target_fwd = t_tf.rotation.mul_vec3(Vec3::Z).as_dvec3();
        let target_heading = if target_fwd.x.abs() > 1e-6 || target_fwd.z.abs() > 1e-6 {
            target_fwd.x.atan2(target_fwd.z)
        } else { 0.0 };

        let damping = arm.damping.unwrap_or(defaults.damping);
        let desired_rot = Quat::from_euler(EulerRot::YXZ, target_heading as f32 + arm.yaw, arm.pitch, 0.0);

        // Exponential smoothing for snappy, jitter-free follow.
        let rot_alpha = 1.0 - (-60.0 * (1.0 - damping) * dt).exp();
        tf.rotation = tf.rotation.slerp(desired_rot, rot_alpha);

        // Desired camera position: behind target along smoothed rotation.
        let offset = tf.rotation.mul_vec3(Vec3::Z).as_dvec3() * arm.distance;
        let desired_pos = target_pos + offset + Vec3::Y.as_dvec3() * arm.vertical_offset as f64;

        // Position: lerp toward desired (spring-like follow).
        let current_pos = grid.grid_position_double(&cell, &tf);
        let pos_alpha = (dt * 150.0 * (1.0 - damping)).min(1.0) as f64;
        let next_pos = current_pos.lerp(desired_pos, pos_alpha);

        let (new_cell, new_tf) = grid.translation_to_grid(next_pos);
        *cell = new_cell;
        tf.translation = new_tf;
    }
}

/// OrbitCamera system: positions the camera at a fixed offset from a target,
/// locked to the ecliptic (star-fixed) reference frame.
///
/// Only runs when `OrbitCamera` is present AND no `FrameBlend` is active.
/// The camera does NOT rotate with the target — stars stay still.
fn orbit_system(
    time: Res<Time>,
    mut q_avatar: Query<(Entity, &mut Transform, &mut CellCoord, &mut OrbitCamera, &ChildOf), (With<Avatar>, Without<FrameBlend>)>,
    q_spatial: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_spatial_abs: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    defaults: Res<CameraDefaults>,
    mut scroll_res: ResMut<CameraScroll>,
    sens: Res<CameraScrollSensitivity>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    // Skip when CTRL is held — user is in momentary free-flight mode.
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) { return; }

    let dt = time.delta_secs();

    for (_avatar_ent, mut tf, mut cell, mut orbit, child_of) in q_avatar.iter_mut() {
        let Ok((t_cell, t_tf)) = q_spatial.get(orbit.target) else { continue; };
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };

        let target_abs_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
            orbit.target, t_cell, t_tf, &q_parents, &q_grids, &q_spatial_abs,
        );
        let cam_grid_abs = if child_of.0 == Entity::PLACEHOLDER { DVec3::ZERO } else {
            lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                child_of.0, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial_abs,
            )
        };
        let target_pos = target_abs_pos - cam_grid_abs;

        // Scroll zoom.
        if scroll_res.delta != 0.0 {
            let zoom = scroll_res.delta as f64 * -sens.value as f64;
            orbit.distance = (orbit.distance + zoom).clamp(1.0, 1.0e11);
            scroll_res.delta = 0.0;
        }

        // Camera rotation is purely user-controlled (ecliptic-locked).
        let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
        let offset = rotation.mul_vec3(Vec3::Z).as_dvec3() * orbit.distance;
        let desired_pos = target_pos + offset + Vec3::Y.as_dvec3() * orbit.vertical_offset as f64;

        let damping = orbit.damping.unwrap_or(defaults.damping);
        let current_pos = grid.grid_position_double(&cell, &tf);
        let dist_to_desired = current_pos.distance(desired_pos);
        let lerp_factor = if dist_to_desired > 100.0 { 1.0 } else { (dt * 150.0 * (1.0 - damping)).min(1.0) as f64 };
        let next_pos = current_pos.lerp(desired_pos, lerp_factor);

        let (new_cell, new_tf) = grid.translation_to_grid(next_pos);
        *cell = new_cell;
        tf.translation = new_tf;

        // Look at target.
        let forward = (target_pos - next_pos).normalize().as_vec3();
        if forward.length_squared() > 0.01 {
            let target_point = new_tf + forward;
            tf.look_at(target_point, Vec3::Y);
        }
    }
}

/// FreeFlightCamera system: moves the camera in absolute coordinates.
///
/// Only runs when `FreeFlightCamera` is present AND no `FrameBlend` is active.
/// Position is set by `avatar_universal_locomotion_system`. This system
/// applies yaw/pitch rotation from user input.
fn freeflight_system(
    mut q_avatar: Query<(&mut Transform, &mut FreeFlightCamera), (With<Avatar>, Without<FrameBlend>)>,
) {
    for (mut tf, ff) in q_avatar.iter_mut() {
        // Apply rotation from accumulated yaw/pitch.
        let rotation = Quat::from_euler(EulerRot::YXZ, ff.yaw, ff.pitch, 0.0);
        tf.rotation = rotation;
    }
}

/// Frame blend system: smoothly interpolates between pre-computed
/// start and end camera positions/rotations.
///
/// On completion, inserts the appropriate behavior component (`SpringArmCamera`
/// for possession, `OrbitCamera` for focus) and removes itself.
fn frame_blend_system(
    time: Res<Time>,
    mut q_avatar: Query<(Entity, &mut Transform, &mut CellCoord, &mut FrameBlend, &ChildOf), With<Avatar>>,
    q_spatial: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_spatial_abs: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();

    for (avatar_ent, mut tf, mut cell, mut blend, child_of) in q_avatar.iter_mut() {
        blend.t += dt;
        let raw_t = (blend.t / blend.duration).clamp(0.0, 1.0);
        // Ease-in-out quadratic.
        let ease_t = if raw_t < 0.5 {
            2.0 * raw_t * raw_t
        } else {
            1.0 - (-2.0 * raw_t + 2.0).powi(2) / 2.0
        };

        let current_pos = blend.start_pos.lerp(blend.end_pos, ease_t as f64);
        let current_rot = blend.start_rot.slerp(blend.end_rot, ease_t);

        let Ok(grid) = q_grids.get(child_of.0) else { continue; };
        let cam_grid_abs = if child_of.0 == Entity::PLACEHOLDER { DVec3::ZERO } else {
            lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                child_of.0, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial_abs,
            )
        };

        let (new_cell, new_tf) = grid.translation_to_grid(current_pos - cam_grid_abs);
        *cell = new_cell;
        tf.translation = new_tf;
        tf.rotation = current_rot;

        if raw_t >= 1.0 {
            // Attach ControllerLink if this was a possession transition.
            if let Some(vessel) = blend.possess_target {
                commands.entity(avatar_ent)
                    .insert(ControllerLink { vessel_entity: vessel });
                commands.entity(vessel).insert((
                    ActionState::<lunco_controller::VesselIntent>::default(),
                    lunco_controller::get_default_input_map(),
                ));

                // Determine SpringArmCamera config based on target archetype.
                let (dist, vert_off, damp) = if q_spatial.get(vessel).is_ok() {
                    // Check if it's a rover — use tighter follow.
                    (15.0, 2.0, 0.05)
                } else {
                    (50.0, 5.0, 0.1)
                };

                commands.entity(avatar_ent)
                    .insert(SpringArmCamera {
                        target: vessel,
                        distance: dist,
                        yaw: 0.0,
                        pitch: -0.25,
                        damping: Some(damp),
                        vertical_offset: vert_off,
                    });
            } else {
                // Focus transition — insert OrbitCamera.
                commands.entity(avatar_ent)
                    .insert(OrbitCamera {
                        target: blend.target,
                        distance: blend.end_pos.distance(blend.start_pos).max(10.0),
                        yaw: 0.0,
                        pitch: blend.end_rot.to_euler(EulerRot::YXZ).1,
                        damping: None,
                        vertical_offset: 0.0,
                    });
            }

            commands.entity(avatar_ent).remove::<FrameBlend>();
        }
    }
}

// ─── Locomotion ──────────────────────────────────────────────────────────────

/// Moves the avatar entity in absolute coordinates.
///
/// Only active when `FreeFlightCamera` is present. When CTRL is held while possessing
/// a vessel, this system temporarily drives the FreeFlightCamera camera independently
/// without removing the underlying `SpringArmCamera`.
fn avatar_universal_locomotion_system(
    mut q_avatar: Query<(&mut Transform, &mut CellCoord, &ChildOf, &IntentAnalogState, Has<FreeFlightCamera>), With<Avatar>>,
    q_grids: Query<&Grid>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    for (mut tf, mut cell, child_of, analog, has_freeflight) in q_avatar.iter_mut() {
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };
        let current_pos = grid.grid_position_double(&cell, &tf);

        // Only move if we have FreeFlightCamera (standalone or CTRL-overlay).
        if !has_freeflight && !ctrl_pressed { continue; }

        let curr_is_moving = analog.forward.abs() > 0.01 || analog.side.abs() > 0.01 || analog.elevation.abs() > 0.01;
        if !curr_is_moving { continue; }

        let mut move_vec = Vec3::ZERO;
        move_vec += *tf.forward() * analog.forward;
        move_vec += *tf.right() * analog.side;
        move_vec += Vec3::Y * analog.elevation;

        let next_pos = current_pos + move_vec.as_dvec3() * 33.0 * (1.0 / 60.0);
        let (new_cell, new_tf) = grid.translation_to_grid(next_pos);
        *cell = new_cell;
        tf.translation = new_tf;
    }
}

// ─── Intent & Input ──────────────────────────────────────────────────────────

/// Captures high-level [UserIntent] signals and forwards zoom input.
fn capture_avatar_intent(
    mut q_avatar: Query<(Entity, &IntentState, &mut IntentAnalogState), With<Avatar>>,
    _mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    mut last_mouse_pos: Local<Option<Vec2>>,
    clock: Option<Res<CelestialClock>>,
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
        let d = intent_state.axis_pair(&UserIntent::Look);
        if d.length_squared() > 0.00001 { delta = d * 10.0; mouse_moved = true; }

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
        analog.timestamp = clock.as_ref().map(|c| c.epoch).unwrap_or_default();

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

/// Applies look deltas from `IntentAnalogState` to whichever behavior
/// component is currently active on the avatar.
///
/// When CTRL is held (momentary free-flight overlay), look deltas are
/// applied directly to the Transform rotation since the behavior systems
/// (SpringArmCamera/OrbitCamera) are skipped during this time.
fn avatar_behavior_input_system(
    q_avatar: Query<&IntentAnalogState, With<Avatar>>,
    mut q_spring: Query<&mut SpringArmCamera, With<Avatar>>,
    mut q_orbit: Query<&mut OrbitCamera, With<Avatar>>,
    mut q_freeflight: Query<&mut FreeFlightCamera, With<Avatar>>,
    mut q_tf: Query<&mut Transform, (With<Avatar>, Without<FrameBlend>)>,
    sensitivity: Res<MouseSensitivity>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    // Only process look input when right mouse button is held.
    if !mouse.pressed(MouseButton::Right) { return; }

    let Ok(analog) = q_avatar.single() else { return; };
    let look_delta = analog.look_delta;
    if look_delta.length_squared() < 0.0001 { return; }

    let delta_yaw = -look_delta.x * sensitivity.sensitivity * 0.01;
    let delta_pitch = -look_delta.y * sensitivity.sensitivity * 0.01;
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    if ctrl_pressed {
        // Momentary free-flight: apply look deltas directly to Transform.
        if let Ok(mut tf) = q_tf.single_mut() {
            let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
            tf.rotation = Quat::from_euler(EulerRot::YXZ, yaw + delta_yaw, (pitch + delta_pitch).clamp(-1.5, 1.5), 0.0);
        }
    } else {
        // Normal mode: apply to the active behavior component.
        if let Ok(mut arm) = q_spring.single_mut() {
            arm.yaw += delta_yaw;
            arm.pitch = (arm.pitch + delta_pitch).clamp(-1.5, 1.5);
        }
        if let Ok(mut orbit) = q_orbit.single_mut() {
            orbit.yaw += delta_yaw;
            orbit.pitch = (orbit.pitch + delta_pitch).clamp(-1.5, 1.5);
        }
        if let Ok(mut ff) = q_freeflight.single_mut() {
            ff.yaw += delta_yaw;
            ff.pitch = (ff.pitch + delta_pitch).clamp(-1.5, 1.5);
        }
    }
}

fn avatar_global_hotkeys(q_avatar: Query<&IntentState, With<Avatar>>, mut clock: Option<ResMut<CelestialClock>>) {
    for intent_state in q_avatar.iter() {
        if intent_state.just_pressed(&UserIntent::Pause) {
            if let Some(clock) = clock.as_deref_mut() {
                clock.paused = !clock.paused;
            }
        }
    }
}

// ─── Drag ────────────────────────────────────────────────────────────────────

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
        "STOP_DRAG" => { commands.entity(avatar_ent).remove::<DragActivity>().remove::<ActiveAction>(); },
        _ => {}
    }
}

// ─── Raycasting ──────────────────────────────────────────────────────────────

fn avatar_raycast_possession(mouse: Res<ButtonInput<MouseButton>>, windows: Query<&Window>, camera_q: Query<(&Camera, &GlobalTransform, Entity), With<Avatar>>, mut commands: Commands, q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>, q_spacecraft: Query<(Entity, &GlobalTransform, &Spacecraft)>, q_rovers: Query<(Entity, &GlobalTransform), With<Vessel>>) {
    if !mouse.just_pressed(MouseButton::Left) { return; }
    let Some(pos) = windows.iter().next().and_then(|w| w.cursor_position()) else { return; };
    let Some((camera, cam_gtf, avatar_entity)) = camera_q.iter().next() else { return; };
    let Ok(ray) = camera.viewport_to_world(cam_gtf, pos) else { return; };
    let mut nearest = None; let mut min_t = f32::INFINITY; let mut is_possessable = false;
    for (entity, gtf, body) in q_bodies.iter() {
        let oc = ray.origin - gtf.translation(); let b = oc.dot(ray.direction.as_vec3()); let c = oc.dot(oc) - (body.radius_m as f32).powi(2);
        let discr = b * b - c; if discr >= 0.0 { let t = -b - discr.sqrt(); if t > 0.0 && t < min_t { min_t = t; nearest = Some(entity); is_possessable = false; } }
    }
    for (entity, gtf, sc) in q_spacecraft.iter() {
        let oc = ray.origin - gtf.translation(); let b = oc.dot(ray.direction.as_vec3()); let c = oc.dot(oc) - sc.hit_radius_m.powi(2);
        let discr = b * b - c; if discr >= 0.0 { let t = -b - discr.sqrt(); if t > 0.0 && t < min_t { min_t = t; nearest = Some(entity); is_possessable = true; } }
    }
    for (entity, gtf) in q_rovers.iter() {
        let oc = ray.origin - gtf.translation(); let b = oc.dot(ray.direction.as_vec3()); let c = oc.dot(oc) - 100.0; // 10m squared radius
        let discr = b * b - c; if discr >= 0.0 { let t = -b - discr.sqrt(); if t > 0.0 && t < min_t { min_t = t; nearest = Some(entity); is_possessable = true; } }
    }
    if let Some(target) = nearest { if is_possessable { commands.trigger(CommandMessage { id: 0, target, name: "POSSESS".to_string(), args: smallvec::smallvec![], source: avatar_entity }); } else { commands.trigger(CommandMessage { id: 0, target, name: "FOCUS".to_string(), args: smallvec::smallvec![], source: avatar_entity }); } }
}

fn avatar_escape_possession(keys: Res<ButtonInput<KeyCode>>, mut q_avatar: Query<Entity, (With<Avatar>, With<ControllerLink>)>, mut commands: Commands) {
    if keys.just_pressed(KeyCode::Backspace) { for entity in q_avatar.iter_mut() { commands.trigger(CommandMessage { id: 0, target: entity, name: "RELEASE".to_string(), args: smallvec::smallvec![], source: entity }); } }
}

// ─── Commands ────────────────────────────────────────────────────────────────

/// Releases possession of a vessel.
///
/// Keeps the camera at its current position — no jarring teleport.
/// Switches to `FreeFlightCamera` mode with the current orientation preserved.
fn on_release_command(
    trigger: On<CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(&Transform, Option<&ControllerLink>), With<Avatar>>,
) {
    let msg = trigger.event();
    if msg.name == "RELEASE" {
        let avatar_ent = msg.target;
        let (yaw, pitch, opt_link) = if let Ok((tf, link)) = q_avatar.get(avatar_ent) {
            let (y, p, _) = tf.rotation.to_euler(EulerRot::YXZ);
            (y, p, link)
        } else { (0.0, 0.0, None) };

        // Hard stop the rover upon disengaging control.
        if let Some(link) = opt_link {
            commands.trigger(CommandMessage {
                id: msg.id,
                target: link.vessel_entity,
                name: "DRIVE_ROVER".to_string(),
                args: smallvec::smallvec![0.0, 0.0],
                source: avatar_ent,
            });
        }

        commands.entity(avatar_ent)
            .remove::<ControllerLink>()
            .remove::<SpringArmCamera>()
            .remove::<OrbitCamera>()
            .remove::<FrameBlend>()
            .insert(FreeFlightCamera {
                yaw,
                pitch,
                damping: None,
            });
        info!("Released possession → FreeFlightCamera at current position");
    }
}

/// Forwards drive commands to the possessed vessel.
///
/// CTRL acts as a momentary inhibit: when held, the rover receives zero
/// intents, preventing accidental movement while the user is flying the camera.
fn on_user_intent(trigger: On<IntentAnalogState>, q_avatar: Query<&ControllerLink, With<Avatar>>, mut commands: Commands, keys: Res<ButtonInput<KeyCode>>, mut was_ctrl: Local<bool>) {
    let analog = trigger.event();
    let avatar_entity = trigger.entity;
    let is_ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let is_moving = analog.forward.abs() > 0.01 || analog.side.abs() > 0.01;

    if let Ok(link) = q_avatar.get(avatar_entity) {
        // When CTRL is held, inhibit rover commands (user is flying the camera).
        if is_ctrl {
            if !*was_ctrl {
                // Just pressed Ctrl — stop the rover.
                commands.trigger(CommandMessage {
                    id: analog.timestamp as u64,
                    target: link.vessel_entity,
                    name: "DRIVE_ROVER".to_string(),
                    args: smallvec::smallvec![0.0, 0.0],
                    source: avatar_entity,
                });
            }
            *was_ctrl = is_ctrl;
            return;
        }

        // Ctrl was just released — send current intent to resume rover.
        let ctrl_just_released = *was_ctrl && !is_ctrl;
        if is_moving || ctrl_just_released {
            commands.trigger(CommandMessage {
                id: analog.timestamp as u64,
                target: link.vessel_entity,
                name: "DRIVE_ROVER".to_string(),
                args: smallvec::smallvec![analog.forward as f64, analog.side as f64],
                source: avatar_entity,
            });
        }
    }
    *was_ctrl = is_ctrl;
}

/// Possesses a vessel with a smooth camera transition.
///
/// Uses `FrameBlend` to smoothly fly to the chase position over 0.8 seconds,
/// then lands in `SpringArmCamera` mode.
fn on_possess_command(
    trigger: On<CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf), With<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial_abs: Query<(&CellCoord, &Transform), Without<Avatar>>,
) {
    let msg = trigger.event();
    if msg.name == "POSSESS" {
        let (avatar_ent, cam_tf, cam_cell, _child_of) = if let Ok(data) = q_avatar.get(msg.source) {
            data
        } else {
            let Some(first) = q_avatar.iter().next() else { return; };
            first
        };

        // Compute current absolute position for smooth transition start.
        let start_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
            avatar_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial_abs,
        );

        // Compute target vessel heading for camera behind-it.
        let end_yaw = if let Ok((_t_cell, t_tf)) = q_spatial_abs.get(msg.target) {
            let fwd = t_tf.rotation.mul_vec3(Vec3::Z);
            if fwd.length_squared() > 0.001 { fwd.x.atan2(fwd.z) } else { 0.0 }
        } else { 0.0 };

        let end_rot = Quat::from_euler(EulerRot::YXZ, end_yaw + 0.0, -0.25, 0.0);
        let end_offset = end_rot.mul_vec3(Vec3::Z).as_dvec3() * 15.0;
        let target_abs_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
            msg.target, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial_abs,
        );
        let end_pos = target_abs_pos + end_offset + Vec3::Y.as_dvec3() * 2.0;

        commands.entity(avatar_ent)
            .remove::<SpringArmCamera>()
            .remove::<OrbitCamera>()
            .remove::<FreeFlightCamera>()
            .remove::<FrameBlend>()
            .remove::<ControllerLink>()
            .insert(FrameBlend {
                start_pos,
                end_pos,
                start_rot: cam_tf.rotation,
                end_rot,
                t: 0.0,
                duration: 0.8,
                target: msg.target,
                possess_target: Some(msg.target),
            });
    }
}

/// Focuses on a target with a smooth transition to OrbitCamera mode.
fn on_focus_command(
    trigger: On<CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf), With<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial_abs: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_bodies: Query<&CelestialBody>,
    q_sc: Query<&Spacecraft>,
) {
    let msg = trigger.event();
    if msg.name == "FOCUS" {
        let (avatar_ent, cam_tf, cam_cell, _child_of) = if let Ok(data) = q_avatar.get(msg.source) {
            data
        } else {
            let Some(first) = q_avatar.iter().next() else { return; };
            first
        };

        let start_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
            avatar_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial_abs,
        );
        let (current_yaw, current_pitch, _) = cam_tf.rotation.to_euler(EulerRot::YXZ);

        // Compute distance based on target type.
        let mut distance = 20.0;
        if msg.args.len() > 0 {
            distance = msg.args[0];
        } else if let Ok(body) = q_bodies.get(msg.target) {
            distance = body.radius_m * 3.0;
        } else if let Ok(sc) = q_sc.get(msg.target) {
            distance = (sc.hit_radius_m as f64 * 5.0).max(100.0);
        }

        let target_abs_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
            msg.target, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial_abs,
        );
        let end_rot = Quat::from_euler(EulerRot::YXZ, current_yaw, current_pitch, 0.0);
        let end_offset = end_rot.mul_vec3(Vec3::Z).as_dvec3() * distance;
        let end_pos = target_abs_pos + end_offset;

        commands.entity(avatar_ent)
            .remove::<SpringArmCamera>()
            .remove::<OrbitCamera>()
            .remove::<FreeFlightCamera>()
            .remove::<FrameBlend>()
            .insert(FrameBlend {
                start_pos,
                end_pos,
                start_rot: cam_tf.rotation,
                end_rot,
                t: 0.0,
                duration: 1.5,
                target: msg.target,
                possess_target: None,
            });
    }
}

/// Initializes avatar entities that lack a behavior component.
///
/// Inserts `FreeFlightCamera` as the default behavior with the entity's
/// current transform orientation.
fn avatar_init_system(
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform), (With<Avatar>, Without<SpringArmCamera>, Without<OrbitCamera>, Without<FreeFlightCamera>, Without<FrameBlend>)>,
    q_proj: Query<Entity, (With<Avatar>, Without<AdaptiveNearPlane>, With<Projection>)>,
) {
    for (entity, tf) in q_avatar.iter() {
        let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
        commands.entity(entity).insert(FreeFlightCamera {
            yaw,
            pitch,
            damping: None,
        });
    }
    for entity in q_proj.iter() {
        commands.entity(entity).insert(AdaptiveNearPlane);
    }
}

// ─── Clip Planes ─────────────────────────────────────────────────────────────

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
                if let Ok(b_grid) = q_grids.get(b_child_of.0) {
                    let d = cam_pos.distance(b_grid.grid_position_double(b_cell, b_tf)) - body.radius_m;
                    if d < min_dist { min_dist = d; }
                }
            }
            perspective.near = (min_dist as f32 * 0.01).clamp(0.1, 100.0);
        }
    }
}
