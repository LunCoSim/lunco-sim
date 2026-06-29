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
use lunco_core::{Vessel, Avatar, CelestialBody, Spacecraft, register_commands, SessionProfiles, LocalSession, NetworkRole, LocalAvatar};
use lunco_core::attach::migrate_to_grid;
use lunco_celestial::{CelestialClock, LocalGravityField, TeleportToSurface, LeaveSurface};
use lunco_environment::{GravityBody, GravityProvider};
use lunco_settings::{AppSettingsExt, ProfileSettings};

pub mod commands;
pub use commands::*;
pub mod screenshot;
pub use screenshot::*;
pub mod recording;
pub use recording::*;

mod intents;

/// Upper bound on parent-chain walks when resolving an entity's owning Grid
/// or nearest clickable root. The scene hierarchies here are shallow (a few
/// levels); this cap purely guards the loop against running away on a
/// malformed/cyclic hierarchy — it does not encode a real structural depth.
/// (Unifies the former ad-hoc `0..10` / `MAX_DEPTH = 8` bounds.)
const MAX_HIERARCHY_WALK_DEPTH: usize = 16;

/// Fallback body radius (Earth mean radius, metres) used when a target
/// `CelestialBody` is missing — keeps altitude math finite instead of
/// collapsing distances to zero.
const EARTH_RADIUS_M_FALLBACK: f64 = 6_371_000.0;

/// UI panels for avatar status, camera mode, and surface coordinates.
#[cfg(feature = "ui")]
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
        Self { sensitivity: 0.1125 }
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
    // TODO(camera-smoothing): the exp-decay math below is hand-rolled. Review
    // existing crates and probably switch: bevy core's
    // `bevy::math::StableInterpolate::smooth_nudge` is exactly this
    // `1 - exp(-rate*dt)` form (drop-in for our manual lines); `bevy_easings`
    // for named easing curves; `smooth-bevy-cameras` / `bevy_dolly` for full
    // rigs (likely need adapting to our Grid/CellCoord floating origin). Also:
    // make smoothing fn + time-constant per-camera properties. See ../TODO.md.
    /// Base responsiveness (Hz) of rotation follow, before per-camera `damping`
    /// scales it. Used as `alpha = 1 - exp(-rotation_rate * (1 - damping) * dt)`.
    pub rotation_rate: f32,
    /// Base responsiveness (Hz) of position follow, before per-camera `damping`
    /// scales it. Same exp-decay form as `rotation_rate`.
    pub position_rate: f32,
    pub transition_duration: f32,
    pub default_distance: f64,
}

