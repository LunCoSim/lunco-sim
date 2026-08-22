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

use bevy::ecs::{lifecycle::HookContext, world::DeferredWorld};
use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, FloatingOrigin, Grid};
use leafwing_input_manager::prelude::*;
use serde::{Deserialize, Serialize};

use lunco_controller::ControllerLink;
use lunco_core::{
    on_command, register_commands, Avatar, CelestialBody, LocalAvatar, LocalSession, NetworkRole,
    SessionProfiles, Spacecraft,
};
/// Capability test for "**accepts commands**": carries an authored intent→port
/// binding (`ControlBinding`, from its USD `Controls` scope) or a Modelica actuation
/// backend (`SimComponent`).
///
/// This is NOT a possess gate — there is none. An avatar may possess anything; WHO
/// may hold a given target is arbitrated by the authority layer
/// (`SessionRegistry::may_possess` / `PossessionPolicy`, checked in
/// `on_possess_command`), and what a possessed thing can DO is decided by whether it
/// accepts commands at all. This alias answers only that second question, and is used
/// for one presentation decision: whether a heading-follow camera should track the
/// target's yaw (a thing that steers has a meaningful heading; a prop tumbles).
type Controllable = bevy::prelude::Or<(
    bevy::prelude::With<lunco_core::ControlBinding>,
    bevy::prelude::With<lunco_cosim::SimComponent>,
)>;
use lunco_celestial::{geo::LocalTangentFrame, LeaveSurface, LocalGravityField, TeleportToSurface};
use lunco_core::attach::migrate_to_grid;
use lunco_environment::{GravityBody, GravityProvider};
use lunco_settings::{AppSettingsExt, ProfileSettings, SettingsSection};
use lunco_time::{SetTimeTransport, TimeTransport, TransportMode, WorldTime};

pub mod commands;
pub use commands::*;
// `screenshot.rs` was DELETED here (2026-07-13, render decoupling): it named
// `bevy::render::view::screenshot::Screenshot`, a genuine render-world readback with
// no render-free form, which made this crate link `bevy_render`.
//
// Two things lived in it, and they went different ways:
//
// - `CaptureScreenshot` was a DEAD duplicate — the executor matched the command by NAME
//   and returned early, so this crate's `#[Command]` + observer was unreachable, and it
//   declared `CaptureScreenshot {}` (no fields) while the real one takes
//   `save_to_file`/`path`/`region`, so the reflected schema behind the MCP tool list was
//   lying. It is simply gone; the one live implementation is `lunco-workbench::screenshot`.
//
// - `CaptureFromCamera` — the typed command behind the `science::take_photo` instrument —
//   is LIVE, but it needs `Camera3d` and a render-world `Screenshot`, neither of which
//   exists in a render-free crate. It moved BODILY to `lunco-workbench::screenshot`,
//   taking its observer AND its `register_closure_tool` registration with it (the closure
//   only needs the command type, so it belongs next to it). `WorkbenchPlugin` installs
//   both, which means every binary that can render can photograph, and a headless one
//   neither registers the command nor advertises the tool — instead of registering a
//   `take_photo` that silently captures nothing.
// See docs/architecture/render-decoupling.md ("What has no intent form").
//
// `recording.rs` was DELETED here for the same reason, one step further along.
// It wrapped Bevy's `EasyScreenRecordPlugin` (libx264 via `bevy_dev_tools`)
// behind an optional `recording` cargo feature. The feature was never enabled in
// any build we ship or record with, so what shipped was a control surface —
// settings section, `ToggleRecording`/`StartRecording`/`StopRecording`, a
// Ctrl+Shift+R hotkey — that captured nothing and logged a warning saying so.
//
// The live capture path is `lunco-workbench::screenshot`'s OFFLINE recorder: it
// owns the clock, writes one PNG per captured frame at a fixed rate, and is
// driven from scenario scripts by the `shot_*` prelude verbs. That is a
// deterministic frame sequence, which is what film work needs; a realtime
// wall-clock encoder is not a substitute for it.
mod intents;

/// Upper bound on parent-chain walks when resolving an entity's owning Grid
/// or nearest clickable root. The scene hierarchies here are shallow (a few
/// levels); this cap purely guards the loop against running away on a
/// malformed/cyclic hierarchy — it does not encode a real structural depth.
/// (Unifies the former ad-hoc `0..10` / `MAX_DEPTH = 8` bounds.)
const MAX_HIERARCHY_WALK_DEPTH: usize = 16;

/// UI panels for avatar status, camera mode, and surface coordinates.
#[cfg(feature = "ui")]
pub mod ui;
pub use intents::*;

// ─── Resources ───────────────────────────────────────────────────────────────

/// Persisted camera-input response.
///
/// Pointer deltas and Bevy camera angles are f32 presentation values. Distances
/// that choose an orbit response remain f64, matching the BigSpace/celestial
/// coordinate boundary; only the resulting dimensionless scale is cast at the
/// final camera-angle write.
#[derive(Resource, Reflect, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[reflect(Resource)]
#[serde(default)]
pub struct CameraInputSettings {
    /// Camera radians per pointer-motion unit before behavior-specific scaling.
    pub look_radians_per_pointer_unit: f32,
    /// Lower bound for orbital rotation at the body's surface.
    pub orbit_surface_min_scale: f64,
    /// Shapes the geometric visible-horizon response. `1` is physically linear;
    /// larger values retain slower rotation farther from the surface.
    pub orbit_distance_curve_exponent: f64,
}

impl Default for CameraInputSettings {
    fn default() -> Self {
        Self {
            look_radians_per_pointer_unit: 0.001125,
            orbit_surface_min_scale: 0.04,
            orbit_distance_curve_exponent: 0.75,
        }
    }
}

impl SettingsSection for CameraInputSettings {
    const KEY: &'static str = "camera_input";
}

#[on_command(SetCameraInput)]
fn on_set_camera_input(trigger: On<SetCameraInput>, mut settings: ResMut<CameraInputSettings>) {
    apply_camera_input(trigger.event(), &mut settings);
}

fn apply_camera_input(command: &SetCameraInput, settings: &mut CameraInputSettings) {
    if let Some(value) = command.look_radians_per_pointer_unit {
        if value.is_finite() && value >= 0.0 {
            settings.look_radians_per_pointer_unit = value;
        } else {
            warn!("SetCameraInput rejected non-finite/negative look sensitivity: {value}");
        }
    }
    if let Some(value) = command.orbit_surface_min_scale {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            settings.orbit_surface_min_scale = value;
        } else {
            warn!("SetCameraInput rejected surface scale outside [0, 1]: {value}");
        }
    }
    if let Some(value) = command.orbit_distance_curve_exponent {
        if value.is_finite() && value > 0.0 {
            settings.orbit_distance_curve_exponent = value;
        } else {
            warn!("SetCameraInput rejected non-positive distance exponent: {value}");
        }
    }
}

/// Scale an orbit gesture from the target body's apparent geometry.
///
/// `sqrt(1 - (r/d)^2)` is the cosine of the body's apparent angular radius:
/// zero at the surface and asymptotically one far away. This gives a continuous,
/// body-size-independent response without altitude bands or scene heuristics.
fn body_orbit_look_scale(distance_m: f64, radius_m: f64, settings: &CameraInputSettings) -> f64 {
    let min_scale = settings.orbit_surface_min_scale.clamp(0.0, 1.0);
    let exponent = settings.orbit_distance_curve_exponent.max(f64::EPSILON);
    if !distance_m.is_finite() || !radius_m.is_finite() || radius_m <= 0.0 {
        return 1.0;
    }
    let ratio = (radius_m / distance_m.max(radius_m)).clamp(0.0, 1.0);
    let visible_horizon = (1.0 - ratio * ratio).max(0.0).sqrt();
    min_scale + (1.0 - min_scale) * visible_horizon.powf(exponent)
}

#[cfg(test)]
mod camera_input_settings_tests {
    use super::*;

    #[test]
    fn orbit_look_scale_is_continuous_monotonic_and_body_size_independent() {
        let settings = CameraInputSettings::default();
        let moon = 1_737_400.0;
        let surface = body_orbit_look_scale(moon, moon, &settings);
        let low = body_orbit_look_scale(moon + 100.0, moon, &settings);
        let high = body_orbit_look_scale(moon + 100_000.0, moon, &settings);
        let far = body_orbit_look_scale(moon * 100.0, moon, &settings);

        assert_eq!(surface, settings.orbit_surface_min_scale);
        assert!(
            surface < low && low < high && high < far,
            "{surface} {low} {high} {far}"
        );
        assert!(far < 1.0);

        let same_ratio_on_earth = body_orbit_look_scale(6_378_137.0 * 2.0, 6_378_137.0, &settings);
        let same_ratio_on_moon = body_orbit_look_scale(moon * 2.0, moon, &settings);
        assert!((same_ratio_on_earth - same_ratio_on_moon).abs() < 1.0e-12);
    }

    #[test]
    fn orbit_look_scale_honours_the_configured_surface_floor() {
        let settings = CameraInputSettings {
            orbit_surface_min_scale: 0.125,
            ..default()
        };
        assert_eq!(body_orbit_look_scale(10.0, 10.0, &settings), 0.125);
    }

    #[test]
    fn runtime_camera_input_update_is_partial_and_rejects_invalid_values() {
        let mut settings = CameraInputSettings::default();
        let original_floor = settings.orbit_surface_min_scale;
        apply_camera_input(
            &SetCameraInput {
                look_radians_per_pointer_unit: Some(0.0005),
                orbit_surface_min_scale: None,
                orbit_distance_curve_exponent: Some(1.25),
            },
            &mut settings,
        );
        assert_eq!(settings.look_radians_per_pointer_unit, 0.0005);
        assert_eq!(settings.orbit_surface_min_scale, original_floor);
        assert_eq!(settings.orbit_distance_curve_exponent, 1.25);

        apply_camera_input(
            &SetCameraInput {
                look_radians_per_pointer_unit: Some(-1.0),
                orbit_surface_min_scale: Some(2.0),
                orbit_distance_curve_exponent: Some(0.0),
            },
            &mut settings,
        );
        assert_eq!(settings.look_radians_per_pointer_unit, 0.0005);
        assert_eq!(settings.orbit_surface_min_scale, original_floor);
        assert_eq!(settings.orbit_distance_curve_exponent, 1.25);
    }
}

/// Tracks cumulative mouse scroll delta for zoom control.
///
/// Per-avatar mouse-wheel zoom accumulator. Fed each frame by
/// [`collect_camera_zoom`] from Bevy's unit-preserving scroll input (gated on
/// `EguiFocus.wants_pointer` so scrolling over a panel doesn't zoom the scene);
/// consumed + reset by whichever camera behavior is active. Lives on the avatar
/// entity — zoom is per-camera state, not a global — replacing the old global
/// `CameraScroll` resource and its two bespoke egui→resource bridges.
#[derive(Component, Default)]
pub struct CameraZoomInput {
    /// Accumulated scroll delta since the last camera system consumed it.
    pub delta: f32,
}

/// Scroll→zoom sensitivity (unitless; feeds the exponential in
/// [`apply_scroll_zoom`]).
///
/// Input is normalized to line units before it reaches this constant. Keeping
/// conversion at the source boundary makes a pixel-mode touchpad and a
/// line-mode wheel produce the same camera response.
const ZOOM_SENSITIVITY: f32 = 5.0;
const ZOOM_FACTOR_MIN: f64 = 0.75;
const ZOOM_FACTOR_MAX: f64 = 1.25;

/// Altitude of the orbital zoom's min-distance floor above a celestial body's
/// surface. Doubles as the scroll-through threshold: one more inward detent
/// while the arm sits on this floor exits the orbital view to the surface
/// camera at the current pose (task: seamless orbit⇄terrain, no clicks).
const SCROLL_EXIT_ALTITUDE_M: f64 = 50_000.0;

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
/// How a [`SpringArmCamera`] derives its orientation from the followed body.
///
/// The one axis on which the three authored `lunco:cameraFollow` modes differ.
/// Everything else about the follow — live-target read, fixed-cadence solve,
/// interpolation-eased render, arm-length easing, obstacle raycast — is shared,
/// so all vessel cameras behave identically (the reason this is DRY: one
/// component, one system, one code path). `WorldLocked` / `FullAttitude` are
/// the 6-DOF flyer + aircraft cases that used to live in the separate
/// (jittering) `OrbitCamera`/`ChaseCamera` solvers.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FollowAttitude {
    /// Heading-follow: yaw taken from the body's forward (when `track_heading`),
    /// up = world-Y or surface normal. Ground vehicles (rovers, astronauts).
    #[default]
    Heading,
    /// Stable external frame the body tumbles inside of: ignore the body's
    /// attitude entirely, orientation is the user's yaw/pitch about world-up.
    /// 6-DOF flyers (a lander that pitches/rolls freely). Authored `"orbit"`.
    WorldLocked,
    /// Cockpit frame: full body orientation (yaw+pitch+roll) times the user's
    /// yaw/pitch offset — the camera rolls with the craft. Authored `"chase"`.
    FullAttitude,
}

/// Unified vessel-follow camera. Position always follows the target; the
/// [`FollowAttitude`] mode selects how orientation is derived. Solved on the
/// render cadence (`spring_arm_system`, after `lunco_time::InteractionRenderSet`)
/// against the target's final render pose, so the camera and the followed body share
/// ONE motion basis.  The camera is deliberately not `InteractionEased`: easing a
/// chase camera independently from the body adds a second phase and makes both the
/// rover and world-anchored overlays oscillate on screen.
///
/// Position snaps directly to the desired offset (no lerp), but rotation
/// slerps smoothly toward the desired attitude + user yaw offset. This creates
/// the natural "swing-around" feel of a proper spring arm camera.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
#[require(CameraZoomInput)]
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
    /// target. Only consulted for [`FollowAttitude::Heading`].
    pub track_heading: bool,
    /// How camera orientation is derived from the followed body.
    pub attitude: FollowAttitude,
}

/// Survey camera: orbits a target fixed to the stars.
///
/// **Reference Frame**: `Ecliptic` — the camera does NOT rotate with the target.
/// This keeps stars stationary while the planet rotates beneath you.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
#[require(CameraZoomInput)]
pub struct OrbitCamera {
    pub target: Entity,
    pub distance: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub damping: Option<f32>,
    pub vertical_offset: f32,
}

/// Exact pre-orbit camera state, owned by the avatar for the duration of one
/// orbital-view session.
///
/// A view transition is a reference-frame transaction: entering orbit stores
/// the original grid parent, cell-local pose, camera behavior and gravity
/// binding; switching Moon/Earth changes only the active orbit target; leaving
/// orbit restores this snapshot atomically. No exit path infers the old frame
/// from components that orbit entry has already removed.
#[derive(Component, Clone, Debug)]
pub struct OrbitViewReturn {
    parent_grid: Entity,
    cell: CellCoord,
    transform: Transform,
    behavior: OrbitReturnBehavior,
    gravity_body: Option<GravityBody>,
    surface_relative: bool,
}

#[derive(Clone, Debug)]
enum OrbitReturnBehavior {
    SpringArm(SpringArmCamera),
    Surface(SurfaceCamera),
    FreeFlight(FreeFlightCamera),
}

/// Marks an `OrbitCamera` that should re-aim onto the SUNLIT side of its
/// (celestial) target before the first orbit step. The orbit writer resolves
/// the Sun and target from their authoritative BigSpace cell chains.
#[derive(Component, Debug, Clone, Copy)]
pub struct SunlitArrival;

/// Marks an `OrbitCamera` whose arm should be derived from the camera's
/// CURRENT position — the pose-preserving arrival of the surface→orbital
/// scroll-out transit (`scroll_out_to_orbit_system`), vs [`SunlitArrival`]
/// which re-aims at the sunlit side. The orbit writer resolves the body and
/// camera through the authoritative BigSpace cell chain into the selected
/// inertial view grid;
/// the body→camera direction becomes the arm's yaw/pitch and the true range
/// becomes the arm length, so the camera does not move on mode entry.
#[derive(Component, Debug, Clone, Copy)]
pub struct RadialArrival;

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

// Camera behavior components are an exclusive sum type at the ECS boundary:
// an avatar may have one active behavior, never two. The mode systems also
// express this with `Without<…>` filters, so enforcing it when a component is
// added keeps every current and future transition on the same single-writer
// contract. The hook queues structural removals; they are applied with the
// normal Bevy command flush before camera systems run.
fn freeflight_camera_added(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    world.commands().queue(move |world: &mut World| {
        if world.get::<FreeFlightCamera>(entity).is_some() {
            world
                .entity_mut(entity)
                .remove::<(SurfaceCamera, SpringArmCamera, OrbitCamera)>();
        }
    });
}

fn surface_camera_added(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    world.commands().queue(move |world: &mut World| {
        if world.get::<SurfaceCamera>(entity).is_some() {
            world.entity_mut(entity).remove::<(
                FreeFlightCamera,
                SpringArmCamera,
                OrbitCamera,
                lunco_time::InteractionEased,
            )>();
        }
    });
}

fn spring_arm_camera_added(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    world.commands().queue(move |world: &mut World| {
        if world.get::<SpringArmCamera>(entity).is_some() {
            world
                .entity_mut(entity)
                .remove::<(FreeFlightCamera, SurfaceCamera, OrbitCamera)>();
        }
    });
}

fn orbit_camera_added(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    world.commands().queue(move |world: &mut World| {
        if world.get::<OrbitCamera>(entity).is_some() {
            world
                .entity_mut(entity)
                .remove::<(FreeFlightCamera, SurfaceCamera, SpringArmCamera)>();
        }
    });
}

