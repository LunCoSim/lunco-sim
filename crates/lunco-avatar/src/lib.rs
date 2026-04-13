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
use lunco_core::{Vessel, Avatar, CommandMessage, CelestialBody, Spacecraft};
use lunco_celestial::{CelestialClock, GravityBody, LocalGravityField};

mod intents;

/// UI panels for avatar status, camera mode, and surface coordinates.
pub mod ui;
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

/// Chase camera: follows a ground vehicle with smooth heading-follow.
///
/// Position snaps directly to the desired offset (no lerp), but rotation
/// slerps smoothly toward the rover's heading + user yaw offset. This creates
/// the natural "swing-around" feel of a proper spring arm camera.
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

/// Surface camera: heading + pitch relative to the local surface normal.
///
/// Unlike `FreeFlightCamera` which accumulates incremental rotations (prone to
/// roll drift from system ordering and coordinate frame mismatches), this
/// component stores absolute heading and pitch angles. The `surface_camera_system`
/// recomputes the full rotation quaternion from scratch every frame using
/// `LocalGravityField.local_up`, guaranteeing zero roll.
///
/// # Design rationale
///
/// The root cause of the surface camera roll bug was threefold:
/// 1. `global_transform_propagation_system` and `big_space` fight over GlobalTransform
/// 2. `freeflight_system` reads `tf.rotation` from the previous frame (may include stale parent rotation)
/// 3. The camera is on the Grid (FloatingOrigin requirement) but math assumed body-local coords
///
/// By recomputing rotation from first principles each frame, all three issues are bypassed.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct SurfaceCamera {
    /// Azimuth from local north, in radians. Positive = counter-clockwise from above.
    pub heading: f32,
    /// Elevation from horizon, in radians. Negative = look down, positive = look up.
    pub pitch: f32,
}

/// Smooth focus transition with target-relative endpoint recomputed each frame.
///
/// Blend positions are in **absolute solar coordinates** (root frame).
/// Each frame, the blended result is converted to the camera's current grid.
/// Rotation is set from `end_yaw`/`end_pitch` so the camera always points
/// at the target during the approach.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct FrameBlend {
    pub target: Entity,
    pub target_grid: Option<Entity>,
    pub source_target: Option<Entity>,
    pub start_offset_from_source: DVec3,
    pub start_rot: Quat,
    pub end_distance: f64,
    pub end_yaw: f32,
    pub end_pitch: f32,
    pub end_vertical_offset: f32,
    pub t: f32,
    pub duration: f32,
    pub possess_target: Option<Entity>,
}

/// Ensures optical stability by adjusting near plane based on surface proximity.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component)]
pub struct AdaptiveNearPlane;

/// Marker component: camera/rover operates in surface-relative mode.
///
/// When present, camera systems use `LocalGravityField.local_up` as "up"
/// instead of the ecliptic Y axis. Movement is tangent to the body surface.
///
/// Inserted/removed automatically by `surface_mode_transition_system` based
/// on altitude thresholds from `SurfaceModeThreshold`.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct SurfaceRelativeMode;

/// Marker for the nested grid created for surface operations.
///
/// **Deprecated**: In the merged Body+Grid design, the camera is parented
/// directly to the Body entity (which IS the Grid). No intermediate surface
/// grid is needed.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct AvatarSurfaceGrid;

/// Tunable thresholds for entering/exiting surface-relative camera mode.
///
/// Hysteresis prevents rapid toggling at boundary altitude:
/// - `engage_altitude` — below this, enter surface mode
/// - `disengage_altitude` — above this, exit surface mode
#[derive(Resource, Reflect, Clone, Debug)]
#[reflect(Resource)]
pub struct SurfaceModeThreshold {
    /// Altitude (m) below which surface mode engages. Default: 50_000.
    pub engage_altitude: f64,
    /// Altitude (m) above which surface mode disengages. Default: 100_000.
    pub disengage_altitude: f64,
}

impl Default for SurfaceModeThreshold {
    fn default() -> Self {
        Self {
            engage_altitude: 50_000.0,
            disengage_altitude: 100_000.0,
        }
    }
}

// ─── Plugin ──────────────────────────────────────────────────────────────────

/// Plugin for managing user avatar logic, input processing, and possession.
pub struct LunCoAvatarPlugin;