impl Default for CameraDefaults {
    fn default() -> Self {
        Self {
            damping: 0.1,
            rotation_rate: 60.0,
            position_rate: 30.0,
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
    /// Whether to derive camera heading from the target's body orientation.
    ///
    /// `true` for steerable vehicles (rovers) whose chassis has a meaningful
    /// "forward". `false` for freely-rolling rigid bodies (a ball, a balloon)
    /// whose body frame tumbles arbitrarily — reading their rotation would
    /// whip the camera around as the body spins. When `false`, heading is
    /// driven solely by the user's yaw (`yaw`); position still follows the
    /// target.
    pub track_heading: bool,
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

/// Marker for a **provisional** avatar camera — a stand-in spawned so the user
/// always has a controllable view while a scene is still loading and hasn't yet
/// authored its own Avatar camera.
///
/// It is *provisional* because the authored USD Avatar is the intended
/// perspective and **takes over** as soon as it materialises (which, on a slow
/// web/HTTP asset load, can be many seconds after the stand-in appeared). The
/// USD-avatar takeover despawns every entity carrying this marker in the **same
/// command flush** that installs the authored camera, so the provisional never
/// coexists with the real one — two simultaneous order-0 window `Camera3d`s
/// would otherwise produce camera-order ambiguity (double scene render) and a
/// duplicate `GizmoCamera`. A scene that authors no Avatar keeps its provisional
/// camera indefinitely: that is the legitimate permanent-fallback case.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component)]
pub struct ProvisionalAvatarCamera;

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

/// Host-only: record that the possessing session now owns the target vessel, so
/// the authority gate ([`lunco_core::authorize`]) accepts that session's
/// `DriveRover`s (gap G4). Runs for both local-host and wire-applied
/// possessions; the origin is the wire-apply guard (remote) or the local
/// session (host's own).
fn record_possession_authority(
    trigger: On<PossessVessel>,
    role: Res<lunco_core::NetworkRole>,
    guard: Res<lunco_core::SyncApplyGuard>,
    local: Res<lunco_core::LocalSession>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    mut registry: ResMut<lunco_core::SessionRegistry>,
) {
    if !role.is_host() {
        return;
    }
    let cmd = trigger.event();
    let origin = guard.0.unwrap_or(local.0);
    if let Ok(gid) = q_gid.get(cmd.target) {
        // One vessel per player. If the new target is claimable (free, or already
        // ours), drop EVERY vessel this session currently holds before claiming
        // it — so clicking through rovers swaps control instead of hoarding
        // ownership and locking every other player out under the Exclusive
        // policy. Frees are broadcast by `broadcast_ownership`; the prior owner's
        // client drops its stale bind via `enforce_ownership`. We check
        // `may_possess` FIRST so a denied claim (vessel owned by someone else)
        // never costs us the vessel we already hold.
        if registry.may_possess(origin, gid.get()) {
            let freed = registry.release_session(origin);
            let _ = registry.claim(origin, gid.get()); // infallible after may_possess
            if freed.is_empty() {
                info!("[auth] session {origin} possesses entity {}", gid.get());
            } else {
                info!(
                    "[auth] session {origin} possesses entity {} (released {} prior vessel(s))",
                    gid.get(),
                    freed.len()
                );
            }
        } else {
            let cur = registry.owner_of(gid.get());
            warn!(
                "[auth] entity {} already owned by {cur:?}; {origin} possession denied",
                gid.get()
            );
        }
    }
}

/// Host-side: free the releasing session's ownership when a [`ReleaseVessel`]
/// fires (local host release or a client's wire-applied one). Frees by SESSION
/// (a player holds one vessel) so it works without resolving the avatar entity
/// the command carries. The next `broadcast_ownership` propagates the freeing.
fn release_possession_authority(
    trigger: On<ReleaseVessel>,
    role: Res<lunco_core::NetworkRole>,
    guard: Res<lunco_core::SyncApplyGuard>,
    local: Res<lunco_core::LocalSession>,
    mut registry: ResMut<lunco_core::SessionRegistry>,
) {
    let _ = trigger;
    if !role.is_host() {
        return;
    }
    let origin = guard.0.unwrap_or(local.0);
    let freed = registry.release_session(origin);
    if !freed.is_empty() {
        info!("[auth] session {origin} released {} vessel(s)", freed.len());
    }
}

/// Client-side correction: drop control of any vessel the synced ownership table
/// no longer attributes to us (we lost a possession race, or the host force-
/// released us). Keeps "only one owner" true even when an optimistic local bind
/// raced another client. No-op on host/standalone and while a claim is pending
/// (owner still `None`).
fn enforce_ownership(
    role: Res<lunco_core::NetworkRole>,
    registry: Res<lunco_core::SessionRegistry>,
    session: Res<lunco_core::LocalSession>,
    q_avatar: Query<(Entity, &ControllerLink), With<Avatar>>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    mut commands: Commands,
) {
    if !matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }
    for (avatar, link) in q_avatar.iter() {
        let Ok(gid) = q_gid.get(link.vessel_entity) else {
            continue;
        };
        if let Some(owner) = registry.owner_of(gid.get()) {
            if owner != session.0 {
                commands.trigger(ReleaseVessel { target: avatar });
            }
        }
    }
}

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
        app.add_observer(record_possession_authority);
        app.add_observer(on_release_command);
        app.add_observer(release_possession_authority);
        app.add_observer(on_focus_command);
        app.add_observer(on_follow_command);
        app.add_observer(on_surface_teleport_command);
        app.add_observer(on_leave_surface_command);
        // Scene-click possession/follow/focus is now bevy_picking-driven: a
        // global `Pointer<Click>` observer (egui occlusion handled by the
        // framework), replacing the old `ScenePointer`-gated Update system.
        app.add_observer(avatar_raycast_possession);

        // Register all commands (generated by register_commands! macro at module scope)
        register_all_commands(app);
        app.register_type::<CaptureScreenshot>();

        // Screen recording: settings section, hotkey, state, and (under the
        // `recording` feature) the EasyScreenRecordPlugin encoder bridge.
        recording::build_recording(app);
        // Possession / follow commands cross the wire (a client takes control of
        // the host's authoritative rover, then drives it). The wire apply path
        // looks them up by reflected short type-path, so they MUST be in the type
        // registry. Their observers are wired manually below (not via the
        // `register_commands!` macro, which is what registers a type), so the
        // types are otherwise never registered — the host then logs "unknown
        // command type 'PossessVessel'", never records the client's ownership,
        // and rejects every subsequent DriveRover as unauthorized (the "client
        // rover won't move" bug). `#[Command]` already derives `#[reflect(Event)]`,
        // so registering the type also attaches the `ReflectEvent` the apply path
        // triggers through.
        app.register_type::<PossessVessel>()
           .register_type::<ReleaseVessel>()
           .register_type::<FollowTarget>()
           .register_type::<FocusTarget>();

        app.register_type::<SpringArmCamera>()
           .register_type::<OrbitCamera>()
           .register_type::<ChaseCamera>()
           .register_type::<FreeFlightCamera>()
           .register_type::<FrameBlend>()
           .register_type::<AdaptiveNearPlane>()
           .register_type::<ProvisionalAvatarCamera>()
           .register_type::<SurfaceRelativeMode>()
           .register_type::<SurfaceCamera>()
           .register_type::<SurfaceModeThreshold>()
           .register_type::<MouseSensitivity>();

        app.register_settings_section::<ProfileSettings>();
        app.register_type::<UpdateProfile>().add_observer(on_update_profile);
        app.init_resource::<RoverNameTagSettings>()
           .register_type::<RoverNameTagSettings>();

        app.add_systems(Update, (
            avatar_init_system,
            capture_avatar_intent,
            avatar_behavior_input_system,
            avatar_escape_possession,
            avatar_global_hotkeys,
            surface_mode_transition_system,
            enforce_ownership,
            sync_profile,
        ));

        // Possessed-rover name tags: an egui screen-space overlay (the scene has
        // only a `Camera3d`, so world-anchored `Text2d` never renders). Registered
        // here — not in `AvatarUiPlugin` — because the sandbox adds only
        // `LunCoAvatarPlugin`; `AvatarUiPlugin` is luncosim-only.
        #[cfg(feature = "ui")]
        app.add_systems(bevy_egui::EguiPrimaryContextPass, crate::ui::draw_rover_name_tags);

        // Chase camera shares the rover's fixed-step time domain so its
        // slerp/lerp uses a constant `dt = 1/60s`. Variable render-frame `dt`
        // produced perceptible jitter (`alpha = 1 - exp(-rate * dt)` made the
        // per-frame step proportional to whatever frame time the renderer
        // happened to deliver). Camera entities also carry
        // `TranslationInterpolation` + `RotationInterpolation`
        // (added at SpringArmCamera insertion) so the renderer eases between
        // fixed-step camera samples — same mechanism rigid bodies use.
        app.add_systems(FixedPostUpdate,
            spring_arm_system
                .after(avian3d::schedule::PhysicsSystems::Writeback));

        app.add_systems(PostUpdate, (
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
    // Initial spawn: anchor `ChildOf` in the bundle so parent + cell +
    // transform land atomically (same contract as `migrate_to_grid`).
    commands.spawn((
        Camera3d::default(),
        FreeFlightCamera { yaw, pitch, damping: None },
        AdaptiveNearPlane,
        Transform::from_translation(initial_offset.as_vec3()),
        GlobalTransform::default(),
        FloatingOrigin,
        CellCoord::default(),
        Avatar,
        LocalAvatar,
        IntentAnalogState::default(),
        ActionState::<lunco_core::UserIntent>::default(),
        lunco_controller::get_avatar_input_map(),
        Name::new("Avatar Camera"),
        ChildOf(grid_entity),
    )).id()
}

// ─── Shared Math Helpers (CQ-113 DRY) ────────────────────────────────────────

/// Radial "up" (outward surface normal) at a grid-local position.
///
/// A body sits at its Grid origin, so the normalized grid-local position vector
/// *is* the local up direction. Falls back to world-Y at/near the origin.
///
/// Consolidates the CQ-113 duplicate `if pos.length() > 1e-6 {
/// (pos / pos.length()).as_vec3() } else { Vec3::Y }` math (was inlined in
/// `spring_arm_system`, `freeflight_system`, `surface_camera_system`,
/// `avatar_universal_locomotion_system`, and `avatar_behavior_input_system`).
fn radial_up(pos: DVec3) -> Vec3 {
    if pos.length() > 1e-6 {
        (pos / pos.length()).as_vec3()
    } else {
        Vec3::Y
    }
}

/// Build a surface-relative camera orientation from a local `up` (surface
/// normal) plus `heading` and `pitch`.
///
/// Forward starts at local north (world-Y projected onto the tangent plane,
/// falling back to Z near the poles), is yawed by `heading` about `up`, then
/// pitched about the resulting right axis. Rebuilt from scratch (no incremental
/// accumulation) so there is zero roll drift.
///
/// Consolidates the CQ-113 duplicate tangent-frame math that was byte-identical
/// in `spring_arm_system` and `surface_camera_system`.
fn tangent_frame(up: Vec3, heading: f32, pitch: f32) -> Quat {
    let ref_dir = if up.dot(Vec3::Y).abs() < 0.9 { Vec3::Y } else { Vec3::Z };
    let east = up.cross(ref_dir).normalize();
    let north = east.cross(up).normalize();
    let heading_q = Quat::from_axis_angle(up, heading);
    let forward = heading_q.mul_vec3(north);
    let right = forward.cross(up).normalize();
    let base_rot = Quat::from_mat3(&Mat3::from_cols(right, up, -forward));
    let pitch_q = Quat::from_axis_angle(right, pitch);
    (pitch_q * base_rot).normalize()
}

/// Apply an accumulated mouse-scroll delta as a multiplicative (exponential)
/// zoom to a camera arm `distance`, clamped to `[min_dist, max_dist]`, then
/// consume the delta. Scroll up (delta > 0) zooms in; down zooms out.
///
/// Consolidates the CQ-113 duplicate zoom math shared by the spring-arm, chase,
/// and orbit camera systems (they differed only in the clamp bounds).
fn apply_scroll_zoom(distance: &mut f64, scroll_delta: &mut f32, sens: f32, min_dist: f64, max_dist: f64) {
    if *scroll_delta != 0.0 {
        let zoom_factor = (-*scroll_delta as f64 * sens as f64 * 0.01).exp();
        *distance = (*distance * zoom_factor).clamp(min_dist, max_dist);
        *scroll_delta = 0.0;
    }
}

/// Migrate the avatar to a target's Grid, placing it at `final_abs_pos` /
/// `final_rot` (root-frame absolute pose) converted into the target grid's
/// local coordinates. No-op when `target_grid` is `None`/placeholder or not a
/// live Grid.
///
/// Consolidates the CQ-113 duplicate migration block shared by
/// `on_possess_command`, `on_follow_command`, and `on_focus_command`.
fn migrate_avatar_to_target_grid(
    commands: &mut Commands,
    avatar_ent: Entity,
    target_grid: Option<Entity>,
    final_abs_pos: DVec3,
    final_rot: Quat,
    q_grids: &Query<&Grid>,
    q_parents: &Query<&ChildOf>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
) {
    if let Some(tg) = target_grid {
        if tg != Entity::PLACEHOLDER {
            if let Ok(target_grid_ref) = q_grids.get(tg) {
                let target_grid_abs = lunco_core::coords::world_position_seeded(
                    tg, &CellCoord::default(), &Transform::default(), q_parents, q_grids, q_spatial,
                );
                let (new_cell, new_translation) =
                    target_grid_ref.translation_to_grid(final_abs_pos - target_grid_abs);
                let local_tf =
                    Transform::from_translation(new_translation).with_rotation(final_rot);
                migrate_to_grid(commands, avatar_ent, tg, new_cell, local_tf);
            }
        }
    }
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
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    _q_parents: Query<&ChildOf>,
    q_dragging: Query<(), With<lunco_core::GizmoDragging>>,

    defaults: Res<CameraDefaults>,

    mut scroll_res: ResMut<CameraScroll>,
    sens: Res<CameraScrollSensitivity>,
    keys: Res<ButtonInput<KeyCode>>,
    spatial_query: Option<avian3d::prelude::SpatialQuery>,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) { return; }

    let dt = time.delta_secs();

    for (_avatar_ent, mut tf, mut cell, mut arm, child_of, surface_mode) in q_avatar.iter_mut() {
        // Skip follow while the target is being dragged by the editor gizmo
        // (marker set by sandbox-edit; never present on a headless server).
        if q_dragging.get(arm.target).is_ok() { continue; }

        let Ok((t_cell, t_tf)) = q_spatial.get(arm.target) else { continue; };
        let t_cell = t_cell.copied().unwrap_or_default();
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };

        // Target position in grid-local coordinates.
        let target_pos = grid.grid_position_double(&t_cell, t_tf);

        // Multiplicative zoom using exponential scaling — same formula as
        // ChaseCamera/OrbitCamera so raw pixel scroll deltas stay well-scaled.
        // Scroll up (delta > 0) -> zoom in. Scroll down (delta < 0) -> zoom out.
        apply_scroll_zoom(&mut arm.distance, &mut scroll_res.delta, sens.value, 5.0, 200.0);

        // Resolve rover heading in double-precision to eliminate quantization
        // jitter. The rover Transform is already render-frame-interpolated by
        // avian's `PhysicsInterpolationPlugin::interpolate_all()` (runs in
        // `RunFixedMainLoop` before Update), so reading it directly here
        // gives a smooth signal — no extra low-pass needed. An additional
        // exp-decay filter would re-introduce jitter under variable frame
        // time because alpha = 1 - exp(-rate*dt) makes the per-frame catch-up
        // step proportional to dt, so the camera's lag wobbles around its
        // mean as frame timing fluctuates.
        // Only steerable vehicles have a meaningful body heading. A freely-
        // rolling rigid body (ball, balloon) tumbles its body frame, so its
        // forward vector flips around as it rolls — deriving heading from it
        // swings the camera wildly. For those, heading is user-only (yaw).
        let target_heading_d = if arm.track_heading {
            let target_fwd_d = t_tf.rotation.mul_vec3(Vec3::NEG_Z).as_dvec3();
            if target_fwd_d.x.abs() > 1e-6 || target_fwd_d.z.abs() > 1e-6 {
                -target_fwd_d.x.atan2(-target_fwd_d.z)
            } else { 0.0 }
        } else {
            0.0
        };

        let final_yaw = (target_heading_d + arm.yaw as f64) as f32;

        // Rotation: surface-relative or ecliptic-locked
        let desired_rot = if surface_mode.is_some() {
            // "Up" = surface normal at the rover's position = rover's grid-local direction from body center.
            // Both rover and camera are on the Body's Grid; body is at Grid origin.
            let up_v = radial_up(target_pos);
            // Surface mode: compute rotation from scratch using local_up as "up".
            // This avoids accumulated roll drift from incremental rotations
            // (see surface_camera_investigation.md for root cause analysis).
            // Combines rover heading with user yaw offset around the surface normal.
            tangent_frame(up_v, final_yaw, arm.pitch)
        } else {
            Quat::from_euler(EulerRot::YXZ, final_yaw, arm.pitch, 0.0)
        };

        // Rotation: exponential decay for snappy but smooth heading follow.
        // Frequency 60.0 — snappy without transmitting physics jitter.
        let damping = arm.damping.unwrap_or(defaults.damping);
        let rot_alpha = 1.0 - (-defaults.rotation_rate * (1.0 - damping) * dt).exp();
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

        // Collision response: only the arm LENGTH is smoothed, and only when an
        // obstacle forces it shorter than the user asked for. The arm DIRECTION
        // (ray_dir) already tracks the user's rotation instantly, so orbiting in
        // open space is 1:1 with the mouse — there the target length equals the
        // desired length equals the current length, and the lerp is a no-op.
        // Smoothing kicks in only when a hit pulls the camera in (and eases back
        // out when the obstacle clears), never on human rotation.
        let desired_len = ray_len;
        let target_len = match hit {
            Some(hit_data) => ((hit_data.distance - 0.5).min(desired_len)).max(0.0),
            None => desired_len,
        };
        let current_pos = grid.grid_position_double(&cell, &tf);
        let current_len = current_pos.distance(target_pos);
        // First frame (camera still at grid origin) or already at target: snap.
        let final_len = if current_len < 1e-3 {
            target_len
        } else {
            let alpha = (1.0 - (-defaults.position_rate * (1.0 - damping) * dt).exp()) as f64;
            current_len + (target_len - current_len) * alpha
        };
        let final_pos = target_pos + ray_dir * final_len;

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
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_dragging: Query<(), With<lunco_core::GizmoDragging>>,
    defaults: Res<CameraDefaults>,
    mut scroll_res: ResMut<CameraScroll>,
    sens: Res<CameraScrollSensitivity>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) { return; }

    let dt = time.delta_secs();

    for (_avatar_ent, mut tf, mut cell, mut chase, child_of) in q_avatar.iter_mut() {
        // Skip follow while the target is being dragged by the editor gizmo
        // (marker set by sandbox-edit; never present on a headless server).
        if q_dragging.get(chase.target).is_ok() { continue; }

        let Ok((t_cell, t_tf)) = q_spatial.get(chase.target) else { continue; };
        let t_cell = t_cell.copied().unwrap_or_default();
        let Ok(grid) = q_grids.get(child_of.0) else { continue; };

        // Target position in grid-local coordinates.
        let target_pos = grid.grid_position_double(&t_cell, t_tf);

        // Multiplicative zoom using exponential scaling.
        // Scroll up (delta > 0) -> zoom in. Scroll down (delta < 0) -> zoom out.
        apply_scroll_zoom(&mut chase.distance, &mut scroll_res.delta, sens.value, 5.0, 1.0e6);

        // Follow target's full 3D orientation (heading + pitch + roll).
        // Same formula as SpringArmCamera: target rotation * user offset.
        let rotation = t_tf.rotation * Quat::from_euler(EulerRot::YXZ, chase.yaw, chase.pitch, 0.0);

        let offset = rotation.mul_vec3(Vec3::Z).as_dvec3() * chase.distance;
        let desired_pos = target_pos + offset + Vec3::Y.as_dvec3() * chase.vertical_offset as f64;

        // Position lerp: same formula as SpringArmCamera and old working code.
        let damping = chase.damping.unwrap_or(defaults.damping);
        let lerp_factor = (1.0 - (-defaults.position_rate * (1.0 - damping) * dt).exp()) as f64;
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
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_bodies: Query<&CelestialBody>,
    q_sc: Query<&Spacecraft>,
    q_dragging: Query<(), With<lunco_core::GizmoDragging>>,
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
        // Skip follow while the target is being dragged by the editor gizmo
        // (marker set by sandbox-edit; never present on a headless server).
        if q_dragging.get(orbit.target).is_ok() { continue; }

        let Ok((_t_cell, _t_tf)) = q_spatial.get(orbit.target) else { continue; };

        // Find the target's grid.
        let mut target_grid = orbit.target;
        for _ in 0..MAX_HIERARCHY_WALK_DEPTH {
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
            let cam_abs = lunco_core::coords::world_position_seeded(
                avatar_ent, &cell, &tf, &q_parents, &q_grids, &q_spatial,
            );
            if let Ok(target_grid_ref) = q_grids.get(target_grid) {
                let target_grid_abs = lunco_core::coords::world_position_seeded(
                    target_grid, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial,
                );
                let (new_cell, new_translation) =
                    target_grid_ref.translation_to_grid(cam_abs - target_grid_abs);
                let local_tf = Transform::from_translation(new_translation).with_rotation(tf.rotation);
                migrate_to_grid(&mut commands, avatar_ent, target_grid, new_cell, local_tf);
            }
            // Migration is deferred; next frame `child_of` resolves to the new grid.
            continue;
        }

        // Now both camera and target are on the same grid — simple position lookup.
        let (t_cell_now, t_tf_now) = if let Ok((c, t)) = q_spatial.get(orbit.target) {
            (c.copied().unwrap_or_default(), t)
        } else { continue; };
        let grid_ref = if let Ok(g) = q_grids.get(child_of.parent()) {
            g
        } else { continue; };

        let target_pos = grid_ref.grid_position_double(&t_cell_now, t_tf_now);

        // Multiplicative zoom: proportional to current distance using exponential scaling.
        // Scroll up (delta > 0) -> zoom in. Scroll down (delta < 0) -> zoom out.
        apply_scroll_zoom(&mut orbit.distance, &mut scroll_res.delta, sens.value, min_dist, 1.0e11);

        // Camera rotation from user yaw/pitch (ecliptic-locked).
        let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
        let offset = rotation.mul_vec3(Vec3::Z).as_dvec3() * orbit.distance;
        let desired_pos = target_pos + offset + Vec3::Y.as_dvec3() * orbit.vertical_offset as f64;

        // Orbit follows the user's rotation instantly: the camera direction
        // tracks yaw/pitch 1:1, only the arm LENGTH is eased (so zoom glides
        // instead of snapping). No collision here, so length only changes on
        // zoom — rotation is never smoothed.
        let damping = orbit.damping.unwrap_or(defaults.damping);
        let dir = (desired_pos - target_pos).normalize_or(DVec3::Y);
        let desired_len = desired_pos.distance(target_pos);
        let current_pos = grid_ref.grid_position_double(&cell, &tf);
        let current_len = current_pos.distance(target_pos);
        let final_len = if current_len < 1e-3 {
            desired_len
        } else {
            let alpha = (1.0 - (-defaults.position_rate * (1.0 - damping) * dt).exp()) as f64;
            current_len + (desired_len - current_len) * alpha
        };
        let next_pos = target_pos + dir * final_len;

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
                radial_up(grid.grid_position_double(cell, &tf))
            } else { Vec3::Y };

            // In surface mode, apply yaw/pitch as incremental rotations.
            let yaw_q = Quat::from_axis_angle(up_v, ff.yaw);
            let right: Vec3 = *tf.right();
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
        let up = radial_up(grid.grid_position_double(cell, &tf));

        // Rebuild the rotation from scratch each frame from heading + pitch
        // around the surface normal (local north = world-Y onto the tangent
        // plane, Z near the poles). No incremental rotations -> zero roll drift.
        tf.rotation = tangent_frame(up, cam.heading, cam.pitch);
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
            radial_up(current_pos)
        } else {
            Vec3::Y
        };