fn register_camera_mode_hooks(app: &mut App) {
    app.world_mut()
        .register_component_hooks::<FreeFlightCamera>()
        .on_insert(freeflight_camera_added);
    app.world_mut()
        .register_component_hooks::<SurfaceCamera>()
        .on_insert(surface_camera_added);
    app.world_mut()
        .register_component_hooks::<SpringArmCamera>()
        .on_insert(spring_arm_camera_added);
    app.world_mut()
        .register_component_hooks::<OrbitCamera>()
        .on_insert(orbit_camera_added);
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
/// `SetPorts` control commands (gap G4). Runs for both local-host and wire-applied
/// possessions; the origin is the wire-apply guard (remote) or the local
/// session (host's own).
fn record_possession_authority(
    trigger: On<PossessVessel>,
    role: Res<lunco_core::NetworkRole>,
    guard: Res<lunco_core::SyncApplyGuard>,
    local: Res<lunco_core::LocalSession>,
    rbac: Res<lunco_core::session::SessionRbac>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_input_ports: Query<&lunco_core::InputPorts>,
    mut registry: ResMut<lunco_core::SessionRegistry>,
) {
    // Record ownership on the authoritative peer: Host, and also single-player
    // Standalone (whose authority is local) so the control-authority yield/takeover
    // works offline. Only a Client defers to the host's table.
    if matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }
    let cmd = trigger.event();
    if !q_input_ports
        .get(cmd.target)
        .is_ok_and(|surface| !surface.values.is_empty())
    {
        warn!(target = ?cmd.target, "[auth] possession refused: target exposes no writable input ports");
        return;
    }
    let origin = guard.0.unwrap_or(local.0);
    if let Ok(gid) = q_gid.get(cmd.target) {
        // Control-authority takeover (spec 034): if the vessel is currently owned by
        // a DIFFERENT session, ask the rhai policy
        // ([`lunco_core::session::CONTROL_AUTHORITY_HOOK`]) whether this possessor may
        // take it. The rule (e.g. "a human may take from an autopilot; an autopilot
        // may not take from a human") is authored in rhai, not here. If allowed,
        // release the prior owner FIRST so the claim below succeeds under the default
        // Exclusive policy; the released autopilot then loses `owns` and stops
        // driving on its own. Fails closed (no policy ⇒ no takeover). One vessel per
        // autopilot session, so releasing that session frees exactly this vessel.
        // `may_control` is the shared predicate (see its doc): the bind leg
        // (`on_possess_command`) asks the SAME question, so the two legs cannot
        // disagree about whether this possession is allowed. Here it decides whether to
        // evict the current owner; there it decides whether to attach the camera.
        if let Some(cur) = registry.owner_of(gid.get()) {
            if cur != origin
                && lunco_core::session::may_control(&registry, &rbac, origin, gid.get())
            {
                registry.release_session(cur);
                info!(
                    "[auth] session {origin} took control of entity {} from {cur} (policy allowed)",
                    gid.get()
                );
            }
        }
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
    // Authoritative peer (Host or single-player Standalone); a Client defers to the
    // host. Mirrors `record_possession_authority`.
    if matches!(*role, lunco_core::NetworkRole::Client) {
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
        register_camera_mode_hooks(app);
        app.init_resource::<CameraDefaults>()
            .init_resource::<SurfaceModeThreshold>();
        // Stepped camera writers use `lunco_time::InteractionSchedule`, while the
        // spring arm follows the final rendered body pose in `PostUpdate` below. The
        // time spine is a hard dependency for both paths; guarantee it rather than
        // silently registering into a schedule no runner ever executes.
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
        }
        // `science::take_photo` is registered by `lunco-workbench`'s `ScreenshotPlugin`,
        // not here: the tool's closure triggers `CaptureFromCamera`, whose observer is a
        // render-world readback this crate deliberately cannot link.
        app.add_plugins(InputManagerPlugin::<UserIntent>::default());
        // Secondary observers on the SAME verbs — the authority-bookkeeping leg,
        // not the command handlers (those go through `register_commands!`).
        app.add_observer(record_possession_authority);
        app.add_observer(release_possession_authority);
        // Scene-click possession/follow/focus is now bevy_picking-driven: a
        // global `Pointer<Click>` observer (egui occlusion handled by the
        // framework), replacing the old `ScenePointer`-gated Update system.
        //
        // The observer reads two click-arbitration resources — `DragModeActive`
        // (gizmo drag in progress) and `SpawnToolActive` (click-to-place armed).
        // Both are normally owned by the editor (`lunco-luncosim-edit`), but the
        // observer lives here and fires on the FIRST pointer event, so a binary
        // that uses the avatar without the editor (luncosim) would panic on the
        // missing `Res`. Guarantee them here — `init_resource` is idempotent, so
        // a host that inserts its own (luncosim) keeps that value.
        app.init_resource::<lunco_core::DragModeActive>();
        app.init_resource::<lunco_core::SpawnToolActive>();
        app.init_resource::<lunco_core::TerrainToolActive>();
        app.init_resource::<lunco_core::WaypointToolActive>();
        app.init_resource::<lunco_core::ArmedScriptTool>();
        app.init_resource::<lunco_core::WaypointMenuOpen>();
        // Populated by `lunco-workbench` when egui is present; guaranteed here so
        // the keyboard gate (`scene_keyboard_active`) has a resource to read on
        // binaries that use the avatar without the workbench (headless server) —
        // there it stays default `false` and the gate is always open.
        app.init_resource::<lunco_core::EguiFocus>();
        app.add_observer(avatar_raycast_possession);
        // The local avatar is a controllable like any vessel: stamp its FSW command
        // surface + control binding so the shared `drive_from_bindings` path moves it.
        app.add_observer(stamp_avatar_controls);
        app.add_observer(demote_former_avatar);
        // Mirror native command events onto the shared script/telemetry bus. The
        // tutorial policy consumes these generic names; it is not embedded in
        // the avatar UI.
        app.add_observer(project_possess_event);
        app.add_observer(project_release_event);

        // Register all commands (generated by register_commands! macro at module scope)
        register_all_commands(app);

        // Possession / follow commands cross the wire (a client takes control of
        // the host's authoritative rover, then drives it), and the wire apply path
        // looks them up by reflected short type-path — so the type MUST be in the
        // registry. They used to be wired observer-by-hand + type-by-hand, and when
        // the second half was forgotten the host logged "unknown command type
        // 'PossessVessel'", never recorded the client's ownership, and rejected
        // every subsequent SetPorts as unauthorized (the "client rover won't move"
        // bug). `register_commands!` now does both halves in one step, so the two
        // can't drift apart again.
        app.register_type::<SpringArmCamera>()
            .register_type::<FollowAttitude>()
            .register_type::<OrbitCamera>()
            .register_type::<FreeFlightCamera>()
            .register_type::<FrameBlend>()
            .register_type::<AdaptiveNearPlane>()
            .register_type::<SurfaceRelativeMode>()
            .register_type::<SurfaceCamera>()
            .register_type::<SurfaceModeThreshold>()
            .register_type::<CameraInputSettings>();

        app.register_settings_section::<CameraInputSettings>();
        app.register_settings_section::<ProfileSettings>();
        app.init_resource::<RoverNameTagSettings>()
            .register_type::<RoverNameTagSettings>();

        // On-screen notifications (rhai `notify(...)` → `ShowNotification`). The
        // command itself is registered as a REAL command via `register_commands!`
        // below (API-discoverable); here we only need its toast queue.
        app.init_resource::<ScreenNotifications>();
        // Notifications are a per-client toast — client-local, so a client-scoped
        // presentation scenario may issue them (see `ClientCommandPolicy`).
        lunco_core::MarkClientLocalExt::mark_client_local::<ShowNotification>(app);

        // Native input → script EVENT bus: project key presses onto the shared
        // `TelemetryEvent` bus so scenarios can `wait_for("key:KeyG")` / `on_event`
        // raw input exactly like a zone enter or an `emit()`. Demonstrates the
        // generic `project_events` registrar — every event source lands on ONE bus
        // that rhai both produces (`emit`) and consumes (`on_event`/`wait_for`).
        {
            use bevy::input::keyboard::KeyboardInput;
            use lunco_core::ScriptEventAppExt;
            app.project_events::<KeyboardInput, _>(|e| {
                e.state.is_pressed().then(|| lunco_core::TelemetryEvent {
                    name: format!("key:{:?}", e.key_code),
                    source: 0, // raw input — no emitting entity
                    severity: lunco_core::Severity::Info,
                    data: lunco_core::TelemetryValue::Bool(true),
                    timestamp: 0.0,
                })
            });
        }

        app.add_systems(
            Update,
            (
                avatar_init_system,
                surface_mode_transition_system,
                enforce_ownership,
                sync_profile,
                tick_notifications,
                // Mouse-wheel → per-avatar zoom accumulator, sourced from the `Zoom`
                // intent and gated on egui pointer capture (replaces the old egui
                // `CameraScroll` bridges). Runs before the camera systems consume it.
                collect_camera_zoom,
            ),
        );
        // Mouse-look capture + apply. Pointer intents — gated internally on
        // `EguiFocus.wants_pointer` (look_delta is zeroed while a panel holds the
        // pointer), NOT on keyboard focus, so typing never freezes the camera.
        app.add_systems(
            Update,
            // The second system consumes the analog state written by the first.
            // Keep this explicit: Bevy otherwise treats the tuple as unordered,
            // which makes a right-drag intermittently apply one frame late or not
            // at all when the camera system samples the old zero delta.
            (capture_avatar_intent, avatar_behavior_input_system).chain(),
        );

        // Discrete KEYBOARD intents: `Cancel` (release possession/follow) and the
        // `Pause` hotkey. Gated so a key typed into a focused egui field doesn't
        // fire them. `Cancel`/Backspace is the two-step Esc pattern: while a field
        // is focused egui consumes the key (guard suppresses the intent); once
        // defocused, the next press acts.
        app.add_systems(
            Update,
            (avatar_escape_possession, avatar_global_hotkeys).run_if(scene_keyboard_active),
        );

        // Possessed-rover name tags: an egui screen-space overlay (the scene has
        // only a `Camera3d`, so world-anchored `Text2d` never renders). Registered
        // here — not in `AvatarUiPlugin` — because the luncosim adds only
        // `LunCoAvatarPlugin`; `AvatarUiPlugin` is luncosim-only.
        #[cfg(feature = "ui")]
        app.add_systems(
            bevy_egui::EguiPrimaryContextPass,
            crate::ui::draw_rover_name_tags.before(lunco_workbench::WorkbenchRenderSet),
        );
        #[cfg(feature = "ui")]
        app.add_systems(
            bevy_egui::EguiPrimaryContextPass,
            crate::ui::draw_notifications,
        );

        // Incremental camera modes are stepped at a constant 60 Hz and eased by
        // `InteractionEased`.  Surface mode is derived directly from its gravity
        // frame and is therefore a direct single-writer mode. The chase camera is different: it follows the body's
        // final render pose and therefore runs once at render cadence below.  Keeping
        // it out of this schedule prevents two independent interpolation phases from
        // fighting over the same camera Transform.
        app.add_systems(
            lunco_time::InteractionSchedule,
            (
                // A celestial site handoff may move the avatar between
                // rotating Grid frames while preserving its authoritative
                // Transform.  FreeFlightCamera's yaw/pitch are local to the
                // direct parent frame; reseed them from that pose before the
                // normal free-flight writer runs, otherwise it would replay
                // the old ENU angles and erase the body-fixed rebranch.
                rebase_freeflight_state,
                // Entry half of the scroll transit — must see the free-flight scroll BEFORE
                // orbit_system's zoom consumption; its mode swap lands next frame (commands).
                freeflight_scroll_transit_system,
                freeflight_system,
                surface_camera_system,
                apply_fly,
            )
                .chain()
                .in_set(AvatarCameraSet),
        );
        // This must run before lunco_time restores the previous eased pose.
        // The camera's Transform is cell-local; after a Grid/CellCoord handoff
        // the old interpolation history is a pose in a different frame.
        app.add_systems(
            lunco_time::InteractionSchedule,
            reset_easing_before_spatial_rebase.before(lunco_time::InteractionRestoreSet),
        );

        app.configure_sets(
            lunco_time::InteractionSchedule,
            // Between restore and record: start from the authoritative stepped pose
            // (never from the previous frame's render interpolation — that is what keeps
            // `apply_fly`'s `pos += vel·dt` from integrating its own smoothing), and let
            // the step's final pose be snapshotted for the render-rate ease.
            AvatarCameraSet
                .after(lunco_time::InteractionRestoreSet)
                .after(lunco_controller::InteractionControlSet)
                .before(lunco_time::InteractionRecordSet),
        );
        // Direct presentation cameras derive their complete pose from the final
        // render-time target state. They do not belong in the fixed interaction
        // cadence and must not interpolate cell-local Transforms across BigSpace
        // cell changes. Orbit is especially sensitive: a camera rotating millions
        // of metres from a body crosses 2 km cells continually.
        app.add_systems(
            PostUpdate,
            (orbit_system, spring_arm_system)
                .chain()
                .after(lunco_time::InteractionRenderSet)
                .before(TransformSystems::Propagate),
        );
        // Clip distances are measured from origin-relative GlobalTransforms,
        // so consume them only after BigSpace has propagated the camera and
        // bodies for this frame. The former pre-propagation registration read
        // stale poses and compensated with distance heuristics; switching
        // grids then made those heuristics visibly clip or unclip a globe.
        app.add_systems(
            PostUpdate,
            update_avatar_clip_planes_system.after(TransformSystems::Propagate),
        );
        // Every avatar gets easing only for incremental stepped camera modes.
        // Surface mode derives a complete local pose from gravity and spring-arm
        // mode follows the final rendered body pose; neither may have a second
        // Transform writer.
        app.add_systems(Update, sync_avatar_easing);

        // Camera drag remains owned by the avatar view systems. Celestial
        // placement is a separate PreUpdate frame migration; no camera input
        // path re-poses the inertial hierarchy.
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct AvatarCameraSet;

/// Preserve a free-flight camera's authoritative orientation across a parent
/// Grid handoff.
///
/// `FreeFlightCamera` stores absolute yaw/pitch values, but the camera system
/// writes them as Euler angles in the camera's direct parent frame.  A
/// rebranch therefore changes the meaning of the stored angles even though
/// the incoming `Transform` already contains the correct converted pose.
/// `Changed<ChildOf>` is the structural handoff signal; deriving the angles
/// from the live local rotation makes the next writer frame-idempotent and
/// does not guess a world/body offset.
fn rebase_freeflight_state(
    mut q_avatar: Query<
        (&mut FreeFlightCamera, &Transform),
        (
            With<Avatar>,
            Changed<ChildOf>,
            Without<lunco_core::CinematicCameraLock>,
        ),
    >,
) {
    for (mut freeflight, transform) in q_avatar.iter_mut() {
        let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
        freeflight.yaw = yaw;
        freeflight.pitch = pitch;
    }
}

/// Own the camera interpolation mode from the authoritative camera component.
///
/// Incremental stepped free-flight cameras use [`lunco_time::InteractionEased`].
/// Spring-arm and orbit cameras derive their complete pose directly at render
/// cadence, while the surface camera derives its complete gravity-relative pose;
/// retaining local-Transform easing for any of them would leave a second writer.
/// For orbit it would also be mathematically invalid whenever [`CellCoord`]
/// changes, because local coordinates from two different cells cannot be lerped.
fn sync_avatar_easing(
    mut commands: Commands,
    q: Query<
        (
            Entity,
            Has<SpringArmCamera>,
            Has<OrbitCamera>,
            Has<SurfaceCamera>,
            Has<lunco_time::InteractionEased>,
            Has<lunco_core::CinematicCameraLock>,
        ),
        With<Avatar>,
    >,
) {
    for (entity, spring_arm, orbit, surface_camera, eased, cinematic_lock) in q.iter() {
        // A cinematic driver owns the complete camera pose. It must never
        // acquire render-rate interaction easing: that component is itself a
        // Transform writer and its initial spawn-pose history can overwrite a
        // path sample in the same PostUpdate. Remove a marker that was added
        // before the lock arrived, then leave the path camera alone.
        if cinematic_lock {
            if eased {
                commands
                    .entity(entity)
                    .remove::<lunco_time::InteractionEased>();
            }
            continue;
        }
        if spring_arm || orbit || surface_camera {
            if eased {
                commands
                    .entity(entity)
                    .remove::<lunco_time::InteractionEased>();
            }
        } else if !eased {
            commands
                .entity(entity)
                .try_insert(lunco_time::InteractionEased::default());
        }
    }
}

/// Drop easing history before the interaction schedule restores its previous
/// pose when the avatar changes BigSpace frame.
///
/// `InteractionEased` interpolates two `Transform`s, which are cell-LOCAL: across a
/// rebase the previous pose is expressed in a different cell, so lerping it toward
/// the new one would slide the camera across a whole cell edge for a frame. Clearing
/// the history makes the ease skip until two poses in the SAME cell exist — one
/// unsmoothed frame at the rebase instead of a visible sweep. (Same class of problem
/// as the body-side rebase jitter `lunco_physics` probes; the fix is to not
/// interpolate across the discontinuity at all.)
///
/// Runs before [`lunco_time::InteractionRestoreSet`], so no old-frame pose can
/// be restored after the handoff.
fn reset_easing_before_spatial_rebase(
    mut q: Query<
        &mut lunco_time::InteractionEased,
        (
            With<Avatar>,
            Without<lunco_core::CinematicCameraLock>,
            Or<(Changed<CellCoord>, Changed<ChildOf>)>,
        ),
    >,
) {
    for mut eased in &mut q {
        eased.reset();
    }
}

/// Run-condition: `true` when the 3D scene may consume raw keyboard input —
/// i.e. egui is NOT holding the keyboard (no focused text field / drag-value).
///
/// [`lunco_core::EguiFocus`] is published each frame by `lunco-workbench` from
/// the primary egui context's `wants_keyboard_input()`. On a headless binary
/// nothing writes it, so it stays default (`false`) and the gate is always open.
/// One-frame latency (the flag reflects the previous egui pass) is imperceptible
/// for held input.
fn scene_keyboard_active(focus: Res<lunco_core::EguiFocus>) -> bool {
    !focus.wants_keyboard
}

// ─── Avatar Camera Factory ───────────────────────────────────────────────────

/// Spawns a fully-configured avatar camera entity.
///
/// Call this from setup code instead of manually assembling the avatar entity.
/// Ensures consistency between the main client and the luncosim binary.
///
/// # Arguments
/// * `commands` — Bevy commands for entity spawning.
/// * `grid_entity` — The big_space grid entity to parent the avatar to.
/// * `initial_offset` — Starting position offset in grid-local coordinates.
/// * `profile` — The already-resolved authoritative graphics profile.
///
/// # Returns
/// The spawned entity ID.
pub fn spawn_avatar_camera(
    commands: &mut Commands,
    grid_entity: Entity,
    initial_offset: DVec3,
    profile: lunco_render::RenderQualityProfile,
) -> Entity {
    let (yaw, pitch) = (std::f32::consts::PI * 0.5, -0.3);
    // Initial spawn: anchor `ChildOf` in the bundle so parent + cell +
    // transform land atomically (same contract as `migrate_to_grid`).
    //
    // `Camera` + `SceneCamera` (both render-FREE) instead of `Camera3d`: the render
    // *pipeline* half — `Camera3d`, tonemapping, MSAA, bloom — is attached by
    // `lunco-render-bevy`'s `SceneCamera` binder in render builds, and simply never
    // attached headless, where the camera stays a fully-formed scene entity (pose,
    // projection, tracking, mounts) with no GPU pipeline. See
    // `lunco_render::camera` and docs/architecture/render-decoupling.md.
    commands
        .spawn((
            // Nested: a bundle tuple maxes out at 16 elements, and `SceneCamera` made 17.
            //
            // The same camera/exposure pair the USD camera projection uses, with
            // no authored opinion to honour here. The caller supplies the
            // authoritative graphics profile; this camera must not invent one.
            // `SceneCamera` is the render-free camera intent. The render-side
            // binder adds `Camera3d` and its complete render graph atomically;
            // inserting a bare `Camera` here would trigger Bevy's missing
            // render-graph warning before that binder runs.
            (
                lunco_render::scene_camera_look_with_profile(None, profile),
                lunco_render::usd_default_perspective_projection(),
                lunco_render::GraphicsCameraDefaults,
            ),
            FreeFlightCamera {
                yaw,
                pitch,
                damping: None,
            },
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
            CameraZoomInput::default(),
            Name::new("Avatar Camera"),
            ChildOf(grid_entity),
        ))
        .id()
}

/// The local avatar is a **controllable described like a rover**: it carries a
/// `InputPorts` surface (`forward`/`side`/`up` input ports) + a
/// `ControlBinding` mapping move intents to those ports. The SAME
/// `lunco_controller::drive_from_bindings`
/// path then drives it — its *self-drive* branch fires for an entity that holds its
/// own `ActionState` + `ControlBinding` and, when free, no `ControllerLink`
/// (possession adds a `ControllerLink→vessel`, which excludes the avatar from
/// self-drive and redirects control to the vessel — no possession-code changes).
/// `apply_fly` reads the resulting `forward`/`side`/`up` ports back.
///
/// The command *vocabulary* is seeded from the binding by
/// `lunco_mobility::sync_input_ports`, exactly like a rover. Authored in
/// code for now; P3 will move it to an `_AvatarControl` USD profile so the avatar
/// is spawned identically via code or USD.
/// The other half of [`stamp_avatar_controls`]: an entity that has just STOPPED
/// being the local avatar gives back everything being the avatar gave it.
///
/// Losing [`LocalAvatar`] is the one signal, and `lunco_core`'s hook is what
/// produces it — the moment a new claimant appears, the previous holder is
/// demoted here, wherever either of them came from (a USD `Avatar` prim, an
/// explicit host-created observation camera, or a scene recompose handing the prim a fresh
/// entity). `lunco-usd-sim` used to do this itself, in a loop it ran only on the
/// authored-prim path, which is why the other paths left a second live avatar:
/// two `Camera3d`s rendering the same window (the viewport flickers between
/// them), input driving both linked vessels, release firing twice.
///
/// The camera is DEACTIVATED, never stripped of `Camera`. Removing `Camera` from
/// a live, already-extracted window camera in the same frame a new scene's
/// shadow-casting sun initialises orphans its render-world view:
/// `build_directional_light_cascades` has dropped the cascade, `prepare_lights`
/// unwraps `None`, and the render app hard-crashes — deterministically, on every
/// elevated scene load. An inactive camera is not extracted, so it neither needs
/// a cascade nor renders a ghost.
fn demote_former_avatar(trigger: On<Remove, LocalAvatar>, mut commands: Commands) {
    let entity = trigger.entity;
    commands.entity(entity).try_remove::<(
        Avatar,
        FreeFlightCamera,
        OrbitCamera,
        SpringArmCamera,
        SurfaceRelativeMode,
        lunco_controller::ControllerLink,
        IntentAnalogState,
    )>();
    // RETIRE IT FROM THE VIEWPORT POOL, not merely from the avatar role.
    //
    // `SceneCamera` is what makes an entity a viewport CANDIDATE: every query that
    // can put a camera on screen filters on it — the explicit viewport reconciler,
    // `cycle_active_camera` (KeyC), and offscreen capture. Deactivating without
    // removing it left the retired camera in that pool forever, so the app
    // accumulated one stale candidate per scene load.
    //
    // That is the "two cameras" bug. An explicit host-created avatar camera
    // (`spawn_avatar_camera`) is NOT a USD prim, so a scene load's `despawn` sweep
    // never touches it; when the incoming scene authored its own `lunco:avatar`
    // camera, the old one was demoted but stayed eligible. It also has a LOWER entity
    // index than anything the new scene spawns, so the moment the binding went
    // momentarily invalid — which it does every load, because the new camera's
    // `Camera3d`/`Projection` is attached later by the deferred `SceneCamera` binder
    // — the old camera could remain visible, undoing this demotion.
    //
    // `Camera`/`Camera3d` are deliberately left in place: stripping `Camera` from a
    // live, already-extracted window camera orphans its render-world view and crashes
    // `prepare_lights` on the cascade unwrap (see this function's docs). Removing only
    // the intent marker retires the camera from selection while leaving the render
    // world untouched — and since the reconciler no longer sees it, the `is_active =
    // false` written below is now final rather than something the next frame undoes.
    commands
        .entity(entity)
        .try_remove::<lunco_render::SceneCamera>();
    commands.queue(move |world: &mut World| {
        if let Some(mut cam) = world.get_mut::<bevy::camera::Camera>(entity) {
            cam.is_active = false;
        }
    });
}

fn stamp_avatar_controls(trigger: On<Add, LocalAvatar>, mut commands: Commands) {
    let binding = lunco_core::ControlBinding::from_intent_entries(&[
        ("forward".to_string(), "forward".to_string(), 1.0),
        ("backward".to_string(), "forward".to_string(), -1.0),
        ("right".to_string(), "side".to_string(), 1.0),
        ("left".to_string(), "side".to_string(), -1.0),
        ("yaw_right".to_string(), "up".to_string(), 1.0),
        ("yaw_left".to_string(), "up".to_string(), -1.0),
    ]);
    // No `ActuatorPorts` (no hardware actuators — `apply_fly` reads the command
    // inputs directly) and no `DriveMix` (an avatar is not a wheeled chassis; it is
    // not a drive-allocation target and must stay out of the chassis queries).
    //
    // The input surface is SEEDED FROM THE BINDING, via `ControlBinding::ports()`,
    // which exists for exactly this ("an endpoint seeds exactly these into its
    // `inputs`"). Seeding matters because the port surface is strict: only keys
    // present in `InputPorts::values` are writable, and
    // `INPUT_PORTS_BACKEND::write_input` returns `false` for anything else. This
    // used to insert `InputPorts::default()` — an EMPTY map — so every
    // `forward`/`side`/`up` write `drive_self_drivers` produced was dropped by the
    // registry, `apply_fly`'s `inputs.cmd("forward")` read a constant 0.0, and the
    // avatar could not move. The only trace was a per-port
    // `[cosim] SetPorts targets unknown input port` warning, which reads like a
    // scene-authoring mistake rather than a dead control path.
    //
    // Applied as a world command so an EXISTING surface is merged, not replaced: a
    // prim that authored its own `inputs:*` ports already has an `InputPorts`, and
    // a blind `insert` would wipe those keys while adding the binding's.
    let entity = trigger.entity;
    commands.queue(move |world: &mut World| {
        let Ok(mut ent) = world.get_entity_mut(entity) else {
            return;
        };
        let ports: Vec<String> = binding
            .iter()
            .flat_map(|b| b.ports())
            .map(str::to_string)
            .collect();
        match ent.get_mut::<lunco_core::InputPorts>() {
            Some(mut existing) => {
                for port in ports {
                    existing.values.entry(port).or_insert(0.0);
                }
            }
            None => {
                let refs: Vec<&str> = ports.iter().map(String::as_str).collect();
                ent.insert(lunco_core::InputPorts::new(&refs));
            }
        }
        if let Some(b) = binding {
            ent.insert(b);
        }
    });
}

// ─── Shared Math Helpers (CQ-113 DRY) ────────────────────────────────────────

/// Return the body's ENU tangent frame in an entity's immediate Grid frame.
///
/// The tangent frame is owned by celestial geodesy and is expressed in the
/// body's body-fixed axes. The two rotations returned by BigSpace's shared
/// `grid_relative_pose` then provide the exact body-local → camera-Grid map.
/// This is the only surface-camera frame boundary: no global-Y projection and
/// no root-frame subtraction can silently change the meaning of heading.
fn surface_axes_in_grid(
    grid_entity: Entity,
    gravity: &LocalGravityField,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
) -> Option<(Vec3, Vec3, Vec3)> {
    let body_entity = gravity.body_entity?;
    let (_, _, grid_to_body_frame, _, body_to_body_frame) = lunco_core::coords::common_grid_poses(
        grid_entity,
        body_entity,
        q_parents,
        q_grids,
        q_spatial,
    )?;
    let body_to_grid = grid_to_body_frame.inverse() * body_to_body_frame;
    Some(surface_axes_from_body_position(
        gravity.body_relative_position,
        body_to_grid,
    ))
}

/// Map a body-fixed ENU frame into a camera Grid after the caller has resolved
/// the exact body-local position and BigSpace rotation for that Grid.
fn surface_axes_from_body_position(
    body_relative_position: DVec3,
    body_to_grid: DQuat,
) -> (Vec3, Vec3, Vec3) {
    let tangent = LocalTangentFrame::from_body_fixed_position(body_relative_position);
    (
        (body_to_grid * tangent.east)
            .normalize_or(DVec3::X)
            .as_vec3(),
        (body_to_grid * tangent.north)
            .normalize_or(DVec3::NEG_Z)
            .as_vec3(),
        (body_to_grid * tangent.up).normalize_or(DVec3::Y).as_vec3(),
    )
}

/// Resolve a body-fixed ENU frame for a point already expressed in a Grid.
///
/// `grid_position` is a BigSpace grid-absolute position, exactly the value
/// returned by [`lunco_core::coords::grid_relative_pose`] for a target. The
/// conversion stays in the shared body-frame branch and never goes through a
/// root `GlobalTransform`.
fn surface_axes_for_grid_position<F: bevy::ecs::query::QueryFilter>(
    grid_entity: Entity,
    grid_position: DVec3,
    body_entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(Vec3, Vec3, Vec3)> {
    let (_, grid_body_position, grid_to_body_frame, body_position, body_to_body_frame) =
        lunco_core::coords::common_grid_poses(
            grid_entity,
            body_entity,
            q_parents,
            q_grids,
            q_spatial,
        )?;
    let body_relative_position = body_to_body_frame.inverse()
        * (grid_body_position + grid_to_body_frame * grid_position - body_position);
    let body_to_grid = grid_to_body_frame.inverse() * body_to_body_frame;
    Some(surface_axes_from_body_position(
        body_relative_position,
        body_to_grid,
    ))
}

/// Return gravity-up in an entity's immediate Grid frame.
///
/// A surface camera's gravity is authored in its body's body-fixed frame. If
/// the camera grid is under that same body frame, convert through only the
/// local BigSpace grid branch. The world-frame path handles flat gravity and
/// non-surface views, which have no body-fixed ENU frame. A broken coordinate
/// hierarchy is an integration error, never a reason to invent a local-up
/// result.
fn gravity_up_in_grid(
    grid_entity: Entity,
    gravity: &LocalGravityField,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
) -> Vec3 {
    if let Some((_, _, up)) =
        surface_axes_in_grid(grid_entity, gravity, q_parents, q_grids, q_spatial)
    {
        return up;
    }

    let (_, grid_rotation) = lunco_core::coords::world_pose(grid_entity, q_parents, q_grids, q_spatial)
        .unwrap_or_else(|error| {
            panic!(
                "camera Grid {grid_entity:?} has no valid BigSpace pose while resolving gravity: {error}"
            )
        });
    (grid_rotation.0.inverse() * gravity.up)
        .normalize_or(DVec3::Y)
        .as_vec3()
}

/// The body explicitly authored by the loaded site.
fn site_body(
    q_site: &Query<&lunco_celestial::GeodeticAnchor, With<lunco_celestial::SiteAnchor>>,
    q_bodies: &Query<(Entity, &CelestialBody)>,
) -> Option<(Entity, f64)> {
    let anchor = q_site.iter().next()?;
    let (ent, body) = q_bodies
        .iter()
        .find(|(_, b)| b.ephemeris_id == anchor.body)?;
    Some((ent, body.radius_m))
}

/// Build a surface-relative camera orientation from the body's ENU axes plus
/// `heading` and `pitch`.
///
/// Forward starts at the geodetic north axis, is yawed by `heading` about the
/// exact surface normal, then pitched about the resulting right axis. Rebuilt
/// from scratch (no incremental accumulation) so there is zero roll drift.
///
pub fn surface_camera_rotation(
    east: Vec3,
    north: Vec3,
    up: Vec3,
    heading: f32,
    pitch: f32,
) -> Quat {
    let (_, north, up) = orthonormal_surface_axes(east, north, up);
    let heading_q = Quat::from_axis_angle(up, heading);
    let forward = heading_q.mul_vec3(north);
    let right = forward.cross(up).normalize();
    let base_rot = Quat::from_mat3(&Mat3::from_cols(right, up, -forward));
    let pitch_q = Quat::from_axis_angle(right, pitch);
    (pitch_q * base_rot).normalize()
}

/// Compose the free-flight movement vector from the camera's forward/right
/// directions and a stable vertical axis.
///
/// Elevation must not use `Transform::up()`: when the view is pitched, Q+W can
/// then cancel the forward vector's horizontal component and make the diagonal
/// appear not to move. World +Y is the vertical axis in free flight; callers in
/// surface mode pass the current gravity-up direction instead.
fn fly_move_direction(
    tf: &Transform,
    forward: f32,
    side: f32,
    elevation: f32,
    up_dir: Vec3,
) -> Vec3 {
    let up_dir = up_dir.normalize_or_zero();
    let up_dir = if up_dir == Vec3::ZERO {
        Vec3::Y
    } else {
        up_dir
    };
    let direction = *tf.forward() * forward + *tf.right() * side + up_dir * elevation;
    // Keyboard diagonals intentionally contribute multiple axes, but the
    // resulting command must still have the same maximum speed as a single
    // axis. Preserve sub-unit analog input and cap only the combined vector.
    let length_sq = direction.length_squared();
    if length_sq > 1.0 {
        direction / length_sq.sqrt()
    } else {
        direction
    }
}

/// Decompose a camera rotation into the same surface-frame heading and pitch
/// consumed by [`surface_camera_rotation`].
///
/// This is the only legal way to enter surface mode from another camera mode:
/// Euler angles are coordinates in the old frame, not surface heading/pitch.
pub fn surface_camera_angles(east: Vec3, north: Vec3, up: Vec3, rotation: Quat) -> (f32, f32) {
    let (east, north, up) = orthonormal_surface_axes(east, north, up);
    let forward = rotation * Vec3::NEG_Z;
    let pitch = forward.dot(up).clamp(-1.0, 1.0).asin();
    let tangent_forward = (forward - up * forward.dot(up)).normalize_or(north);
    let heading = (-tangent_forward.dot(east)).atan2(tangent_forward.dot(north));
    (heading, pitch)
}

/// Normalize a surface ENU basis with the engine's right-handed convention.
fn orthonormal_surface_axes(east: Vec3, north: Vec3, up: Vec3) -> (Vec3, Vec3, Vec3) {
    let east = east.normalize_or(Vec3::X);
    let up = up.normalize_or(Vec3::Y);
    // Re-derive north from the handed ENU pair so a tiny input drift cannot
    // introduce roll. The fallback is only for the physically undefined
    // body-centre frame.
    let north = up.cross(east).normalize_or(north.normalize_or(Vec3::NEG_Z));
    (east, north, up)
}

/// Resolve a surface-bound target's local up vector and authored heading.
///
/// The target and the surface grid are siblings under the body's rotating
/// frame, so this composes only that local branch. It never subtracts solar
/// coordinates and never assumes that the surface grid is world-Y aligned.
fn surface_target_frame(
    target_position: DVec3,
    target_rotation: DQuat,
    target_grid: Entity,
    body_entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
) -> Option<(Vec3, Vec3, Vec3, f32)> {
    let (east, north, up) = surface_axes_for_grid_position(
        target_grid,
        target_position,
        body_entity,
        q_parents,
        q_grids,
        q_spatial,
    )?;
    let heading = surface_camera_angles(east, north, up, target_rotation.as_quat()).0;
    Some((east, north, up, heading))
}

/// Apply an accumulated mouse-scroll delta as a multiplicative (exponential)
/// zoom to a camera arm `distance`, clamped to `[min_dist, max_dist]`, then
/// consume the delta. Scroll up (delta > 0) zooms in; down zooms out.
///
/// Consolidates the CQ-113 duplicate zoom math shared by the spring-arm, chase,
/// and orbit camera systems (they differed only in the clamp bounds).
fn apply_scroll_zoom(
    distance: &mut f64,
    scroll_delta: &mut f32,
    sens: f32,
    min_dist: f64,
    max_dist: f64,
) {
    if *scroll_delta != 0.0 {
        let zoom_factor = (-*scroll_delta as f64 * sens as f64 * 0.01)
            .exp()
            .clamp(ZOOM_FACTOR_MIN, ZOOM_FACTOR_MAX);
        *distance = (*distance * zoom_factor).clamp(min_dist, max_dist);
        *scroll_delta = 0.0;
    }
}

/// Migrate the avatar to a target's Grid, placing it at a pose already expressed
/// in that Grid's local frame.
///
/// This is intentionally a local-frame boundary. A possession/follow target is
/// resolved with [`lunco_core::coords::grid_relative_pose`], so the camera is
/// never converted through the heliocentric root and then subtracted back into
/// the target grid. That round trip is numerically valid at ordinary distances,
/// but it is the wrong contract for a live BigSpace hierarchy: site pinning and
/// cell rebranching are allowed to change the root representation while the
/// target and camera remain in the same body-local frame.
///
/// No-op when `target_grid` is `None`/placeholder or not a live Grid.
///
/// Consolidates the CQ-113 duplicate migration block shared by
/// `on_possess_command`, `on_follow_command`, and `on_focus_command`.
fn migrate_avatar_to_target_grid(
    commands: &mut Commands,
    avatar_ent: Entity,
    target_grid: Option<Entity>,
    final_local_pos: DVec3,
    final_rot: Quat,
    q_grids: &Query<&Grid>,
) {
    if let Some(tg) = target_grid {
        if tg != Entity::PLACEHOLDER {
            if let Ok(target_grid_ref) = q_grids.get(tg) {
                let (new_cell, translation) = target_grid_ref.translation_to_grid(final_local_pos);
                let local_tf = Transform::from_translation(translation).with_rotation(final_rot);
                info!(
                    avatar = ?avatar_ent,
                    target_grid = ?tg,
                    final_local = ?final_local_pos,
                    cell = ?new_cell,
                    local = ?local_tf.translation,
                    "[possess] migrated avatar into target grid"
                );
                migrate_to_grid(commands, avatar_ent, tg, new_cell, local_tf);
            }
        }
    }
}

// ─── Behavior Systems ────────────────────────────────────────────────────────

/// Unified vessel-follow solver (all three [`FollowAttitude`] modes).
///
/// Appends every descendant of `root` to `out` (the root itself is the caller's).
/// Used to exclude a followed vessel's own colliders — which live on child prims,
/// not the root — from the spring arm's collision cast.
fn collect_subtree(root: Entity, q_children: &Query<&Children>, out: &mut Vec<Entity>) {
    if let Ok(children) = q_children.get(root) {
        for &c in children {
            out.push(c);
            collect_subtree(c, q_children, out);
        }
    }
}

/// Every avian joint type as one connectivity view — the spring arm needs to know
/// what is *attached* to the vessel, not merely what is parented under it.
#[derive(bevy::ecs::system::SystemParam)]
pub struct VesselJoints<'w, 's> {
    revolute: Query<'w, 's, &'static avian3d::prelude::RevoluteJoint>,
    fixed: Query<'w, 's, &'static avian3d::prelude::FixedJoint>,
    prismatic: Query<'w, 's, &'static avian3d::prelude::PrismaticJoint>,
    spherical: Query<'w, 's, &'static avian3d::prelude::SphericalJoint>,
    distance: Query<'w, 's, &'static avian3d::prelude::DistanceJoint>,
}

impl VesselJoints<'_, '_> {
    /// Undirected adjacency over every joint edge in the world.
    ///
    /// Built once per call and indexed, not rescanned per BFS step — the walk is
    /// then O(edges + members) instead of O(members × edges).
    fn adjacency(&self) -> bevy::platform::collections::HashMap<Entity, Vec<Entity>> {
        let mut adj: bevy::platform::collections::HashMap<Entity, Vec<Entity>> =
            bevy::platform::collections::HashMap::default();
        let mut link = |a: Entity, b: Entity| {
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        };
        self.revolute.iter().for_each(|j| link(j.body1, j.body2));
        self.fixed.iter().for_each(|j| link(j.body1, j.body2));
        self.prismatic.iter().for_each(|j| link(j.body1, j.body2));
        self.spherical.iter().for_each(|j| link(j.body1, j.body2));
        self.distance.iter().for_each(|j| link(j.body1, j.body2));
        adj
    }
}

/// Structural inputs for the spring-arm self-collision filter.
///
/// Joint motor/frame updates are observed as well, but the cache compares their
/// endpoint pairs before rebuilding. This keeps a frequently driven joint from
/// turning a structure cache back into per-frame work.
#[derive(bevy::ecs::system::SystemParam)]
struct VesselCollisionTopology<'w, 's> {
    children: Query<'w, 's, (), Or<(Added<Children>, Changed<Children>)>>,
    parents: Query<'w, 's, (), Or<(Added<ChildOf>, Changed<ChildOf>)>>,
    revolute: Query<
        'w,
        's,
        (Entity, &'static avian3d::prelude::RevoluteJoint),
        Or<(
            Added<avian3d::prelude::RevoluteJoint>,
            Changed<avian3d::prelude::RevoluteJoint>,
        )>,
    >,
    fixed: Query<
        'w,
        's,
        (Entity, &'static avian3d::prelude::FixedJoint),
        Or<(
            Added<avian3d::prelude::FixedJoint>,
            Changed<avian3d::prelude::FixedJoint>,
        )>,
    >,
    prismatic: Query<
        'w,
        's,
        (Entity, &'static avian3d::prelude::PrismaticJoint),
        Or<(
            Added<avian3d::prelude::PrismaticJoint>,
            Changed<avian3d::prelude::PrismaticJoint>,
        )>,
    >,
    spherical: Query<
        'w,
        's,
        (Entity, &'static avian3d::prelude::SphericalJoint),
        Or<(
            Added<avian3d::prelude::SphericalJoint>,
            Changed<avian3d::prelude::SphericalJoint>,
        )>,
    >,
    distance: Query<
        'w,
        's,
        (Entity, &'static avian3d::prelude::DistanceJoint),
        Or<(
            Added<avian3d::prelude::DistanceJoint>,
            Changed<avian3d::prelude::DistanceJoint>,
        )>,
    >,
    removed_children: RemovedComponents<'w, 's, Children>,
    removed_parents: RemovedComponents<'w, 's, ChildOf>,
    removed_revolute: RemovedComponents<'w, 's, avian3d::prelude::RevoluteJoint>,
    removed_fixed: RemovedComponents<'w, 's, avian3d::prelude::FixedJoint>,
    removed_prismatic: RemovedComponents<'w, 's, avian3d::prelude::PrismaticJoint>,
    removed_spherical: RemovedComponents<'w, 's, avian3d::prelude::SphericalJoint>,
    removed_distance: RemovedComponents<'w, 's, avian3d::prelude::DistanceJoint>,
}

/// Render-rate cache for spring-arm self-collision filters.
///
/// The exclusion set is a function of hierarchy and joint topology, not of the
/// followed body's pose. Keep the derived filter in RAM and invalidate it only
/// when one of those structural inputs changes.
#[derive(Default)]
struct VesselCollisionFilterCache {
    adjacency: bevy::platform::collections::HashMap<Entity, Vec<Entity>>,
    joint_bodies: bevy::ecs::entity::EntityHashMap<[Entity; 2]>,
    filters: bevy::ecs::entity::EntityHashMap<avian3d::prelude::SpatialQueryFilter>,
    initialized: bool,
}

impl VesselCollisionFilterCache {
    fn observe_joint(&mut self, entity: Entity, bodies: [Entity; 2]) -> bool {
        let changed = self.joint_bodies.get(&entity) != Some(&bodies);
        self.joint_bodies.insert(entity, bodies);
        changed
    }

    fn remove_joint(&mut self, entity: Entity) -> bool {
        self.joint_bodies.remove(&entity).is_some()
    }

    fn refresh(&mut self, joints: &VesselJoints, topology: &mut VesselCollisionTopology) {
        let mut dirty = !self.initialized;
        dirty |= !topology.children.is_empty() || !topology.parents.is_empty();
        dirty |= topology.removed_children.read().next().is_some();
        dirty |= topology.removed_parents.read().next().is_some();

        for (entity, joint) in &topology.revolute {
            dirty |= self.observe_joint(entity, [joint.body1, joint.body2]);
        }
        for (entity, joint) in &topology.fixed {
            dirty |= self.observe_joint(entity, [joint.body1, joint.body2]);
        }
        for (entity, joint) in &topology.prismatic {
            dirty |= self.observe_joint(entity, [joint.body1, joint.body2]);
        }
        for (entity, joint) in &topology.spherical {
            dirty |= self.observe_joint(entity, [joint.body1, joint.body2]);
        }
        for (entity, joint) in &topology.distance {
            dirty |= self.observe_joint(entity, [joint.body1, joint.body2]);
        }

        for entity in topology.removed_revolute.read() {
            self.remove_joint(entity);
            dirty = true;
        }
        for entity in topology.removed_fixed.read() {
            self.remove_joint(entity);
            dirty = true;
        }
        for entity in topology.removed_prismatic.read() {
            self.remove_joint(entity);
            dirty = true;
        }
        for entity in topology.removed_spherical.read() {
            self.remove_joint(entity);
            dirty = true;
        }
        for entity in topology.removed_distance.read() {
            self.remove_joint(entity);
            dirty = true;
        }

        if !dirty {
            return;
        }

        self.adjacency = joints.adjacency();
        self.filters.clear();
        self.initialized = true;
    }

    fn filter_for(
        &mut self,
        target: Entity,
        q_children: &Query<&Children>,
    ) -> &avian3d::prelude::SpatialQueryFilter {
        if !self.filters.contains_key(&target) {
            let excluded =
                vessel_collision_exclusions_from_adjacency(target, q_children, &self.adjacency);
            let mut filter = avian3d::prelude::SpatialQueryFilter::from_excluded_entities(excluded);
            filter.mask = avian3d::prelude::LayerMask(!lunco_core::NON_PHYSICAL_QUERY_LAYERS);
            self.filters.insert(target, filter);
        }
        self.filters
            .get(&target)
            .expect("spring-arm filter inserted above")
    }
}

/// [`vessel_collision_exclusions`] against a `&mut World`, for tests.
///
/// The system form takes `SystemParam` queries, which a test would otherwise have
/// to build a whole schedule to obtain; this runs the identical code path through a
/// one-shot system so the test pins the real behaviour, not a re-implementation.
pub fn vessel_collision_exclusions_for_test(world: &mut World, target: Entity) -> Vec<Entity> {
    let mut sys = bevy::ecs::system::IntoSystem::into_system(
        move |q_children: Query<&Children>, joints: VesselJoints| {
            vessel_collision_exclusions(target, &q_children, &joints)
        },
    );
    sys.initialize(world);
    sys.run((), world)
        .expect("exclusion query cannot fail — it only reads")
}

/// Everything the spring arm must NOT collide with while following `target`: the
/// possessed vessel itself, its ECS subtree, and every body joined to it —
/// transitively — plus each of those bodies' own subtrees.
///
/// The subtree alone is not the vessel. A physical rover is a JOINTED ASSEMBLY:
/// the chassis carries the `RigidBody` the camera follows, and each wheel is its
/// own dynamic body held on by a revolute + prismatic pair. Whether a wheel ends
/// up parented under the chassis or as a sibling under the grid is a physics
/// detail (a dynamic body's `Transform` is a writeback target), and it changed
/// per drivetrain — so a subtree-only exclusion let the arm's ray hit the
/// vessel's own wheels. The hit pulled `target_len` to nearly zero and the camera
/// dropped inside the rover it was meant to be looking at.
///
/// Joint connectivity is what "part of this vehicle" actually means, and it is
/// the same set `RecoverVessel` moves as a unit for the same reason.
fn vessel_collision_exclusions(
    target: Entity,
    q_children: &Query<&Children>,
    joints: &VesselJoints,
) -> Vec<Entity> {
    let adj = joints.adjacency();
    vessel_collision_exclusions_from_adjacency(target, q_children, &adj)
}

fn vessel_collision_exclusions_from_adjacency(
    target: Entity,
    q_children: &Query<&Children>,
    adj: &bevy::platform::collections::HashMap<Entity, Vec<Entity>>,
) -> Vec<Entity> {
    // BFS the joint-connected component containing the target.
    let mut members = vec![target];
    let mut seen = bevy::platform::collections::HashSet::from([target]);
    let mut queue = std::collections::VecDeque::from([target]);
    while let Some(e) = queue.pop_front() {
        for &n in adj.get(&e).into_iter().flatten() {
            if seen.insert(n) {
                members.push(n);
                queue.push_back(n);
            }
        }
    }
    // Each member's own descendants carry the actual collider prims.
    let mut out = members.clone();
    for m in members {
        collect_subtree(m, q_children, &mut out);
    }
    out
}

/// The CHASE camera: position follows the target; [`FollowAttitude`] selects how
/// orientation is derived (heading-lock, world-locked survey, or full-attitude cockpit).
///
/// Runs at render cadence after [`lunco_time::InteractionRenderSet`].  `Time<Real>` is
/// used only for the camera's visual rotation/obstacle response; the followed body is
/// read from the final render-frame `Transform`, so no separate stepped pose or camera
/// interpolation can fight the body.
fn spring_arm_system(
    time: Res<Time<Real>>,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &mut SpringArmCamera,
            &ChildOf,
            Option<&SurfaceRelativeMode>,
            &mut CameraZoomInput,
        ),
        (
            With<Avatar>,
            Without<Grid>,
            Without<FrameBlend>,
            Without<OrbitCamera>,
            Without<FreeFlightCamera>,
            Without<SurfaceCamera>,
            Without<lunco_core::CinematicCameraLock>,
        ),
    >,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    gravity: Res<LocalGravityField>,
    q_dragging: Query<(), With<lunco_core::GizmoDragging>>,
    q_children: Query<&Children>,
    defaults: Res<CameraDefaults>,
    keys: Res<ButtonInput<KeyCode>>,
    spatial_query: Option<lunco_physics::GridSpatialQuery>,
    joints: VesselJoints,
    mut collision_filters: Local<VesselCollisionFilterCache>,
    mut topology: VesselCollisionTopology,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
        return;
    }
    let dt = time.delta_secs();
    collision_filters.refresh(&joints, &mut topology);

    for (_avatar_ent, mut tf, mut cell, mut arm, child_of, surface_mode, mut zoom) in
        q_avatar.iter_mut()
    {
        // Skip follow while the target is being dragged by the editor gizmo
        // (marker set by luncosim-edit; never present on a headless server).
        if q_dragging.get(arm.target).is_ok() {
            continue;
        }

        let Ok(grid) = q_grids.get(child_of.0) else {
            continue;
        };
        // Possession/follow migration puts the target and avatar in one live
        // body-local BigSpace frame. Keep the render-rate follow solve in that
        // frame as well. A target -> solar root -> avatar-grid round trip makes
        // the camera depend on site pinning and celestial propagation order.
        let Some((target_pos, target_rotation)) = lunco_core::coords::grid_relative_pose(
            arm.target, child_of.0, &q_parents, &q_grids, &q_spatial,
        ) else {
            continue;
        };
        let surface_axes = surface_mode.and_then(|_| {
            gravity.body_entity.and_then(|body_entity| {
                surface_axes_for_grid_position(
                    child_of.0,
                    target_pos,
                    body_entity,
                    &q_parents,
                    &q_grids,
                    &q_spatial,
                )
            })
        });

        // Multiplicative zoom using exponential scaling — same formula as
        // ChaseCamera/OrbitCamera so raw pixel scroll deltas stay well-scaled.
        // Scroll up (delta > 0) -> zoom in. Scroll down (delta < 0) -> zoom out.
        apply_scroll_zoom(
            &mut arm.distance,
            &mut zoom.delta,
            ZOOM_SENSITIVITY,
            5.0,
            200.0,
        );

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
        // Desired orientation — the ONE axis the three follow modes differ on.
        let desired_rot = match arm.attitude {
            // Cockpit frame: full body orientation × user yaw/pitch offset. The
            // camera rolls with the craft (was the separate `ChaseCamera`).
            FollowAttitude::FullAttitude => {
                target_rotation.as_quat() * Quat::from_euler(EulerRot::YXZ, arm.yaw, arm.pitch, 0.0)
            }
            // Stable external frame: ignore the body's attitude entirely, so a
            // 6-DOF flyer tumbles inside a steady view. World-up, user yaw/pitch
            // (was the celestial `OrbitCamera`, reused wrongly for vessels).
            FollowAttitude::WorldLocked => Quat::from_euler(EulerRot::YXZ, arm.yaw, arm.pitch, 0.0),
            // Heading-follow: yaw from the body's forward (steerable vehicles),
            // up = surface normal or world-Y.
            FollowAttitude::Heading => {
                let target_heading_d = if arm.track_heading {
                    if let Some((east, north, up)) = surface_axes {
                        surface_camera_angles(east, north, up, target_rotation.as_quat()).0 as f64
                    } else {
                        let target_fwd_d = target_rotation.mul_vec3(Vec3::NEG_Z.as_dvec3());
                        if target_fwd_d.x.abs() > 1e-6 || target_fwd_d.z.abs() > 1e-6 {
                            -target_fwd_d.x.atan2(-target_fwd_d.z)
                        } else {
                            0.0
                        }
                    }
                } else {
                    0.0
                };
                let final_yaw = (target_heading_d + arm.yaw as f64) as f32;
                if let Some((east, north, up)) = surface_axes {
                    surface_camera_rotation(east, north, up, final_yaw, arm.pitch)
                } else {
                    Quat::from_euler(EulerRot::YXZ, final_yaw, arm.pitch, 0.0)
                }
            }
        };

        // Rotation: exponential decay for snappy but smooth heading follow.
        // Frequency 60.0 — snappy without transmitting physics jitter.
        let damping = arm.damping.unwrap_or(defaults.damping);
        let rot_alpha = 1.0 - (-defaults.rotation_rate * (1.0 - damping) * dt).exp();
        tf.rotation = tf.rotation.slerp(desired_rot, rot_alpha);

        // Desired camera position: behind target along smoothed rotation.
        let offset = tf.rotation.mul_vec3(Vec3::Z).as_dvec3() * arm.distance;
        let vertical_offset: DVec3 = if surface_mode.is_some() {
            let up = surface_axes
                .map(|(_, _, up)| up)
                .unwrap_or_else(|| {
                    gravity_up_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial)
                })
                .as_dvec3();
            up * arm.vertical_offset as f64
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
        // Mask out the TRIGGER layer so the camera doesn't clip on invisible
        // trigger-zone sensors (waypoints etc.).
        //
        // The exclusion set is the whole JOINTED VESSEL — see
        // `vessel_collision_exclusions`.
        // A non-finite origin is not a camera problem to solve, but it IS one this
        // cast cannot survive: obvhs asserts `origin.is_finite()`, so following a
        // vessel whose solve has diverged would panic the compute pool from inside
        // the camera. No hit means the arm simply does not shorten, which is the
        // right behaviour for a frame with nothing valid to look at.
        let castable = ray_origin.is_finite() && ray_len.is_finite();
        let hit = match &spatial_query {
            Some(sq) if castable => {
                let filter = collision_filters.filter_for(arm.target, &q_children);
                sq.cast_ray_in_grid(
                    child_of.0,
                    lunco_core::coords::GridPos(ray_origin),
                    bevy::math::Dir3::new(ray_dir.as_vec3()).unwrap_or(bevy::math::Dir3::Y),
                    ray_len,
                    true,
                    filter,
                )
            }
            _ => None,
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

/// OrbitCamera system: positions the camera at a fixed offset from a target,
/// locked to the ecliptic (star-fixed) reference frame.
///
/// Only runs when `OrbitCamera` is present AND no `FrameBlend` is active.
/// The camera does NOT rotate with the target — stars stay still.
fn orbit_angles_from_arm(direction: DVec3) -> (f32, f32) {
    let direction = direction.normalize_or(DVec3::Z);
    (
        direction.x.atan2(direction.z) as f32,
        (-direction.y.clamp(-1.0, 1.0).asin()) as f32,
    )
}

fn orbit_system(
    // Wall-rooted render clock: orbit is presentation and must neither rate-scale
    // with simulation time nor be quantized to the fixed interaction cadence.
    time: Res<Time<Real>>,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &mut OrbitCamera,
            &ChildOf,
            &mut CameraZoomInput,
            Has<SunlitArrival>,
            Has<RadialArrival>,
        ),
        (
            With<Avatar>,
            Without<FrameBlend>,
            Without<SpringArmCamera>,
            Without<FreeFlightCamera>,
            Without<SurfaceCamera>,
            Without<lunco_core::CinematicCameraLock>,
        ),
    >,
    q_world_grid: Query<Entity, With<lunco_core::WorldGrid>>,
    frame_index: Res<lunco_celestial::ReferenceFrameIndex>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_sc: Query<&Spacecraft>,
    q_dragging: Query<(), With<lunco_core::GizmoDragging>>,
    defaults: Res<CameraDefaults>,
    keys: Res<ButtonInput<KeyCode>>,
    q_children: Query<&Children>,
    mut commands: Commands,
    mut log_countdown: Local<u32>,
    mut orbital_pin: Option<ResMut<lunco_celestial::OrbitalViewPin>>,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
        return;
    }

    let Ok(root_grid) = q_world_grid.single() else {
        return;
    };
    let dt = time.delta_secs();

    for (avatar_ent, mut tf, mut cell, mut orbit, child_of, mut zoom, wants_sunlit, wants_radial) in
        q_avatar.iter_mut()
    {
        if q_dragging.get(orbit.target).is_ok() {
            continue;
        }

        let physical_target = get_physical_body(orbit.target, &q_children, &q_bodies);
        let body = q_bodies.get(physical_target).ok().map(|(_, body)| body);
        // Celestial bodies own an explicit star-fixed camera grid. This is the
        // same nested-grid shape as big_space's planets example: body-fixed
        // terrain/vehicles stay under the rotating frame while the camera
        // lives in a co-located inertial sibling. Non-celestial targets use the
        // canonical world grid.
        let orbit_grid = if let Some(body) = body {
            let Some(entity) =
                frame_index.resolve(lunco_celestial::ReferenceFrame::EclipticJ2000 {
                    center: body.ephemeris_id,
                })
            else {
                warn!(
                    "ORBIT: body {} has no inertial reference frame; refusing an ambiguous camera frame",
                    body.ephemeris_id
                );
                continue;
            };
            entity
        } else {
            root_grid
        };
        let Ok(orbit_grid_ref) = q_grids.get(orbit_grid) else {
            continue;
        };
        let centre_entity = body.map_or(orbit.target, |_| physical_target);
        let Some((target_orbit, _)) = lunco_core::coords::pose_in_grid(
            centre_entity,
            orbit_grid,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            continue;
        };
        // The avatar is mutably borrowed, so seed the canonical cross-grid
        // conversion with its live cell/local pair. Orbit math stays directly
        // in the selected inertial body grid; no subtraction of ~AU root-frame
        // coordinates can consume local precision.
        let Some((cam_orbit, _)) = lunco_core::coords::pose_in_grid_seeded(
            avatar_ent,
            orbit_grid,
            Some(&*cell),
            &tf,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            continue;
        };

        // Resolve arrival directions in the same authoritative inertial grid
        // used by the steady-state writer.
        if wants_radial {
            let arm = cam_orbit - target_orbit;
            if arm.length_squared() > 1.0 {
                (orbit.yaw, orbit.pitch) = orbit_angles_from_arm(arm);
                orbit.distance = arm.length();
                info!(
                    "ORBIT ARRIVAL: radial yaw={:.2} pitch={:.2} dist={:.3e}",
                    orbit.yaw, orbit.pitch, orbit.distance
                );
            }
            commands.entity(avatar_ent).remove::<RadialArrival>();
        } else if wants_sunlit {
            if let Some((sun_entity, _)) = q_bodies.iter().find(|(entity, body)| {
                body.ephemeris_id == lunco_celestial::ephemeris_id::SUN
                    && *entity != physical_target
            }) {
                if let Some((sun_orbit, _)) = lunco_core::coords::pose_in_grid(
                    sun_entity, orbit_grid, &q_parents, &q_grids, &q_spatial,
                ) {
                    let arm = sun_orbit - target_orbit;
                    if arm.length_squared() > 1.0 {
                        (orbit.yaw, orbit.pitch) = orbit_angles_from_arm(arm);
                        // Keep the terminator visible instead of arriving on
                        // the exact, visually flat full-body line.
                        orbit.yaw += 0.4;
                        info!(
                            "ORBIT ARRIVAL: sunlit yaw={:.2} pitch={:.2}",
                            orbit.yaw, orbit.pitch
                        );
                    }
                }
            }
            commands.entity(avatar_ent).remove::<SunlitArrival>();
        }

        let min_dist = if let Some(body) = body {
            body.radius_m + SCROLL_EXIT_ALTITUDE_M
        } else if let Ok(spacecraft) = q_sc.get(orbit.target) {
            (spacecraft.hit_radius_m as f64).max(10.0)
        } else {
            10.0
        };

        let current_len = cam_orbit.distance(target_orbit);

        let surface_exit = body.is_some()
            && orbital_pin.as_ref().is_some_and(|pin| {
                pin.active
                    && zoom.delta > 0.0
                    && orbit.distance <= min_dist * 1.0005
                    && current_len <= min_dist * 1.02
            });
        if surface_exit {
            zoom.delta = 0.0;
            commands.trigger(ReturnFromOrbit { target: avatar_ent });
            info!("ORBITAL SCROLL-THROUGH: exiting to surface at current pose");
            continue;
        }

        apply_scroll_zoom(
            &mut orbit.distance,
            &mut zoom.delta,
            ZOOM_SENSITIVITY,
            min_dist,
            1.0e11,
        );

        if let (Some(body), Some(pin)) = (body, orbital_pin.as_mut()) {
            let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
            let direction = rotation.mul_vec3(Vec3::Z).as_dvec3();
            let next_pin = lunco_celestial::OrbitalViewPin {
                active: true,
                body: body.ephemeris_id,
                dir: direction,
                distance: orbit.distance,
            };
            if **pin != next_pin {
                **pin = next_pin;
            }
        } else if let Some(pin) = orbital_pin.as_mut() {
            if pin.active {
                pin.active = false;
            }
        }

        // Yaw/pitch are local to the explicit star-fixed orbit grid. Nested
        // `LocalFloatingOrigin` propagation rebases every ancestor grid; the
        // camera's Transform therefore stays cell-local even at lunar/solar
        // distances.
        let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
        let desired_offset = rotation.mul_vec3(Vec3::Z).as_dvec3() * orbit.distance
            + DVec3::Y * orbit.vertical_offset as f64;
        let direction_orbit = desired_offset.normalize_or(DVec3::Z);
        let desired_len = desired_offset.length();

        // Rotation responds immediately. Only radial zoom distance is eased,
        // using the shared camera time constant.
        let final_len = if child_of.parent() != orbit_grid || current_len < 1e-3 {
            desired_len
        } else {
            let damping = orbit.damping.unwrap_or(defaults.damping);
            let alpha = (1.0 - (-defaults.position_rate * (1.0 - damping) * dt).exp()) as f64;
            let next = current_len + (desired_len - current_len) * alpha;
            if (next - desired_len).abs() <= desired_len * 1e-9 {
                desired_len
            } else {
                next
            }
        };
        let next_orbit = target_orbit + direction_orbit * final_len;
        let (new_cell, new_translation) = orbit_grid_ref.translation_to_grid(next_orbit);
        let next_transform = Transform::from_translation(new_translation).with_rotation(rotation);

        if child_of.parent() != orbit_grid {
            migrate_to_grid(
                &mut commands,
                avatar_ent,
                orbit_grid,
                new_cell,
                next_transform,
            );
        } else {
            if *cell != new_cell {
                *cell = new_cell;
            }
            if tf.translation != new_translation {
                tf.translation = new_translation;
            }
            if tf.rotation != rotation {
                tf.rotation = rotation;
            }
        }

        if *log_countdown == 0 {
            *log_countdown = 240;
            debug!(
                "ORBIT: arm {:.4e}→{:.4e} (cmd {:.3e}) cell=({},{},{}) target=({:.4e},{:.4e},{:.4e})",
                current_len,
                final_len,
                orbit.distance,
                new_cell.x,
                new_cell.y,
                new_cell.z,
                target_orbit.x,
                target_orbit.y,
                target_orbit.z,
            );
        }
        *log_countdown = log_countdown.saturating_sub(1);
    }
}
/// FreeFlightCamera system: moves the camera in absolute coordinates.
///
/// Only runs when `FreeFlightCamera` is present AND no `FrameBlend` is active.
/// Position is set by `apply_fly`. This system
/// applies yaw/pitch rotation from user input.
///
/// In surface mode, the rotation is built around the local gravity up vector
/// using sequential quaternion composition — guaranteed unit-length.
///
/// Note: `FreeFlightCamera` and `SurfaceCamera` are mutually exclusive.
/// The surface teleport removes `FreeFlightCamera`, so the surface-mode
/// branch here is effectively dead code. Kept for completeness.
fn freeflight_system(
    // `Without<OrbitCamera>`: the two are mutually exclusive camera modes. If an
    // avatar ever carries both (a stray insert), each writes `Transform` every
    // frame and they fight — the camera drifts and the view jitters. Make the
    // exclusion structural rather than relying on every insert site to strip the
    // other mode first.
    mut q_avatar: Query<
        (
            &mut Transform,
            &mut FreeFlightCamera,
            &CellCoord,
            &ChildOf,
            Option<&SurfaceRelativeMode>,
        ),
        (
            With<Avatar>,
            Without<FrameBlend>,
            Without<OrbitCamera>,
            Without<SpringArmCamera>,
            Without<SurfaceCamera>,
            Without<lunco_core::CinematicCameraLock>,
        ),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    gravity: Res<LocalGravityField>,
) {
    for (mut tf, mut ff, _cell, child_of, surface_mode) in q_avatar.iter_mut() {
        let rot = if surface_mode.is_some() {
            // In surface mode, apply yaw/pitch as incremental rotations.
            let axes = surface_axes_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial);
            let up_v = axes.map(|(_, _, up)| up).unwrap_or(Vec3::Y);
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

/// Free-flight scroll transit — the ENTRY half of the scroll loop (the exit
/// half is the ORBITAL SCROLL-THROUGH in `orbit_system`). On a site-anchored
/// celestial scene, the wheel DOLLIES the free-flight camera along its LOOK
/// direction with an exponential step scaled by altitude (approach slows near
/// the ground, retreat accelerates with height) — "scroll toward what you
/// look at". Once a scroll-OUT carries the camera past the orbital zoom
/// floor, the avatar hands over to the celestial `OrbitCamera` AT ITS
/// CURRENT POSE: [`RadialArrival`] derives the arm from the camera's present
/// position (no `SunlitArrival` re-aim), and because the handover altitude
/// equals the orbital floor the arm is already legal — no clamp jump, one
/// continuous gesture from ground to orbit. The descent mirrors it:
/// scroll-through at the floor releases back to free flight (pose parked in
/// the pin on this entry), where scroll-in keeps dollying down.
fn freeflight_scroll_transit_system(
    mut commands: Commands,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &ChildOf,
            &mut CameraZoomInput,
            Option<&Camera>,
            Option<&SurfaceCamera>,
            Option<&FreeFlightCamera>,
            Option<&GravityBody>,
            Has<SurfaceRelativeMode>,
            Has<lunco_core::CinematicCameraLock>,
        ),
        (
            With<Avatar>,
            Or<(With<FreeFlightCamera>, With<SurfaceCamera>)>,
            Without<OrbitCamera>,
            Without<SpringArmCamera>,
            Without<FrameBlend>,
        ),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_site: Query<&lunco_celestial::GeodeticAnchor, With<lunco_celestial::SiteAnchor>>,
    q_bodies: Query<(Entity, &CelestialBody)>,
) {
    // Only meaningful on a site-anchored scene whose solar hierarchy is up.
    let Some((body_ent, radius_m)) = site_body(&q_site, &q_bodies) else {
        return;
    };
    for (
        avatar_ent,
        mut tf,
        mut cell,
        child_of,
        mut zoom,
        cam,
        surface_camera,
        freeflight_camera,
        gravity_body,
        surface_relative,
        cinematic_lock,
    ) in q_avatar.iter_mut()
    {
        if cinematic_lock {
            continue;
        }
        if zoom.delta == 0.0 {
            continue;
        }
        // Only the active render camera transits (scenes carry inactive
        // Avatar-tagged spawn cameras — same guard as `on_focus_command`).
        if !cam.is_some_and(|c| c.is_active) {
            zoom.delta = 0.0;
            continue;
        }
        let Ok(grid) = q_grids.get(child_of.parent()) else {
            zoom.delta = 0.0;
            continue;
        };
        let Some((center, _)) = lunco_core::coords::pose_in_grid(
            body_ent,
            child_of.parent(),
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            zoom.delta = 0.0;
            continue;
        };
        let pos = grid.grid_position_double(&cell, &tf);
        let alt = (pos - center).length() - radius_m;
        // Same exponential the orbit arm uses (`apply_scroll_zoom`), applied
        // to the altitude scale: factor > 1 on scroll-out, < 1 on scroll-in.
        // Clamped to ±25% per FRAME: wheel events batch, and an accumulated
        // delta must never become a teleport-sized step.
        let factor = (-zoom.delta as f64 * ZOOM_SENSITIVITY as f64 * 0.01)
            .exp()
            .clamp(ZOOM_FACTOR_MIN, ZOOM_FACTOR_MAX);
        let scroll_out = zoom.delta < 0.0;
        zoom.delta = 0.0;
        // Signed dolly step: negative (forward) on scroll-in. The 50 m floor
        // keeps ground-level scrolling responsive; free flight is a ghost
        // camera, so overshooting into terrain is no worse than flying there.
        let step = alt.abs().max(50.0) * (factor - 1.0);
        let fwd = (tf.rotation * Vec3::NEG_Z).as_dvec3();
        let next = pos - fwd * step;
        let (new_cell, new_tf) = grid.translation_to_grid(next);
        *cell = new_cell;
        tf.translation = new_tf;

        // Past the orbital floor going OUT → hand over to the celestial
        // OrbitCamera. Same mode swap as `on_focus_command`, with ONE
        // deliberate difference: `RadialArrival` instead of `SunlitArrival`
        // (preserve the pose; `distance` below is a fallback that only
        // survives if the arrival cannot resolve).
        if scroll_out && (next - center).length() - radius_m > SCROLL_EXIT_ALTITUDE_M {
            let behavior = surface_camera
                .cloned()
                .map(OrbitReturnBehavior::Surface)
                .or_else(|| {
                    freeflight_camera
                        .cloned()
                        .map(OrbitReturnBehavior::FreeFlight)
                })
                .expect("surface scroll transit requires one camera behavior");
            commands
                .entity(avatar_ent)
                .try_insert(OrbitViewReturn {
                    parent_grid: child_of.parent(),
                    cell: *cell,
                    transform: *tf,
                    behavior,
                    gravity_body: gravity_body.copied(),
                    surface_relative,
                })
                .remove::<SpringArmCamera>()
                .remove::<FreeFlightCamera>()
                .remove::<FrameBlend>()
                .remove::<SurfaceCamera>()
                .remove::<SurfaceRelativeMode>()
                .remove::<GravityBody>()
                .try_insert(OrbitCamera {
                    target: body_ent,
                    distance: radius_m * 3.0,
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                    vertical_offset: 0.0,
                })
                .try_insert(RadialArrival);
            info!("SURFACE SCROLL-OUT: entering orbital view at current pose");
        }
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
    mut q_avatar: Query<
        (&mut Transform, &SurfaceCamera, &CellCoord, &ChildOf),
        (
            With<Avatar>,
            Without<FrameBlend>,
            Without<SpringArmCamera>,
            Without<FreeFlightCamera>,
            Without<OrbitCamera>,
            Without<lunco_core::CinematicCameraLock>,
        ),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    // The avatar transform is mutably borrowed above.  Grid ancestors are
    // ordinary scene entities, so make that ownership boundary explicit to
    // Bevy's query validator instead of relying on a runtime-only hierarchy
    // assumption.
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    gravity: Res<LocalGravityField>,
) {
    for (mut tf, cam, _cell, child_of) in q_avatar.iter_mut() {
        if q_grids.get(child_of.0).is_err() {
            continue;
        }

        let Some((east, north, up)) =
            surface_axes_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial)
        else {
            continue;
        };

        // Rebuild the rotation from the body's exact ENU frame each frame.
        // The camera remains upright relative to the curved surface while the
        // heading stays tied to the body's prime meridian.
        tf.rotation = surface_camera_rotation(east, north, up, cam.heading, cam.pitch);
    }
}

// ─── Locomotion ──────────────────────────────────────────────────────────────

/// Kinematic actuator for the avatar — the port-driven analog of a rover's
/// `apply_drive_mix`. Reads the avatar's FSW input ports (`forward`/`side`/`up`,
/// written through the shared `SetPorts` path by `drive_from_bindings`) and
/// translates the avatar entity in absolute coordinates. No forces (a free-fly
/// observer has no physics) — this is the whole "mechanism" for the avatar.
///
/// Only active with a `FreeFlightCamera`/`SurfaceCamera`, or when CTRL is held while
/// possessing a vessel (a momentary free-flight overlay). `Shift` boosts speed ×10.
/// Q/E elevation follows world up in free flight and gravity up in surface mode.
/// Runs in PostUpdate at render rate on wall-clock time, so the ghost camera
/// keeps moving even when the sim's virtual clock is paused/slowed.
fn apply_fly(
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &ChildOf,
            &lunco_core::InputPorts,
            Has<FreeFlightCamera>,
            Has<SurfaceCamera>,
            Option<&SurfaceRelativeMode>,
        ),
        (With<Avatar>, Without<lunco_core::CinematicCameraLock>),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    gravity: Res<LocalGravityField>,
    keys: Res<ButtonInput<KeyCode>>,
    // The INTERACTION clock (wall-rooted): the avatar keeps flying while the sim is
    // paused, because pausing the simulation is not supposed to paralyse the user. Runs
    // at render rate in `PostUpdate` — no lockstep needed, free-flight follows nothing.
    time: Res<Time>,
    mut commands: Commands,
) {
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let boost = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
        10.0
    } else {
        1.0
    };

    for (
        entity,
        mut tf,
        mut cell,
        child_of,
        inputs,
        has_freeflight,
        has_surface_camera,
        surface_mode,
    ) in q_avatar.iter_mut()
    {
        let Ok(grid) = q_grids.get(child_of.0) else {
            continue;
        };
        let current_pos = grid.grid_position_double(&cell, &tf);

        // Only move if we have a camera mode or CTRL-overlay.
        if !has_freeflight && !has_surface_camera && !ctrl_pressed {
            continue;
        }

        // Input values (each −1..=1 from the
        // `ControlBinding`), then boosted. When free (no ControllerLink)
        // `drive_from_bindings` writes these; while possessing they stay 0 (control is
        // redirected to the vessel).
        let forward = (inputs.cmd("forward") * boost) as f32;
        let side = (inputs.cmd("side") * boost) as f32;
        let elevation = (inputs.cmd("up") * boost) as f32;
        if forward.abs() < 0.01 && side.abs() < 0.01 && elevation.abs() < 0.01 {
            continue;
        }

        // Actively moving → cancel any idle auto-action.
        commands.entity(entity).remove::<lunco_core::ActiveAction>();

        // Q/E are vertical movement relative to the current world/surface, not
        // the camera's pitched up vector. A camera-relative elevation basis can
        // cancel W/S's horizontal component at a particular pitch (most visibly
        // Q+W), making a valid diagonal look stationary.
        let up_dir = if surface_mode.is_some() {
            gravity_up_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial)
        } else {
            Vec3::Y
        };
        let move_vec = fly_move_direction(&tf, forward, side, elevation, up_dir);

        // 23.1 m/s base fly speed × the real frame delta.
        let next_pos = current_pos + move_vec.as_dvec3() * 23.1 * time.delta_secs_f64();
        let (new_cell, new_tf) = grid.translation_to_grid(next_pos);
        *cell = new_cell;
        tf.translation = new_tf;
    }
}

// ─── Intent & Input ──────────────────────────────────────────────────────────

/// Captures the avatar's mouse **look** delta (and forwards zoom) into
/// `IntentAnalogState` for the camera behaviour systems.
///
/// Movement (forward/side/up) is NO LONGER read here: it now flows through the
/// shared port path (leafwing `ActionState` → `ControlBinding` → `SetPorts` →
/// FSW `forward`/`side`/`up` → `apply_fly`), exactly like a vessel. This system
/// keeps only the look axis, which stays mouse-direct until the P2 camera decouple.
fn capture_avatar_intent(
    mut q_avatar: Query<(Entity, &IntentState, &mut IntentAnalogState), With<Avatar>>,
    world: Option<Res<WorldTime>>,
    egui_focus: Res<lunco_core::EguiFocus>,
    waypoint_menu_open: Option<Res<lunco_core::WaypointMenuOpen>>,
    mut commands: Commands,
) {
    // Mouse look is a POINTER intent: suppress it while egui holds the pointer so
    // right-dragging over a panel doesn't orbit the scene. (Keyboard focus is
    // irrelevant to look — that gate guards movement/Cancel elsewhere.)
    //
    // A waypoint context menu counts too, and needs its own flag: `wants_pointer` only
    // goes true once the cursor is already ON the menu, so the camera would spin all
    // the way there and the menu could never be reached comfortably.
    let pointer_captured =
        egui_focus.wants_pointer || waypoint_menu_open.map(|m| m.0).unwrap_or(false);

    for (entity, intent_state, mut analog) in q_avatar.iter_mut() {
        let mut delta = Vec2::ZERO;
        let mut mouse_moved = false;
        if !pointer_captured {
            let d = intent_state.axis_pair(&UserIntent::Look);
            if d.length_squared() > 0.00001 {
                delta = d * 10.0;
                mouse_moved = true;
            }
        }

        analog.look_delta = delta;
        analog.timestamp = world.as_ref().map(|w| w.epoch_jd).unwrap_or_default();

        commands.entity(entity).trigger(|e| {
            let mut a = (*analog).clone();
            a.entity = e;
            a
        });

        // Look activity cancels an idle auto-action (movement does so in `apply_fly`,
        // zoom in `collect_camera_zoom`).
        if mouse_moved {
            commands.entity(entity).remove::<lunco_core::ActiveAction>();
        }
    }
}

/// Convert Bevy's accumulated mouse-scroll input to line units.
///
/// Bevy preserves the unit supplied by the OS/device. Leafwing's
/// `MouseScrollAxis` exposes only a scalar, so using that axis here loses the
/// distinction and makes pixel-mode touchpads produce enormous zoom deltas.
fn normalized_scroll_delta(scroll: &AccumulatedMouseScroll) -> f32 {
    match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / MouseScrollUnit::SCROLL_UNIT_CONVERSION_FACTOR,
    }
}

/// Mouse-wheel → per-avatar [`CameraZoomInput`], gated on egui pointer capture.
///
/// Camera zoom is presentation state, not a vessel control port. It consumes the
/// unit-preserving Bevy input at this boundary, then accumulates it per avatar for
/// the active camera behavior to consume + reset.
fn collect_camera_zoom(
    egui_focus: Res<lunco_core::EguiFocus>,
    scroll: Res<AccumulatedMouseScroll>,
    mut q_avatar: Query<(Entity, &mut CameraZoomInput), With<Avatar>>,
    mut commands: Commands,
) {
    if egui_focus.wants_pointer {
        return;
    }
    let d = normalized_scroll_delta(&scroll);
    if d.abs() <= f32::EPSILON {
        return;
    }
    for (entity, mut zoom) in q_avatar.iter_mut() {
        zoom.delta += d;
        commands.entity(entity).remove::<lunco_core::ActiveAction>();
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
    mut q_spring: Query<
        &mut SpringArmCamera,
        (With<Avatar>, Without<lunco_core::CinematicCameraLock>),
    >,
    mut q_orbit: Query<&mut OrbitCamera, (With<Avatar>, Without<lunco_core::CinematicCameraLock>)>,
    mut q_freeflight: Query<
        &mut FreeFlightCamera,
        (With<Avatar>, Without<lunco_core::CinematicCameraLock>),
    >,
    mut q_surface: Query<
        &mut SurfaceCamera,
        (With<Avatar>, Without<lunco_core::CinematicCameraLock>),
    >,
    mut q_tf: Query<
        (&mut Transform, &CellCoord, &ChildOf),
        (
            With<Avatar>,
            Without<FrameBlend>,
            Without<lunco_core::CinematicCameraLock>,
        ),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    settings: Res<CameraInputSettings>,
    keys: Res<ButtonInput<KeyCode>>,
    gravity: Res<LocalGravityField>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_children: Query<&Children>,
) {
    let Some((analog, surface_mode)) = q_avatar.iter().next() else {
        return;
    };
    let look_delta = analog.look_delta;
    if look_delta.length_squared() < 0.0001 {
        return;
    }

    let delta_yaw = -look_delta.x * settings.look_radians_per_pointer_unit;
    let delta_pitch = -look_delta.y * settings.look_radians_per_pointer_unit;
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    if ctrl_pressed {
        // Momentary free-flight: apply look deltas directly to Transform.
        if let Some((mut tf, _cell, child_of)) = q_tf.iter_mut().next() {
            if surface_mode.is_some() {
                let up_v =
                    surface_axes_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial)
                        .map(|(_, _, up)| up)
                        .unwrap_or(Vec3::Y);
                let yaw_q = Quat::from_axis_angle(up_v, delta_yaw);
                let right: Vec3 = *tf.right();
                let right_yawed = yaw_q.mul_vec3(right);
                let pitch_q = Quat::from_axis_angle(right_yawed, delta_pitch);
                tf.rotation = pitch_q * yaw_q * tf.rotation;
            } else {
                // Ecliptic: YXZ euler decomposition
                let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                tf.rotation = Quat::from_euler(
                    EulerRot::YXZ,
                    yaw + delta_yaw,
                    (pitch + delta_pitch).clamp(-1.5, 1.5),
                    0.0,
                );
            }
        }
    } else {
        // Normal mode: apply to the active behavior component.
        if let Some(mut arm) = q_spring.iter_mut().next() {
            (arm.yaw, arm.pitch) = look_angles(arm.yaw, arm.pitch, look_delta, &settings, 1.0);
        }
        if let Some(mut orbit) = q_orbit.iter_mut().next() {
            let physical_target = get_physical_body(orbit.target, &q_children, &q_bodies);
            let scale = q_bodies.get(physical_target).map_or(1.0, |(_, body)| {
                body_orbit_look_scale(orbit.distance, body.radius_m, &settings)
            }) as f32;
            (orbit.yaw, orbit.pitch) =
                look_angles(orbit.yaw, orbit.pitch, look_delta, &settings, scale as f32);
        }
        if let Some(mut ff) = q_freeflight.iter_mut().next() {
            (ff.yaw, ff.pitch) = look_angles(ff.yaw, ff.pitch, look_delta, &settings, 1.0);
        }
        if let Some(mut sc) = q_surface.iter_mut().next() {
            (sc.heading, sc.pitch) = look_angles(sc.heading, sc.pitch, look_delta, &settings, 1.0);
        }
    }
}

/// Apply one semantic Look intent to a camera's yaw/pitch state.
///
/// All pointer buttons are resolved by the controller into this intent before
/// reaching here. Keeping the angle conversion in one function makes the
/// right-button path identical for free-flight, orbit, spring-arm, and surface
/// cameras; no camera mode is allowed to reinterpret a raw mouse button.
fn look_angles(
    yaw: f32,
    pitch: f32,
    look_delta: Vec2,
    settings: &CameraInputSettings,
    scale: f32,
) -> (f32, f32) {
    let scale = scale.max(0.0);
    let yaw = yaw + -look_delta.x * settings.look_radians_per_pointer_unit * scale;
    let pitch =
        (pitch - look_delta.y * settings.look_radians_per_pointer_unit * scale).clamp(-1.5, 1.5);
    (yaw, pitch)
}

fn avatar_global_hotkeys(
    q_avatar: Query<&IntentState, With<Avatar>>,
    transport: Option<Res<TimeTransport>>,
    mut commands: Commands,
) {
    for intent_state in q_avatar.iter() {
        if intent_state.just_pressed(&UserIntent::Pause) {
            if let Some(transport) = transport.as_deref() {
                commands.trigger(SetTimeTransport {
                    playing: Some(matches!(transport.mode, TransportMode::Paused)),
                    ..default()
                });
            }
        }
    }
}

// ─── Raycasting ──────────────────────────────────────────────────────────────

/// Resolves a picked vehicle part to its nearest public input-port surface.
///
/// `SelectableRoot` is an editor boundary, and every independently simulated
/// wheel may carry it. [`lunco_core::InputPorts`] is the public interface:
/// its nonempty vocabulary is the input surface a session may own. A
/// [`lunco_core::ControlBinding`] is merely one optional avatar-input adapter.
/// Walking to this owner makes a click on any vehicle part possess its vehicle.
fn find_control_owner_from_hit(
    mut entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_input_ports: &Query<&lunco_core::InputPorts>,
    q_ground: &Query<Entity, With<lunco_core::Ground>>,
) -> Option<Entity> {
    for _ in 0..MAX_HIERARCHY_WALK_DEPTH {
        if q_ground.get(entity).is_ok() {
            return None;
        }
        if q_input_ports
            .get(entity)
            .is_ok_and(|surface| !surface.values.is_empty())
        {
            return Some(entity);
        }
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
/// two typed commands.
///
/// | Hit                         | Command          |
/// |-----------------------------|------------------|
/// | opened input-port surface   | `PossessVessel`  |
/// | `CelestialBody`             | `FocusTarget`    |
/// | everything else             | no action        |
///
/// Idempotency lives in each observer (no-op if state already matches).
/// `DragModeActive` blocks clicks while a transform gizmo is up so the user
/// can drag a handle without flipping the camera.
/// Whether a plain left-click may focus a **celestial body** (the analytic
/// hit-sphere branch of [`avatar_raycast_possession`]).
///
/// **OFF, deliberately — TODO: fix the occlusion test and turn this back on.**
///
/// # The bug this switches off
///
/// Standing on the surface at a site twin (summer-space-school), every click that
/// did not land on a rover flung the camera into lunar orbit. The body hit-sphere
/// is the Moon itself — radius 1737 km, centred below your feet — so a
/// surface camera's ray ALWAYS intersects it. The only thing that was ever meant to
/// stop that is the occlusion test above: `min_t` starts at `click.hit.depth` so the
/// ground shadows the sphere.
///
/// That test silently stopped working for DEM terrain. `min_t` falls back to
/// `f32::INFINITY` when `click.hit.position` is `None`, and a streamed terrain tile
/// can never produce a mesh hit: `stream_viz.rs` bakes LOD tile meshes with
/// `RenderAssetUsages::RENDER_WORLD` only ("picking rides the oracle"), so
/// `MeshPickingPlugin` has no CPU vertex data to hit-test. The ground is therefore
/// invisible to picking, `min_t` stays infinite, and the Moon wins every click —
/// exactly the leak the comment above documents for Earth, via a route it did not
/// anticipate.
///
/// # The real fix (why this is a switch and not a patch)
///
/// Occlusion must not depend on a mesh pick. The analytic spheres should be tested
/// against the terrain the same way every other placement tool already does — cast
/// the click ray at the surface oracle (`lunco_terrain_surface::GridSurfaceQuery::raycast`,
/// which `spawn.rs` and `checkpoint_click.rs` both use) and fold that distance into
/// `min_t` before the sphere loop. That fixes Earth-through-the-ground too, and stops
/// the behaviour depending on whether a terrain happens to be tile-streamed.
///
/// Doing it here means giving this observer a `GridSurfaceQuery`, which pulls
/// `lunco-terrain-surface` into `lunco-avatar`'s dependency set — a call the crate
/// boundary owner should make, not something to slip into a bug fix. Until then:
/// off. Focus is still reachable through the `FocusTarget` command and the
/// `focus_target` API/MCP verb; only the click gesture is suppressed.
const CELESTIAL_CLICK_FOCUS: bool = false;

pub fn avatar_raycast_possession(
    // Driven by bevy_picking: a global `On<Pointer<Click>>` observer. The
    // egui-vs-scene guard is `EguiFocus.wants_pointer` (via `scene_click_ray`) —
    // a global flag, fed by the workbench's egui-authoritative `pointer_over_scene`
    // signal, so a click on any real chrome is stood down here even though this
    // global observer can fire on a scene entity behind the panel.
    mut click: On<bevy::picking::events::Pointer<bevy::picking::events::Click>>,
    keys: Res<ButtonInput<KeyCode>>,
    camera_q: Query<(&Camera, &GlobalTransform, Entity, &IntentState), With<Avatar>>,
    egui_focus: Res<lunco_core::EguiFocus>,
    drag_mode_active: Res<lunco_core::DragModeActive>,
    spawn_tool_active: Res<lunco_core::SpawnToolActive>,
    terrain_tool_active: Res<lunco_core::TerrainToolActive>,
    waypoint_tool_active: Res<lunco_core::WaypointToolActive>,
    armed_script_tool: Res<lunco_core::ArmedScriptTool>,
    mut commands: Commands,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>,
    q_spacecraft: Query<(Entity, &GlobalTransform, &Spacecraft)>,
    q_input_ports: Query<&lunco_core::InputPorts>,
    q_parents: Query<&ChildOf>,
    q_ground: Query<Entity, With<lunco_core::Ground>>,
) {
    use bevy::picking::pointer::PointerButton;
    // Left button only.
    if click.button != PointerButton::Primary {
        return;
    }
    // Shift+click is reserved for entity selection / gizmo multi-select in
    // lunco-luncosim-edit (`on_scene_click_select`, the other global
    // `Pointer<Click>` observer). A plain left-click possesses/follows/focuses;
    // a Shift+click never does. This modifier split is what keeps the two
    // observers from both acting on a single click.
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
        return;
    }
    // Waypoint placement is a semantic intent, not an Alt-key convention. The
    // bundled map currently binds Alt, but any user-authored rebind must reserve
    // the click from possession in exactly the same way.
    let Some((camera, cam_gtf, avatar_entity, intents)) = camera_q.iter().next() else {
        return;
    };
    if intents.pressed(&UserIntent::PlaceWaypoint) {
        return;
    }
    // Ctrl-click appends a patrol checkpoint (`on_scene_click_checkpoint`, the
    // third global `Pointer<Click>` observer). Both observers see the same click
    // — `propagate(false)` stops bubbling, not sibling observers — so without
    // this guard every checkpoint placement would ALSO possess/follow whatever
    // the ray hit, yanking the camera onto the terrain.
    if keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]) {
        return;
    }
    // Mid-drag on a transform gizmo: don't flip the camera under the user.
    if drag_mode_active.active {
        return;
    }
    // Spawn placement tool armed: clicks place objects, don't possess.
    if spawn_tool_active.0 {
        return;
    }
    // Terrain brush armed: clicks sculpt the terrain, don't possess.
    if terrain_tool_active.0 {
        return;
    }
    // Waypoint Move/Insert armed: that click places the waypoint, don't possess.
    if waypoint_tool_active.0 {
        return;
    }
    // A script tool is armed: that click belongs to the tool, don't possess.
    if armed_script_tool.armed() {
        return;
    }

    // This observer handles the plain click now (it passed every guard above), so
    // stop the auto-propagation to ancestor entities — otherwise a global
    // observer re-fires once per ancestor. The analytic spacecraft/celestial
    // sphere tests below depend on the ray, not on `click.entity`, so they'd
    // re-trigger `PossessVessel`/`FocusTarget` for every ancestor in the chain
    // (we must not gate this on a *mesh* hit being found, the earlier bug).
    click.propagate(false);

    // Shared egui-vs-scene guard + camera ray (replaces the old
    // `hit.position.is_none()` chrome check). Returns `None` on an egui-chrome
    // click; the ray drives the analytic hit-sphere tests (celestial bodies /
    // spacecraft, which have no pickable mesh) alongside the mesh pick.
    let Some(ray) = lunco_core::scene_click_ray(
        &egui_focus,
        camera,
        cam_gtf,
        click.pointer_location.position,
    ) else {
        return;
    };

    // The mesh the pick resolved to (rover, prop, ground, …). `hit.depth` is
    // the along-ray distance to compare against the analytic spheres below.
    // Depth is recorded for ANY real mesh hit, clickable or not. Occlusion is a
    // geometric fact, not a property of being click-targetable: the terrain has no
    // `SelectableRoot`, but it is still solid, and a click on it must still shadow
    // the analytic spheres below. Coupling the two (recording `depth` only when a
    // root was found) left `min_t = INFINITY` on every ground click, so the Earth
    // hit-sphere — which a camera standing on the surface ALWAYS intersects —
    // passed `t < min_t` and the click "leaked" through the ground into a
    // `FocusTarget` on the planet.
    let mut min_t = if click.hit.position.is_some() {
        click.hit.depth
    } else {
        f32::INFINITY
    };

    let control_target =
        find_control_owner_from_hit(click.entity, &q_parents, &q_input_ports, &q_ground);

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
                spacecraft_hit = Some(entity);
            }
        }
    }

    // Celestial bodies — focus only (orbit-distance scale).
    //
    // TEMPORARILY DISABLED. See `CELESTIAL_CLICK_FOCUS`.
    let mut body_hit: Option<Entity> = None;
    if CELESTIAL_CLICK_FOCUS {
        for (entity, gtf, body) in q_bodies.iter() {
            let oc = ray.origin - gtf.translation();
            let b = oc.dot(ray.direction.as_vec3());
            let c = oc.dot(oc) - (body.radius_m as f32).powi(2);
            let discr = b * b - c;
            if discr >= 0.0 {
                let t = -b - discr.sqrt();
                if t > 0.0 && t < min_t {
                    min_t = t;
                    spacecraft_hit = None;
                    body_hit = Some(entity);
                }
            }
        }
    }

    if let Some(target) = body_hit {
        commands.trigger(FocusTarget {
            avatar: Some(avatar_entity),
            target,
        });
    } else if let Some(target) = spacecraft_hit {
        commands.trigger(PossessVessel {
            avatar: Some(avatar_entity),
            target,
            bind_camera: true,
        });
    } else if let Some(target) = control_target {
        commands.trigger(PossessVessel {
            avatar: Some(avatar_entity),
            target,
            bind_camera: true,
        });
    }
}

