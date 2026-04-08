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
use lunco_celestial::{CelestialClock, GravityBody, LocalGravityField};

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
        app.add_observer(on_surface_teleport_command);
        app.add_observer(on_leave_surface_command);

        app.register_type::<SpringArmCamera>()
           .register_type::<OrbitCamera>()
           .register_type::<ChaseCamera>()
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

        app.add_systems(PostUpdate, (
            // frame_blend_system,
            spring_arm_system,
            chase_camera_system,
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
    mut q_avatar: Query<(Entity, &mut Transform, &mut CellCoord, &mut SpringArmCamera, &ChildOf), (With<Avatar>, Without<FrameBlend>)>,
    q_spatial: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_spatial_abs: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    defaults: Res<CameraDefaults>,
    mut scroll_res: ResMut<CameraScroll>,
    sens: Res<CameraScrollSensitivity>,
    keys: Res<ButtonInput<KeyCode>>,
    spatial_query: Option<avian3d::prelude::SpatialQuery>,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) { return; }

    let dt = time.delta_secs();

    for (_avatar_ent, mut tf, mut cell, mut arm, child_of) in q_avatar.iter_mut() {
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
        let desired_rot = Quat::from_euler(EulerRot::YXZ, final_yaw, arm.pitch, 0.0);

        // Rotation: exponential decay for snappy but smooth heading follow.
        // Frequency 60.0 — snappy without transmitting physics jitter.
        let damping = arm.damping.unwrap_or(defaults.damping);
        let rot_alpha = 1.0 - (-60.0 * (1.0 - damping) * dt).exp();
        tf.rotation = tf.rotation.slerp(desired_rot, rot_alpha);

        // Desired camera position: behind target along smoothed rotation.
        let offset = tf.rotation.mul_vec3(Vec3::Z).as_dvec3() * arm.distance;
        let desired_pos = target_pos + offset + Vec3::Y.as_dvec3() * arm.vertical_offset as f64;

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
    mut commands: Commands,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) { return; }

    let dt = time.delta_secs();

    for (avatar_ent, mut tf, mut cell, mut orbit, child_of) in q_avatar.iter_mut() {
        let Ok((t_cell, t_tf)) = q_spatial.get(orbit.target) else { continue; };

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
        let min_dist = if let Ok(body) = q_bodies.get(orbit.target) {
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
fn freeflight_system(
    mut q_avatar: Query<(&mut Transform, &mut FreeFlightCamera), (With<Avatar>, Without<FrameBlend>)>,
) {
    for (mut tf, ff) in q_avatar.iter_mut() {
        // Apply rotation from accumulated yaw/pitch.
        let rotation = Quat::from_euler(EulerRot::YXZ, ff.yaw, ff.pitch, 0.0);
        tf.rotation = rotation;
    }
}

/// Frame blend system: smoothly interpolates between start position and a
/// target-relative endpoint that is recomputed every frame.
///
/// This mirrors the old working `avatar_transition_system` design:
/// blend in absolute solar coordinates, convert result to camera's current grid.
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
        let ease_t = if raw_t < 0.5 {
            2.0 * raw_t * raw_t
        } else {
            1.0 - (-2.0 * raw_t + 2.0).powi(2) / 2.0
        };

        if let Some(target_grid) = blend.target_grid {
            if child_of.0 != target_grid && child_of.0 != Entity::PLACEHOLDER {
                // Skip frame to wait for grid reparenting command to take effect.
                // Prevents massive coordinate artifacts during cross-grid blends.
                continue;
            }
        }

        let Ok(grid) = q_grids.get(child_of.0) else { continue; };

        // Recompute end position from target's CURRENT pose every frame.
        let end_rot = Quat::from_euler(EulerRot::YXZ, blend.end_yaw, blend.end_pitch, 0.0);
        let end_offset_base = end_rot.mul_vec3(Vec3::Z).as_dvec3() * blend.end_distance;
        let target_abs = if let Ok((t_cell, t_tf)) = q_spatial.get(blend.target) {
            lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                blend.target, t_cell, t_tf, &q_parents, &q_grids, &q_spatial_abs,
            )
        } else {
            DVec3::ZERO
        };
        let end_offset = end_offset_base + Vec3::Y.as_dvec3() * blend.end_vertical_offset as f64;
        let end_pos = target_abs + end_offset;

        // Recompute start position from source's CURRENT pose every frame.
        let dynamic_start_pos = if let Some(src) = blend.source_target {
            let src_abs = if let Ok((s_cell, s_tf)) = q_spatial.get(src) {
                lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                    src, s_cell, s_tf, &q_parents, &q_grids, &q_spatial_abs,
                )
            } else {
                target_abs // Fallback to avoid snapping
            };
            src_abs + blend.start_offset_from_source
        } else {
            blend.start_offset_from_source // acts as absolute position if no source
        };

        // Straight-line blend in the dynamic frames
        let blended_pos = dynamic_start_pos.lerp(end_pos, ease_t as f64);

        // Keep the planet centered by recomputing the look-at rotation every frame.
        // This prevents the target from "slipping" off-center during the flight.
        let from_target = blended_pos - target_abs;
        let look_at_yaw = from_target.x.atan2(from_target.z) as f32;
        let look_at_pitch = (-from_target.y).atan2((from_target.x * from_target.x + from_target.z * from_target.z).sqrt()) as f32;
        let look_at_rot = Quat::from_euler(EulerRot::YXZ, look_at_yaw, look_at_pitch, 0.0);
        let current_rot = blend.start_rot.slerp(look_at_rot, ease_t as f32);

        // Convert to camera's current grid (same grid the camera is currently parented to).
        let cam_grid_abs = if child_of.0 == Entity::PLACEHOLDER { DVec3::ZERO } else {
            lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
                child_of.0, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial_abs,
            )
        };
        let (new_cell, new_tf) = grid.translation_to_grid(blended_pos - cam_grid_abs);
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

                // Choose camera mode: OrbitCamera for spacecraft (no "up" in space),
                // SpringArmCamera for surface rovers (heading-follow).
                if blend.end_vertical_offset == 0.0 {
                    commands.entity(avatar_ent)
                        .insert(OrbitCamera {
                            target: vessel,
                            distance: blend.end_distance,
                            yaw: look_at_yaw,
                            pitch: look_at_pitch,
                            damping: None,
                            vertical_offset: 0.0,
                        });
                } else {
                    commands.entity(avatar_ent)
                        .insert(SpringArmCamera {
                            target: vessel,
                            distance: blend.end_distance,
                            yaw: 0.0,
                            pitch: look_at_pitch,
                            damping: Some(0.05),
                            vertical_offset: blend.end_vertical_offset,
                        });
                }
            } else {
                // Focus transition — insert OrbitCamera.
                commands.entity(avatar_ent)
                    .insert(OrbitCamera {
                        target: blend.target,
                        distance: blend.end_distance,
                        yaw: look_at_yaw,
                        pitch: look_at_pitch,
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
            commands.entity(avatar_ent)
                .insert(SpringArmCamera {
                    target: msg.target,
                    distance: end_distance,
                    yaw: 0.0,
                    pitch: end_pitch,
                    damping: Some(0.05),
                    vertical_offset: end_vert_off,
                });
        }

        commands.entity(avatar_ent)
            .remove::<FreeFlightCamera>()
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
        let cam_abs = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
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
        if msg.args.len() > 0 {
            distance = msg.args[0];
        } else if let Ok(body) = q_bodies.get(msg.target) {
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
/// Migrates avatar to be a child of the Body entity, sets up surface-relative
/// FreeFlightCamera, and initializes LocalGravityField.
fn on_surface_teleport_command(
    trigger: On<CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf), With<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial_abs: Query<(&CellCoord, &Transform)>,
    q_bodies: Query<(Entity, &CelestialBody, &ChildOf)>,
    q_frames: Query<&lunco_celestial::CelestialReferenceFrame>,
    mut field: ResMut<LocalGravityField>,
) {
    let msg = trigger.event();
    if msg.name != "TELEPORT_SURFACE" { return; }

    let Some((avatar_ent, cam_tf, cam_cell, cam_child_of)) = q_avatar.iter().next() else { return };

    // Find target body entity
    let target_body_entity: Option<Entity> = if !msg.args.is_empty() {
        // args[0] = target body entity index
        let idx = msg.args[0] as u64;
        Entity::from_bits(idx).into()
    } else { None };

    let (body_entity, body_radius, _body_child_of) = if let Some(e) = target_body_entity {
        if let Ok((e, b, c)) = q_bodies.get(e) {
            (e, b.radius_m, c)
        } else { return; }
    } else {
        // Find the body the camera is closest to (by distance to body surface)
        let cam_abs = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(
            avatar_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial_abs,
        );
        let mut best = None;
        let mut best_dist = f64::MAX;
        for (e, b, _) in q_bodies.iter() {
            let d = (cam_abs.length() - b.radius_m).abs();
            if d < best_dist { best_dist = d; best = Some((e, b.radius_m)); }
        }
        if let Some((e, r)) = best {
            if let Ok((_, _, c)) = q_bodies.get(e) {
                (e, r, c)
            } else { return; }
        } else { return; }
    };

    // Compute surface position
    let lat_deg = if msg.args.len() > 1 { Some(msg.args[1]) } else { None };
    let lon_deg = if msg.args.len() > 2 { Some(msg.args[2]) } else { Some(0.0) };

    let (surface_local_pos, surface_normal) = if let Some(lat) = lat_deg {
        // Specific coordinates
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
        // Project camera look direction onto body sphere
        let cam_dir = -cam_tf.forward().as_dvec3();
        let surface_normal = cam_dir.normalize();
        (surface_normal * (body_radius + 50.0), surface_normal)
    };

    let surface_g = body_radius; // placeholder; actual GM/R² from GravityProvider

    // Migrate avatar to Body entity
    let spawn_pos = Vec3::new(surface_local_pos.x as f32, surface_local_pos.y as f32, surface_local_pos.z as f32);
    let surface_rot = Quat::from_rotation_arc(DVec3::Y.as_vec3(), surface_normal.as_vec3());

    commands.entity(avatar_ent)
        .insert(Transform::from_translation(spawn_pos).with_rotation(surface_rot))
        .insert(CellCoord::default())
        .insert(GravityBody { body_entity })
        .insert(FreeFlightCamera {
            yaw: 0.0,
            pitch: -0.2,
            damping: None,
        })
        .remove::<OrbitCamera>()
        .remove::<SpringArmCamera>()
        .remove::<FrameBlend>();

    // Re-parent to Body entity
    commands.entity(body_entity).add_child(avatar_ent);

    // Update LocalGravityField
    field.body_entity = Some(body_entity);
    field.local_up = surface_normal;
    field.surface_g = surface_g;
    field.up = surface_normal;

    info!("Teleported to surface of body {:?} at {:?}", body_entity, surface_local_pos);
}

/// Leaves the surface and returns to orbit view.
///
/// Command: `LEAVE_SURFACE`
/// Teleports camera to 3x body radius altitude and switches to OrbitCamera.
fn on_leave_surface_command(
    trigger: On<CommandMessage>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, Option<&GravityBody>), With<Avatar>>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_frames: Query<&lunco_celestial::CelestialReferenceFrame>,
    mut field: ResMut<LocalGravityField>,
) {
    let msg = trigger.event();
    if msg.name != "LEAVE_SURFACE" { return; }

    let Some((avatar_ent, cam_tf, gravity_body)) = q_avatar.iter().next() else { return };

    // Find the body we're leaving
    let body_entity = if let Some(gb) = gravity_body {
        gb.body_entity
    } else {
        // Try to find from current grid's CelestialReferenceFrame
        let Some(cam_child_of) = q_parents.get(avatar_ent).ok() else { return; };
        if let Ok(frame) = q_frames.get(cam_child_of.0) {
            q_bodies.iter()
                .find(|(_, b)| b.ephemeris_id == frame.ephemeris_id)
                .map(|(e, _)| e)
                .unwrap_or(Entity::PLACEHOLDER)
        } else { return; }
    };

    let body_radius = if let Ok((_, body)) = q_bodies.get(body_entity) {
        body.radius_m
    } else { return; };

    // Find the Body's Grid
    let body_child_of = q_parents.get(body_entity).ok();
    let body_grid = body_child_of.map(|c| c.0);

    // Teleport to 3x body radius altitude
    let altitude = body_radius * 3.0;
    let orbit_pos_local = DVec3::new(0.0, altitude, altitude * 0.5);
    let orbit_pos = Vec3::new(orbit_pos_local.x as f32, orbit_pos_local.y as f32, orbit_pos_local.z as f32);

    // Re-parent to Grid (orbit frame)
    if let Some(grid_entity) = body_grid {
        if q_grids.contains(grid_entity) {
            // Compute grid-local position
            // Body is at Grid origin, so orbit_pos_local IS grid-local
            let (new_cell, new_tf) = if let Ok(grid) = q_grids.get(grid_entity) {
                grid.translation_to_grid(orbit_pos_local)
            } else {
                (CellCoord::default(), orbit_pos)
            };

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
                .remove::<SpringArmCamera>()
                .remove::<FrameBlend>()
                .remove::<GravityBody>();

            commands.entity(grid_entity).add_child(avatar_ent);
        }
    }

    // Clear gravity field
    field.body_entity = None;
    field.local_up = DVec3::Y;
    field.surface_g = 0.0;
    field.up = DVec3::Y;

    info!("Left surface, returned to orbit around {:?}", body_entity);
}