        let mut move_vec = Vec3::ZERO;
        move_vec += *tf.forward() * analog.forward;
        move_vec += *tf.right() * analog.side;
        move_vec += up_dir * analog.elevation;

        let next_pos = current_pos + move_vec.as_dvec3() * 23.1 * (1.0 / 60.0);
        let (new_cell, new_tf) = grid.translation_to_grid(next_pos);
        *cell = new_cell;
        tf.translation = new_tf;
    }
}

// ─── Intent & Input ──────────────────────────────────────────────────────────

/// Captures high-level [UserIntent] signals and forwards zoom input.
fn capture_avatar_intent(
    mut q_avatar: Query<(Entity, &IntentState, &mut IntentAnalogState), With<Avatar>>,
    clock: Option<Res<CelestialClock>>,
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    scroll_res: ResMut<CameraScroll>,
) {
    let mut delta = Vec2::ZERO;
    let mut mouse_moved = false;

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
                    radial_up(grid.grid_position_double(cell, &tf))
                } else { Vec3::Y };
                let yaw_q = Quat::from_axis_angle(up_v, delta_yaw);
                let right: Vec3 = *tf.right();
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

/// Walks up the parent chain from a raycast hit to find the nearest
/// click-target — anything tagged `SelectableRoot` (which includes
/// vessels, balloons, props, panels). Ground/terrain hits return `None`.
fn find_clickable_from_hit(
    mut entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_selectable: &Query<Entity, With<lunco_core::SelectableRoot>>,
    q_ground: &Query<Entity, With<lunco_core::Ground>>,
) -> Option<Entity> {
    for _ in 0..MAX_HIERARCHY_WALK_DEPTH {
        if q_ground.get(entity).is_ok() { return None; }
        if q_selectable.get(entity).is_ok() { return Some(entity); }
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
/// Plain-click dispatcher: routes a left-click on a world entity to one of
/// three typed commands.
///
/// | Hit                         | Command          |
/// |-----------------------------|------------------|
/// | `Vessel` (rover, spacecraft)| `PossessVessel`  |
/// | other `SelectableRoot`      | `FollowTarget`   |
/// | `CelestialBody` (no marker) | `FocusTarget`    |
///
/// Idempotency lives in each observer (no-op if state already matches).
/// `DragModeActive` blocks clicks while a transform gizmo is up so the user
/// can drag a handle without flipping the camera.
pub fn avatar_raycast_possession(
    // Driven by bevy_picking: a global `On<Pointer<Click>>` observer. egui's
    // picking backend resolves panel-vs-scene occlusion, so a click on chrome
    // never reaches a scene vessel. The chrome guard is `hit.position.is_none()`
    // (egui's pick carries no world position; a real mesh hit always does).
    mut click: On<bevy::picking::events::Pointer<bevy::picking::events::Click>>,
    keys: Res<ButtonInput<KeyCode>>,
    camera_q: Query<(&Camera, &GlobalTransform, Entity), With<Avatar>>,
    drag_mode_active: Res<lunco_core::DragModeActive>,
    spawn_tool_active: Res<lunco_core::SpawnToolActive>,
    mut commands: Commands,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>,
    q_spacecraft: Query<(Entity, &GlobalTransform, &Spacecraft)>,
    q_vessel: Query<Entity, With<Vessel>>,
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_parents: Query<&ChildOf>,
    q_ground: Query<Entity, With<lunco_core::Ground>>,
) {
    use bevy::picking::pointer::PointerButton;
    // Left button only.
    if click.button != PointerButton::Primary { return; }
    // Chrome guard — egui's pick has no world position.
    if click.hit.position.is_none() { return; }
    // Shift+click is reserved for entity selection / gizmo multi-select in
    // lunco-sandbox-edit (`on_scene_click_select`, the other global
    // `Pointer<Click>` observer). A plain left-click possesses/follows/focuses;
    // a Shift+click never does. This modifier split is what keeps the two
    // observers from both acting on a single click.
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) { return; }
    // Alt-click is likewise reserved for the editor.
    if keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]) { return; }
    // Mid-drag on a transform gizmo: don't flip the camera under the user.
    if drag_mode_active.active { return; }
    // Spawn placement tool armed: clicks place objects, don't possess.
    if spawn_tool_active.0 { return; }

    // This observer handles the plain click now (it passed every guard above), so
    // stop the auto-propagation to ancestor entities — otherwise a global
    // observer re-fires once per ancestor. The analytic spacecraft/celestial
    // sphere tests below depend on the ray, not on `click.entity`, so they'd
    // re-trigger `PossessVessel`/`FocusTarget` for every ancestor in the chain
    // (we must not gate this on a *mesh* hit being found, the earlier bug).
    click.propagate(false);

    // Build the world ray from the avatar camera through the click position, so
    // the analytic hit-sphere tests (celestial bodies / spacecraft, which have
    // no pickable mesh) still work alongside the mesh pick.
    let Some((camera, cam_gtf, avatar_entity)) = camera_q.iter().next() else { return; };
    let Ok(ray) = camera.viewport_to_world(cam_gtf, click.pointer_location.position) else { return; };

    // The mesh the pick resolved to (rover, prop, ground, …); resolve to its
    // clickable root. `hit.depth` is the along-ray distance to compare against
    // the analytic spheres below.
    let mut nearest_clickable: Option<Entity> = None;
    let mut min_t = f32::INFINITY;

    if let Some(root) = find_clickable_from_hit(click.entity, &q_parents, &q_selectable, &q_ground) {
        min_t = click.hit.depth;
        nearest_clickable = Some(root);
    }

    // Spacecraft hit-spheres (no real colliders) — possessable, not selectable.
    let mut spacecraft_hit: Option<Entity> = None;
    for (entity, gtf, sc) in q_spacecraft.iter() {
        let oc = ray.origin - gtf.translation();
        let b = oc.dot(ray.direction.as_vec3());
        let c = oc.dot(oc) - sc.hit_radius_m.powi(2);
        let discr = b * b - c;
        if discr >= 0.0 {
            let t = -b - discr.sqrt();
            if t > 0.0 && t < min_t {
                min_t = t;
                nearest_clickable = None;
                spacecraft_hit = Some(entity);
            }
        }
    }

    // Celestial bodies — focus only (orbit-distance scale).
    let mut body_hit: Option<Entity> = None;
    for (entity, gtf, body) in q_bodies.iter() {
        let oc = ray.origin - gtf.translation();
        let b = oc.dot(ray.direction.as_vec3());
        let c = oc.dot(oc) - (body.radius_m as f32).powi(2);
        let discr = b * b - c;
        if discr >= 0.0 {
            let t = -b - discr.sqrt();
            if t > 0.0 && t < min_t {
                min_t = t;
                nearest_clickable = None;
                spacecraft_hit = None;
                body_hit = Some(entity);
            }
        }
    }

    if let Some(target) = body_hit {
        commands.trigger(FocusTarget { avatar: Some(avatar_entity), target });
    } else if let Some(target) = spacecraft_hit {
        commands.trigger(PossessVessel { avatar: Some(avatar_entity), target });
    } else if let Some(target) = nearest_clickable {
        if q_vessel.get(target).is_ok() {
            commands.trigger(PossessVessel { avatar: Some(avatar_entity), target });
        } else {
            commands.trigger(FollowTarget { avatar: Some(avatar_entity), target });
        }
    }
}