/// The `Cancel` intent (default `Backspace`) releases possession, plain follow
/// **and** body-orbit focus — all unwind through the same `ReleaseVessel` path
/// (which strips ControllerLink, SpringArm, OrbitCamera, interpolation, and
/// reinstates a free-flight camera).
///
/// Reads the intent (not the raw key) so it flows through the shared
/// `UserIntent` vocabulary; the system is `run_if(scene_keyboard_active)` gated so
/// a `Backspace` typed into a focused egui field edits text instead (the two-step
/// Esc/defocus pattern).
fn avatar_escape_possession(
    q_avatar: Query<
        (Entity, &IntentState),
        (
            With<Avatar>,
            Or<(
                With<ControllerLink>,
                With<SpringArmCamera>,
                With<OrbitCamera>,
            )>,
        ),
    >,
    cursor_mode: lunco_core::CursorModeActive,
    mut commands: Commands,
) {
    // `Cancel` unwinds the INNERMOST mode first. While ANY cursor-driven mode owns the
    // pointer — a waypoint placement/menu, the spawn ghost, the terrain brush — Cancel
    // belongs to that mode, not to possession: releasing the vessel out from under the
    // user as well would be two undos for one keypress. With nothing up, Cancel means
    // what it always did and releases the vessel. Same gate family the click handlers
    // already honour, so keyboard and mouse agree on who owns the interaction.
    if cursor_mode.any() {
        return;
    }
    for (entity, intent) in q_avatar.iter() {
        if intent.just_pressed(&UserIntent::Cancel) {
            commands.trigger(ReleaseVessel { target: entity });
        }
    }
}