impl Plugin for LunCoAvatarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraScroll>()
           .init_resource::<CameraScrollSensitivity>()
           .init_resource::<MouseSensitivity>()
           .init_resource::<CameraDefaults>()
           .init_resource::<SurfaceModeThreshold>();
        app.add_plugins(InputManagerPlugin::<UserIntent>::default());
        app.add_observer(on_user_intent);
        app.add_observer(on_possess_command);
        app.add_observer(on_release_command);
        app.add_observer(on_focus_command);
        app.add_observer(on_surface_teleport_command);
        app.add_observer(on_leave_surface_command);

        app.register_type::<SpringArmCamera>()
           .register_type::<OrbitCamera>()
           .register_type::<ChaseCamera>()
           .register_type::<FreeFlightCamera>()
           .register_type::<FrameBlend>()
           .register_type::<AdaptiveNearPlane>()
           .register_type::<SurfaceRelativeMode>()
           .register_type::<SurfaceCamera>()
           .register_type::<SurfaceModeThreshold>()
           .register_type::<MouseSensitivity>();

        app.add_systems(Update, (
            avatar_init_system,
            capture_avatar_intent,
            avatar_behavior_input_system,
            avatar_raycast_possession,
            avatar_escape_possession,
            avatar_global_hotkeys,
            surface_mode_transition_system,
        ));

        app.add_systems(PostUpdate, (
            spring_arm_system,
            chase_camera_system,
            orbit_system,
            freeflight_system,
            surface_camera_system,
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

/// Chase camera: follows a target in 3D, respecting full orientation
/// (heading, pitch, roll). Used for aircraft and flying vehicles.
///
/// **Reference Frame**: `Target` — the camera rotates with the target's
/// full orientation, offset behind and above it.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct ChaseCamera {
    pub target: Entity,
    pub distance: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub damping: Option<f32>,
    pub vertical_offset: f32,
}

/// SpringArmCamera system: positions the camera behind a ground vehicle with
/// heading-locked offset.
///
/// **Ground-vehicle only** — no surface reference for spacecraft.
/// For aircraft use `ChaseCamera`. For orbit use `OrbitCamera`.
fn spring_arm_system(
    time: Res<Time>,
    mut q_avatar: Query<(
        Entity,
        &mut Transform,
        &mut CellCoord,
        &mut SpringArmCamera,
        &ChildOf,
        Option<&SurfaceRelativeMode>,
    ), (With<Avatar>, Without<FrameBlend>)>,
    q_spatial: Query<(&CellCoord, &Transform), Without<Avatar>>,
    _q_spatial_abs: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    _q_parents: Query<&ChildOf>,
    defaults: Res<CameraDefaults>,
    mut scroll_res: ResMut<CameraScroll>,
    _sens: Res<CameraScrollSensitivity>,
    keys: Res<ButtonInput<KeyCode>>,
    spatial_query: Option<avian3d::prelude::SpatialQuery>,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) { return; }

    let dt = time.delta_secs();

    for (_avatar_ent, mut tf, mut cell, mut arm, child_of, surface_mode) in q_avatar.iter_mut() {
        let Ok((t_cell, t_tf)) = q_spatial.get(arm.target) else { continue; };
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };

        // Target position in grid-local coordinates.
        let target_pos = grid.grid_position_double(t_cell, t_tf);

        // Scroll zoom: fixed multiplier (matches old working code).
        let min_dist = 5.0;
        if scroll_res.delta != 0.0 {
            arm.distance = (arm.distance - scroll_res.delta as f64 * 5.0).clamp(min_dist, 200.0);
            scroll_res.delta = 0.0;
        }

        // Resolve rover heading in double-precision to eliminate quantization jitter.
        let target_fwd_d = t_tf.rotation.mul_vec3(Vec3::Z).as_dvec3();
        let target_heading_d = if target_fwd_d.x.abs() > 1e-6 || target_fwd_d.z.abs() > 1e-6 {
            target_fwd_d.x.atan2(target_fwd_d.z)
        } else { 0.0 };

        // Combine rover heading with user yaw offset.
        let final_yaw = (target_heading_d + arm.yaw as f64) as f32;

        // Rotation: surface-relative or ecliptic-locked
        let desired_rot = if surface_mode.is_some() {
            // "Up" = surface normal at the rover's position = rover's grid-local direction from body center.
            // Both rover and camera are on the Body's Grid; body is at Grid origin.
            let up_d = target_pos;
            let up_v = if up_d.length() > 1e-6 {
                (up_d / up_d.length()).as_vec3()
            } else {
                Vec3::Y
            };

            // Surface mode: compute rotation from scratch using local_up as "up".
            // This avoids accumulated roll drift from incremental rotations
            // (see surface_camera_investigation.md for root cause analysis).
            let ref_dir = if up_v.dot(Vec3::Y).abs() < 0.9 { Vec3::Y } else { Vec3::Z };
            let east = up_v.cross(ref_dir).normalize();
            let north = east.cross(up_v).normalize();

            // Combine rover heading with user yaw offset, applied around surface normal.
            let heading_q = Quat::from_axis_angle(up_v, final_yaw);
            let forward = heading_q.mul_vec3(north);
            let right = forward.cross(up_v).normalize();
            let base_rot = Quat::from_mat3(&Mat3::from_cols(right, up_v, -forward));
            let pitch_q = Quat::from_axis_angle(right, arm.pitch);
            (pitch_q * base_rot).normalize()
        } else {
            Quat::from_euler(EulerRot::YXZ, final_yaw, arm.pitch, 0.0)
        };

        // Rotation: exponential decay for snappy but smooth heading follow.
        // Frequency 60.0 — snappy without transmitting physics jitter.
        let damping = arm.damping.unwrap_or(defaults.damping);
        let rot_alpha = 1.0 - (-60.0 * (1.0 - damping) * dt).exp();
        tf.rotation = tf.rotation.slerp(desired_rot, rot_alpha);

        // Desired camera position: behind target along smoothed rotation.
        let offset = tf.rotation.mul_vec3(Vec3::Z).as_dvec3() * arm.distance;
        let vertical_offset: DVec3 = if surface_mode.is_some() {
            // "Up" = surface normal at rover's position (same computation as rotation)
            if target_pos.length() > 1e-6 {
                (target_pos / target_pos.length()) * arm.vertical_offset as f64
            } else {
                DVec3::Y * arm.vertical_offset as f64
            }
        } else {
            DVec3::Y * arm.vertical_offset as f64
        };
        let desired_pos = target_pos + offset + vertical_offset;

        // Raycast from rover toward desired camera position.
        // If something blocks (wall, ramp, etc.), place the camera on the
        // SAME SIDE as the rover so the user can see through the obstacle.
        let ray_origin = target_pos;
        let ray_dir = (desired_pos - target_pos).normalize_or(DVec3::Y);
        let ray_len = desired_pos.distance(target_pos);
        let filter = avian3d::prelude::SpatialQueryFilter::from_excluded_entities([arm.target]);
        let hit = if let Some(ref sq) = spatial_query {
            sq.cast_ray(
                ray_origin,
                bevy::math::Dir3::new(ray_dir.as_vec3()).unwrap_or(bevy::math::Dir3::Y),
                ray_len,
                true,
                &filter,
            )
        } else {
            None
        };

        let final_pos = if let Some(hit_data) = hit {
            ray_origin + ray_dir * (hit_data.distance - 0.5)
        } else {
            desired_pos
        };

        let (new_cell, new_tf) = grid.translation_to_grid(final_pos);
        *cell = new_cell;
        tf.translation = new_tf;
    }
}