/// Backspace releases possession **and** plain follow — both unwind through
/// the same `ReleaseVessel` path (which strips ControllerLink, SpringArm,
/// interpolation, and reinstates a free-flight camera).
fn avatar_escape_possession(
    keys: Res<ButtonInput<KeyCode>>,
    q_avatar: Query<Entity, (With<Avatar>, Or<(With<ControllerLink>, With<SpringArmCamera>)>)>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::Backspace) { return; }
    for entity in q_avatar.iter() {
        commands.trigger(ReleaseVessel { target: entity });
    }
}

// ─── Commands ────────────────────────────────────────────────────────────────

/// Releases possession of a vessel.
///
/// Keeps the camera at its current position — no jarring teleport.
/// Switches to `FreeFlightCamera` mode with the current orientation preserved.
fn on_release_command(
    trigger: On<ReleaseVessel>,
    mut commands: Commands,
    q_avatar: Query<(&Transform, Option<&ControllerLink>, Option<&SurfaceRelativeMode>), With<Avatar>>,
    guard: Res<lunco_core::SyncApplyGuard>,
) {
    // A wire-applied release (a client telling the host it let go) carries that
    // client's avatar, which is meaningless here — the host frees ownership in
    // `release_possession_authority`, not by touching a local camera.
    if guard.is_from_sync() {
        return;
    }
    let cmd = trigger.event();
    let avatar_ent = cmd.target;
    let (yaw, pitch, opt_link, is_surface) = if let Ok((tf, link, surface)) = q_avatar.get(avatar_ent) {
        let (y, p, _) = tf.rotation.to_euler(EulerRot::YXZ);
        (y, p, link, surface.is_some())
    } else { (0.0, 0.0, None, false) };

    // Hard stop the rover upon disengaging control.
    if let Some(link) = opt_link {
        commands.trigger(lunco_mobility::DriveRover {
            target: link.vessel_entity,
            forward: 0.0,
            steer: 0.0,
            seq: 0,
            tick: 0,
        });
    }

    commands.entity(avatar_ent)
        .remove::<ControllerLink>()
        .remove::<ActionState<lunco_controller::VesselIntent>>()
        .remove::<InputMap<lunco_controller::VesselIntent>>()
        .remove::<SpringArmCamera>()
        .remove::<OrbitCamera>()
        .remove::<FrameBlend>()
        // FreeFlight/Surface cameras run in PostUpdate at render rate, so
        // strip the fixed-step interpolation that SpringArmCamera relied on.
        .remove::<avian3d::prelude::TranslationInterpolation>()
        .remove::<avian3d::prelude::RotationInterpolation>();

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

/// No-op placeholder.
///
/// **History**: this observer used to forward analog WASD into `DriveRover`
/// commands, racing the typed `lunco-controller::translate_intents_to_commands`
/// path on the same physical keys. Two writers on the same steer port produced
/// per-frame torque oscillation (jitter on rotation) and the embedded
/// "Ctrl-zeroes-rover" hack made Ctrl stop the wheels even though Ctrl is now
/// strictly a camera modifier. The vessel-driving logic lives entirely in
/// `lunco-controller` now; this observer is left in place only so the
/// `IntentAnalogState` event still has a registered handler if other crates
/// rely on it firing.
fn on_user_intent(_trigger: On<IntentAnalogState>) {}

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
    trigger: On<PossessVessel>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf, Option<&ControllerLink>), With<Avatar>>,
    q_spatial_abs: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_sc: Query<&Spacecraft>,
    q_vessel: Query<&Vessel>,
    q_vessel_gravity: Query<&GravityBody>,
    guard: Res<lunco_core::SyncApplyGuard>,
    registry: Res<lunco_core::SessionRegistry>,
    session: Res<lunco_core::LocalSession>,
    q_owned: Query<&lunco_core::GlobalEntityId>,
) {
    let cmd = trigger.event();
    // A *remote* possession applied from the wire (host attributing a client's
    // claim) must NOT bind a local camera — the host has no camera for that
    // player. Authority is recorded separately by `record_possession_authority`;
    // here we only do the local camera-bind for our own (non-wire) possessions.
    if guard.is_from_sync() {
        return;
    }
    // Possession arbitration (policy-driven): under the default `Exclusive`
    // policy, refuse to bind a vessel the (synced) ownership table says another
    // session already controls; under `LastWins` the bind is always allowed and
    // steals the vessel. The host's `SessionRegistry` is authoritative; clients
    // hold a replicated copy (via `OwnershipMsg`). Single-player's table is empty
    // so this never blocks.
    if let Ok(gid) = q_owned.get(cmd.target) {
        if !registry.may_possess(session.0, gid.get()) {
            info!(
                "[possess] vessel {} owned by another session — refused (exclusive policy)",
                gid.get()
            );
            return;
        }
    }
    // Resolve the avatar to bind the camera to: the command's avatar if it
    // names a live one, else any local avatar. With no avatar at all (headless /
    // direct control) there is nothing to bind — the authority claim already
    // ran in `record_possession_authority`, so just skip the camera work.
    let resolved = cmd.avatar
        .and_then(|a| q_avatar.get(a).ok())
        .or_else(|| q_avatar.iter().next());
    let Some((avatar_ent, cam_tf, cam_cell, _child_of, existing_link)) = resolved else { return; };

    // Idempotent: already controlling this exact target — no-op.
    if let Some(link) = existing_link {
        if link.vessel_entity == cmd.target { return; }
    }

    // Compute camera absolute position in root frame.
    let cam_abs = lunco_core::coords::world_position_seeded(
        avatar_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial_abs,
    );

    // Compute target absolute position.
    let target_abs = if let Ok((t_cell, t_tf)) = q_spatial_abs.get(cmd.target) {
        let cell = t_cell.copied().unwrap_or_default();
        lunco_core::coords::world_position_seeded(
            cmd.target, &cell, t_tf, &q_parents, &q_grids, &q_spatial_abs,
        )
    } else {
        cam_abs // Fallback
    };

    let target_grid = get_grid_for_entity(cmd.target, &q_parents, &q_grids);
    let is_spacecraft = q_sc.contains(cmd.target);
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
    migrate_avatar_to_target_grid(
        &mut commands, avatar_ent, target_grid, final_abs_pos, final_rot,
        &q_grids, &q_parents, &q_spatial_abs,
    );

    // VesselIntent state + input map go on the **avatar**, not the vessel.
    // `lunco-controller::translate_intents_to_commands` joins on
    // `(VesselIntentState, ControllerLink)` — both must live on the same
    // entity for the query to match. Putting the action state on the vessel
    // (as a stranded component) silently disabled the entire keyboard drive
    // path; the legacy `on_user_intent` observer was masking the bug.
    commands.entity(avatar_ent).insert((
        ControllerLink { vessel_entity: cmd.target },
        ActionState::<lunco_controller::VesselIntent>::default(),
        lunco_controller::get_default_input_map(),
    ));

    // Detect if target is a surface vehicle (has GravityBody) and propagate surface mode.
    let is_surface_vehicle = q_vessel_gravity.get(cmd.target).is_ok();

    if end_vert_off == 0.0 {
        commands.entity(avatar_ent)
            .insert(OrbitCamera {
                target: cmd.target,
                distance: end_distance,
                yaw: if is_spacecraft { current_yaw } else { end_yaw },
                pitch: if is_spacecraft { current_pitch } else { end_pitch },
                damping: None,
                vertical_offset: 0.0,
            });
    } else {
        let mut cmd_ent = commands.entity(avatar_ent);
        cmd_ent.insert((
            SpringArmCamera {
                target: cmd.target,
                distance: end_distance,
                yaw: 0.0,
                pitch: end_pitch,
                damping: Some(0.05),
                vertical_offset: end_vert_off,
                // Only steerable vessels (rovers) have a meaningful heading;
                // a possessed ball/prop tumbles, so track user yaw only.
                track_heading: q_vessel.contains(cmd.target),
            },
            // Camera updates in FixedPostUpdate; ease its Transform between
            // fixed samples so the rendered camera doesn't staircase at 60Hz.
            avian3d::prelude::TranslationInterpolation,
            avian3d::prelude::RotationInterpolation,
        ));
        // If possessing a surface vehicle, enable surface-relative camera mode
        if is_surface_vehicle {
            if let Ok(gb) = q_vessel_gravity.get(cmd.target) {
                cmd_ent.insert(*gb);
            }
            cmd_ent.insert(SurfaceRelativeMode);
        }
    }

    commands.entity(avatar_ent)
        .remove::<FreeFlightCamera>()
        .remove::<SurfaceCamera>()
        .remove::<FrameBlend>();
}