// ─── Commands ────────────────────────────────────────────────────────────────

/// Install the behavior and frame-owned components captured by one
/// [`OrbitViewReturn`] transaction. Control authority is intentionally not
/// touched here; returning from a view and releasing a vessel are separate
/// domain actions.
fn apply_orbit_return(commands: &mut Commands, avatar: Entity, state: &OrbitViewReturn) {
    let mut entity = commands.entity(avatar);
    entity
        .remove::<SpringArmCamera>()
        .remove::<OrbitCamera>()
        .remove::<FreeFlightCamera>()
        .remove::<SurfaceCamera>()
        .remove::<OrbitViewReturn>()
        .remove::<SunlitArrival>()
        .remove::<RadialArrival>()
        .remove::<FrameBlend>();

    match &state.behavior {
        OrbitReturnBehavior::SpringArm(spring_arm) => {
            entity.try_insert(spring_arm.clone());
        }
        OrbitReturnBehavior::Surface(surface) => {
            entity.try_insert(surface.clone());
        }
        OrbitReturnBehavior::FreeFlight(freeflight) => {
            entity.try_insert(freeflight.clone());
        }
    }
    if let Some(gravity_body) = state.gravity_body {
        entity.try_insert(gravity_body);
    } else {
        entity.remove::<GravityBody>();
    }
    if state.surface_relative {
        entity.try_insert(SurfaceRelativeMode);
    } else {
        entity.remove::<SurfaceRelativeMode>();
    }
}