/// ChaseCamera system: follows a target with full 3D orientation follow.
///
/// Used for aircraft and flying vehicles. Respects the target's roll, pitch,
/// and heading — the camera rotates with the vehicle in all axes.
fn chase_camera_system(
    time: Res<Time>,
    mut q_avatar: Query<(Entity, &mut Transform, &mut CellCoord, &mut ChaseCamera, &ChildOf), (With<Avatar>, Without<FrameBlend>)>,
    q_spatial: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    defaults: Res<CameraDefaults>,
    mut scroll_res: ResMut<CameraScroll>,
    sens: Res<CameraScrollSensitivity>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) { return; }

    let dt = time.delta_secs();

    for (_avatar_ent, mut tf, mut cell, mut chase, child_of) in q_avatar.iter_mut() {
        let Ok((t_cell, t_tf)) = q_spatial.get(chase.target) else { continue; };
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };

        // Target position in grid-local coordinates.
        let target_pos = grid.grid_position_double(t_cell, t_tf);

        // Multiplicative zoom using exponential scaling.
        // Scroll up (delta > 0) -> zoom in. Scroll down (delta < 0) -> zoom out.
        let min_dist = 5.0;
        if scroll_res.delta != 0.0 {
            let zoom_factor = (-scroll_res.delta as f64 * sens.value as f64 * 0.01).exp();
            chase.distance = (chase.distance * zoom_factor).clamp(min_dist, 1.0e6);
            scroll_res.delta = 0.0;
        }

        // Follow target's full 3D orientation (heading + pitch + roll).
        // Same formula as SpringArmCamera: target rotation * user offset.
        let rotation = t_tf.rotation * Quat::from_euler(EulerRot::YXZ, chase.yaw, chase.pitch, 0.0);

        let offset = rotation.mul_vec3(Vec3::Z).as_dvec3() * chase.distance;
        let desired_pos = target_pos + offset + Vec3::Y.as_dvec3() * chase.vertical_offset as f64;

        // Position lerp: same formula as SpringArmCamera and old working code.
        let damping = chase.damping.unwrap_or(defaults.damping);
        let lerp_factor = (dt * 30.0 * (1.0 - damping)).min(1.0) as f64;
        let current_pos = grid.grid_position_double(&cell, &tf);
        let next_pos = current_pos.lerp(desired_pos, lerp_factor);

        let (new_cell, new_tf) = grid.translation_to_grid(next_pos);
        *cell = new_cell;
        tf.translation = new_tf;
        tf.rotation = rotation;
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
    q_bodies: Query<&CelestialBody>,
    q_sc: Query<&Spacecraft>,
    defaults: Res<CameraDefaults>,
    mut scroll_res: ResMut<CameraScroll>,
    sens: Res<CameraScrollSensitivity>,
    keys: Res<ButtonInput<KeyCode>>,
    q_children: Query<&Children>,
    mut commands: Commands,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) { return; }

    let dt = time.delta_secs();

    for (avatar_ent, mut tf, mut cell, mut orbit, child_of) in q_avatar.iter_mut() {
        let Ok((_t_cell, _t_tf)) = q_spatial.get(orbit.target) else { continue; };

        // Find the target's grid.
        let mut target_grid = orbit.target;
        for _ in 0..10 {
            if q_grids.contains(target_grid) { break; }
            if let Ok(parent) = q_parents.get(target_grid) {
                target_grid = parent.parent();
            } else { break; }
        }
        if !q_grids.contains(target_grid) { continue; }

        // Compute minimum distance to prevent zooming inside the target body.
        let physical_target = get_physical_body(orbit.target, &q_children, &q_bodies);
        let min_dist = if let Ok(body) = q_bodies.get(physical_target) {
            body.radius_m * 1.5
        } else if let Ok(sc) = q_sc.get(orbit.target) {
            (sc.hit_radius_m as f64).max(10.0)
        } else {
            10.0 // Generic fallback minimum distance.
        };
        let current_grid = child_of.parent();

        // If the target is on a different grid, migrate the camera to it.
        // Preserve the camera's CURRENT absolute position during migration
        // — don't snap to the target body.
        if current_grid != target_grid {
            let cam_abs = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                avatar_ent, &cell, &tf, &q_parents, &q_grids, &q_spatial_abs,
            );
            if let Ok(target_grid_ref) = q_grids.get(target_grid) {
                let target_grid_abs = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                    target_grid, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial_abs,
                );
                let local_pos = cam_abs - target_grid_abs;
                let (new_cell, new_tf) = target_grid_ref.translation_to_grid(local_pos);
                *cell = new_cell;
                tf.translation = new_tf;
                commands.entity(target_grid).add_child(avatar_ent);
            }
            // Skip this frame — set_parent command runs at end of stage.
            // Next frame, child_of will resolve to the new grid.
            continue;
        }

        // Now both camera and target are on the same grid — simple position lookup.
        let (t_cell_now, t_tf_now) = if let Ok((c, t)) = q_spatial.get(orbit.target) {
            (c, t)
        } else { continue; };
        let grid_ref = if let Ok(g) = q_grids.get(child_of.parent()) {
            g
        } else { continue; };

        let target_pos = grid_ref.grid_position_double(t_cell_now, t_tf_now);

        // Multiplicative zoom: proportional to current distance using exponential scaling.
        // Scroll up (delta > 0) -> zoom in. Scroll down (delta < 0) -> zoom out.
        if scroll_res.delta != 0.0 {
            let zoom_factor = (-scroll_res.delta as f64 * sens.value as f64 * 0.01).exp();
            orbit.distance = (orbit.distance * zoom_factor).clamp(min_dist, 1.0e11);
            scroll_res.delta = 0.0;
        }

        // Camera rotation from user yaw/pitch (ecliptic-locked).
        let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
        let offset = rotation.mul_vec3(Vec3::Z).as_dvec3() * orbit.distance;
        let desired_pos = target_pos + offset + Vec3::Y.as_dvec3() * orbit.vertical_offset as f64;

        // Damped position lerp toward desired orbit slot.
        let damping = orbit.damping.unwrap_or(defaults.damping);
        let lerp_factor = (dt * 30.0 * (1.0 - damping)).min(1.0) as f64;
        let current_pos = grid_ref.grid_position_double(&cell, &tf);
        let next_pos = current_pos.lerp(desired_pos, lerp_factor);

        let (new_cell, new_tf) = grid_ref.translation_to_grid(next_pos);
        *cell = new_cell;
        tf.translation = new_tf;

        // Apply rotation directly (no look_at — that clobbered yaw/pitch).
        tf.rotation = rotation;
    }
}

/// FreeFlightCamera system: moves the camera in absolute coordinates.
///
/// Only runs when `FreeFlightCamera` is present AND no `FrameBlend` is active.
/// Position is set by `avatar_universal_locomotion_system`. This system
/// applies yaw/pitch rotation from user input.
///
/// In surface mode, the rotation is built around the local gravity up vector
/// using sequential quaternion composition — guaranteed unit-length.
///
/// Note: `FreeFlightCamera` and `SurfaceCamera` are mutually exclusive.
/// The surface teleport removes `FreeFlightCamera`, so the surface-mode
/// branch here is effectively dead code. Kept for completeness.
fn freeflight_system(
    mut q_avatar: Query<(
        &mut Transform,
        &mut FreeFlightCamera,
        &CellCoord,
        &ChildOf,
        Option<&SurfaceRelativeMode>,
    ), (With<Avatar>, Without<FrameBlend>)>,
    q_grids: Query<&Grid>,
    _gravity_field: Res<LocalGravityField>,
) {
    for (mut tf, mut ff, cell, child_of, surface_mode) in q_avatar.iter_mut() {
        let rot = if surface_mode.is_some() {
            // Compute "up" from camera's grid-local position (body center → camera).
            let up_v = if let Ok(grid) = q_grids.get(child_of.0) {
                let pos = grid.grid_position_double(cell, &tf);
                if pos.length() > 1e-6 {
                    (pos / pos.length()).as_vec3()
                } else { Vec3::Y }
            } else { Vec3::Y };

            // In surface mode, apply yaw/pitch as incremental rotations.
            let yaw_q = Quat::from_axis_angle(up_v, ff.yaw);
            let right: Vec3 = (*tf.right()).into();
            let right_after_yaw = yaw_q.mul_vec3(right);
            let pitch_q = Quat::from_axis_angle(right_after_yaw, ff.pitch);
            let new_rot = (pitch_q * yaw_q * tf.rotation).normalize();

            // Consume the deltas — they were applied as increments this frame.
            ff.yaw = 0.0;
            ff.pitch = 0.0;

            new_rot
        } else {
            Quat::from_euler(EulerRot::YXZ, ff.yaw, ff.pitch, 0.0)
        };
        tf.rotation = rot;
    }
}