/// Follows a target with the chase camera but without taking control.
///
/// Conceptually `PossessVessel` minus the controller binding: the avatar
/// rides along behind the target, but keyboard input no longer drives any
/// vessel. Used for non-`Vessel` objects (balloons, props, observation
/// targets). Idempotent — clicking the same already-followed target is a
/// no-op so we don't churn components every frame.
fn on_follow_command(
    trigger: On<FollowTarget>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf, Option<&SpringArmCamera>), With<Avatar>>,
    q_spatial_abs: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_vessel: Query<&Vessel>,
    q_vessel_gravity: Query<&GravityBody>,
) {
    let cmd = trigger.event();
    let resolved = cmd.avatar
        .and_then(|a| q_avatar.get(a).ok())
        .or_else(|| q_avatar.iter().next());
    let Some((avatar_ent, cam_tf, cam_cell, _child_of, existing_spring)) = resolved else { return; };

    // Idempotent: already following this target — no-op.
    if let Some(arm) = existing_spring {
        if arm.target == cmd.target { return; }
    }

    // Target absolute position in root frame.
    let target_abs = if let Ok((t_cell, t_tf)) = q_spatial.get(cmd.target) {
        let cell = t_cell.copied().unwrap_or_default();
        lunco_core::coords::world_position_seeded(
            cmd.target, &cell, t_tf, &q_parents, &q_grids, &q_spatial_abs,
        )
    } else {
        // Fallback: keep camera where it is.
        lunco_core::coords::world_position_seeded(
            avatar_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial_abs,
        )
    };

    let target_grid = get_grid_for_entity(cmd.target, &q_parents, &q_grids);
    let end_distance = 15.0_f64;
    let end_vert_off = 2.0_f32;
    let end_pitch = -0.25_f32;

    // Snap behind the target with a default chase pose.
    let final_rot = Quat::from_euler(EulerRot::YXZ, 0.0, end_pitch, 0.0);
    let final_offset = final_rot.mul_vec3(Vec3::Z).as_dvec3() * end_distance;
    let final_abs_pos = target_abs + final_offset + Vec3::Y.as_dvec3() * end_vert_off as f64;

    migrate_avatar_to_target_grid(
        &mut commands, avatar_ent, target_grid, final_abs_pos, final_rot,
        &q_grids, &q_parents, &q_spatial_abs,
    );

    // Strip any prior controller binding — follow ≠ possess.
    let mut cmd_ent = commands.entity(avatar_ent);
    cmd_ent
        .remove::<ControllerLink>()
        .remove::<ActionState<lunco_controller::VesselIntent>>()
        .remove::<InputMap<lunco_controller::VesselIntent>>()
        .remove::<FreeFlightCamera>()
        .remove::<SurfaceCamera>()
        .remove::<OrbitCamera>()
        .remove::<FrameBlend>()
        .insert((
            SpringArmCamera {
                target: cmd.target,
                distance: end_distance,
                yaw: 0.0,
                pitch: end_pitch,
                damping: Some(0.05),
                vertical_offset: end_vert_off,
                // Followed props (balloons, balls) tumble — heading is user-only.
                track_heading: q_vessel.contains(cmd.target),
            },
            avian3d::prelude::TranslationInterpolation,
            avian3d::prelude::RotationInterpolation,
        ));

    // Surface-relative mode if following a body on a gravity well.
    if let Ok(gb) = q_vessel_gravity.get(cmd.target) {
        cmd_ent.insert(*gb).insert(SurfaceRelativeMode);
    } else {
        cmd_ent.remove::<SurfaceRelativeMode>();
    }
}