/// Return from an orbital presentation view without changing possession.
///
/// The pre-orbit parent grid, cell and local pose are authoritative. Restoring
/// those values directly avoids a root-frame round trip and therefore cannot
/// lose precision or infer the wrong body-fixed orientation.
#[on_command(ReturnFromOrbit)]
fn on_return_from_orbit(
    trigger: On<ReturnFromOrbit>,
    mut commands: Commands,
    mut q_avatar: Query<
        (
            &mut Transform,
            &mut CellCoord,
            &ChildOf,
            &OrbitViewReturn,
            Has<lunco_core::CinematicCameraLock>,
        ),
        With<Avatar>,
    >,
    mut orbital_pin: Option<ResMut<lunco_celestial::OrbitalViewPin>>,
) {
    let avatar = trigger.event().target;
    let Ok((mut transform, mut cell, child_of, return_state, cinematic_lock)) =
        q_avatar.get_mut(avatar)
    else {
        return;
    };
    if cinematic_lock {
        return;
    }
    let return_state = return_state.clone();

    if child_of.parent() == return_state.parent_grid {
        *cell = return_state.cell;
        *transform = return_state.transform;
    } else {
        migrate_to_grid(
            &mut commands,
            avatar,
            return_state.parent_grid,
            return_state.cell,
            return_state.transform,
        );
    }
    apply_orbit_return(&mut commands, avatar, &return_state);

    if let Some(pin) = orbital_pin.as_mut() {
        pin.active = false;
    }
    commands.trigger(lunco_core::RequestLocalAvatarView);
    info!("ORBITAL EXIT: restored exact pre-orbit camera transaction");
}