/// Surface camera system: computes rotation from absolute heading + pitch
/// relative to the local surface normal, recomputed from scratch every frame.
///
/// This completely avoids accumulated roll drift because no incremental
/// rotations are used — the rotation quaternion is built fresh each frame
/// from heading, pitch, and the position-derived "up" direction.
///
/// ## Why position-derived "up" (not LocalGravityField)?
///
/// The camera is parented to the Body's Grid. The Body sits at the Grid origin.
/// Therefore the camera's grid-local position (CellCoord + Transform.translation)
/// IS the world-space vector from body center to camera. No hierarchy walk needed.
/// This is always correct regardless of timing, system ordering, or stale data.
///
/// Only runs when `SurfaceCamera` is present (replaces `FreeFlightCamera`
/// while on a body's surface).
fn surface_camera_system(
    mut q_avatar: Query<(
        &mut Transform,
        &SurfaceCamera,
        &CellCoord,
        &ChildOf,
    ), (With<Avatar>, Without<FrameBlend>)>,
    q_grids: Query<&Grid>,
) {
    for (mut tf, cam, cell, child_of) in q_avatar.iter_mut() {
        let Ok(grid) = q_grids.get(child_of.0) else { continue };

        // Grid-local position = world-space vector from body center to camera.
        // Body is at Grid origin (Transform::default()), so no offset needed.
        let up_d = grid.grid_position_double(cell, &tf);
        let up = if up_d.length() > 1e-6 {
            (up_d / up_d.length()).as_vec3()
        } else {
            Vec3::Y
        };

        // Build local tangent frame: "north" = projection of world Y onto tangent plane.
        // At the poles (where Y is parallel to up), fall back to Z.
        let ref_dir = if up.dot(Vec3::Y).abs() < 0.9 { Vec3::Y } else { Vec3::Z };
        let east = up.cross(ref_dir).normalize();
        let north = east.cross(up).normalize();

        // Apply heading rotation around up axis.
        let heading_q = Quat::from_axis_angle(up, cam.heading);
        let forward = heading_q.mul_vec3(north);
        let right = forward.cross(up).normalize();

        // Build base rotation: local -Z (Bevy forward) maps to world `forward`.
        let base_rot = Quat::from_mat3(&Mat3::from_cols(right, up, -forward));

        // Apply pitch around right axis.
        let pitch_q = Quat::from_axis_angle(right, cam.pitch);

        tf.rotation = (pitch_q * base_rot).normalize();
    }
}

// ─── Locomotion ──────────────────────────────────────────────────────────────