/// Focuses on a target with an instant transition to OrbitCamera mode.
fn on_focus_command(
    trigger: On<FocusTarget>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf), With<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_spatial_abs: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_bodies: Query<&CelestialBody>,
    q_sc: Query<&Spacecraft>,
    q_children: Query<&Children>,
    _q_orbit: Query<&OrbitCamera>,
    _q_spring: Query<&SpringArmCamera>,
    _q_chase: Query<&ChaseCamera>,
) {
    let cmd = trigger.event();
    let resolved = cmd.avatar
        .and_then(|a| q_avatar.get(a).ok())
        .or_else(|| q_avatar.iter().next());
    let Some((avatar_ent, cam_tf, cam_cell, _child_of)) = resolved else { return; };

    // Compute camera absolute position in root frame.
    let _cam_abs = lunco_core::coords::world_position_seeded(
        avatar_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial_abs,
    );

    // Compute target absolute position.
    let target_abs = if let Ok((t_cell, t_tf)) = q_spatial.get(cmd.target) {
        let cell = t_cell.copied().unwrap_or_default();
        lunco_core::coords::world_position_seeded(
            cmd.target, &cell, t_tf, &q_parents, &q_grids, &q_spatial_abs,
        )
    } else {
        lunco_core::coords::world_position_seeded(
            cmd.target, &CellCoord::default(), &Transform::default(), &q_parents, &q_grids, &q_spatial_abs,
        )
    };

    // Compute distance based on target type.
    let mut distance = 20.0;
    let physical_target = get_physical_body(cmd.target, &q_children, &q_bodies);
    if let Ok(body) = q_bodies.get(physical_target) {
        distance = body.radius_m * 3.0;
    } else if let Ok(sc) = q_sc.get(cmd.target) {
        distance = (sc.hit_radius_m as f64 * 5.0).max(100.0);
    }

    let target_grid = get_grid_for_entity(cmd.target, &q_parents, &q_grids);

    // Snap to target immediately.
    let (current_yaw, current_pitch, _) = cam_tf.rotation.to_euler(EulerRot::YXZ);
    let final_rot = Quat::from_euler(EulerRot::YXZ, current_yaw, current_pitch, 0.0);
    let final_offset = final_rot.mul_vec3(Vec3::Z).as_dvec3() * distance;
    let final_abs_pos = target_abs + final_offset;

    // Migrate to target grid immediately
    migrate_avatar_to_target_grid(
        &mut commands, avatar_ent, target_grid, final_abs_pos, final_rot,
        &q_grids, &q_parents, &q_spatial_abs,
    );

    commands.entity(avatar_ent)
        .remove::<SpringArmCamera>()
        .remove::<OrbitCamera>()
        .remove::<FreeFlightCamera>()
        .remove::<FrameBlend>()
        .insert(OrbitCamera {
            target: cmd.target,
            distance,
            yaw: current_yaw,
            pitch: current_pitch,
            damping: None,
            vertical_offset: 0.0,
        });
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
            // Adaptive near AND far, both derived from the bodies in frame.
            // `near` tracks the nearest body surface (no near-clipping on
            // approach); `far` tracks the FARTHEST body surface (+5% margin)
            // instead of a static 1e15, so the depth dynamic range collapses to
            // what the scene actually spans when no distant body is visible —
            // e.g. ~Earth distance (4e8 m) on the lunar surface rather than 1e15
            // (≈4 orders of magnitude of reverse-Z range recovered). The 1e7 m
            // (10 000 km) floor keeps a sane frustum when no body is registered
            // (e.g. the offscreen USD preview camera).
            let mut min_dist = 1.0e15_f64;
            let mut max_far = 0.0_f64;
            for (body, b_tf, b_cell, b_child_of) in q_bodies.iter() {
                if let Ok(b_grid) = q_grids.get(b_child_of.0) {
                    let center_d = cam_pos.distance(b_grid.grid_position_double(b_cell, b_tf));
                    let near_edge = center_d - body.radius_m;
                    let far_edge = center_d + body.radius_m;
                    if near_edge < min_dist { min_dist = near_edge; }
                    if far_edge > max_far { max_far = far_edge; }
                }
            }
            if max_far <= 0.0 {
                // No `CelestialBody` contributed (flat sandbox scene, or the
                // offscreen USD preview camera). The body-derived `min_dist` is
                // still its 1e15 sentinel here — feeding it to the clamp below
                // pins `near` to the 100 m ceiling, which clips away the ENTIRE
                // nearby scene (rovers, ground) and renders black. Use a small
                // near + the 10 000 km far floor so a body-less scene renders.
                perspective.near = 0.1;
                perspective.far = 1.0e7;
            } else {
                perspective.near = (min_dist as f32 * 0.01).clamp(0.1, 100.0);
                perspective.far = ((max_far * 1.05).max(1.0e7)) as f32;
            }
        }
    }
}