/// Releases possession of a vessel.
///
/// Keeps the camera at its current position — no jarring teleport.
/// Switches to `FreeFlightCamera` mode with the current orientation preserved.
#[on_command(ReleaseVessel)]
fn on_release_command(
    trigger: On<ReleaseVessel>,
    mut commands: Commands,
    mut q_avatar: Query<
        (
            &mut Transform,
            &mut CellCoord,
            Option<&ControllerLink>,
            Option<&SurfaceRelativeMode>,
            &ChildOf,
            Option<&OrbitViewReturn>,
            Has<lunco_core::CinematicCameraLock>,
        ),
        With<Avatar>,
    >,
    guard: Res<lunco_core::SyncApplyGuard>,
    mut orbital_pin: Option<ResMut<lunco_celestial::OrbitalViewPin>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    gravity: Res<LocalGravityField>,
    mut authority: Option<ResMut<lunco_core::markers::FlightAuthority>>,
) {
    // The stick goes back to the guidance law — publish it for the UI that
    // shows WHO is flying (the overlay's AUTO/MANUAL badge).
    if let Some(a) = authority.as_mut() {
        a.piloted = false;
    }
    // A wire-applied release (a client telling the host it let go) carries that
    // client's avatar, which is meaningless here — the host frees ownership in
    // `release_possession_authority`, not by touching a local camera.
    if guard.is_from_sync() {
        return;
    }
    // Orbital presentation state is global, but the exact return frame belongs
    // to the avatar and is restored below from `OrbitViewReturn`.
    if let Some(pin) = orbital_pin.as_mut() {
        if pin.active {
            pin.active = false;
        }
    }
    let cmd = trigger.event();
    let avatar_ent = cmd.target;
    let (yaw, pitch, opt_vessel, is_surface, local_translation, return_state) =
        if let Ok((mut tf, mut cell, link, surface, child_of, return_state, cinematic_lock)) =
            q_avatar.get_mut(avatar_ent)
        {
            let opt_vessel = link.map(|link| link.vessel_entity);
            if cinematic_lock {
                if let Some(vessel_entity) = opt_vessel {
                    commands.trigger(lunco_cosim::SetPorts {
                        target: vessel_entity,
                        writes: vec![
                            ("throttle".into(), 0.0),
                            ("steer".into(), 0.0),
                            ("brake".into(), 1.0),
                        ],
                        seq: 0,
                        tick: 0,
                    });
                }
                commands.entity(avatar_ent).remove::<ControllerLink>();
                return;
            }
            let return_state = return_state.cloned();
            if let Some(state) = &return_state {
                if child_of.parent() == state.parent_grid {
                    *cell = state.cell;
                    *tf = state.transform;
                } else {
                    migrate_to_grid(
                        &mut commands,
                        avatar_ent,
                        state.parent_grid,
                        state.cell,
                        state.transform,
                    );
                }
            }
            let rot = return_state
                .as_ref()
                .map(|state| state.transform.rotation)
                .unwrap_or(tf.rotation);
            let returning_surface = return_state
                .as_ref()
                .is_some_and(|state| matches!(state.behavior, OrbitReturnBehavior::Surface(_)));
            let (y, p) = if surface.is_some() {
                let axes =
                    surface_axes_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial);
                axes.map(|(east, north, up)| surface_camera_angles(east, north, up, rot))
                    .unwrap_or_else(|| {
                        let (y, p, _) = rot.to_euler(EulerRot::YXZ);
                        (y, p)
                    })
            } else {
                let (y, p, _) = rot.to_euler(EulerRot::YXZ);
                (y, p)
            };
            (
                y,
                p,
                opt_vessel,
                returning_surface || surface.is_some(),
                return_state
                    .as_ref()
                    .map(|state| state.transform.translation)
                    .unwrap_or(tf.translation),
                return_state,
            )
        } else {
            (0.0, 0.0, None, false, Vec3::ZERO, None)
        };

    // Hard stop the rover upon disengaging control: zero throttle/steer, full brake.
    if let Some(vessel_entity) = opt_vessel {
        commands.trigger(lunco_cosim::SetPorts {
            target: vessel_entity,
            writes: vec![
                ("throttle".into(), 0.0),
                ("steer".into(), 0.0),
                ("brake".into(), 1.0),
            ],
            seq: 0,
            tick: 0,
        });
    }

    // Dropping the `ControllerLink` stops `drive_from_bindings` (the vessel keeps
    // its own `ControlBinding` for the next possession).
    commands
        .entity(avatar_ent)
        .remove::<ControllerLink>()
        .remove::<SpringArmCamera>()
        .remove::<OrbitCamera>()
        // Release is a mode transition. Clear both stepped camera modes before
        // installing the one selected below; otherwise a stale mode can
        // survive the transition and make both mode systems exclude each
        // other from their queries.
        .remove::<FreeFlightCamera>()
        .remove::<SurfaceCamera>()
        .remove::<OrbitViewReturn>()
        .remove::<SunlitArrival>()
        .remove::<RadialArrival>()
        .remove::<FrameBlend>();

    if let Some(state) = return_state {
        let mut entity = commands.entity(avatar_ent);
        match state.behavior {
            OrbitReturnBehavior::SpringArm(_) => {
                let (yaw, pitch, _) = state.transform.rotation.to_euler(EulerRot::YXZ);
                entity.try_insert(FreeFlightCamera {
                    yaw,
                    pitch,
                    damping: None,
                });
            }
            OrbitReturnBehavior::Surface(surface) => {
                entity.try_insert(surface);
            }
            OrbitReturnBehavior::FreeFlight(freeflight) => {
                entity.try_insert(freeflight);
            }
        }
        if let Some(gravity_body) = state.gravity_body {
            entity.try_insert(gravity_body);
        } else {
            entity.remove::<GravityBody>();
        }
        if state.surface_relative {
            entity.try_insert(SurfaceRelativeMode);
        } else {
            entity.remove::<SurfaceRelativeMode>();
        }
    // In surface mode, use SurfaceCamera (recomputed from scratch each frame);
    // otherwise use FreeFlightCamera (incremental euler angles).
    } else if is_surface {
        commands.entity(avatar_ent).try_insert(SurfaceCamera {
            heading: yaw,
            pitch,
        });
    } else {
        commands.entity(avatar_ent).try_insert(FreeFlightCamera {
            yaw,
            pitch,
            damping: None,
        });
    }
    // Give the viewport back to the player's own eye through the shared camera
    // intent. The camera subsystem resolves the LocalAvatar and records this as
    // an explicit user selection; it never falls through to another camera.
    commands.trigger(lunco_core::RequestLocalAvatarView);
    info!(
        "Released possession → camera at local {:?} (surface={})",
        local_translation, is_surface
    );
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
#[on_command(PossessVessel)]
fn on_possess_command(
    trigger: On<PossessVessel>,
    mut commands: Commands,
    q_avatar: Query<
        (
            Entity,
            &Transform,
            &ChildOf,
            Option<&ControllerLink>,
            Has<lunco_core::CinematicCameraLock>,
        ),
        With<Avatar>,
    >,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    // Used ONLY for the heading-follow camera decision below. Possession is
    // gated by the target's public input ports, then authority.
    q_vessel: Query<Entity, Controllable>,
    q_vessel_gravity: Query<&GravityBody>,
    q_follow: Query<&lunco_core::CameraFollow>,
    q_input_ports: Query<&lunco_core::InputPorts>,
    guard: Res<lunco_core::SyncApplyGuard>,
    registry: Res<lunco_core::SessionRegistry>,
    rbac: Res<lunco_core::session::SessionRbac>,
    session: Res<lunco_core::LocalSession>,
    q_owned: Query<&lunco_core::GlobalEntityId>,
    mut authority: Option<ResMut<lunco_core::markers::FlightAuthority>>,
) {
    let cmd = trigger.event();
    if !q_input_ports
        .get(cmd.target)
        .is_ok_and(|surface| !surface.values.is_empty())
    {
        warn!(target = ?cmd.target, "[possess] refused: target exposes no writable input ports");
        return;
    }
    // A *remote* possession applied from the wire (host attributing a client's
    // claim) must NOT bind a local camera — the host has no camera for that
    // player. Authority is recorded separately by `record_possession_authority`;
    // here we only do the local camera-bind for our own (non-wire) possessions.
    if guard.is_from_sync() {
        return;
    }
    // A session has the stick — publish it for the AUTO/MANUAL badge. Set after
    // the sync guard so a remote player's possession does not relabel OUR view.
    if let Some(a) = authority.as_mut() {
        a.piloted = true;
    }
    // Possession arbitration — ONE predicate, shared with the authority leg
    // (`record_possession_authority`). `may_control` = `may_possess` (free / already
    // ours / `LastWins`) OR an authored takeover of another session's vessel. This
    // used to call `may_possess` alone, which does NOT know about takeover: it agreed
    // with the authority leg only because that leg is registered first and had already
    // rewritten the table. Asking the same question both legs ask removes the ordering
    // assumption — otherwise a granted takeover could still be refused a camera here,
    // logging "session N possesses entity" over an empty cockpit.
    if let Ok(gid) = q_owned.get(cmd.target) {
        if !lunco_core::session::may_control(&registry, &rbac, session.0, gid.get()) {
            info!(
                "[possess] vessel {} owned by another session — refused (policy)",
                gid.get()
            );
            return;
        }
    }
    // Resolve the avatar to bind the camera to: the command's avatar if it
    // names a live one, else any local avatar. With no avatar at all (headless /
    // direct control) there is nothing to bind — the authority claim already
    // ran in `record_possession_authority`, so just skip the camera work.
    let resolved = cmd
        .avatar
        .and_then(|a| q_avatar.get(a).ok())
        .or_else(|| q_avatar.iter().next());
    let Some((avatar_ent, cam_tf, _child_of, existing_link, cinematic_lock)) = resolved else {
        return;
    };

    // Idempotent: already controlling this exact target — no-op.
    if let Some(link) = existing_link {
        if link.vessel_entity == cmd.target {
            return;
        }
    }

    // Camera-less possession. The authority claim — what actually flips the
    // vessel's `piloted` gate — already ran in `record_possession_authority`,
    // and the caller has declared the VIEW is not ours to touch: a recording
    // scenario drives the vessel through ports while an authored camera path
    // owns the camera, and the chase-camera bind below would land on that very
    // path-driven avatar and steal the shot. The `ControllerLink` is NOT camera
    // work though — it is the control/telemetry binding consumed by the generic
    // exposure publisher and runtime HUD — so it is still bound before bailing.
    if !cmd.bind_camera {
        commands.entity(avatar_ent).try_insert(ControllerLink {
            vessel_entity: cmd.target,
        });
        return;
    }

    // A possession command may still establish control, but it cannot replace
    // the pose owner of a cinematic camera. The caller must explicitly release
    // the authored camera path before requesting an interactive camera bind.
    if cinematic_lock {
        commands.entity(avatar_ent).try_insert(ControllerLink {
            vessel_entity: cmd.target,
        });
        return;
    }

    let target_grid = get_grid_for_entity(cmd.target, &q_parents, &q_grids);
    let Some(target_grid_entity) = target_grid else {
        warn!(target = ?cmd.target, "[possess] refused: target has no live Grid frame");
        return;
    };
    let Some((target_local_pos, target_local_rotation)) = lunco_core::coords::grid_relative_pose(
        cmd.target,
        target_grid_entity,
        &q_parents,
        &q_grids,
        &q_spatial,
    ) else {
        warn!(
            target = ?cmd.target,
            target_grid = ?target_grid_entity,
            "[possess] refused: target is not spatially reachable from its Grid"
        );
        return;
    };

    // Camera-follow mode is authored on the vessel's control profile
    // (`lunco_core::CameraFollow`) — that, not any hardcoded marker, decides
    // whether the camera tracks the body's attitude. `Heading` follows yaw only
    // (surface vehicles); `Orbit` keeps a stable external frame a 6-DOF flyer
    // rotates inside of; `Chase` copies full orientation. A vessel with no
    // authored mode (or no control profile) defaults to `Heading`.
    use lunco_core::CameraFollow;
    let follow = q_follow.get(cmd.target).copied().unwrap_or_default();

    // Per-mode framing. Orbit sits well out (whole vehicle in view); the
    // body-relative modes ride close behind.
    let (end_distance, end_vert_off) = match follow {
        CameraFollow::Orbit => (50.0, 0.0),
        CameraFollow::Chase => (25.0, 3.0),
        CameraFollow::Heading => (15.0, 2.0),
    };
    let end_yaw = 0.0;
    let end_pitch = -0.25;

    let surface_frame = q_vessel_gravity.get(cmd.target).ok().and_then(|gb| {
        surface_target_frame(
            target_local_pos,
            target_local_rotation,
            target_grid_entity,
            gb.body_entity,
            &q_parents,
            &q_grids,
            &q_spatial,
        )
    });

    // Snap to vessel immediately. Orbit/Chase preserve the current look angles so
    // possession doesn't jerk the view; Heading adopts the fixed rover start pose.
    let (current_yaw, current_pitch, _) = cam_tf.rotation.to_euler(EulerRot::YXZ);
    let (init_yaw, init_pitch) = match follow {
        CameraFollow::Heading => (end_yaw, end_pitch),
        _ => (current_yaw, current_pitch),
    };
    let final_rot = if matches!(follow, CameraFollow::Heading) {
        if let Some((east, north, up, target_heading)) = surface_frame {
            surface_camera_rotation(east, north, up, target_heading + init_yaw, init_pitch)
        } else {
            Quat::from_euler(EulerRot::YXZ, init_yaw, init_pitch, 0.0)
        }
    } else {
        Quat::from_euler(EulerRot::YXZ, init_yaw, init_pitch, 0.0)
    };
    let final_offset = final_rot.mul_vec3(Vec3::Z).as_dvec3() * end_distance;
    let final_local_pos = target_local_pos
        + final_offset
        + surface_frame
            .map(|(_, _, up, _)| up.as_dvec3())
            .unwrap_or(DVec3::Y)
            * end_vert_off as f64;

    info!(
        avatar = ?avatar_ent,
        target = ?cmd.target,
        target_local = ?target_local_pos,
        target_grid = ?target_grid,
        camera_local = ?final_local_pos,
        "[possess] resolved click target"
    );

    // Migrate to target grid immediately
    migrate_avatar_to_target_grid(
        &mut commands,
        avatar_ent,
        target_grid,
        final_local_pos,
        final_rot,
        &q_grids,
    );

    // The controller link goes on the **avatar** (it carries the shared
    // `ActionState<UserIntent>` that `drive_from_bindings` reads); the intent→port
    // `ControlBinding` lives on the **vessel** as its own property, authored purely
    // from USD (a `Controls` child scope referencing a shared profile in
    // `control_profiles.usda`). There is NO Rust topology default: a vessel is
    // drivable iff its USD carries that scope. `drive_from_bindings` reads the
    // binding off the vessel and skips any vessel that has none, so possession is a
    // pure camera+link bind here.
    commands.entity(avatar_ent).try_insert(ControllerLink {
        vessel_entity: cmd.target,
    });

    // Detect if target is a surface vehicle (has GravityBody) and propagate surface mode.
    let is_surface_vehicle = q_vessel_gravity.get(cmd.target).is_ok();

    // One follow solver serves the vessel follow modes. The spring arm owns the
    // render-rate chase pose; stepped camera modes own their pose through the
    // interaction schedule and `InteractionEased`. They differ only in derived
    // attitude. `OrbitCamera` is NOT used here — it is the celestial orbital-view
    // solver; reusing it for a fast-flying vessel was the source of the old
    // frame-stale target sampling jitter. Strip the celestial orbit component in
    // case a prior focus left it on the avatar.
    use lunco_core::CameraFollow as CF;
    let (attitude, track_heading, damping) = match follow {
        // Stable external frame: track position, keep world up, ignore attitude.
        // The right frame for a lander that pitches/rolls — the craft tumbles
        // inside a steady view instead of dragging the camera with it.
        CF::Orbit => (FollowAttitude::WorldLocked, false, None),
        // Full-attitude follow (yaw+pitch+roll) — a cockpit frame that rolls with
        // the craft. Opt-in; the camera intentionally DOES track the body.
        CF::Chase => (FollowAttitude::FullAttitude, false, Some(0.1)),
        // Heading-follow: yaw only, surface-normal up. Ground vehicles. Only
        // steerable vessels have a meaningful heading; a ball/prop tumbles, so
        // track user yaw only there.
        CF::Heading => (
            FollowAttitude::Heading,
            q_vessel.contains(cmd.target),
            Some(0.05),
        ),
    };
    let mut cmd_ent = commands.entity(avatar_ent);
    cmd_ent
        .remove::<OrbitCamera>()
        .try_insert((SpringArmCamera {
            target: cmd.target,
            distance: end_distance,
            yaw: init_yaw,
            pitch: init_pitch,
            damping,
            vertical_offset: end_vert_off,
            track_heading,
            attitude,
        },));
    // Surface-relative up only makes sense for Heading-follow ground vehicles;
    // the flyer frames (Orbit/Chase) keep world/body up. Strip it otherwise so a
    // prior possession's surface mode doesn't leak in.
    if matches!(follow, CF::Heading) && is_surface_vehicle {
        if let Ok(gb) = q_vessel_gravity.get(cmd.target) {
            cmd_ent.try_insert(*gb);
        }
        cmd_ent.try_insert(SurfaceRelativeMode);
    } else {
        cmd_ent.remove::<SurfaceRelativeMode>();
    }

    commands
        .entity(avatar_ent)
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
#[on_command(FollowTarget)]
fn on_follow_command(
    trigger: On<FollowTarget>,
    mut commands: Commands,
    q_avatar: Query<
        (
            Entity,
            &ChildOf,
            Option<&SpringArmCamera>,
            Has<lunco_core::CinematicCameraLock>,
        ),
        With<Avatar>,
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_vessel: Query<Entity, Controllable>,
    q_vessel_gravity: Query<&GravityBody>,
) {
    let cmd = trigger.event();
    let resolved = cmd
        .avatar
        .and_then(|a| q_avatar.get(a).ok())
        .or_else(|| q_avatar.iter().next());
    let Some((avatar_ent, _child_of, existing_spring, cinematic_lock)) = resolved else {
        return;
    };

    if cinematic_lock {
        return;
    }

    // Idempotent: already following this target — no-op.
    if let Some(arm) = existing_spring {
        if arm.target == cmd.target {
            return;
        }
    }

    let target_grid = get_grid_for_entity(cmd.target, &q_parents, &q_grids);
    let Some(target_grid_entity) = target_grid else {
        warn!(target = ?cmd.target, "[follow] refused: target has no live Grid frame");
        return;
    };
    let Some((target_local_pos, target_local_rotation)) = lunco_core::coords::grid_relative_pose(
        cmd.target,
        target_grid_entity,
        &q_parents,
        &q_grids,
        &q_spatial,
    ) else {
        warn!(
            target = ?cmd.target,
            target_grid = ?target_grid_entity,
            "[follow] refused: target is not spatially reachable from its Grid"
        );
        return;
    };
    let end_distance = 15.0_f64;
    let end_vert_off = 2.0_f32;
    let end_pitch = -0.25_f32;

    let surface_frame = q_vessel_gravity.get(cmd.target).ok().and_then(|gb| {
        surface_target_frame(
            target_local_pos,
            target_local_rotation,
            target_grid_entity,
            gb.body_entity,
            &q_parents,
            &q_grids,
            &q_spatial,
        )
    });

    // Snap behind the target with a default chase pose.
    let final_rot = surface_frame
        .map(|(east, north, up, target_heading)| {
            surface_camera_rotation(east, north, up, target_heading, end_pitch)
        })
        .unwrap_or_else(|| Quat::from_euler(EulerRot::YXZ, 0.0, end_pitch, 0.0));
    let final_offset = final_rot.mul_vec3(Vec3::Z).as_dvec3() * end_distance;
    let final_local_pos = target_local_pos
        + final_offset
        + surface_frame
            .map(|(_, _, up, _)| up.as_dvec3())
            .unwrap_or(DVec3::Y)
            * end_vert_off as f64;

    migrate_avatar_to_target_grid(
        &mut commands,
        avatar_ent,
        target_grid,
        final_local_pos,
        final_rot,
        &q_grids,
    );

    // Drop the controller link — follow ≠ possess (the vessel keeps its own
    // `ControlBinding`).
    let mut cmd_ent = commands.entity(avatar_ent);
    cmd_ent
        .remove::<ControllerLink>()
        .remove::<FreeFlightCamera>()
        .remove::<SurfaceCamera>()
        .remove::<OrbitCamera>()
        .remove::<FrameBlend>()
        .try_insert((SpringArmCamera {
            target: cmd.target,
            distance: end_distance,
            yaw: 0.0,
            pitch: end_pitch,
            damping: Some(0.05),
            vertical_offset: end_vert_off,
            // Followed props (balloons, balls) tumble — heading is user-only.
            track_heading: q_vessel.contains(cmd.target),
            // Follow (no possession) rides behind like a rover chase: a
            // heading frame with world/surface up, not a body-locked cockpit.
            attitude: FollowAttitude::Heading,
        },));

    // Surface-relative mode if following a body on a gravity well.
    if let Ok(gb) = q_vessel_gravity.get(cmd.target) {
        cmd_ent.try_insert(*gb).try_insert(SurfaceRelativeMode);
    } else {
        cmd_ent.remove::<SurfaceRelativeMode>();
    }
}

/// Focuses on a target with an instant transition to OrbitCamera mode.
///
/// Intent-only: this observer picks the orbit *parameters* (target, distance,
/// arrival yaw/pitch) and swaps the behavior component. All spatial placement
/// — explicit inertial-grid selection, cell split and position easing — is owned by
/// `orbit_system`, which runs at a fixed schedule point on frame-consistent
/// transforms. (An earlier version teleported the avatar here through
/// `world_position_seeded`, which drops the site-anchored solar grids'
/// rotations — landing the camera on a phantom point.)
#[on_command(FocusTarget)]
fn on_focus_command(
    trigger: On<FocusTarget>,
    mut commands: Commands,
    q_avatar: Query<
        (
            Entity,
            &Transform,
            &CellCoord,
            &ChildOf,
            Option<&Camera>,
            Option<&OrbitCamera>,
            Option<&OrbitViewReturn>,
            Option<&SpringArmCamera>,
            Option<&SurfaceCamera>,
            Option<&FreeFlightCamera>,
            Option<&GravityBody>,
            Has<SurfaceRelativeMode>,
            Has<lunco_core::CinematicCameraLock>,
        ),
        With<Avatar>,
    >,
    q_bodies: Query<&CelestialBody>,
    q_body_decls: Query<&lunco_celestial::CelestialBodyDecl>,
    q_body_entities: Query<(Entity, &CelestialBody)>,
    q_sc: Query<&Spacecraft>,
    q_children: Query<&Children>,
) {
    let cmd = trigger.event();
    // Prefer the avatar carrying the ACTIVE render camera when the command
    // doesn't name one (API/rhai path) — scenes can contain Avatar-tagged
    // prims (spawn points, `is_active: false` spawn cameras) that must not
    // steal the focus.
    let resolved = cmd
        .avatar
        .and_then(|a| q_avatar.get(a).ok())
        .or_else(|| {
            q_avatar
                .iter()
                .find(|(_, _, _, _, cam, _, _, _, _, _, _, _, _)| cam.is_some_and(|c| c.is_active))
        })
        .or_else(|| q_avatar.iter().next());
    let Some((
        avatar_ent,
        cam_tf,
        cam_cell,
        cam_parent,
        _,
        current_orbit,
        return_state,
        spring_arm,
        surface_camera,
        freeflight_camera,
        gravity_body,
        surface_relative,
        cinematic_lock,
    )) = resolved
    else {
        return;
    };

    // Focus is an interactive camera-mode transition. A cinematic path owns
    // this entity's complete pose, so the command has no valid camera-side
    // effect while the lock is present.
    if cinematic_lock {
        return;
    }

    // Compute distance based on target type.
    let mut distance = 20.0;
    let physical_target = resolve_declared_body(cmd.target, &q_body_decls, &q_body_entities)
        .unwrap_or_else(|| get_physical_body(cmd.target, &q_children, &q_body_entities));
    let is_body = q_bodies.get(physical_target).is_ok();

    // Already orbiting this very body (clicking the focused globe, re-clicking
    // its view pill): a repeat focus must be a NO-OP. Re-running the swap
    // re-arms `SunlitArrival`, which snaps the camera back to the arrival
    // pose — "I jump to the original position".
    if let Some(orbit) = current_orbit {
        if resolve_declared_body(orbit.target, &q_body_decls, &q_body_entities)
            .unwrap_or_else(|| get_physical_body(orbit.target, &q_children, &q_body_entities))
            == physical_target
        {
            return;
        }
    }
    if let Ok(body) = q_bodies.get(physical_target) {
        distance = body.radius_m * 3.0;
    } else if let Ok(sc) = q_sc.get(cmd.target) {
        distance = (sc.hit_radius_m as f64 * 5.0).max(100.0);
    }

    let (yaw, pitch, _) = cam_tf.rotation.to_euler(EulerRot::YXZ);

    let mut ent = commands.entity(avatar_ent);
    // First body focus opens one orbit-view transaction. A Moon → Earth switch
    // keeps the original surface snapshot instead of replacing it with the
    // current orbital pose.
    if return_state.is_none() {
        let behavior = if let Some(spring_arm) = spring_arm {
            OrbitReturnBehavior::SpringArm(spring_arm.clone())
        } else if let Some(surface) = surface_camera {
            OrbitReturnBehavior::Surface(surface.clone())
        } else if let Some(freeflight) = freeflight_camera {
            OrbitReturnBehavior::FreeFlight(freeflight.clone())
        } else {
            OrbitReturnBehavior::FreeFlight(FreeFlightCamera {
                yaw,
                pitch,
                damping: None,
            })
        };
        ent.try_insert(OrbitViewReturn {
            parent_grid: cam_parent.parent(),
            cell: *cam_cell,
            transform: *cam_tf,
            behavior,
            gravity_body: gravity_body.copied(),
            surface_relative,
        });
    }
    ent.remove::<SpringArmCamera>()
        .remove::<FreeFlightCamera>()
        .remove::<FrameBlend>()
        // Surface state must go too: `surface_camera_system` runs after
        // `orbit_system` and would rebuild the rotation as a ground-level
        // tangent frame every frame — the camera orbits the target but looks
        // at the horizon (planet off-screen, view jitters as the arm eases).
        .remove::<SurfaceCamera>()
        .remove::<SurfaceRelativeMode>()
        .remove::<GravityBody>()
        .try_insert(OrbitCamera {
            target: physical_target,
            distance,
            yaw,
            pitch,
            damping: None,
            vertical_offset: 0.0,
        });
    // Celestial bodies arrive on their sunlit side. The orbit writer resolves
    // this from the same root-grid cell poses it uses for steady-state motion.
    if is_body {
        ent.try_insert(SunlitArrival);
    }
    info!(
        "FOCUS: avatar={avatar_ent:?} target={:?} (physical {physical_target:?}) body={is_body} distance={distance:.3e}",
        cmd.target,
    );
}

/// Initializes avatar entities that lack a behavior component.
///
/// Inserts `FreeFlightCamera` as the default behavior with the entity's
/// current transform orientation.
///
/// `Without<CinematicCameraLock>` is load-bearing, not hygiene: a path-driven
/// camera has no interactive mode, and this initializer must never create one
/// after the authored path has claimed pose ownership.
fn avatar_init_system(
    mut commands: Commands,
    q_avatar: Query<
        (Entity, &Transform),
        (
            With<Avatar>,
            Without<SpringArmCamera>,
            Without<OrbitCamera>,
            Without<FreeFlightCamera>,
            // SurfaceCamera is a complete interactive mode, not an absent
            // behavior component. Without this guard init would reinsert
            // FreeFlightCamera over it on the next Update tick.
            Without<SurfaceCamera>,
            Without<FrameBlend>,
            Without<lunco_core::CinematicCameraLock>,
        ),
    >,
    q_proj: Query<Entity, (With<Avatar>, Without<AdaptiveNearPlane>, With<Projection>)>,
) {
    for (entity, tf) in q_avatar.iter() {
        let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
        commands.entity(entity).try_insert(FreeFlightCamera {
            yaw,
            pitch,
            damping: None,
        });
    }
    for entity in q_proj.iter() {
        commands.entity(entity).try_insert(AdaptiveNearPlane);
    }
}

// ─── Clip Planes ─────────────────────────────────────────────────────────────

fn update_avatar_clip_planes_system(
    mut q_camera: Query<
        (&mut Projection, &GlobalTransform),
        (With<Camera>, With<AdaptiveNearPlane>),
    >,
    q_bodies: Query<(&CelestialBody, &GlobalTransform)>,
) {
    for (mut projection, cam_gt) in q_camera.iter_mut() {
        // Camera↔body distances come from `GlobalTransform`s: big_space
        // rebases them around the floating origin, so both sides are in ONE
        // consistent frame every frame. (The previous `Transform`-based
        // query required `CellCoord` on bodies — which carry none by design —
        // so zero bodies matched and the fallback `far = 1e7 m` clipped
        // Earth, 1.9e7 m out at focus distance, to a black screen. And
        // `world_position_seeded` is NOT a fix: it sums nested grid
        // translations without grid rotations, so with the site-anchored
        // solar grid — rotation `align`, translation ~1.5e11 m — the mixed-
        // frame "distances" swing by kilometres per epoch tick and the clip
        // planes flap, strobing the whole viewport.)
        let cam_pos = cam_gt.translation().as_dvec3();
        // Peek through `&*` — NOT `*projection`. Deref-mut on a `Mut<Projection>`
        // flags the component `Changed` even when the value it writes is
        // identical, so a completely static camera re-triggered a frustum
        // recompute and a view-uniform re-upload EVERY PostUpdate. Read here,
        // compute, and take the mutable deref below only if a plane really moved.
        let Projection::Perspective(current) = &*projection else {
            continue;
        };
        {
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
            for (body, b_gt) in q_bodies.iter() {
                let center_d = cam_pos.distance(b_gt.translation().as_dvec3());
                let near_edge = center_d - body.radius_m;
                let far_edge = center_d + body.radius_m;
                if near_edge < min_dist {
                    min_dist = near_edge;
                }
                if far_edge > max_far {
                    max_far = far_edge;
                }
            }
            let (near, far) = if max_far <= 0.0 {
                // No `CelestialBody` contributed (flat luncosim scene, or the
                // offscreen USD preview camera). The body-derived `min_dist` is
                // still its 1e15 sentinel here — feeding it to the clamp below
                // pins `near` to the 100 m ceiling, which clips away the ENTIRE
                // nearby scene (rovers, ground) and renders black. Use a small
                // near + the 10 000 km far floor so a body-less scene renders.
                (0.1_f32, 1.0e7_f32)
            } else {
                // Near plane rides just in front of the NEAREST body surface, so
                // it scales with viewing distance. The old `* 0.01` + clamp to
                // [0.1, 100] pinned `near` ≤ 100 m: fine on the surface — near
                // terrain hogs the 1/z (reverse-Z) depth precision even with a
                // distant `far` — but in ORBITAL view the focused body sits ~2e7 m
                // out while `far` reaches the Sun at ~1.5e11 m, so the globe lands
                // ~0.01% into the depth range, in the starved tail where adjacent
                // LOD tile seams z-fight and strobe frame-to-frame. Anchoring
                // `near` to `min_dist` keeps the viewed surface AT the near plane,
                // where reverse-Z precision peaks — killing the orbital flicker
                // without touching the (already-fine) surface case, where
                // `min_dist` collapses to ~0 and `near` floors at 0.1 m.
                //
                // Keep the reference surface halfway between the camera and
                // near plane. The current-frame BigSpace propagation above
                // makes this a geometric depth-precision policy, not stale-pose
                // compensation. On or below the reference surface `min_dist`
                // is non-positive and the ordinary 0.1 m camera floor applies.
                (
                    (min_dist * 0.5).max(0.1) as f32,
                    ((max_far * 1.05).max(1.0e7)) as f32,
                )
            };

            // Relative-epsilon gate. The GTs jitter by metres at 1e8 m, so an
            // exact compare would still fire most frames; 1e-4 relative is far
            // below any visible clip-plane motion and leaves a parked camera
            // byte-stable, which is what keeps `Changed<Projection>` quiet.
            let moved = (current.near - near).abs() > near.abs() * 1e-4
                || (current.far - far).abs() > far.abs() * 1e-4;
            if !moved {
                continue;
            }
            if let Projection::Perspective(perspective) = &mut *projection {
                perspective.near = near;
                perspective.far = far;
            }
        }
    }
}

// ─── Surface Teleport Commands ───────────────────────────────────────────────

/// Teleports the avatar to a body's surface.
///
/// The camera is parented to the body's surface Grid, not to the Body entity.
/// That keeps the camera in the same body-fixed BigSpace branch as streamed
/// terrain while `SurfaceCamera` derives its orientation from the canonical
/// body-fixed ENU frame. `FloatingOrigin` must be on a Grid.
#[on_command(TeleportToSurface)]
fn on_surface_teleport_command(
    trigger: On<TeleportToSurface>,
    mut commands: Commands,
    q_avatar: Query<
        (
            Entity,
            &Transform,
            &CellCoord,
            &ChildOf,
            Has<lunco_core::CinematicCameraLock>,
        ),
        With<Avatar>,
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial_abs: Query<(Option<&CellCoord>, &Transform)>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_globe_lods: Query<&lunco_celestial::GlobeLod>,
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
        warn!(
            "TELEPORT: body entity {:?} not found in q_bodies",
            body_entity
        );
        return;
    };

    if body_entity == Entity::PLACEHOLDER {
        warn!("TELEPORT: no body found");
        return;
    }

    debug!("TELEPORT: triggered for avatar {:?}", avatar_ent);

    // Get camera cell for position lookup
    let Ok((_, _cam_tf, _cam_cell, _cam_child_of, cinematic_lock)) = q_avatar.get(avatar_ent)
    else {
        return;
    };
    if cinematic_lock {
        return;
    }

    // GlobeLod is the authoritative owner of a body's surface Grid. This is
    // deliberately data-driven: adding another celestial body does not add a
    // second Rust-side list of surface-grid marker types.
    let Ok(globe_lod) = q_globe_lods.get(body_entity) else {
        warn!("TELEPORT: body {:?} has no surface LOD Grid", body_entity);
        return;
    };
    let target_grid = globe_lod.surface_grid;
    let Ok(target_grid_ref) = q_grids.get(target_grid) else {
        warn!(
            "TELEPORT: target surface Grid {:?} is not live",
            target_grid
        );
        return;
    };
    debug!(
        "TELEPORT: parenting camera to surface grid {:?}",
        target_grid
    );

    {
        // Resolve the camera pose and its look direction in the same shared
        // BigSpace branch as the body. The old code read `Transform::forward`
        // directly, which is only correct while the avatar and body happen to
        // share one parent frame; it becomes a sideways teleport after an
        // orbital/body-grid handoff.
        let Some((_common_grid, avatar_position, avatar_rotation, body_position, body_rotation)) =
            lunco_core::coords::common_grid_poses(
                avatar_ent,
                body_entity,
                &q_parents,
                &q_grids,
                &q_spatial_abs,
            )
        else {
            warn!("TELEPORT: avatar and body have no shared BigSpace Grid");
            return;
        };
        let Some((_, grid_position, grid_to_common, _, body_to_common)) =
            lunco_core::coords::common_grid_poses(
                target_grid,
                body_entity,
                &q_parents,
                &q_grids,
                &q_spatial_abs,
            )
        else {
            warn!("TELEPORT: target Grid cannot be composed with the body");
            return;
        };
        let body_to_grid = grid_to_common.inverse() * body_to_common;
        let origin_body = body_rotation.inverse() * (avatar_position - body_position);
        let direction_body = body_rotation.inverse() * (avatar_rotation * Vec3::NEG_Z.as_dvec3());
        let b = origin_body.dot(direction_body);
        let c = origin_body.length_squared() - body_radius * body_radius;
        let discriminant = b * b - c;
        if discriminant < 0.0 {
            warn!("TELEPORT: avatar view does not intersect the body's surface");
            return;
        }
        let root = discriminant.sqrt();
        let Some(t) = [-b - root, -b + root]
            .into_iter()
            .filter(|t| *t > 0.0)
            .next()
        else {
            warn!("TELEPORT: camera ray does not intersect the body's forward surface");
            return;
        };
        let surface_body_pos = origin_body + direction_body * t;
        let surface_normal = surface_body_pos.normalize_or(DVec3::Y);
        let body_center_in_grid = grid_to_common.inverse() * (body_position - grid_position);
        let surface_local_pos = body_center_in_grid + body_to_grid * surface_body_pos;
        let Some((east, north, up)) = surface_axes_for_grid_position(
            target_grid,
            surface_local_pos,
            body_entity,
            &q_parents,
            &q_grids,
            &q_spatial_abs,
        ) else {
            warn!("TELEPORT: body-fixed tangent frame is not reachable from the target Grid");
            return;
        };

        let (new_cell, new_tf_translation) = target_grid_ref.translation_to_grid(surface_local_pos);

        // Surface gravity from body's GravityProvider
        let surface_g = if let Ok(gp) = q_gravity_providers.get(body_entity) {
            let accel = gp.model.acceleration(surface_body_pos);
            accel.length()
        } else {
            0.0
        };

        // Build the initial attitude from the same body-fixed ENU frame used
        // by SurfaceCamera. No world-axis reference is valid here.
        let surface_rot = surface_camera_rotation(east, north, up, 0.0, -0.2);

        // Parent camera to the same surface Grid as terrain and rover content.
        // FloatingOrigin must be on a Grid.
        let local_tf = Transform::from_translation(new_tf_translation).with_rotation(surface_rot);
        migrate_to_grid(&mut commands, avatar_ent, target_grid, new_cell, local_tf);

        commands
            .entity(avatar_ent)
            .try_insert(GravityBody { body_entity })
            .try_insert(SurfaceRelativeMode)
            .try_insert(SurfaceCamera {
                heading: 0.0,
                pitch: -0.2,
            })
            .remove::<FreeFlightCamera>()
            .remove::<OrbitCamera>()
            .remove::<SpringArmCamera>()
            .remove::<FrameBlend>();

        // Update LocalGravityField (world-space "up")
        field.body_entity = Some(body_entity);
        field.body_relative_position = surface_body_pos;
        field.local_up = surface_normal;
        field.surface_g = surface_g;
        let Some((_, body_world_rotation)) =
            lunco_core::coords::world_pose(body_entity, &q_parents, &q_grids, &q_spatial_abs).ok()
        else {
            warn!("TELEPORT: body has no complete world BigSpace pose");
            return;
        };
        field.up = body_world_rotation.0 * surface_normal;

        debug!(
            "TELEPORT: done — camera now on surface grid {:?} at alt ~50m",
            target_grid
        );
    }
}