/// Moves the avatar entity in absolute coordinates.
///
/// Only active when `FreeFlightCamera` is present. When CTRL is held while possessing
/// a vessel, this system temporarily drives the FreeFlightCamera camera independently
/// without removing the underlying `SpringArmCamera`.
///
/// In surface mode, elevation uses `LocalGravityField.local_up` instead of world Y,
/// so forward/side/elevation move along the tangent plane at the surface position.
fn avatar_universal_locomotion_system(
    mut q_avatar: Query<(
        &mut Transform,
        &mut CellCoord,
        &ChildOf,
        &IntentAnalogState,
        Has<FreeFlightCamera>,
        Has<SurfaceCamera>,
        Option<&SurfaceRelativeMode>,
    ), With<Avatar>>,
    q_grids: Query<&Grid>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    for (mut tf, mut cell, child_of, analog, has_freeflight, has_surface_camera, surface_mode) in q_avatar.iter_mut() {
        let Ok(grid) = q_grids.get(child_of.0) else { continue };
        let current_pos = grid.grid_position_double(&cell, &tf);

        // Only move if we have a camera mode or CTRL-overlay.
        if !has_freeflight && !has_surface_camera && !ctrl_pressed { continue; }

        let curr_is_moving = analog.forward.abs() > 0.01 || analog.side.abs() > 0.01 || analog.elevation.abs() > 0.01;
        if !curr_is_moving { continue; }

        // In surface mode, "up" = radial direction from body center (= current_pos normalized).
        // Otherwise use world Y.
        let up_dir = if surface_mode.is_some() {
            if current_pos.length() > 1e-6 {
                (current_pos / current_pos.length()).as_vec3()
            } else {
                Vec3::Y
            }
        } else {
            Vec3::Y
        };

        let mut move_vec = Vec3::ZERO;
        move_vec += *tf.forward() * analog.forward;
        move_vec += *tf.right() * analog.side;
        move_vec += up_dir * analog.elevation;

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
///
/// In surface mode, CTRL+look applies yaw around `local_up` and pitch around
/// the yawed-right axis, matching the surface-relative camera orientation.
fn avatar_behavior_input_system(
    q_avatar: Query<(&IntentAnalogState, Option<&SurfaceRelativeMode>), With<Avatar>>,
    mut q_spring: Query<&mut SpringArmCamera, With<Avatar>>,
    mut q_orbit: Query<&mut OrbitCamera, With<Avatar>>,
    mut q_freeflight: Query<&mut FreeFlightCamera, With<Avatar>>,
    mut q_surface: Query<&mut SurfaceCamera, With<Avatar>>,
    mut q_tf: Query<(&mut Transform, &CellCoord, &ChildOf), (With<Avatar>, Without<FrameBlend>)>,
    q_grids: Query<&Grid>,
    sensitivity: Res<MouseSensitivity>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    // Only process look input when right mouse button is held.
    if !mouse.pressed(MouseButton::Right) { return; }

    let Some((analog, surface_mode)) = q_avatar.iter().next() else { return; };
    let look_delta = analog.look_delta;
    if look_delta.length_squared() < 0.0001 { return; }

    let delta_yaw = -look_delta.x * sensitivity.sensitivity * 0.01;
    let delta_pitch = -look_delta.y * sensitivity.sensitivity * 0.01;
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    if ctrl_pressed {
        // Momentary free-flight: apply look deltas directly to Transform.
        if let Some((mut tf, cell, child_of)) = q_tf.iter_mut().next() {
            if surface_mode.is_some() {
                // "Up" = camera's grid-local position (body center → camera direction).
                let up_v = if let Ok(grid) = q_grids.get(child_of.0) {
                    let pos = grid.grid_position_double(cell, &tf);
                    if pos.length() > 1e-6 {
                        (pos / pos.length()).as_vec3()
                    } else { Vec3::Y }
                } else { Vec3::Y };
                let yaw_q = Quat::from_axis_angle(up_v, delta_yaw);
                let right: Vec3 = (*tf.right()).into();
                let right_yawed = yaw_q.mul_vec3(right);
                let pitch_q = Quat::from_axis_angle(right_yawed, delta_pitch);
                tf.rotation = pitch_q * yaw_q * tf.rotation;
            } else {
                // Ecliptic: YXZ euler decomposition
                let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                tf.rotation = Quat::from_euler(EulerRot::YXZ, yaw + delta_yaw, (pitch + delta_pitch).clamp(-1.5, 1.5), 0.0);
            }
        }
    } else {
        // Normal mode: apply to the active behavior component.
        if let Some(mut arm) = q_spring.iter_mut().next() {
            arm.yaw += delta_yaw;
            arm.pitch = (arm.pitch + delta_pitch).clamp(-1.5, 1.5);
        }
        if let Some(mut orbit) = q_orbit.iter_mut().next() {
            orbit.yaw += delta_yaw;
            orbit.pitch = (orbit.pitch + delta_pitch).clamp(-1.5, 1.5);
        }
        if let Some(mut ff) = q_freeflight.iter_mut().next() {
            ff.yaw += delta_yaw;
            ff.pitch = (ff.pitch + delta_pitch).clamp(-1.5, 1.5);
        }
        if let Some(mut sc) = q_surface.iter_mut().next() {
            sc.heading += delta_yaw;
            sc.pitch = (sc.pitch + delta_pitch).clamp(-1.5, 1.5);
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

// ─── Raycasting ──────────────────────────────────────────────────────────────

/// Finds the root Vessel entity from a hit collider by walking up the parent chain.
/// Returns None if no vessel is found or if the hit is on ground/terrain.
fn find_vessel_from_hit(
    mut entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_vessel: &Query<Entity, With<Vessel>>,
    q_ground: &Query<Entity, With<lunco_core::Ground>>,
) -> Option<Entity> {
    let mut depth = 0;
    const MAX_DEPTH: usize = 8;

    // Walk up parent chain looking for a Vessel
    loop {
        // Skip ground/terrain entities
        if q_ground.get(entity).is_ok() {
            return None;
        }

        // Check if this entity is a vessel
        if q_vessel.get(entity).is_ok() {
            return Some(entity);
        }

        depth += 1;
        if depth >= MAX_DEPTH {
            break;
        }

        // Walk up to parent
        if let Ok(parent) = q_parents.get(entity) {
            entity = parent.parent();
        } else {
            break;
        }
    }

    None
}

/// Raycasts possession against actual collider geometry.
///
/// Uses Avian3D SpatialQuery to hit real mesh colliders, not invisible spheres.
/// Walks up parent chain to find the root Vessel entity for possession.
/// Celestial bodies still use sphere intersection (they have no colliders).
pub fn avatar_raycast_possession(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform, Entity), With<Avatar>>,
    q_link: Query<&lunco_controller::ControllerLink, With<Avatar>>,
    drag_mode_active: Res<lunco_core::DragModeActive>,
    mut commands: Commands,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>,
    q_spacecraft: Query<(Entity, &GlobalTransform, &Spacecraft)>,
    q_rovers: Query<Entity, With<Vessel>>,
    q_parents: Query<&ChildOf>,
    q_ground: Query<Entity, With<lunco_core::Ground>>,
    raycaster: avian3d::prelude::SpatialQuery,
) {
    if !mouse.just_pressed(MouseButton::Left) { return; }

    // Skip possession check if entity dragging is active
    // This prevents camera possession from interfering with drag operations
    if drag_mode_active.active { return; }

    // If the avatar is already possessing a vessel, skip raycast possession entirely.
    if q_link.iter().next().is_some() { return; }

    let Some(pos) = windows.iter().next().and_then(|w| w.cursor_position()) else { return; };
    let Some((camera, cam_gtf, avatar_entity)) = camera_q.iter().next() else { return; };
    let Ok(ray) = camera.viewport_to_world(cam_gtf, pos) else { return; };

    // Raycast against colliders to find vessels
    let filter = avian3d::prelude::SpatialQueryFilter::default();
    let hit = raycaster.cast_ray(ray.origin.into(), ray.direction, 1000.0, false, &filter);

    let mut nearest_vessel: Option<Entity> = None;
    let mut min_vessel_t = f32::INFINITY;

    if let Some(hit_data) = hit {
        // Walk up parent chain to find the vessel
        let vessel = find_vessel_from_hit(hit_data.entity, &q_parents, &q_rovers, &q_ground);
        if let Some(vessel_entity) = vessel {
            min_vessel_t = hit_data.distance as f32;
            nearest_vessel = Some(vessel_entity);
        }
    }

    // Also check celestial bodies and spacecraft (no colliders)
    let mut nearest = nearest_vessel;
    let mut min_t = min_vessel_t;
    let mut is_possessable = nearest_vessel.is_some();

    // Check spacecraft with sphere intersection (they may not have colliders)
    for (entity, gtf, sc) in q_spacecraft.iter() {
        let oc = ray.origin - gtf.translation();
        let b = oc.dot(ray.direction.as_vec3());
        let c = oc.dot(oc) - sc.hit_radius_m.powi(2);
        let discr = b * b - c;
        if discr >= 0.0 {
            let t = -b - discr.sqrt();
            if t > 0.0 && t < min_t {
                min_t = t;
                nearest = Some(entity);
                is_possessable = true;
            }
        }
    }

    // Check celestial bodies with sphere intersection
    for (entity, gtf, body) in q_bodies.iter() {
        let oc = ray.origin - gtf.translation();
        let b = oc.dot(ray.direction.as_vec3());
        let c = oc.dot(oc) - (body.radius_m as f32).powi(2);
        let discr = b * b - c;
        if discr >= 0.0 {
            let t = -b - discr.sqrt();
            if t > 0.0 && t < min_t {
                min_t = t;
                nearest = Some(entity);
                is_possessable = false; // Focus, not possess
            }
        }
    }

    if let Some(target) = nearest {
        if is_possessable {
            commands.trigger(CommandMessage {
                id: 0,
                target,
                name: "POSSESS".to_string(),
                args: smallvec::smallvec![],
                source: avatar_entity,
            });
        } else {
            commands.trigger(CommandMessage {
                id: 0,
                target,
                name: "FOCUS".to_string(),
                args: smallvec::smallvec![],
                source: avatar_entity,
            });
        }
    }
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
    q_avatar: Query<(&Transform, Option<&ControllerLink>, Option<&SurfaceRelativeMode>), With<Avatar>>,
) {
    let msg = trigger.event();
    if msg.name == "RELEASE" {
        let avatar_ent = msg.target;
        let (yaw, pitch, opt_link, is_surface) = if let Ok((tf, link, surface)) = q_avatar.get(avatar_ent) {
            let (y, p, _) = tf.rotation.to_euler(EulerRot::YXZ);
            (y, p, link, surface.is_some())
        } else { (0.0, 0.0, None, false) };

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
            .remove::<FrameBlend>();

        // In surface mode, use SurfaceCamera (recomputed from scratch each frame);
        // otherwise use FreeFlightCamera (incremental euler angles).
        if is_surface {
            commands.entity(avatar_ent).insert(SurfaceCamera {
                heading: yaw, // approximate mapping from euler yaw
                pitch,
            });
        } else {
            commands.entity(avatar_ent).insert(FreeFlightCamera {
                yaw,
                pitch,
                damping: None,
            });
        }
        info!("Released possession → camera at current position (surface={})", is_surface);
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

/// Helper function to find the grid an entity belongs to.
fn get_grid_for_entity(
    mut entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
) -> Option<Entity> {
    if q_grids.contains(entity) {
        return Some(entity);
    }
    while let Ok(child_of) = q_parents.get(entity) {
        let parent = child_of.parent();
        if q_grids.contains(parent) {
            return Some(parent);
        }
        entity = parent;
    }
    None
}

/// Possesses a vessel with an instant camera transition.
fn on_possess_command(
    trigger: On<CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf), With<Avatar>>,
    q_spatial_abs: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_sc: Query<&Spacecraft>,
    q_vessel_gravity: Query<&GravityBody>,
    _q_orbit: Query<&OrbitCamera>,
    _q_spring: Query<&SpringArmCamera>,
    _q_chase: Query<&ChaseCamera>,
) {
    let msg = trigger.event();
    if msg.name == "POSSESS" {
        let (avatar_ent, cam_tf, cam_cell, _child_of) = if let Ok(data) = q_avatar.get(msg.source) {
            data
        } else {
            let Some(first) = q_avatar.iter().next() else { return; };
            first
        };

        // Compute camera absolute position in root frame.
        let cam_abs = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
            avatar_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial_abs,
        );

        // Compute target absolute position.
        let target_abs = if let Ok((t_cell, t_tf)) = q_spatial.get(msg.target) {
            lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                msg.target, t_cell, t_tf, &q_parents, &q_grids, &q_spatial_abs,
            )
        } else {
            cam_abs // Fallback
        };

        let target_grid = get_grid_for_entity(msg.target, &q_parents, &q_grids);
        let is_spacecraft = q_sc.contains(msg.target);
        let end_distance = if is_spacecraft { 50.0 } else { 15.0 };
        let end_vert_off = if is_spacecraft { 0.0 } else { 2.0 };
        let end_yaw = 0.0;
        let end_pitch = -0.25;

        // Snap to vessel immediately.
        let (current_yaw, current_pitch, _) = cam_tf.rotation.to_euler(EulerRot::YXZ);
        let final_rot = if is_spacecraft {
            Quat::from_euler(EulerRot::YXZ, current_yaw, current_pitch, 0.0)
        } else {
            // Rovers use specific starting view angles.
            Quat::from_euler(EulerRot::YXZ, end_yaw, end_pitch, 0.0)
        };
        let final_offset = final_rot.mul_vec3(Vec3::Z).as_dvec3() * end_distance;
        let final_abs_pos = target_abs + final_offset + Vec3::Y.as_dvec3() * end_vert_off as f64;

        // Migrate to target grid immediately
        if let Some(tg) = target_grid {
            if tg != Entity::PLACEHOLDER {
                if let Ok(target_grid_ref) = q_grids.get(tg) {
                    let target_grid_abs = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                        tg, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial_abs,
                    );
                    let local_pos = final_abs_pos - target_grid_abs;
                    let (new_cell, new_tf) = target_grid_ref.translation_to_grid(local_pos);

                    commands.entity(avatar_ent)
                        .insert(new_cell)
                        .insert(Transform::from_translation(new_tf).with_rotation(final_rot));
                    commands.entity(tg).add_child(avatar_ent);
                }
            }
        }

        commands.entity(avatar_ent)
            .insert(ControllerLink { vessel_entity: msg.target });
        commands.entity(msg.target).insert((
            ActionState::<lunco_controller::VesselIntent>::default(),
            lunco_controller::get_default_input_map(),
        ));

        // Detect if target is a surface vehicle (has GravityBody) and propagate surface mode.
        let is_surface_vehicle = q_vessel_gravity.get(msg.target).is_ok();

        if end_vert_off == 0.0 {
            commands.entity(avatar_ent)
                .insert(OrbitCamera {
                    target: msg.target,
                    distance: end_distance,
                    yaw: if is_spacecraft { current_yaw } else { end_yaw },
                    pitch: if is_spacecraft { current_pitch } else { end_pitch },
                    damping: None,
                    vertical_offset: 0.0,
                });
        } else {
            let mut cmd = commands.entity(avatar_ent);
            cmd.insert(SpringArmCamera {
                target: msg.target,
                distance: end_distance,
                yaw: 0.0,
                pitch: end_pitch,
                damping: Some(0.05),
                vertical_offset: end_vert_off,
            });
            // If possessing a surface vehicle, enable surface-relative camera mode
            if is_surface_vehicle {
                if let Ok(gb) = q_vessel_gravity.get(msg.target) {
                    cmd.insert(*gb);
                }
                cmd.insert(SurfaceRelativeMode);
            }
        }

        commands.entity(avatar_ent)
            .remove::<FreeFlightCamera>()
            .remove::<SurfaceCamera>()
            .remove::<FrameBlend>();
    }
}

/// Focuses on a target with an instant transition to OrbitCamera mode.
fn on_focus_command(
    trigger: On<CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf), With<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_spatial_abs: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_bodies: Query<&CelestialBody>,
    q_sc: Query<&Spacecraft>,
    q_children: Query<&Children>,
    _q_orbit: Query<&OrbitCamera>,
    _q_spring: Query<&SpringArmCamera>,
    _q_chase: Query<&ChaseCamera>,
) {
    let msg = trigger.event();
    if msg.name == "FOCUS" {
        let (avatar_ent, cam_tf, cam_cell, _child_of) = if let Ok(data) = q_avatar.get(msg.source) {
            data
        } else {
            let Some(first) = q_avatar.iter().next() else { return; };
            first
        };

        // Compute camera absolute position in root frame.
        let _cam_abs = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
            avatar_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial_abs,
        );

        // Compute target absolute position.
        let target_abs = if let Ok((t_cell, t_tf)) = q_spatial.get(msg.target) {
            lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                msg.target, t_cell, t_tf, &q_parents, &q_grids, &q_spatial_abs,
            )
        } else {
            lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                msg.target, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial_abs,
            )
        };

        // Compute distance based on target type.
        let mut distance = 20.0;
        let physical_target = get_physical_body(msg.target, &q_children, &q_bodies);
        if msg.args.len() > 0 {
            distance = msg.args[0];
        } else if let Ok(body) = q_bodies.get(physical_target) {
            distance = body.radius_m * 3.0;
        } else if let Ok(sc) = q_sc.get(msg.target) {
            distance = (sc.hit_radius_m as f64 * 5.0).max(100.0);
        }

        let target_grid = get_grid_for_entity(msg.target, &q_parents, &q_grids);

        // Snap to target immediately.
        let (current_yaw, current_pitch, _) = cam_tf.rotation.to_euler(EulerRot::YXZ);
        let final_rot = Quat::from_euler(EulerRot::YXZ, current_yaw, current_pitch, 0.0);
        let final_offset = final_rot.mul_vec3(Vec3::Z).as_dvec3() * distance;
        let final_abs_pos = target_abs + final_offset;

        // Migrate to target grid immediately
        if let Some(tg) = target_grid {
            if tg != Entity::PLACEHOLDER {
                if let Ok(target_grid_ref) = q_grids.get(tg) {
                    let target_grid_abs = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                        tg, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial_abs,
                    );
                    let local_pos = final_abs_pos - target_grid_abs;
                    let (new_cell, new_tf) = target_grid_ref.translation_to_grid(local_pos);
                    
                    commands.entity(avatar_ent)
                        .insert(new_cell)
                        .insert(Transform::from_translation(new_tf).with_rotation(final_rot));
                    commands.entity(tg).add_child(avatar_ent);
                }
            }
        }

        commands.entity(avatar_ent)
            .remove::<SpringArmCamera>()
            .remove::<OrbitCamera>()
            .remove::<FreeFlightCamera>()
            .remove::<FrameBlend>()
            .insert(OrbitCamera {
                target: msg.target,
                distance,
                yaw: current_yaw,
                pitch: current_pitch,
                damping: None,
                vertical_offset: 0.0,
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

// ─── Surface Teleport Commands ───────────────────────────────────────────────

/// Teleports the avatar to a body's surface.
///
/// Command: `TELEPORT_SURFACE`
/// - `args[0]` = target body entity index (as i64)
/// - `args[1]` = latitude in degrees (optional, defaults to camera look projection)
/// - `args[2]` = longitude in degrees (optional)
///
/// The camera is parented to the Body's Grid (inertial anchor), NOT the Body
/// itself. `SurfaceCamera` rebuilds world-space rotation every frame from
/// `LocalGravityField.local_up`, so the camera stays surface-relative without
/// inheriting the Body's rotation. `FloatingOrigin` must be on a Grid.
fn on_surface_teleport_command(
    trigger: On<CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf), With<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial_abs: Query<(&CellCoord, &Transform)>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_gravity_providers: Query<&lunco_celestial::GravityProvider>,
    mut field: ResMut<LocalGravityField>,
) {
    let msg = trigger.event();
    if msg.name != "TELEPORT_SURFACE" { return; }

    let Some((avatar_ent, cam_tf, cam_cell, _cam_child_of)) = q_avatar.iter().next() else { return };

    warn!("TELEPORT: triggered for avatar {:?}", avatar_ent);

    // Find target body entity
    let target_body_entity: Option<Entity> = if !msg.args.is_empty() {
        let idx = msg.args[0] as u64;
        warn!("TELEPORT: target body index={}", idx);
        Entity::from_bits(idx).into()
    } else { None };

    let (body_entity, body_radius) = if let Some(e) = target_body_entity {
        if let Ok((e, b)) = q_bodies.get(e) {
            warn!("TELEPORT: found body {:?} radius={:.0}m", e, b.radius_m);
            (e, b.radius_m)
        } else {
            warn!("TELEPORT: body entity {:?} not found in q_bodies", e);
            return;
        }
    } else {
        let cam_abs = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
            avatar_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial_abs,
        );
        let mut best = None;
        let mut best_dist = f64::MAX;
        for (e, b) in q_bodies.iter() {
            let d = (cam_abs.length() - b.radius_m).abs();
            if d < best_dist { best_dist = d; best = Some((e, b.radius_m)); }
        }
        best.unwrap_or_else(|| (Entity::PLACEHOLDER, 6_371_000.0))
    };
    if body_entity == Entity::PLACEHOLDER {
        warn!("TELEPORT: no body found");
        return;
    }

    // Find the Body's Grid (the inertial anchor that the Body is a child of).
    let body_grid = q_parents.get(body_entity)
        .ok()
        .map(|c| c.0)
        .filter(|e| q_grids.contains(*e));

    let Some(grid_entity) = body_grid else {
        warn!("TELEPORT: body {:?} has no Grid parent", body_entity);
        return;
    };
    warn!("TELEPORT: parenting camera to grid {:?}", grid_entity);

    // Compute surface position in absolute coordinates, then convert to Grid-local.
    let lat_deg = if msg.args.len() > 1 { Some(msg.args[1]) } else { None };
    let lon_deg = if msg.args.len() > 2 { Some(msg.args[2]) } else { Some(0.0) };

    let (surface_local_pos, surface_normal) = if let Some(lat) = lat_deg {
        let lon = lon_deg.unwrap_or(0.0);
        let lat_r = lat.to_radians();
        let lon_r = lon.to_radians();
        let normal = DVec3::new(
            lat_r.cos() * lon_r.sin(),
            lat_r.sin(),
            lat_r.cos() * lon_r.cos(),
        );
        (normal * (body_radius + 50.0), normal)
    } else {
        let surface_normal = -cam_tf.forward().as_dvec3().normalize();
        (surface_normal * (body_radius + 50.0), surface_normal)
    };

    // Since the Body sits at the Grid origin (CellCoord::default, Transform::default),
    // the camera's grid-local position IS the body-relative position.
    // No absolute coordinate math needed.

    if let Ok(grid_ref) = q_grids.get(grid_entity) {
        let (new_cell, new_tf_translation) = grid_ref.translation_to_grid(surface_local_pos);

        // Surface gravity from body's GravityProvider
        let surface_g = if let Ok(gp) = q_gravity_providers.get(body_entity) {
            let accel = gp.model.acceleration(surface_normal * body_radius);
            accel.length()
        } else {
            0.0
        };

        // Build camera rotation in world space: Y = surface_normal (up), Z = horizontal.
        // Since the camera is on the Grid (identity rotation), world-space = local-space.
        let up_n = surface_normal.normalize();
        let up_v = up_n.as_vec3();
        let ref_north = if up_n.abs().dot(DVec3::Y) < 0.9 { DVec3::Y } else { DVec3::Z };
        let right_v = up_n.cross(ref_north).normalize().as_vec3();
        let fwd_v = up_v.cross(right_v);
        let surface_rot = Quat::from_mat3(&Mat3::from_cols(right_v, up_v, -fwd_v));

        commands.entity(avatar_ent)
            .insert(new_cell)
            .insert(Transform::from_translation(new_tf_translation).with_rotation(surface_rot))
            .insert(GravityBody { body_entity })
            .insert(SurfaceRelativeMode)
            .insert(SurfaceCamera {
                heading: 0.0,
                pitch: -0.2,
            })
            .remove::<FreeFlightCamera>()
            .remove::<OrbitCamera>()
            .remove::<SpringArmCamera>()
            .remove::<FrameBlend>();

        // Parent camera to the Body's Grid (inertial), NOT the Body.
        // FloatingOrigin must be on a Grid.
        commands.entity(grid_entity).add_child(avatar_ent);

        // Update LocalGravityField (world-space "up")
        field.body_entity = Some(body_entity);
        field.local_up = surface_normal;
        field.surface_g = surface_g;
        field.up = surface_normal;

        warn!("TELEPORT: done — camera now on grid {:?} at alt ~50m", grid_entity);
    } else {
        warn!("TELEPORT: grid entity {:?} not found", grid_entity);
    }
}