// ─── Surface Teleport Commands ───────────────────────────────────────────────

/// Teleports the avatar to a body's surface.
///
/// The camera is parented to the Body's Grid (inertial anchor), NOT the Body
/// itself. `SurfaceCamera` rebuilds world-space rotation every frame from
/// `LocalGravityField.local_up`, so the camera stays surface-relative without
/// inheriting the Body's rotation. `FloatingOrigin` must be on a Grid.
fn on_surface_teleport_command(
    trigger: On<TeleportToSurface>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf), With<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    _q_spatial_abs: Query<(Option<&CellCoord>, &Transform)>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_gravity_providers: Query<&GravityProvider>,
    mut field: ResMut<LocalGravityField>,
) {
    let cmd = trigger.event();
    let avatar_ent = cmd.target;

    // Resolve body entity from bits
    let body_entity = Entity::from_bits(cmd.body_entity);

    let (body_entity, body_radius) = if let Ok((e, b)) = q_bodies.get(body_entity) {
        debug!("TELEPORT: found body {:?} radius={:.0}m", e, b.radius_m);
        (e, b.radius_m)
    } else {
        warn!("TELEPORT: body entity {:?} not found in q_bodies", body_entity);
        return;
    };

    if body_entity == Entity::PLACEHOLDER {
        warn!("TELEPORT: no body found");
        return;
    }

    debug!("TELEPORT: triggered for avatar {:?}", avatar_ent);

    // Get camera cell for position lookup
    let Some((_, cam_tf, _cam_cell, _cam_child_of)) = q_avatar.iter().next() else { return };

    // Find the Body's Grid (the inertial anchor that the Body is a child of).
    let body_grid = q_parents.get(body_entity)
        .ok()
        .map(|c| c.0)
        .filter(|e| q_grids.contains(*e));

    let Some(grid_entity) = body_grid else {
        warn!("TELEPORT: body {:?} has no Grid parent", body_entity);
        return;
    };
    debug!("TELEPORT: parenting camera to grid {:?}", grid_entity);

    // Compute surface position: use camera look direction projected onto body.
    let (surface_local_pos, surface_normal) = {
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

        // Parent camera to the Body's Grid (inertial), NOT the Body.
        // FloatingOrigin must be on a Grid.
        let local_tf =
            Transform::from_translation(new_tf_translation).with_rotation(surface_rot);
        migrate_to_grid(&mut commands, avatar_ent, grid_entity, new_cell, local_tf);

        commands.entity(avatar_ent)
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

        // Update LocalGravityField (world-space "up")
        field.body_entity = Some(body_entity);
        field.local_up = surface_normal;
        field.surface_g = surface_g;
        field.up = surface_normal;

        debug!("TELEPORT: done — camera now on grid {:?} at alt ~50m", grid_entity);
    } else {
        warn!("TELEPORT: grid entity {:?} not found", grid_entity);
    }
}