/// Leaves the surface and returns to orbit view.
///
/// Opens the same transactional orbit view as every other body-focus path.
/// Spatial placement is owned exclusively by `orbit_system`, which migrates
/// the avatar to the body's explicit star-fixed
/// [`lunco_celestial::ReferenceFrame::EclipticJ2000`].
#[on_command(LeaveSurface)]
fn on_leave_surface_command(
    trigger: On<LeaveSurface>,
    mut commands: Commands,
    q_avatar: Query<
        (
            Entity,
            Option<&GravityBody>,
            Has<lunco_core::CinematicCameraLock>,
        ),
        With<Avatar>,
    >,
    mut field: ResMut<LocalGravityField>,
) {
    let avatar_ent = trigger.event().target;
    let Ok((_, gravity_body, cinematic_lock)) = q_avatar.get(avatar_ent) else {
        warn!(?avatar_ent, "LEAVE SURFACE: target is not an avatar");
        return;
    };
    if cinematic_lock {
        return;
    }

    // Find the body we're leaving
    let body_entity = gravity_body
        .map(|gb| gb.body_entity)
        .unwrap_or(Entity::PLACEHOLDER);

    if body_entity == Entity::PLACEHOLDER {
        warn!("LEAVE SURFACE: avatar has no gravity body");
        return;
    }

    commands.trigger(FocusTarget {
        avatar: Some(avatar_ent),
        target: body_entity,
    });

    // Clear gravity field
    field.body_entity = None;
    field.body_relative_position = DVec3::ZERO;
    field.local_up = DVec3::Y;
    field.surface_g = 0.0;
    field.up = DVec3::Y;

    info!("Left surface, opened orbit view around {:?}", body_entity);
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
    q_avatar: Query<
        (
            Entity,
            &Transform,
            &ChildOf,
            Option<&GravityBody>,
            Option<&SurfaceRelativeMode>,
            Option<&SurfaceCamera>,
            Option<&SpringArmCamera>,
        ),
        (
            With<Avatar>,
            Without<OrbitCamera>,
            Without<lunco_core::CinematicCameraLock>,
        ),
    >,
    q_bodies: Query<&CelestialBody>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    thresholds: Res<SurfaceModeThreshold>,
    field: Res<LocalGravityField>,
    q_site: Query<(), With<lunco_celestial::SiteAnchor>>,
    mut commands: Commands,
) {
    // `Without<OrbitCamera>`: focusing a celestial body activates the orbital
    // world-pin, which slides the celestial tree so the focused body lands in
    // front of the PARKED camera. The camera's GT-delta altitude above the site
    // body then reads as enormous, so the disengage branch below fired, stripped
    // surface mode and inserted a `FreeFlightCamera`. `freeflight_system` has no
    // `Without<OrbitCamera>` filter, so it then fought `orbit_system` for the
    // Transform every frame — the camera drifted off the site and right-drag
    // flew the view away ("right click moved somewhere else"), while the two
    // writers alternating produced the residual per-frame wobble. An orbital
    // view owns the camera; leave it alone.
    let Some((avatar_ent, transform, child_of, maybe_gb, maybe_mode, maybe_sc, maybe_spring)) =
        q_avatar.iter().next()
    else {
        return;
    };

    // Altitude comes from the same body-local position used by gravity and the
    // surface camera.  Mixing a root-frame GlobalTransform delta here with a
    // grid-relative camera writer is precisely what allowed the mode to flap
    // when the celestial frame rotated at high time warp.
    let engage_body = maybe_gb.map(|gb| gb.body_entity);
    let disengage_body = engage_body.or(field.body_entity);
    let altitude_to = |b: Entity| {
        (field.body_entity == Some(b))
            .then_some(field.body_relative_position.length())
            .zip(q_bodies.get(b).ok())
            .map(|(distance, body)| distance - body.radius_m)
    };
    let engage_altitude_m = engage_body.and_then(altitude_to).unwrap_or(f64::MAX);
    let altitude = disengage_body.and_then(altitude_to).unwrap_or(f64::MAX);

    // SurfaceRelativeMode is a coordinate-policy marker, not a camera mode.
    // `SurfaceCamera` owns a free camera's complete surface-relative pose;
    // `SpringArmCamera` owns a followed vessel pose and consumes the marker to
    // choose body-fixed ENU instead of world-Y.  Treating SurfaceCamera as the
    // only valid writer used to insert it over an active spring arm immediately
    // after possession. The camera-mode hook then removed SpringArmCamera, so
    // the rover drove away while the view remained at the possession pose.
    let camera_is_surface = maybe_sc.is_some();
    let spring_is_surface = maybe_spring.is_some();
    let has_surface_relative_writer = camera_is_surface || spring_is_surface;
    let marker_is_surface = maybe_mode.is_some();

    // Site-anchored scenes NEVER altitude-disengage: the user's frame of
    // reference is the anchor body at every height below the orbital handover
    // ("the planetary body always at the bottom of the screen, following the
    // direction of gravity"). Falling back to the world-euler FreeFlight
    // camera up there levels the view to world +Y instead of the local up —
    // the tilted-horizon / "moon in the corner" report. Beyond ~50 km the
    // scroll transit hands the camera to the orbital mode anyway.
    let site_anchored = !q_site.is_empty();

    if has_surface_relative_writer && altitude > thresholds.disengage_altitude && !site_anchored {
        // Too high → leave the surface coordinate policy. A free surface
        // camera swaps back to free flight; a spring arm remains the same
        // writer and simply resumes its non-surface heading basis.
        commands.entity(avatar_ent).remove::<SurfaceRelativeMode>();
        if let Some(sc) = maybe_sc {
            // Note: heading→yaw is approximate (different reference frames)
            // but provides a reasonable starting orientation.
            commands
                .entity(avatar_ent)
                .remove::<SurfaceCamera>()
                .try_insert(FreeFlightCamera {
                    yaw: sc.heading,
                    pitch: sc.pitch,
                    damping: None,
                });
        }
    } else if engage_altitude_m < thresholds.engage_altitude {
        // Low enough and explicitly bound to a body → enter surface mode.
        commands.entity(avatar_ent).try_insert(SurfaceRelativeMode);
        // A free camera needs the dedicated surface writer. A spring arm
        // already is the sole writer and derives its ENU orientation itself.
        if !has_surface_relative_writer {
            if let Some((east, north, up)) =
                surface_axes_in_grid(child_of.0, &field, &q_parents, &q_grids, &q_spatial)
            {
                let (heading, pitch) = surface_camera_angles(east, north, up, transform.rotation);
                commands
                    .entity(avatar_ent)
                    .remove::<FreeFlightCamera>()
                    .try_insert(SurfaceCamera { heading, pitch });
            }
        }
    } else if marker_is_surface && !has_surface_relative_writer {
        // Repair a stale marker even when the body is no longer in the engage
        // band.  Leaving it behind is not a valid intermediate state.
        commands.entity(avatar_ent).remove::<SurfaceRelativeMode>();
    }
}