/// Leaves the surface and returns to orbit view.
///
/// Command: `LEAVE_SURFACE`
/// Teleports camera to 3x body radius altitude and switches to OrbitCamera.
/// Re-parents the camera back to the EMB Grid (star-fixed frame).
fn on_leave_surface_command(
    trigger: On<CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, Option<&GravityBody>), With<Avatar>>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_grids: Query<&Grid>,
    q_emb: Query<Entity, With<lunco_celestial::EMBRoot>>,
    mut field: ResMut<LocalGravityField>,
) {
    let msg = trigger.event();
    if msg.name != "LEAVE_SURFACE" { return; }

    let Some((avatar_ent, cam_tf, gravity_body)) = q_avatar.iter().next() else { return };

    // Find the body we're leaving
    let body_entity = gravity_body.map(|gb| gb.body_entity)
        .or_else(|| Some(Entity::PLACEHOLDER))
        .unwrap_or(Entity::PLACEHOLDER);

    let body_radius = q_bodies.get(body_entity)
        .map(|(_, b)| b.radius_m)
        .unwrap_or(6_371_000.0); // fallback: Earth radius

    // Find EMB Grid (the star-fixed orbit frame)
    let Some(emb_grid) = q_emb.iter().next() else { return; };
    let Ok(emb_grid_ref) = q_grids.get(emb_grid) else { return; };

    // Teleport to 3x body radius altitude, relative to EMB Grid.
    let altitude = body_radius * 3.0;
    let orbit_pos_local = DVec3::new(0.0, altitude, altitude * 0.5);
    let (new_cell, new_tf) = emb_grid_ref.translation_to_grid(orbit_pos_local);

    commands.entity(avatar_ent)
        .insert(new_cell)
        .insert(Transform::from_translation(new_tf).with_rotation(cam_tf.rotation))
        .insert(OrbitCamera {
            target: body_entity,
            distance: altitude,
            yaw: 0.0,
            pitch: -0.3,
            damping: None,
            vertical_offset: 0.0,
        })
        .remove::<FreeFlightCamera>()
        .remove::<SurfaceCamera>()
        .remove::<SpringArmCamera>()
        .remove::<FrameBlend>()
        .remove::<GravityBody>()
        .remove::<SurfaceRelativeMode>();

    commands.entity(emb_grid).add_child(avatar_ent);

    // Clear gravity field
    field.body_entity = None;
    field.local_up = DVec3::Y;
    field.surface_g = 0.0;
    field.up = DVec3::Y;

    info!("Left surface, returned to orbit around {:?}", body_entity);
}