/// Leaves the surface and returns to orbit view.
///
/// Teleports camera to 3x body radius altitude and switches to OrbitCamera.
/// Re-parents the camera back to the EMB Grid (star-fixed frame).
fn on_leave_surface_command(
    trigger: On<LeaveSurface>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, Option<&GravityBody>), With<Avatar>>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_grids: Query<&Grid>,
    q_emb: Query<Entity, With<lunco_celestial::EMBRoot>>,
    mut field: ResMut<LocalGravityField>,
) {
    let cmd = trigger.event();
    let avatar_ent = cmd.target;

    let Some((_, cam_tf, gravity_body)) = q_avatar.iter().next() else { return };

    // Find the body we're leaving
    let body_entity = gravity_body.map(|gb| gb.body_entity)
        .unwrap_or(Entity::PLACEHOLDER);

    let body_radius = q_bodies.get(body_entity)
        .map(|(_, b)| b.radius_m)
        .unwrap_or(EARTH_RADIUS_M_FALLBACK);

    // Find EMB Grid (the star-fixed orbit frame)
    let Some(emb_grid) = q_emb.iter().next() else { return; };
    let Ok(emb_grid_ref) = q_grids.get(emb_grid) else { return; };

    // Teleport to 3x body radius altitude, relative to EMB Grid.
    let altitude = body_radius * 3.0;
    let orbit_pos_local = DVec3::new(0.0, altitude, altitude * 0.5);
    let (new_cell, new_tf) = emb_grid_ref.translation_to_grid(orbit_pos_local);

    let local_tf = Transform::from_translation(new_tf).with_rotation(cam_tf.rotation);
    migrate_to_grid(&mut commands, avatar_ent, emb_grid, new_cell, local_tf);

    commands.entity(avatar_ent)
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
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_bodies: Query<&CelestialBody>,
    thresholds: Res<SurfaceModeThreshold>,
    field: Res<LocalGravityField>,
    mut commands: Commands,
) {
    let Some((avatar_ent, tf, cell, _, maybe_gb, maybe_mode, maybe_ff, maybe_sc)) = q_avatar.iter().next() else { return };

    // Use absolute coordinates to handle nested grids correctly
    let cam_abs = lunco_core::coords::world_position_seeded(
        avatar_ent, cell, tf, &q_parents, &q_grids, &q_spatial,
    );

    // Compute altitude above the bound body
    let (_full_body_local, altitude) = if let Some(gb) = maybe_gb {
        if let Ok((b_cell, b_tf)) = q_spatial.get(gb.body_entity) {
            let cell = b_cell.copied().unwrap_or_default();
            let body_abs = lunco_core::coords::world_position_seeded(
                gb.body_entity, &cell, b_tf, &q_parents, &q_grids, &q_spatial,
            );
            let rel_pos = cam_abs - body_abs;
            let alt = if let Ok(body) = q_bodies.get(gb.body_entity) {
                rel_pos.length() - body.radius_m
            } else { f64::MAX };
            (rel_pos, alt)
        } else { (cam_abs, f64::MAX) }
    } else if let Some(body_ent) = field.body_entity {
        if let Ok((b_cell, b_tf)) = q_spatial.get(body_ent) {
            let cell = b_cell.copied().unwrap_or_default();
            let body_abs = lunco_core::coords::world_position_seeded(
                body_ent, &cell, b_tf, &q_parents, &q_grids, &q_spatial,
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

/// Global visual settings for floating rover name tags.
///
/// The tags are drawn as an egui overlay (see [`crate::ui::draw_rover_name_tags`])
/// rather than as `Text2d` world entities: this app renders the scene through a
/// single `Camera3d` and owns the only 2D camera for egui, so world-anchored
/// `Text2d` never projects into the 3D viewport. The overlay instead projects
/// each possessed rover's world position through the avatar camera every frame.
#[derive(Resource, Reflect, Clone, Debug)]
#[reflect(Resource)]
pub struct RoverNameTagSettings {
    /// Nominal font size, rendered at exactly [`reference_distance`](Self::reference_distance)
    /// from the camera. Closer rovers scale the tag up, farther ones scale it down.
    pub font_size: f32,
    /// Color of the floating name tag text.
    pub text_color: Color,
    /// Vertical offset of the tag above the rover's origin, in world units.
    pub vertical_offset: f32,
    /// Camera distance (world units) at which the tag renders at [`font_size`](Self::font_size).
    /// The on-screen size scales as `reference_distance / distance`.
    pub reference_distance: f32,
    /// Camera distance (world units) past which the tag is fully faded out and culled.
    /// Tags begin fading from [`reference_distance`](Self::reference_distance) toward this.
    pub max_distance: f32,
}

impl Default for RoverNameTagSettings {
    fn default() -> Self {
        Self {
            font_size: 26.0,
            text_color: Color::WHITE,
            vertical_offset: 2.0,
            reference_distance: 15.0,
            max_distance: 150.0,
        }
    }
}

fn on_update_profile(
    trigger: On<UpdateProfile>,
    guard: Res<lunco_core::SyncApplyGuard>,
    local: Res<LocalSession>,
    mut profiles: ResMut<SessionProfiles>,
) {
    let origin = guard.0.unwrap_or(local.0);
    profiles.profiles.insert(origin.0, trigger.event().name.clone());
    info!("[net] session {} set name to '{}'", origin.0, trigger.event().name);
}


fn sync_profile(
    role: Res<NetworkRole>,
    local: Res<LocalSession>,
    settings: Res<ProfileSettings>,
    mut last_sent: Local<Option<u64>>,
    mut last_name: Local<Option<String>>,
    mut commands: Commands,
) {
    let session = local.0.0;
    if *role == NetworkRole::Client && session == 0 {
        *last_sent = None;
        return;
    }
    let current_name = settings.username.clone();
    let should_send = last_sent.is_none_or(|s| s != session) 
        || last_name.as_ref().is_none_or(|n| *n != current_name);
    if should_send {
        commands.trigger(UpdateProfile { name: current_name.clone() });
        *last_sent = Some(session);
        *last_name = Some(current_name);
    }
}


// ── Command Registration ────────────────────────────────────────────────────────

// Wires CaptureScreenshot + recording commands into `register_all_commands(app)`,
// called from LunCoAvatarPlugin::build().
register_commands!(
    on_capture_screenshot,
    on_toggle_recording,
    on_start_recording,
    on_stop_recording
);