/// Resolves a focus target (which might be a Grid/Frame) to its primary physical Body.
///
/// If the entity itself has a `CelestialBody`, it is returned.
/// Otherwise, its immediate children are searched for a `CelestialBody`.
fn get_physical_body(
    target: Entity,
    q_children: &Query<&Children>,
    bodies: &Query<(Entity, &CelestialBody)>,
) -> Entity {
    // If the target itself is the body, we are done.
    if bodies.contains(target) {
        return target;
    }

    // Body projections may be wrapped by a scene grid, a body frame, and a
    // render/physics holder. Walk the composed hierarchy rather than assuming
    // the body is an immediate child; API focus targets are stable USD prims,
    // while the physical CelestialBody component is projected deeper.
    let mut pending = vec![target];
    for _ in 0..8 {
        let mut next = Vec::new();
        for parent in pending.drain(..) {
            if let Ok(children) = q_children.get(parent) {
                for child in children.iter() {
                    if bodies.contains(child) {
                        return child;
                    }
                    next.push(child);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        pending = next;
    }

    target // Fallback
}

fn resolve_declared_body(
    target: Entity,
    declarations: &Query<&lunco_celestial::CelestialBodyDecl>,
    bodies: &Query<(Entity, &CelestialBody)>,
) -> Option<Entity> {
    let decl = declarations.get(target).ok()?;
    bodies
        .iter()
        .find(|(_, body)| body.ephemeris_id == decl.naif)
        .map(|(entity, _)| entity)
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
    /// Force the tags on even in single-player. Name tags exist to identify OTHER
    /// players, so by default they are **suppressed in solo play** (a standalone
    /// session — including one where a local AI autopilot drives a rover; that's
    /// still solo, not a wire peer). Set `true` to always render them.
    pub show_always: bool,
}

impl Default for RoverNameTagSettings {
    fn default() -> Self {
        Self {
            font_size: 26.0,
            text_color: Color::WHITE,
            vertical_offset: 2.0,
            reference_distance: 15.0,
            max_distance: 150.0,
            show_always: false,
        }
    }
}

/// Project native possession onto the script/telemetry bus as `cmd:PossessVessel`.
///
/// UI possession triggers the typed [`PossessVessel`] event *directly* (raycast /
/// hotkey — `commands.trigger(PossessVessel{..})`), bypassing `ApiCommandEvent`, so
/// lunco-api's generic `cmd:*` projector (which taps `ApiCommandEvent`) never sees
/// it. Observing the typed event here makes `wait_for("cmd:PossessVessel")` /
/// an objective's `requires_event:"cmd:PossessVessel"` fire for BOTH the UI path
/// and the API path (the API dispatcher also ends at a typed `PossessVessel`
/// trigger — a harmless duplicate the bus latches). This is the pattern any
/// native-triggered command needs to become a tutorial/script trigger.
fn project_possess_event(_t: On<PossessVessel>, mut commands: Commands) {
    commands.trigger(lunco_core::command_telemetry_event("PossessVessel"));
}

fn project_release_event(_t: On<ReleaseVessel>, mut commands: Commands) {
    commands.trigger(lunco_core::command_telemetry_event("ReleaseVessel"));
}

#[on_command(UpdateProfile)]
fn on_update_profile(
    trigger: On<UpdateProfile>,
    guard: Res<lunco_core::SyncApplyGuard>,
    local: Res<LocalSession>,
    mut profiles: ResMut<SessionProfiles>,
) {
    let origin = guard.0.unwrap_or(local.0);
    profiles
        .profiles
        .insert(origin.0, trigger.event().name.clone());
    info!(
        "[net] session {} set name to '{}'",
        origin.0,
        trigger.event().name
    );
}

/// One active on-screen toast (see [`ScreenNotifications`]).
#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    /// "info" | "success" | "warn" | "error" — drives color.
    pub kind: String,
    /// Seconds left before it disappears (counts down on REAL time, so it fades
    /// even while the sim is paused). Also drives the fade-out in the last second.
    pub remaining: f32,
}

/// Queue of transient on-screen notifications drawn by the ui-gated
/// `draw_notifications` overlay. Written by [`commands::ShowNotification`] (rhai
/// `notify(...)`), aged by [`tick_notifications`]. Always present (headless too)
/// so the command never panics on a missing resource; only the draw is gated.
#[derive(Resource, Default)]
pub struct ScreenNotifications {
    pub toasts: Vec<Toast>,
}

/// Real command (registered via `register_commands!`, so it's API-discoverable
/// and dispatchable through `/api/commands` and rhai `cmd("ShowNotification")`).
/// Pushes a toast onto [`ScreenNotifications`]; the ui overlay renders it.
#[on_command(ShowNotification)]
pub fn on_show_notification(trigger: On<ShowNotification>, mut notes: ResMut<ScreenNotifications>) {
    let secs = if cmd.secs > 0.0 { cmd.secs } else { 4.5 };
    let kind = if cmd.kind.is_empty() {
        "info"
    } else {
        cmd.kind.as_str()
    }
    .to_string();
    info!("[notify:{kind}] {}", cmd.text);
    notes.toasts.push(Toast {
        text: cmd.text.clone(),
        kind,
        remaining: secs,
    });
    // Cap the backlog so a chatty script can't grow it unbounded.
    let overflow = notes.toasts.len().saturating_sub(6);
    if overflow > 0 {
        notes.toasts.drain(0..overflow);
    }
}

/// Age out toasts on REAL time (independent of sim pause / rate).
fn tick_notifications(mut notes: ResMut<ScreenNotifications>, time: Res<Time<Real>>) {
    if notes.toasts.is_empty() {
        return;
    }
    let dt = time.delta_secs();
    for t in &mut notes.toasts {
        t.remaining -= dt;
    }
    notes.toasts.retain(|t| t.remaining > 0.0);
}

fn sync_profile(
    role: Res<NetworkRole>,
    local: Res<LocalSession>,
    settings: Res<ProfileSettings>,
    mut last_sent: Local<Option<u64>>,
    mut last_name: Local<Option<String>>,
    mut commands: Commands,
) {
    let session = local.0 .0;
    if *role == NetworkRole::Client && session == 0 {
        *last_sent = None;
        return;
    }
    let current_name = settings.username.clone();
    let should_send = last_sent.is_none_or(|s| s != session)
        || last_name.as_ref().is_none_or(|n| *n != current_name);
    if should_send {
        commands.trigger(UpdateProfile {
            name: current_name.clone(),
        });
        *last_sent = Some(session);
        *last_name = Some(current_name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::SystemState;

    #[test]
    fn wheel_click_resolves_to_owning_vehicle_command_root() {
        let mut world = World::new();
        let rover = world
            .spawn((lunco_core::InputPorts::new(&["drive"]), Name::new("Rover")))
            .id();
        let wheel = world
            .spawn((
                lunco_core::SelectableRoot,
                Name::new("Wheel"),
                ChildOf(rover),
            ))
            .id();
        let wheel_mesh = world.spawn((Name::new("WheelMesh"), ChildOf(wheel))).id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&lunco_core::InputPorts>,
            Query<Entity, With<lunco_core::Ground>>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_input_ports, q_ground) = state.get(&world).unwrap();

        assert_eq!(
            find_control_owner_from_hit(wheel_mesh, &q_parents, &q_input_ports, &q_ground),
            Some(rover)
        );
    }

    #[test]
    fn scroll_units_are_normalized_before_zoom() {
        let line = AccumulatedMouseScroll {
            delta: Vec2::new(0.0, 1.0),
            unit: MouseScrollUnit::Line,
        };
        let pixel = AccumulatedMouseScroll {
            delta: Vec2::new(0.0, MouseScrollUnit::SCROLL_UNIT_CONVERSION_FACTOR),
            unit: MouseScrollUnit::Pixel,
        };
        assert_eq!(normalized_scroll_delta(&line), 1.0);
        assert_eq!(normalized_scroll_delta(&pixel), 1.0);
    }

    #[test]
    fn scroll_zoom_limits_one_frame_to_a_safe_factor() {
        let mut distance = 100.0;
        let mut delta = -10_000.0;
        apply_scroll_zoom(&mut distance, &mut delta, ZOOM_SENSITIVITY, 1.0, 1_000.0);
        assert_eq!(distance, 125.0);

        let mut distance = 100.0;
        let mut delta = 10_000.0;
        apply_scroll_zoom(&mut distance, &mut delta, ZOOM_SENSITIVITY, 1.0, 1_000.0);
        assert_eq!(distance, 75.0);
    }

    #[test]
    fn semantic_look_intent_rotates_camera_angles() {
        let settings = CameraInputSettings {
            look_radians_per_pointer_unit: 0.01,
            ..default()
        };
        let mut yaw = 0.0;
        let mut pitch = 0.0;

        // This is the delta produced by the configured pointer-button chord.
        // Positive horizontal motion turns the camera left, and upward motion
        // raises the view, matching the live camera convention.
        (yaw, pitch) = look_angles(yaw, pitch, Vec2::new(10.0, -5.0), &settings, 1.0);

        assert!((yaw + 0.1).abs() < 1.0e-6);
        assert!((pitch - 0.05).abs() < 1.0e-6);
    }

    #[test]
    fn pitched_qw_keeps_a_forward_component() {
        // Looking down by 45° makes camera-relative `forward - up` lose its
        // horizontal component. Q+W must still travel forward while descending.
        let tf = Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_4));
        let direction = fly_move_direction(&tf, 1.0, 0.0, -1.0, Vec3::Y);

        assert!(
            direction.z < -0.3,
            "Q+W must retain forward travel at a pitched view, got {direction:?}"
        );
        assert!(
            direction.y < -0.5,
            "Q+W must retain downward travel at a pitched view, got {direction:?}"
        );
        assert!(direction.length() <= 1.0 + 1e-6);
    }

    #[test]
    fn diagonal_flight_is_capped_without_reducing_single_axis_speed() {
        let tf = Transform::default();
        let forward = fly_move_direction(&tf, 1.0, 0.0, 0.0, Vec3::Y);
        let diagonal = fly_move_direction(&tf, 1.0, 1.0, 0.0, Vec3::Y);

        assert!((forward.length() - 1.0).abs() < 1e-6);
        assert!((diagonal.length() - 1.0).abs() < 1e-6);
        assert!(diagonal.x > 0.0 && diagonal.z < 0.0);
    }

    #[test]
    fn orbit_angles_round_trip_the_body_to_camera_arm() {
        for arm in [
            DVec3::Z,
            DVec3::X,
            -DVec3::Z,
            DVec3::new(0.3, 0.8, -0.5).normalize(),
        ] {
            let (yaw, pitch) = orbit_angles_from_arm(arm);
            let rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
            let reconstructed = rotation.mul_vec3(Vec3::Z).as_dvec3();
            assert!(
                reconstructed.abs_diff_eq(arm.normalize(), 1e-6),
                "arm {arm:?} reconstructed as {reconstructed:?}"
            );
        }
    }

    #[test]
    fn celestial_orbit_camera_uses_the_explicit_inertial_body_frame() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Time<Real>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<CameraDefaults>()
            .init_resource::<lunco_celestial::ReferenceFrameIndex>()
            .add_systems(First, lunco_celestial::update_reference_frame_index)
            .add_systems(Update, orbit_system);

        let root_grid = app
            .world_mut()
            .spawn((
                lunco_core::WorldGrid,
                Grid::new(2_000.0, 100.0),
                CellCoord::ZERO,
                Transform::default(),
            ))
            .id();
        let host_rotation = Quat::from_rotation_y(0.7);
        let host_grid = app
            .world_mut()
            .spawn((
                Grid::new(2_000.0, 100.0),
                CellCoord::new(75_000_000, 0, 0),
                Transform::from_rotation(host_rotation),
                ChildOf(root_grid),
            ))
            .id();
        let orbit_grid = app
            .world_mut()
            .spawn((
                lunco_celestial::ReferenceFrame::EclipticJ2000 {
                    center: lunco_celestial::ephemeris_id::MOON,
                },
                Grid::new(2_000.0, 100.0),
                CellCoord::new(75_000_000, 0, 0),
                Transform::default(),
                ChildOf(root_grid),
            ))
            .id();
        let body = app
            .world_mut()
            .spawn((
                CelestialBody {
                    name: "precision test moon".into(),
                    ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                    radius_m: 1_000.0,
                },
                CellCoord::new(100_000, -20_000, 50_000),
                Transform::from_xyz(125.0, -350.0, 700.0),
                ChildOf(host_grid),
            ))
            .id();
        let yaw = 0.35;
        let pitch = -0.2;
        let distance = 10_000.0;
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                CellCoord::ZERO,
                Transform::from_xyz(10.0, 20.0, 30.0),
                ChildOf(host_grid),
                OrbitCamera {
                    target: body,
                    distance,
                    yaw,
                    pitch,
                    damping: None,
                    vertical_offset: 0.0,
                },
                CameraZoomInput::default(),
            ))
            .id();

        app.update();

        let world = app.world();
        assert_eq!(
            world.get::<ChildOf>(avatar).unwrap().parent(),
            orbit_grid,
            "a celestial orbit camera must be a direct child of the target's explicit inertial frame"
        );
        let inertial = world.get::<Grid>(orbit_grid).unwrap();
        let actual_in_inertial = inertial.grid_position_double(
            world.get::<CellCoord>(avatar).unwrap(),
            world.get::<Transform>(avatar).unwrap(),
        );
        let root = world.get::<Grid>(root_grid).unwrap();
        let host_position = root.grid_position_double(
            world.get::<CellCoord>(host_grid).unwrap(),
            world.get::<Transform>(host_grid).unwrap(),
        );
        let host = world.get::<Grid>(host_grid).unwrap();
        let body_local = host.grid_position_double(
            world.get::<CellCoord>(body).unwrap(),
            world.get::<Transform>(body).unwrap(),
        );
        let body_root = host_position + host_rotation.as_dquat() * body_local;
        let arm = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0)
            .mul_vec3(Vec3::Z)
            .as_dvec3()
            * distance;
        let orbit_origin = root.grid_position_double(
            world.get::<CellCoord>(orbit_grid).unwrap(),
            world.get::<Transform>(orbit_grid).unwrap(),
        );
        let expected_in_inertial = body_root + arm - orbit_origin;
        assert!(
            actual_in_inertial.abs_diff_eq(expected_in_inertial, 1e-3),
            "inertial-grid orbit pose differs: expected {expected_in_inertial:?}, got {actual_in_inertial:?}"
        );
    }

    #[test]
    fn orbital_release_restores_pose_and_mode_in_one_transition() {
        let mut app = App::new();
        app.init_resource::<lunco_core::SyncApplyGuard>()
            .init_resource::<LocalGravityField>()
            .add_observer(on_release_command);

        let root_grid = app
            .world_mut()
            .spawn((
                lunco_core::WorldGrid,
                Grid::new(2_000.0, 100.0),
                CellCoord::ZERO,
                Transform::default(),
            ))
            .id();
        let surface_grid = app
            .world_mut()
            .spawn((
                Grid::new(2_000.0, 100.0),
                CellCoord::new(17, -4, 8),
                Transform::from_rotation(Quat::from_rotation_y(0.7)),
                ChildOf(root_grid),
            ))
            .id();
        let body = app.world_mut().spawn_empty().id();
        let return_cell = CellCoord::new(12, -3, 9);
        let return_transform = Transform::from_xyz(125.0, -42.0, 9.0)
            .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.4, -0.25, 0.0));
        let return_surface = SurfaceCamera {
            heading: 0.4,
            pitch: -0.25,
        };
        app.insert_resource(lunco_celestial::OrbitalViewPin {
            active: true,
            body: lunco_celestial::ephemeris_id::MOON,
            dir: DVec3::Z,
            distance: 5_000_000.0,
        });
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                CellCoord::new(-80_000, 30_000, 20_000),
                Transform::from_xyz(700.0, -600.0, 500.0),
                ChildOf(root_grid),
                OrbitCamera {
                    target: Entity::PLACEHOLDER,
                    distance: 5_000_000.0,
                    yaw: 1.0,
                    pitch: 0.3,
                    damping: None,
                    vertical_offset: 0.0,
                },
                OrbitViewReturn {
                    parent_grid: surface_grid,
                    cell: return_cell,
                    transform: return_transform,
                    behavior: OrbitReturnBehavior::Surface(return_surface.clone()),
                    gravity_body: Some(GravityBody { body_entity: body }),
                    surface_relative: true,
                },
            ))
            .id();

        app.world_mut().trigger(ReleaseVessel { target: avatar });
        app.world_mut().flush();

        let world = app.world();
        assert_eq!(world.get::<ChildOf>(avatar).unwrap().parent(), surface_grid);
        assert_eq!(*world.get::<CellCoord>(avatar).unwrap(), return_cell);
        let restored = world.get::<Transform>(avatar).unwrap();
        assert!(restored
            .translation
            .abs_diff_eq(return_transform.translation, 1e-6));
        assert!(restored
            .rotation
            .abs_diff_eq(return_transform.rotation, 1e-6));
        assert!(!world.resource::<lunco_celestial::OrbitalViewPin>().active);
        assert!(world.get::<OrbitCamera>(avatar).is_none());
        assert_eq!(
            world.get::<SurfaceCamera>(avatar).unwrap().heading,
            return_surface.heading
        );
        assert_eq!(world.get::<GravityBody>(avatar).unwrap().body_entity, body);
        assert!(world.get::<SurfaceRelativeMode>(avatar).is_some());
        assert!(world.get::<FreeFlightCamera>(avatar).is_none());
        assert!(world.get::<OrbitViewReturn>(avatar).is_none());
    }

    #[test]
    fn moon_earth_surface_round_trip_preserves_the_original_surface_transaction() {
        let mut app = App::new();
        app.init_resource::<lunco_core::SyncApplyGuard>()
            .init_resource::<LocalGravityField>()
            .init_resource::<lunco_celestial::OrbitalViewPin>()
            .add_observer(on_focus_command)
            .add_observer(on_return_from_orbit);

        let surface_grid = app
            .world_mut()
            .spawn((Grid::new(2_000.0, 100.0), CellCoord::ZERO))
            .id();
        let moon = app
            .world_mut()
            .spawn(CelestialBody {
                name: "Moon".into(),
                ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                radius_m: 1_737_400.0,
            })
            .id();
        let earth = app
            .world_mut()
            .spawn(CelestialBody {
                name: "Earth".into(),
                ephemeris_id: lunco_celestial::ephemeris_id::EARTH,
                radius_m: 6_371_000.0,
            })
            .id();
        let original_cell = CellCoord::new(41, -7, 13);
        let original_transform = Transform::from_xyz(175.0, 23.0, -440.0)
            .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.3, -0.4, 0.0));
        let original_surface = SurfaceCamera {
            heading: 0.3,
            pitch: -0.4,
        };
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                Camera {
                    is_active: true,
                    ..default()
                },
                original_cell,
                original_transform,
                ChildOf(surface_grid),
                original_surface.clone(),
                GravityBody { body_entity: moon },
                SurfaceRelativeMode,
            ))
            .id();

        app.world_mut().trigger(FocusTarget {
            avatar: Some(avatar),
            target: moon,
        });
        app.world_mut().flush();
        let first_snapshot = app.world().get::<OrbitViewReturn>(avatar).unwrap().clone();
        assert_eq!(first_snapshot.parent_grid, surface_grid);
        assert_eq!(first_snapshot.cell, original_cell);
        assert!(matches!(
            first_snapshot.behavior,
            OrbitReturnBehavior::Surface(_)
        ));

        app.world_mut().trigger(FocusTarget {
            avatar: Some(avatar),
            target: earth,
        });
        app.world_mut().flush();
        let second_snapshot = app.world().get::<OrbitViewReturn>(avatar).unwrap();
        assert_eq!(second_snapshot.parent_grid, first_snapshot.parent_grid);
        assert_eq!(second_snapshot.cell, first_snapshot.cell);
        assert!(second_snapshot
            .transform
            .translation
            .abs_diff_eq(first_snapshot.transform.translation, 1e-6));
        assert_eq!(
            app.world().get::<OrbitCamera>(avatar).unwrap().target,
            earth
        );

        app.world_mut().trigger(ReturnFromOrbit { target: avatar });
        app.world_mut().flush();
        let world = app.world();
        assert_eq!(world.get::<ChildOf>(avatar).unwrap().parent(), surface_grid);
        assert_eq!(*world.get::<CellCoord>(avatar).unwrap(), original_cell);
        let restored = world.get::<Transform>(avatar).unwrap();
        assert!(restored
            .translation
            .abs_diff_eq(original_transform.translation, 1e-6));
        assert!(restored
            .rotation
            .abs_diff_eq(original_transform.rotation, 1e-6));
        assert_eq!(
            world.get::<SurfaceCamera>(avatar).unwrap().heading,
            original_surface.heading
        );
        assert_eq!(world.get::<GravityBody>(avatar).unwrap().body_entity, moon);
        assert!(world.get::<SurfaceRelativeMode>(avatar).is_some());
    }

    #[test]
    fn possessed_orbit_view_round_trip_preserves_control_and_spring_arm() {
        let mut app = App::new();
        app.init_resource::<lunco_core::SyncApplyGuard>()
            .init_resource::<LocalGravityField>()
            .init_resource::<lunco_celestial::OrbitalViewPin>()
            .add_observer(on_focus_command)
            .add_observer(on_return_from_orbit);

        let surface_grid = app
            .world_mut()
            .spawn((Grid::new(2_000.0, 100.0), CellCoord::ZERO))
            .id();
        let orbit_grid = app
            .world_mut()
            .spawn((Grid::new(2_000.0, 100.0), CellCoord::ZERO))
            .id();
        let moon = app
            .world_mut()
            .spawn(CelestialBody {
                name: "Moon".into(),
                ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                radius_m: 1_737_400.0,
            })
            .id();
        let rover = app.world_mut().spawn_empty().id();
        let original_cell = CellCoord::new(8, -2, 5);
        let original_transform = Transform::from_xyz(31.0, 12.0, -17.0)
            .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.6, -0.2, 0.0));
        let original_spring = SpringArmCamera {
            target: rover,
            distance: 14.0,
            yaw: 0.25,
            pitch: -0.35,
            damping: Some(0.4),
            vertical_offset: 2.0,
            track_heading: true,
            attitude: FollowAttitude::Heading,
        };
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                Camera {
                    is_active: true,
                    ..default()
                },
                original_cell,
                original_transform,
                ChildOf(surface_grid),
                original_spring.clone(),
                ControllerLink {
                    vessel_entity: rover,
                },
                GravityBody { body_entity: moon },
                SurfaceRelativeMode,
            ))
            .id();

        app.world_mut().trigger(FocusTarget {
            avatar: Some(avatar),
            target: moon,
        });
        app.world_mut().flush();
        assert!(matches!(
            app.world().get::<OrbitViewReturn>(avatar).unwrap().behavior,
            OrbitReturnBehavior::SpringArm(_)
        ));
        assert_eq!(
            app.world()
                .get::<ControllerLink>(avatar)
                .unwrap()
                .vessel_entity,
            rover,
            "entering a presentation view must not release control"
        );

        app.world_mut().entity_mut(avatar).insert((
            ChildOf(orbit_grid),
            CellCoord::new(-50_000, 20_000, 9_000),
            Transform::from_xyz(700.0, -600.0, 500.0),
        ));
        app.world_mut().trigger(ReturnFromOrbit { target: avatar });
        app.world_mut().flush();

        let world = app.world();
        assert_eq!(world.get::<ChildOf>(avatar).unwrap().parent(), surface_grid);
        assert_eq!(*world.get::<CellCoord>(avatar).unwrap(), original_cell);
        let restored_transform = world.get::<Transform>(avatar).unwrap();
        assert!(restored_transform
            .translation
            .abs_diff_eq(original_transform.translation, 1e-6));
        assert!(restored_transform
            .rotation
            .abs_diff_eq(original_transform.rotation, 1e-6));
        let restored_spring = world.get::<SpringArmCamera>(avatar).unwrap();
        assert_eq!(restored_spring.target, original_spring.target);
        assert_eq!(restored_spring.distance, original_spring.distance);
        assert_eq!(restored_spring.yaw, original_spring.yaw);
        assert_eq!(restored_spring.pitch, original_spring.pitch);
        assert_eq!(
            world.get::<ControllerLink>(avatar).unwrap().vessel_entity,
            rover,
            "returning from a presentation view must preserve possession"
        );
        assert!(world.get::<OrbitCamera>(avatar).is_none());
        assert!(world.get::<OrbitViewReturn>(avatar).is_none());
        assert!(!world.resource::<lunco_celestial::OrbitalViewPin>().active);
    }

    #[test]
    fn freeflight_reseeds_after_grid_parent_handoff() {
        let mut app = App::new();
        app.add_systems(Update, rebase_freeflight_state);

        let first_grid = app.world_mut().spawn(Grid::new(2_000.0, 0.0)).id();
        let second_grid = app.world_mut().spawn(Grid::new(2_000.0, 0.0)).id();
        let authored_rotation = Quat::from_euler(EulerRot::YXZ, 1.2, -0.4, 0.0);
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                FreeFlightCamera {
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                },
                Transform::from_rotation(authored_rotation),
                ChildOf(first_grid),
            ))
            .id();

        app.update();
        app.world_mut().entity_mut(avatar).insert((
            ChildOf(second_grid),
            Transform::from_rotation(authored_rotation),
        ));
        app.update();

        let freeflight = app.world().get::<FreeFlightCamera>(avatar).unwrap();
        assert!((freeflight.yaw - 1.2).abs() < 1e-5);
        assert!((freeflight.pitch + 0.4).abs() < 1e-5);
    }

    #[test]
    fn avatar_init_does_not_reinsert_freeflight_over_surface_camera() {
        let mut app = App::new();
        app.add_systems(Update, avatar_init_system);

        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                Transform::default(),
                SurfaceCamera {
                    heading: 0.0,
                    pitch: -0.2,
                },
            ))
            .id();

        app.update();

        assert!(app.world().get::<SurfaceCamera>(avatar).is_some());
        assert!(
            app.world().get::<FreeFlightCamera>(avatar).is_none(),
            "camera initialization must not create two mutually-exclusive modes"
        );
    }

    #[test]
    fn camera_mode_additions_remove_all_other_modes() {
        let mut app = App::new();
        register_camera_mode_hooks(&mut app);

        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                FreeFlightCamera {
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                },
                SurfaceCamera {
                    heading: 0.0,
                    pitch: 0.0,
                },
                OrbitCamera {
                    target: Entity::PLACEHOLDER,
                    distance: 1.0,
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                    vertical_offset: 0.0,
                },
            ))
            .id();
        app.world_mut().flush();

        let world = app.world();
        let mode_count = [
            world.get::<FreeFlightCamera>(avatar).is_some(),
            world.get::<SurfaceCamera>(avatar).is_some(),
            world.get::<SpringArmCamera>(avatar).is_some(),
            world.get::<OrbitCamera>(avatar).is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        assert_eq!(mode_count, 1);
    }

    #[test]
    fn surface_transition_preserves_possessed_spring_arm_writer() {
        let mut app = App::new();
        register_camera_mode_hooks(&mut app);
        app.init_resource::<SurfaceModeThreshold>()
            .insert_resource(LocalGravityField {
                body_entity: None,
                body_relative_position: DVec3::ZERO,
                up: DVec3::Y,
                local_up: DVec3::Y,
                surface_g: 1.0,
            })
            .add_systems(Update, surface_mode_transition_system);

        let body = app
            .world_mut()
            .spawn(CelestialBody {
                name: "test body".into(),
                ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                radius_m: 100.0,
            })
            .id();
        app.world_mut()
            .resource_mut::<LocalGravityField>()
            .body_entity = Some(body);
        app.world_mut()
            .resource_mut::<LocalGravityField>()
            .body_relative_position = DVec3::Y * 101.0;

        let grid = app.world_mut().spawn(Grid::new(2_000.0, 0.0)).id();
        let target = app.world_mut().spawn_empty().id();
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                Transform::default(),
                CellCoord::ZERO,
                ChildOf(grid),
                GravityBody { body_entity: body },
                SurfaceRelativeMode,
                SpringArmCamera {
                    target,
                    distance: 15.0,
                    yaw: 0.0,
                    pitch: -0.25,
                    damping: None,
                    vertical_offset: 2.0,
                    track_heading: true,
                    attitude: FollowAttitude::Heading,
                },
            ))
            .id();

        app.update();

        assert!(
            app.world().get::<SpringArmCamera>(avatar).is_some(),
            "surface policy must not replace the possessed spring-arm writer"
        );
        assert!(app.world().get::<SurfaceCamera>(avatar).is_none());
        assert!(app.world().get::<SurfaceRelativeMode>(avatar).is_some());
    }

    #[test]
    fn spring_arm_owns_render_pose_without_interaction_easing() {
        let mut app = App::new();
        app.add_systems(Update, sync_avatar_easing);

        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                SpringArmCamera {
                    target: Entity::PLACEHOLDER,
                    distance: 10.0,
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                    vertical_offset: 2.0,
                    track_heading: true,
                    attitude: FollowAttitude::Heading,
                },
                lunco_time::InteractionEased::default(),
            ))
            .id();

        app.update();
        assert!(
            app.world()
                .get::<lunco_time::InteractionEased>(avatar)
                .is_none(),
            "spring-arm cameras must not have a second interpolation writer"
        );

        app.world_mut()
            .entity_mut(avatar)
            .remove::<SpringArmCamera>();
        app.update();
        assert!(
            app.world()
                .get::<lunco_time::InteractionEased>(avatar)
                .is_some(),
            "stepped camera modes must regain interaction-rate easing"
        );
    }

    #[test]
    fn orbit_camera_owns_complete_big_space_pose_without_local_easing() {
        let mut app = App::new();
        app.add_systems(Update, sync_avatar_easing);

        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                OrbitCamera {
                    target: Entity::PLACEHOLDER,
                    distance: 1_800_000.0,
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                    vertical_offset: 0.0,
                },
                lunco_time::InteractionEased::default(),
            ))
            .id();

        app.update();
        assert!(
            app.world()
                .get::<lunco_time::InteractionEased>(avatar)
                .is_none(),
            "an orbit camera must never lerp cell-local transforms across cells"
        );
    }

    #[test]
    fn surface_camera_owns_local_pose_without_interaction_easing() {
        let mut app = App::new();
        app.add_systems(Update, sync_avatar_easing);

        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                SurfaceCamera {
                    heading: 0.0,
                    pitch: -0.2,
                },
                lunco_time::InteractionEased::default(),
            ))
            .id();

        app.update();
        assert!(
            app.world()
                .get::<lunco_time::InteractionEased>(avatar)
                .is_none(),
            "surface cameras must have one authoritative local-pose writer"
        );

        app.world_mut().entity_mut(avatar).remove::<SurfaceCamera>();
        app.update();
        assert!(
            app.world()
                .get::<lunco_time::InteractionEased>(avatar)
                .is_some(),
            "an avatar without a direct-pose camera mode must regain easing"
        );
    }

    #[test]
    fn cinematic_camera_owns_render_pose_without_interaction_easing() {
        let mut app = App::new();
        app.add_systems(Update, sync_avatar_easing);

        let avatar = app
            .world_mut()
            .spawn((Avatar, lunco_core::CinematicCameraLock))
            .id();

        app.update();
        assert!(
            app.world()
                .get::<lunco_time::InteractionEased>(avatar)
                .is_none(),
            "cinematic cameras must not acquire the avatar interpolation writer"
        );

        app.world_mut()
            .entity_mut(avatar)
            .insert(lunco_time::InteractionEased::default());
        app.update();
        assert!(
            app.world()
                .get::<lunco_time::InteractionEased>(avatar)
                .is_none(),
            "a cinematic lock must remove stale avatar interpolation history"
        );
    }

    /// **A retired avatar camera must leave the viewport candidate pool.**
    ///
    /// Regression for the "two cameras / the view jumps between them" report. The
    /// An explicit host-created avatar camera can be spawned in code
    /// (`spawn_avatar_camera`), not from a prim, so a scene load's despawn sweep
    /// never removes it. When the
    /// incoming scene authors its own `lunco:avatar` camera, `LocalAvatar` moves
    /// (singular by construction) and this observer demotes the old one — but it used
    /// to leave `SceneCamera` on, and `SceneCamera` is exactly what every
    /// camera-selection query filters on. The stale camera stayed eligible, kept a
    /// lower entity index than anything the new scene spawns, and used to be picked by
    /// implicit selection the moment the binding went briefly invalid — which happens
    /// on every load, since the new camera's
    /// `Projection` arrives with a deferred `Camera3d`.
    ///
    /// `Camera` must SURVIVE: stripping it from a live extracted window camera
    /// crashes the render app on the shadow cascade unwrap.
    #[test]
    fn demoted_avatar_camera_stops_being_a_viewport_candidate() {
        let mut app = App::new();
        app.init_resource::<lunco_core::TheLocalAvatar>();
        app.add_observer(demote_former_avatar);

        let old = app
            .world_mut()
            .spawn((
                Camera::default(),
                lunco_render::scene_camera_look_with_profile(
                    None,
                    lunco_render::RenderingQuality::Balanced.profile(),
                ),
                lunco_core::Avatar,
                LocalAvatar,
            ))
            .id();
        assert!(
            app.world().get::<lunco_render::SceneCamera>(old).is_some(),
            "precondition: the explicit host camera starts as a viewport candidate"
        );

        // The scene's own avatar camera arrives and claims the role. `lunco_core`'s
        // component hook demotes `old`, which fires the observer under test.
        let new = app
            .world_mut()
            .spawn((
                Camera::default(),
                lunco_render::scene_camera_look_with_profile(
                    None,
                    lunco_render::RenderingQuality::Balanced.profile(),
                ),
                lunco_core::Avatar,
                LocalAvatar,
            ))
            .id();
        app.update();

        assert_eq!(
            app.world().resource::<lunco_core::TheLocalAvatar>().0,
            Some(new),
            "the incoming camera holds the avatar role"
        );
        assert!(
            app.world().get::<lunco_render::SceneCamera>(old).is_none(),
            "the retired camera must leave the viewport pool, or implicit selection \
             can put it back on screen — the two-camera bug"
        );
        assert!(
            app.world().get::<lunco_render::SceneCamera>(new).is_some(),
            "the live camera stays a candidate"
        );
        assert!(
            app.world().get::<Camera>(old).is_some(),
            "`Camera` must survive demotion: removing it from an extracted window \
             camera crashes the render app on the shadow cascade unwrap"
        );
    }
}

// ── Command Registration ────────────────────────────────────────────────────────

/// Diagnostic read-out of every **commandable** vessel's *control authority* state —
/// the chain that decides whether the stick actually flies it:
/// `GlobalEntityId` (needed for ownership + the model's `piloted` sensor),
/// `ControlBinding` (intent→port map from the USD `Controls` scope), and whether
/// the `SessionRegistry` currently records an owner (⇒ `piloted = 1`). Logs one
/// `[inspect]` line per vessel at INFO. API-driven: `{"type":"ExecuteCommand","command":"InspectVessels"}`.
#[lunco_core::Command(default)]
pub struct InspectVessels {}

#[on_command(InspectVessels)]
fn on_inspect_vessels(_t: On<InspectVessels>, mut commands: Commands) {
    commands.queue(|world: &mut World| {
        // Collect first so the &mut World query borrow ends before the immutable
        // per-entity component reads below.
        let mut q = world.query_filtered::<Entity, bevy::prelude::Or<(
            bevy::prelude::With<lunco_core::ControlBinding>,
            bevy::prelude::With<lunco_cosim::SimComponent>,
        )>>();
        let ents: Vec<Entity> = q.iter(world).collect();
        info!("[inspect] {} commandable vessel(s)", ents.len());
        for e in ents {
            let name = world
                .get::<Name>(e)
                .map(|n| n.as_str().to_string())
                .unwrap_or_default();
            let gid = world.get::<lunco_core::GlobalEntityId>(e).map(|g| g.get());
            let has_cmd = world.get::<lunco_core::InputPorts>(e).is_some();
            let has_sim = world.get::<lunco_cosim::SimComponent>(e).is_some();
            let has_sel = world.get::<lunco_core::SelectableRoot>(e).is_some();
            let binding = world.get::<lunco_core::ControlBinding>(e).map(|b| {
                let ports: Vec<&str> = b.ports().collect();
                (b.binds.len(), ports.join(","))
            });
            let owner = gid.and_then(|g| {
                world
                    .get_resource::<lunco_core::SessionRegistry>()
                    .and_then(|r| r.owner_of(g))
            });
            info!(
                "[inspect] {e:?} name={name:?} gid={gid:?} cmd_surface={has_cmd} sim={has_sim} \
                 selectable={has_sel} binding={binding:?} owner={owner:?} piloted={}",
                owner.is_some() as u8
            );
        }
    });
}

// Wires the avatar's commands into `register_all_commands(app)`, called from
// LunCoAvatarPlugin::build(). (`CaptureScreenshot` used to be first in this list; it
// was a dead duplicate and is gone — the one live registration is `lunco-api`'s.
// See the `screenshot` note at the top of this file.)
register_commands!(
    on_show_notification,
    on_set_camera_input,
    on_surface_teleport_command,
    on_leave_surface_command,
    on_possess_command,
    on_return_from_orbit,
    on_release_command,
    on_focus_command,
    on_follow_command,
    on_update_profile,
    on_inspect_vessels
);