// ─── Surface Mode Transition ────────────────────────────────────────────────

/// Auto-inserts/removes `SurfaceRelativeMode` based on avatar altitude.
///
/// Uses hysteresis to prevent rapid toggling at the boundary:
/// - Below `engage_altitude` → insert `SurfaceRelativeMode`
/// - Above `disengage_altitude` → remove `SurfaceRelativeMode`
///
/// Altitude is computed as `|body_local_position| - body_radius` from the
/// avatar's `GravityBody` binding. Runs in `Update` so camera systems
/// see the mode change immediately.
fn surface_mode_transition_system(
    q_avatar: Query<(
        Entity, &Transform, &CellCoord, &ChildOf,
        Option<&GravityBody>, Option<&SurfaceRelativeMode>,
        Option<&FreeFlightCamera>, Option<&SurfaceCamera>,
    ), With<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(&CellCoord, &Transform)>,
    q_bodies: Query<&CelestialBody>,
    thresholds: Res<SurfaceModeThreshold>,
    field: Res<LocalGravityField>,
    mut commands: Commands,
) {
    let Some((avatar_ent, tf, cell, _, maybe_gb, maybe_mode, maybe_ff, maybe_sc)) = q_avatar.iter().next() else { return };

    // Use absolute coordinates to handle nested grids correctly
    let cam_abs = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
        avatar_ent, cell, tf, &q_parents, &q_grids, &q_spatial,
    );

    // Compute altitude above the bound body
    let (_full_body_local, altitude) = if let Some(gb) = maybe_gb {
        if let Ok((b_cell, b_tf)) = q_spatial.get(gb.body_entity) {
            let body_abs = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                gb.body_entity, b_cell, b_tf, &q_parents, &q_grids, &q_spatial,
            );
            let rel_pos = cam_abs - body_abs;
            let alt = if let Ok(body) = q_bodies.get(gb.body_entity) {
                rel_pos.length() - body.radius_m
            } else { f64::MAX };
            (rel_pos, alt)
        } else { (cam_abs, f64::MAX) }
    } else if let Some(body_ent) = field.body_entity {
        if let Ok((b_cell, b_tf)) = q_spatial.get(body_ent) {
            let body_abs = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                body_ent, b_cell, b_tf, &q_parents, &q_grids, &q_spatial,
            );
            let rel_pos = cam_abs - body_abs;
            let alt = if let Ok(body) = q_bodies.get(body_ent) {
                rel_pos.length() - body.radius_m
            } else { f64::MAX };
            (rel_pos, alt)
        } else { (cam_abs, f64::MAX) }
    } else {
        (cam_abs, f64::MAX)
    };

    let in_surface_mode = maybe_mode.is_some();

    if in_surface_mode && altitude > thresholds.disengage_altitude {
        // Too high → exit surface mode. Swap SurfaceCamera → FreeFlightCamera.
        commands.entity(avatar_ent).remove::<SurfaceRelativeMode>();
        if let Some(sc) = maybe_sc {
            // Note: heading→yaw is approximate (different reference frames)
            // but provides a reasonable starting orientation.
            commands.entity(avatar_ent)
                .remove::<SurfaceCamera>()
                .insert(FreeFlightCamera {
                    yaw: sc.heading,
                    pitch: sc.pitch,
                    damping: None,
                });
        }
    } else if !in_surface_mode && altitude < thresholds.engage_altitude {
        // Low enough and bound to a body → enter surface mode.
        let has_body = maybe_gb.is_some() || field.body_entity.is_some();
        if has_body {
            commands.entity(avatar_ent).insert(SurfaceRelativeMode);
            // Swap FreeFlightCamera → SurfaceCamera.
            if let Some(ff) = maybe_ff {
                commands.entity(avatar_ent)
                    .remove::<FreeFlightCamera>()
                    .insert(SurfaceCamera {
                        heading: ff.yaw,
                        pitch: ff.pitch,
                    });
            }
        }
    }
}

/// Resolves a focus target (which might be a Grid/Frame) to its primary physical Body.
/// 
/// If the entity itself has a `CelestialBody`, it is returned. 
/// Otherwise, its immediate children are searched for a `CelestialBody`.
fn get_physical_body(
    target: Entity,
    q_children: &Query<&Children>,
    bodies: &Query<&CelestialBody>,
) -> Entity {
    // If the target itself is the body, we are done.
    if bodies.contains(target) { return target; }
    
    // Search children (one level deep is enough for our current Grid -> Body setup).
    if let Ok(children) = q_children.get(target) {
        for child in children.iter() {
            if bodies.contains(child) {
                return child;
            }
        }
    }
    
    target // Fallback
}
