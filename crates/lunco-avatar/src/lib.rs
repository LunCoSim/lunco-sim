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
use lunco_core::{Avatar, CelestialBody, Spacecraft, register_commands, on_command, SessionProfiles, LocalSession, NetworkRole, LocalAvatar};
/// Topology test for "possessable/controllable": has a control surface
/// (`FlightSoftware`) or a Modelica actuation backend (`SimComponent`). Replaces
/// the removed `Vessel` marker — a click possesses anything matching this.
type Controllable = bevy::prelude::Or<(
    bevy::prelude::With<lunco_fsw::FlightSoftware>,
    bevy::prelude::With<lunco_cosim::SimComponent>,
)>;
use lunco_core::attach::migrate_to_grid;
use lunco_celestial::{LocalGravityField, TeleportToSurface, LeaveSurface};
use lunco_time::{TimeTransport, TransportMode, WorldTime};
use lunco_environment::{GravityBody, GravityProvider};
use lunco_settings::{AppSettingsExt, ProfileSettings};

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
/// Per-avatar mouse-wheel zoom accumulator. Fed each frame by
/// [`collect_camera_zoom`] from the `UserIntent::Zoom` axis (gated on
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
/// ~50× the old `CameraScrollSensitivity` default (0.1): that value was tuned for
/// the egui bridge's **pixel** scroll deltas (~50 px/notch), but the `Zoom` intent
/// now comes from leafwing `MouseScrollAxis::Y` in **line** units (~1.0/notch), so
/// the same feel needs a proportionally larger constant. `5.0` ≈ the old ~5%
/// zoom-per-notch.
const ZOOM_SENSITIVITY: f32 = 5.0;

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
/// [`FollowAttitude`] mode selects how orientation is derived. Runs at the
/// fixed physics cadence (`spring_arm_system`, `FixedPostUpdate`) and reads the
/// target's live interpolated pose, so the camera and the followed body share
/// ONE motion basis — the fix for the fast-flyer jitter that came from solving
/// the follow at render rate against a frame-stale target sample.
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

/// Marks an `OrbitCamera` that should re-aim onto the SUNLIT side of its
/// (celestial) target before the first orbit step. The arrival direction
/// needs sun/target `GlobalTransform`s, which are only frame-consistent in
/// `First` — so `sample_orbit_frame` computes it there and removes the marker.
#[derive(Component, Debug, Clone, Copy)]
pub struct SunlitArrival;

/// Marks an `OrbitCamera` whose arm should be derived from the camera's
/// CURRENT position — the pose-preserving arrival of the surface→orbital
/// scroll-out transit (`scroll_out_to_orbit_system`), vs [`SunlitArrival`]
/// which re-aims at the sunlit side. Resolved in `sample_orbit_frame`
/// (`First`, frame-consistent GTs): the body→camera direction becomes the
/// arm's yaw/pitch in the body's inertial host-grid axes, and the true range
/// becomes the arm length — the camera doesn't move on mode entry.
#[derive(Component, Debug, Clone, Copy)]
pub struct RadialArrival;

/// Frame-consistent spatial inputs for `orbit_system`, sampled in `First`.
///
/// `orbit_system` runs in `PostUpdate` after Avian's writeback and BEFORE
/// `TransformSystems::Propagate` — at that point `GlobalTransform`s are a
/// mid-frame mixture: physics re-propagates parts of the tree with plain f32
/// math while the site-anchored solar subtree still holds last epoch's (or a
/// heliocentric) pose. Reading the target GT there aimed the camera at a
/// phantom ~1.5e11 m out and the orbit ran away. In `First` nothing has
/// written a transform yet, so last frame's fully-propagated GTs are
/// mutually consistent by construction (same guarantee `PendingFocus` uses).
#[derive(Component, Debug, Clone, Copy)]
pub struct OrbitFrameSample {
    /// The orbit target this sample was taken for — a refocus invalidates it.
    pub target: Entity,
    /// Target's absolute position in the root grid's frame, snapshotted at
    /// sample time: `cam_abs(sample) + R⁻¹·(tgt_gt − cam_gt)`, both GTs from
    /// the SAME (last) frame. Deriving it in orbit_system from the CURRENT
    /// camera position instead leaked the camera's own last step into the
    /// target estimate — a metres-scale limit cycle ("jumps back and forth")
    /// while the target drifts with the epoch.
    pub target_pos: DVec3,
    /// Camera position in the root grid's frame (for re-anchoring migration).
    pub cam_root: DVec3,
    /// Camera rotation in the root grid's frame — the world-axes counterpart
    /// of the live orbital rotation (which is in HOST-grid axes). The
    /// scroll-through exit stamps this into `pin.anchor_rotation` so
    /// free-flight resumes looking exactly where the orbital zoom left off.
    pub cam_rot_root: Quat,
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
    mut registry: ResMut<lunco_core::SessionRegistry>,
) {
    // Record ownership on the authoritative peer: Host, and also single-player
    // Standalone (whose authority is local) so the control-authority yield/takeover
    // works offline. Only a Client defers to the host's table.
    if matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }
    let cmd = trigger.event();
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
            if cur != origin && lunco_core::session::may_control(&registry, &rbac, origin, gid.get()) {
                registry.release_session(cur);
                info!("[auth] session {origin} took control of entity {} from {cur} (policy allowed)", gid.get());
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
        app.init_resource::<MouseSensitivity>()
           .init_resource::<CameraDefaults>()
           .init_resource::<SurfaceModeThreshold>();
        // The render-rate camera systems (free-flight, the paused-follow twin) read the
        // wall-rooted `lunco_time::InteractionTime` so they keep moving while the sim is
        // paused. That clock is part of the time spine, so guarantee the spine rather
        // than degrade to a raw `Time<Real>` fallback.
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
        }
        // `science::take_photo` is registered by `lunco-workbench`'s `ScreenshotPlugin`,
        // not here: the tool's closure triggers `CaptureFromCamera`, whose observer is a
        // render-world readback this crate deliberately cannot link.
        app.add_plugins(InputManagerPlugin::<UserIntent>::default());
        app.add_observer(on_user_intent);
        // Secondary observers on the SAME verbs — the authority-bookkeeping leg,
        // not the command handlers (those go through `register_commands!`).
        app.add_observer(record_possession_authority);
        app.add_observer(release_possession_authority);
        // Frame-consistent GT sampling for the orbit camera (see
        // `OrbitFrameSample`): First is the only point in the frame where
        // camera and site-anchored celestial GTs share one convention.
        app.add_systems(bevy::app::First, sample_orbit_frame);
        // Scene-click possession/follow/focus is now bevy_picking-driven: a
        // global `Pointer<Click>` observer (egui occlusion handled by the
        // framework), replacing the old `ScenePointer`-gated Update system.
        //
        // The observer reads two click-arbitration resources — `DragModeActive`
        // (gizmo drag in progress) and `SpawnToolActive` (click-to-place armed).
        // Both are normally owned by the editor (`lunco-sandbox-edit`), but the
        // observer lives here and fires on the FIRST pointer event, so a binary
        // that uses the avatar without the editor (luncosim) would panic on the
        // missing `Res`. Guarantee them here — `init_resource` is idempotent, so
        // a host that inserts its own (sandbox) keeps that value.
        app.init_resource::<lunco_core::DragModeActive>();
        app.init_resource::<lunco_core::SpawnToolActive>();
        app.init_resource::<lunco_core::TerrainToolActive>();
        app.init_resource::<lunco_core::WaypointToolActive>();
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
        // Mirror native possession onto the `cmd:*` script/telemetry bus (the UI
        // path bypasses ApiCommandEvent) so tutorials can advance on it.
        app.add_observer(project_possess_event);

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
           .register_type::<ProvisionalAvatarCamera>()
           .register_type::<SurfaceRelativeMode>()
           .register_type::<SurfaceCamera>()
           .register_type::<SurfaceModeThreshold>()
           .register_type::<MouseSensitivity>();

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

        app.add_systems(Update, (
            avatar_init_system,
            surface_mode_transition_system,
            enforce_ownership,
            sync_profile,
            tick_notifications,
            // Mouse-wheel → per-avatar zoom accumulator, sourced from the `Zoom`
            // intent and gated on egui pointer capture (replaces the old egui
            // `CameraScroll` bridges). Runs before the camera systems consume it.
            collect_camera_zoom,
        ));
        // Mouse-look capture + apply. Pointer intents — gated internally on
        // `EguiFocus.wants_pointer` (look_delta is zeroed while a panel holds the
        // pointer), NOT on keyboard focus, so typing never freezes the camera.
        app.add_systems(Update, (
            capture_avatar_intent,
            avatar_behavior_input_system,
        ));

        // Headless drag simulator: `LUNCO_AUTO_ORBIT=<rad/s>`.
        //
        // Orbital rotate/zoom are raw mouse input the API cannot inject, and
        // `FocusEntityById`'s `distance` is ignored once the pin owns the view —
        // so orbit-line drag jitter was not reproducible without a human. A
        // right-drag does exactly one thing: add a delta to `OrbitCamera::yaw`
        // (see `avatar_behavior_input_system`). Driving that same field exercises
        // the identical `orbit_system` → `OrbitalViewPin` → `anchor_solar_frame_to_site`
        // → `trajectory_alignment_system` path. Diagnostic only; off by default.
        if let Some(rate) = std::env::var("LUNCO_AUTO_ORBIT")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            app.add_systems(
                Update,
                move |time: Res<Time>, mut q: Query<&mut OrbitCamera, With<Avatar>>| {
                    for mut orbit in q.iter_mut() {
                        orbit.yaw += rate * time.delta_secs();
                    }
                },
            );
        }
        // Discrete KEYBOARD intents: `Cancel` (release possession/follow) and the
        // `Pause` hotkey. Gated so a key typed into a focused egui field doesn't
        // fire them. `Cancel`/Backspace is the two-step Esc pattern: while a field
        // is focused egui consumes the key (guard suppresses the intent); once
        // defocused, the next press acts.
        app.add_systems(Update, (
            avatar_escape_possession,
            avatar_global_hotkeys,
        ).run_if(scene_keyboard_active));

        // Possessed-rover name tags: an egui screen-space overlay (the scene has
        // only a `Camera3d`, so world-anchored `Text2d` never renders). Registered
        // here — not in `AvatarUiPlugin` — because the sandbox adds only
        // `LunCoAvatarPlugin`; `AvatarUiPlugin` is luncosim-only.
        #[cfg(feature = "ui")]
        app.add_systems(bevy_egui::EguiPrimaryContextPass, crate::ui::draw_rover_name_tags);
        #[cfg(feature = "ui")]
        app.add_systems(bevy_egui::EguiPrimaryContextPass, crate::ui::draw_notifications);

        // The CHASE camera shares the followed body's FIXED-STEP time domain: it runs in
        // `FixedPostUpdate`, reads the body's pose on the SAME cadence avian integrates
        // it, and the camera entity carries avian's `TranslationInterpolation`/
        // `RotationInterpolation` (added at camera insertion). So
        // `bevy_transform_interpolation` eases the camera between fixed samples with the
        // SAME `overstep` it eases the body — camera and body stay in LOCKSTEP at render
        // rate. That lockstep is what hides the body's own big_space cell-rebase
        // interpolation jitter (see `lunco_physics`'s `[JITTER]` probe): a follow camera
        // on a *different* clock reads a body that jumps under it, and the jump shows as
        // camera jitter. `spring_arm_paused_system` (below) is its wall-clock twin for
        // when the sim is paused and `FixedPostUpdate` stops.
        app.add_systems(
            FixedPostUpdate,
            spring_arm_system.after(avian3d::schedule::PhysicsSystems::Writeback),
        );

        // The render-rate camera systems. These read the wall-rooted INTERACTION clock
        // (`lunco_time::InteractionTime`) so the avatar stays live while the sim is
        // PAUSED and keeps its real-time feel while the sim is WARPED — pausing or
        // fast-forwarding the simulation must not paralyse or double-speed the user's
        // view. None of them follow an avian body at chase range, so they need no
        // fixed-step lockstep.
        // The cinematic-lock janitor runs in PreUpdate so a lock inserted (or a
        // mode re-inserted by ReleaseVessel) is stripped before this frame's
        // camera systems reason about mode components. The Without<…> guards on
        // those systems are the same-frame protection; this is the cleanup.
        app.add_systems(PreUpdate, strip_camera_modes_from_locked);

        app.add_systems(PostUpdate, (
            orbital_exit_restore_system,
            // Entry half of the scroll transit — must see the free-flight scroll BEFORE
            // orbit_system's zoom consumption; its mode swap lands next frame (commands).
            freeflight_scroll_transit_system,
            freeflight_system,
            surface_camera_system,
            apply_fly,
            // The ORBIT (survey) camera belongs here, not in `FixedPostUpdate` where it
            // used to sit next to `spring_arm_system`. It already reads `InteractionTime`
            // — a per-FRAME wall delta — so running it on the fixed cadence applied one
            // whole frame's delta once PER SUBSTEP: at 4x its smoothing converged ~4x too
            // fast, and when paused it froze outright (unlike the chase camera it has no
            // paused twin). Its own doc asks for "a constant `dt` that does not rate-scale
            // with the sim"; the registration was what defeated that. It is a star-fixed
            // survey camera, so it does not need the chase camera's interpolation lockstep.
            orbit_system,
            update_avatar_clip_planes_system,
            // Paused-follow twin: `FixedPostUpdate` stops when the sim pauses, so this
            // render-rate twin keeps the follow camera live on the wall clock while
            // paused. Gated so it and `spring_arm_system` never both write the camera.
            spring_arm_paused_system.run_if(|transport: Option<Res<lunco_time::TimeTransport>>| {
                transport.map_or(false, |t| matches!(t.mode, lunco_time::TransportMode::Paused))
            }),
        ).chain().in_set(AvatarCameraSet));

        app.configure_sets(
            PostUpdate,
            AvatarCameraSet
                .after(avian3d::schedule::PhysicsSystems::Writeback)
                .before(bevy::transform::TransformSystems::Propagate),
        );
        // Keep avian's fixed-step interpolation only on the fixed-step follow camera,
        // regardless of which camera-swap path an avatar takes.
        app.add_systems(Update, strip_non_follow_camera_interpolation);

        // NOTE: there used to be a second, PostUpdate registration of
        // `anchor_solar_frame_to_site` here for same-frame drag re-pins. The
        // orbital view no longer re-poses the world (the camera itself flies —
        // see `orbit_system`'s celestial branch), so the pin runs only at its
        // canonical PreUpdate slot on epoch/site changes.
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct AvatarCameraSet;

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
    //
    // `Camera` + `SceneCamera` (both render-FREE) instead of `Camera3d`: the render
    // *pipeline* half — `Camera3d`, tonemapping, MSAA, bloom — is attached by
    // `lunco-render-bevy`'s `SceneCamera` binder in render builds, and simply never
    // attached headless, where the camera stays a fully-formed scene entity (pose,
    // projection, tracking, mounts) with no GPU pipeline. See
    // `lunco_render::camera` and docs/architecture/render-decoupling.md.
    commands.spawn((
        // Nested: a bundle tuple maxes out at 16 elements, and `SceneCamera` made 17.
        (Camera::default(), lunco_render::SceneCamera::default()),
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
        CameraZoomInput::default(),
        Name::new("Avatar Camera"),
        ChildOf(grid_entity),
    )).id()
}

/// The local avatar is a **controllable described like a rover**: it carries an FSW
/// command surface (`forward`/`side`/`up` input ports) + a `ControlBinding` mapping
/// move intents to those ports. The SAME `lunco_controller::drive_from_bindings`
/// path then drives it — its *self-drive* branch fires for an entity that holds its
/// own `ActionState` + `ControlBinding` and, when free, no `ControllerLink`
/// (possession adds a `ControllerLink→vessel`, which excludes the avatar from
/// self-drive and redirects control to the vessel — no possession-code changes).
/// `apply_fly` reads the resulting `forward`/`side`/`up` ports back.
///
/// The command *vocabulary* is seeded from the binding by
/// `lunco_mobility::sync_fsw_command_surface`, exactly like a rover. Authored in
/// code for now; P3 will move it to an `_AvatarControl` USD profile so the avatar
/// is spawned identically via code or USD.
fn stamp_avatar_controls(trigger: On<Add, LocalAvatar>, mut commands: Commands) {
    let binding = lunco_core::ControlBinding::from_intent_entries(&[
        ("MoveForward".to_string(), "forward".to_string(), 1.0),
        ("MoveBackward".to_string(), "forward".to_string(), -1.0),
        ("MoveRight".to_string(), "side".to_string(), 1.0),
        ("MoveLeft".to_string(), "side".to_string(), -1.0),
        ("MoveUp".to_string(), "up".to_string(), 1.0),
        ("MoveDown".to_string(), "up".to_string(), -1.0),
    ]);
    let mut ec = commands.entity(trigger.entity);
    // Empty port_map (no hardware actuators — `apply_fly` reads the command inputs
    // directly); the `forward`/`side`/`up` surface is filled from the binding.
    ec.try_insert(lunco_fsw::FlightSoftware::new(std::collections::HashMap::new(), &[]));
    if let Some(b) = binding {
        ec.try_insert(b);
    }
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
/// `apply_fly`, and `avatar_behavior_input_system`).
fn radial_up(pos: DVec3) -> Vec3 {
    if pos.length() > 1e-6 {
        (pos / pos.length()).as_vec3()
    } else {
        Vec3::Y
    }
}

/// The site anchor's body: `(body entity, radius, body centre in the SITE
/// frame)` — `None` when the scene isn't site-anchored. The world origin IS
/// the anchor point with up = +Y (`anchor_solar_frame_to_site`), so the
/// centre sits `radius + anchor height` straight down.
fn site_body_center(
    q_site: &Query<&lunco_celestial::GeodeticAnchor, With<lunco_celestial::SiteAnchor>>,
    q_bodies: &Query<(Entity, &CelestialBody)>,
) -> Option<(Entity, f64, DVec3)> {
    let anchor = q_site.iter().next()?;
    let (ent, body) = q_bodies.iter().find(|(_, b)| b.ephemeris_id == anchor.body)?;
    Some((
        ent,
        body.radius_m,
        DVec3::new(0.0, -(body.radius_m + anchor.geodetic.height_m), 0.0),
    ))
}

/// Radial "up" for a camera at `pos` in ITS parent grid's frame. In the old
/// rover sandbox the grid origin IS the body centre, so `pos` normalized is
/// the surface normal. A site-anchored scene parents free cameras to the
/// WorldGrid, whose origin is the SITE point on the sphere — there the body
/// centre sits `radius + height` straight down (`site_center`), and ignoring
/// that offset made "up" wrong everywhere except directly over the site.
fn surface_up(pos: DVec3, parent_is_world_grid: bool, site_center: Option<DVec3>) -> Vec3 {
    match site_center {
        Some(c) if parent_is_world_grid => radial_up(pos - c),
        _ => radial_up(pos),
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

/// Position follows the target; [`FollowAttitude`] selects how orientation is
/// derived (heading-lock, world-locked survey, or full-attitude cockpit). Shared by
/// the fixed-step [`spring_arm_system`] (running) and the render-rate
/// [`spring_arm_paused_system`] (paused) — same follow math, different clock.
fn update_spring_arm_impl(
    dt: f32,
    mut q_avatar: Query<(
        Entity,
        &mut Transform,
        &mut CellCoord,
        &mut SpringArmCamera,
        &ChildOf,
        Option<&SurfaceRelativeMode>,
        &mut CameraZoomInput,
    ), (With<Avatar>, Without<FrameBlend>, Without<lunco_core::CinematicCameraLock>)>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_dragging: Query<(), With<lunco_core::GizmoDragging>>,
    q_children: Query<&Children>,
    defaults: &CameraDefaults,
    keys: &ButtonInput<KeyCode>,
    spatial_query: &Option<avian3d::prelude::SpatialQuery>,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) { return; }

    for (_avatar_ent, mut tf, mut cell, mut arm, child_of, surface_mode, mut zoom) in q_avatar.iter_mut() {
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
        apply_scroll_zoom(&mut arm.distance, &mut zoom.delta, ZOOM_SENSITIVITY, 5.0, 200.0);

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
                t_tf.rotation * Quat::from_euler(EulerRot::YXZ, arm.yaw, arm.pitch, 0.0)
            }
            // Stable external frame: ignore the body's attitude entirely, so a
            // 6-DOF flyer tumbles inside a steady view. World-up, user yaw/pitch
            // (was the celestial `OrbitCamera`, reused wrongly for vessels).
            FollowAttitude::WorldLocked => {
                Quat::from_euler(EulerRot::YXZ, arm.yaw, arm.pitch, 0.0)
            }
            // Heading-follow: yaw from the body's forward (steerable vehicles),
            // up = surface normal or world-Y.
            FollowAttitude::Heading => {
                let target_heading_d = if arm.track_heading {
                    let target_fwd_d = t_tf.rotation.mul_vec3(Vec3::NEG_Z).as_dvec3();
                    if target_fwd_d.x.abs() > 1e-6 || target_fwd_d.z.abs() > 1e-6 {
                        -target_fwd_d.x.atan2(-target_fwd_d.z)
                    } else { 0.0 }
                } else {
                    0.0
                };
                let final_yaw = (target_heading_d + arm.yaw as f64) as f32;
                if surface_mode.is_some() {
                    // "Up" = surface normal at the rover's position = rover's grid-local direction from body center.
                    // Both rover and camera are on the Body's Grid; body is at Grid origin.
                    // Compute rotation from scratch using local_up as "up" —
                    // avoids accumulated roll drift from incremental rotations
                    // (see surface_camera_investigation.md for root cause analysis).
                    tangent_frame(radial_up(target_pos), final_yaw, arm.pitch)
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
        // Mask out the TRIGGER layer so the camera doesn't clip on invisible
        // trigger-zone sensors (waypoints etc.).
        //
        // The exclusion set is the target's WHOLE SUBTREE, not just its root. On a
        // USD vessel the root carries the RigidBody but the colliders are child
        // prims (chassis, wheels), so excluding the root alone left the arm's ray
        // hitting the rover's own body ~0.5 m out — `target_len` collapsed to ~0 and
        // the camera sank into/under the rover on possess. Same set autopilot builds
        // for its own casts (`lunco-autopilot/src/lib.rs:1508`).
        let mut excluded = vec![arm.target];
        collect_subtree(arm.target, &q_children, &mut excluded);
        let mut filter = avian3d::prelude::SpatialQueryFilter::from_excluded_entities(excluded);
        filter.mask = avian3d::prelude::LayerMask(!lunco_core::TRIGGER_COLLISION_LAYER);
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

/// SpringArm camera, fixed-step half. Runs in `FixedPostUpdate` so its slerp/lerp uses
/// a constant `dt = 1/60s` AND it reads the followed body's pose on the SAME cadence
/// avian integrates it. The camera entity carries avian's
/// `TranslationInterpolation`/`RotationInterpolation`, so `bevy_transform_interpolation`
/// eases it between fixed samples in LOCKSTEP with the body — which hides the body's own
/// big_space cell-rebase interpolation jitter (a follow camera on a *different* clock
/// reads a body that jumps under it, and shows the jump as camera jitter).
///
/// `FixedPostUpdate` stops when the sim pauses, so [`spring_arm_paused_system`] is the
/// render-rate twin that keeps the follow camera live on the wall clock while paused.
fn spring_arm_system(
    time: Res<Time>,
    q_avatar: Query<(
        Entity,
        &mut Transform,
        &mut CellCoord,
        &mut SpringArmCamera,
        &ChildOf,
        Option<&SurfaceRelativeMode>,
        &mut CameraZoomInput,
    ), (With<Avatar>, Without<FrameBlend>, Without<lunco_core::CinematicCameraLock>)>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_dragging: Query<(), With<lunco_core::GizmoDragging>>,
    q_children: Query<&Children>,
    defaults: Res<CameraDefaults>,
    keys: Res<ButtonInput<KeyCode>>,
    spatial_query: Option<avian3d::prelude::SpatialQuery>,
) {
    update_spring_arm_impl(
        time.delta_secs(),
        q_avatar,
        q_spatial,
        q_grids,
        q_dragging,
        q_children,
        &defaults,
        &keys,
        &spatial_query,
    );
}

/// SpringArm camera, paused half — see [`spring_arm_system`]. `FixedUpdate` (and so
/// `FixedPostUpdate`) stops when the sim pauses, so this render-rate twin keeps the
/// follow camera live on the wall-rooted INTERACTION clock. It runs only while paused
/// (gated at registration), so it never fights the fixed-step system for the camera.
fn spring_arm_paused_system(
    time_real: lunco_time::InteractionTime,
    q_avatar: Query<(
        Entity,
        &mut Transform,
        &mut CellCoord,
        &mut SpringArmCamera,
        &ChildOf,
        Option<&SurfaceRelativeMode>,
        &mut CameraZoomInput,
    ), (With<Avatar>, Without<FrameBlend>, Without<lunco_core::CinematicCameraLock>)>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_dragging: Query<(), With<lunco_core::GizmoDragging>>,
    q_children: Query<&Children>,
    defaults: Res<CameraDefaults>,
    keys: Res<ButtonInput<KeyCode>>,
    spatial_query: Option<avian3d::prelude::SpatialQuery>,
) {
    update_spring_arm_impl(
        time_real.delta_secs(),
        q_avatar,
        q_spatial,
        q_grids,
        q_dragging,
        q_children,
        &defaults,
        &keys,
        &spatial_query,
    );
}

/// Maintains the invariant: avian's fixed-step interpolation lives ONLY on the
/// fixed-step follow camera (`SpringArmCamera` → `spring_arm_system`, `FixedPostUpdate`).
/// Every other camera (`OrbitCamera` runs fixed too, but free-flight/surface write at
/// render rate) would have the interpolation FIGHT its writer — a render-rate write
/// eased against a stale fixed sample. So strip it from any avatar that is not currently
/// a `SpringArmCamera`, no matter which transition path got it there. Change-driven:
/// only touches entities that actually carry a stale interpolation marker.
fn strip_non_follow_camera_interpolation(
    mut commands: Commands,
    q: Query<
        Entity,
        (
            With<Avatar>,
            Without<SpringArmCamera>,
            Or<(
                With<avian3d::prelude::TranslationInterpolation>,
                With<avian3d::prelude::RotationInterpolation>,
            )>,
        ),
    >,
) {
    for e in &q {
        commands
            .entity(e)
            .remove::<avian3d::prelude::TranslationInterpolation>()
            .remove::<avian3d::prelude::RotationInterpolation>();
    }
}

/// Samples the frame-consistent spatial inputs for `orbit_system` (see
/// [`OrbitFrameSample`]) and resolves a pending [`SunlitArrival`] aim.
///
/// Runs in `First`: the only schedule point where the camera's and a
/// site-anchored celestial target's `GlobalTransform`s are guaranteed to be
/// in ONE convention (last frame's final propagation).
fn sample_orbit_frame(
    mut q_avatar: Query<
        (
            Entity,
            &CellCoord,
            &Transform,
            &ChildOf,
            &mut OrbitCamera,
            Has<SunlitArrival>,
            Has<RadialArrival>,
        ),
        With<Avatar>,
    >,
    q_globals: Query<&GlobalTransform>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_bodies: Query<&CelestialBody>,
    q_body_ents: Query<(Entity, &CelestialBody)>,
    q_children: Query<&Children>,
    q_frames_ids: Query<&lunco_celestial::CelestialReferenceFrame>,
    mut commands: Commands,
) {
    for (avatar_ent, cam_cell, cam_tf, cam_child_of, mut orbit, wants_sunlit, wants_radial) in
        q_avatar.iter_mut()
    {
        // Root grid of the target's hierarchy (the grid orbit_system anchors to).
        let root_grid = {
            let mut g = orbit.target;
            let mut found = None;
            for _ in 0..MAX_HIERARCHY_WALK_DEPTH {
                if q_grids.contains(g) { found = Some(g); }
                match q_parents.get(g) {
                    Ok(parent) => g = parent.parent(),
                    Err(_) => break,
                }
            }
            found
        };
        let sample = root_grid.and_then(|root| {
            let (Ok(cam_gt), Ok(tgt_gt), Ok(root_gt)) = (
                q_globals.get(avatar_ent),
                q_globals.get(orbit.target),
                q_globals.get(root),
            ) else { return None };
            let root_inv = root_gt.rotation().inverse();
            // Camera's exact position at sample time (cell math when it is
            // already on the root grid — consistent with the same-frame GTs,
            // since nothing has moved it yet this frame); the target's
            // absolute position is anchored to it.
            let cam_abs = if cam_child_of.parent() == root {
                q_grids
                    .get(root)
                    .map(|g| g.grid_position_double(cam_cell, cam_tf))
                    .unwrap_or_else(|_| {
                        (root_inv * (cam_gt.translation() - root_gt.translation())).as_dvec3()
                    })
            } else {
                (root_inv * (cam_gt.translation() - root_gt.translation())).as_dvec3()
            };
            Some(OrbitFrameSample {
                target: orbit.target,
                target_pos: cam_abs
                    + (root_inv * (tgt_gt.translation() - cam_gt.translation())).as_dvec3(),
                cam_root: (root_inv * (cam_gt.translation() - root_gt.translation())).as_dvec3(),
                cam_rot_root: (root_inv * cam_gt.rotation()).normalize(),
            })
        });
        match sample {
            Some(s) => {
                commands.entity(avatar_ent).try_insert(s);

                // Resolve a pending arrival in the same consistent frame.
                if wants_sunlit || wants_radial {
                    let physical_target = get_physical_body(orbit.target, &q_children, &q_bodies);
                    // The aim must be expressed in the frame the celestial
                    // branch renders in: the body's INERTIAL host grid
                    // (+Y = engine north; skip the body's own spinning
                    // frame) — NOT the world/site-ENU axes.
                    let body_eph = q_bodies
                        .get(physical_target)
                        .map(|b| b.ephemeris_id)
                        .unwrap_or(i32::MIN);
                    let mut walk = physical_target;
                    let mut host = None;
                    for _ in 0..MAX_HIERARCHY_WALK_DEPTH {
                        let Ok(co) = q_parents.get(walk) else { break };
                        let parent = co.parent();
                        if q_grids.contains(parent) {
                            let own_spin = q_frames_ids
                                .get(parent)
                                .is_ok_and(|f| f.ephemeris_id == body_eph)
                                && q_parents
                                    .get(parent)
                                    .is_ok_and(|pp| q_grids.contains(pp.parent()));
                            if !own_spin {
                                host = Some(parent);
                                break;
                            }
                        }
                        walk = parent;
                    }
                    let host_inv = host
                        .and_then(|h| q_globals.get(h).ok())
                        .map(|g| g.rotation().inverse())
                        .unwrap_or(Quat::IDENTITY);
                    if wants_radial {
                        // Pose-preserving arrival (surface→orbital scroll-out):
                        // the arm is the live body→camera vector, so the mode
                        // flip doesn't move the camera — only the subsequent
                        // scroll does.
                        if let (Ok(cam_gt), Ok(tgt_gt)) =
                            (q_globals.get(avatar_ent), q_globals.get(physical_target))
                        {
                            let d = cam_gt.translation() - tgt_gt.translation();
                            if d.length_squared() > 1.0 {
                                let dir = (host_inv * d).normalize();
                                orbit.pitch = dir.y.clamp(-1.0, 1.0).asin();
                                orbit.yaw = (-dir.x).atan2(-dir.z);
                                orbit.distance = d.length() as f64;
                                info!(
                                    "ORBIT ARRIVAL: radial (pose-preserving) yaw={:.2} pitch={:.2} dist={:.3e}",
                                    orbit.yaw, orbit.pitch, orbit.distance
                                );
                            }
                        }
                        commands.entity(avatar_ent).remove::<RadialArrival>();
                    } else {
                        // Aim the arrival at the sunlit side. A body focused
                        // from a random side is usually its night hemisphere —
                        // a black disc on black space.
                        let sun = q_body_ents
                            .iter()
                            .find(|(e, b)| b.ephemeris_id == 10 && *e != physical_target);
                        if let Some((sun_ent, _)) = sun {
                            if let (Ok(sun_gt), Ok(tgt_gt)) =
                                (q_globals.get(sun_ent), q_globals.get(physical_target))
                            {
                                let to_sun =
                                    host_inv * (sun_gt.translation() - tgt_gt.translation());
                                if to_sun.length_squared() > 1.0 {
                                    let fwd = -to_sun.normalize();
                                    orbit.pitch = fwd.y.clamp(-1.0, 1.0).asin();
                                    // Small yaw offset off the exact sun line keeps
                                    // the terminator visible — a gibbous disk has
                                    // depth, a dead-on full disk is flat.
                                    orbit.yaw = (-fwd.x).atan2(-fwd.z) + 0.4;
                                    info!(
                                        "ORBIT ARRIVAL: sunlit aim yaw={:.2} pitch={:.2}",
                                        orbit.yaw, orbit.pitch
                                    );
                                }
                            }
                        }
                        commands.entity(avatar_ent).remove::<SunlitArrival>();
                    }
                }
            }
            None => {
                commands.entity(avatar_ent).remove::<OrbitFrameSample>();
            }
        }
    }
}

/// Marker: the camera is currently placed on a celestial grid by the orbital
/// view (`orbit_system` inserts it when it migrates the camera to the focused
/// body's host grid). Removed by [`orbital_exit_restore_system`], or by an
/// explicit `SetCamera`, which does its own root-grid migration.
#[derive(Component)]
pub struct OrbitalViewCamera;

/// Exit path of the orbital view: when the pin deactivates (Backspace
/// release, refocus on a scene entity, an API camera command, …) the camera
/// is still parented to a celestial grid near the focused body. Migrate it
/// back to the root grid at the pose parked on mode entry. Runs at the head
/// of `AvatarCameraSet` so this frame's camera systems see the restored
/// state.
fn orbital_exit_restore_system(
    orbital_pin: Option<Res<lunco_celestial::OrbitalViewPin>>,
    q_avatar: Query<Entity, (With<Avatar>, With<OrbitalViewCamera>)>,
    q_world_grid: Query<Entity, With<lunco_core::WorldGrid>>,
    q_grids: Query<&Grid>,
    mut commands: Commands,
) {
    let Some(pin) = orbital_pin else { return };
    if pin.active {
        return;
    }
    let Some(root) = q_world_grid.iter().next() else { return };
    let Ok(root_grid) = q_grids.get(root) else { return };
    for avatar_ent in q_avatar.iter() {
        let (cell, translation) = root_grid.translation_to_grid(pin.anchor_world);
        migrate_to_grid(
            &mut commands,
            avatar_ent,
            root,
            cell,
            Transform::from_translation(translation).with_rotation(pin.anchor_rotation),
        );
        commands.entity(avatar_ent).remove::<OrbitalViewCamera>();
        info!("ORBITAL EXIT: camera restored to parked surface pose");
    }
}

/// OrbitCamera system: positions the camera at a fixed offset from a target,
/// locked to the ecliptic (star-fixed) reference frame.
///
/// Only runs when `OrbitCamera` is present AND no `FrameBlend` is active.
/// The camera does NOT rotate with the target — stars stay still.
fn orbit_system(
    // Wall-rooted interaction clock: orbit smoothing (`alpha = 1 - exp(-rate·dt)`) wants
    // a constant `dt` that does not rate-scale with the sim.
    time: lunco_time::InteractionTime,
    mut q_avatar: Query<(Entity, &mut Transform, &mut CellCoord, &mut OrbitCamera, &ChildOf, &mut CameraZoomInput, Option<&OrbitFrameSample>), (With<Avatar>, Without<FrameBlend>, Without<lunco_core::CinematicCameraLock>)>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_bodies: Query<&CelestialBody>,
    q_frames_ids: Query<&lunco_celestial::CelestialReferenceFrame>,
    q_spatial: Query<(&CellCoord, &Transform), Without<Avatar>>,
    q_sc: Query<&Spacecraft>,
    q_dragging: Query<(), With<lunco_core::GizmoDragging>>,
    defaults: Res<CameraDefaults>,
    keys: Res<ButtonInput<KeyCode>>,
    q_children: Query<&Children>,
    mut commands: Commands,
    mut log_countdown: Local<u32>,
    mut last_pose: (Local<f32>, Local<f32>),
    mut orbital_pin: Option<ResMut<lunco_celestial::OrbitalViewPin>>,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) { return; }

    let dt = time.delta_secs();

    for (avatar_ent, mut tf, mut cell, mut orbit, child_of, mut zoom, sample) in q_avatar.iter_mut() {
        // Skip follow while the target is being dragged by the editor gizmo
        // (marker set by sandbox-edit; never present on a headless server).
        if q_dragging.get(orbit.target).is_ok() { continue; }

        // Spatial inputs come exclusively from the `First`-schedule sample —
        // GlobalTransforms are NOT frame-consistent at this point of the
        // frame (see `OrbitFrameSample`). No sample yet (focus landed this
        // frame) or a stale one (target changed) → wait one frame.
        let Some(sample) = sample else { continue };
        if sample.target != orbit.target { continue; }

        // The orbit camera ALWAYS lives on the ROOT grid (WorldGrid), NOT the
        // target body's grid. big_space rebases every entity relative to the
        // floating origin's cell in its IMMEDIATE grid only — it does NOT
        // subtract ancestor-grid cells. A site-anchored scene pushes the Solar
        // Grid ~1 AU (1.06e11 m) out via a CellCoord; a body's grid inherits
        // that as an ANCESTOR offset. Parenting the floating-origin camera
        // under such a body grid leaves the ancestor 1.06e11 m in the camera's
        // own GlobalTransform → f32 quantises in ~16 km steps → the whole view
        // strobes every frame. Anchored to the ROOT grid, the heliocentric
        // offset lives in the camera's OWN cell, which big_space cancels, so
        // camGT stays small and stable. (Ground view already worked precisely
        // because the avatar is a direct WorldGrid child there.)
        let root_grid = {
            let mut g = orbit.target;
            let mut found = None;
            for _ in 0..MAX_HIERARCHY_WALK_DEPTH {
                if q_grids.contains(g) { found = Some(g); }
                match q_parents.get(g) {
                    Ok(parent) => g = parent.parent(),
                    Err(_) => break,
                }
            }
            match found { Some(g) => g, None => continue }
        };
        let Ok(root_ref) = q_grids.get(root_grid) else { continue };

        // Compute minimum distance to prevent zooming inside the target body.
        // For a celestial body the floor sits ~50 km above the surface — low
        // enough that zooming in reads as a descent, and the floor doubles as
        // the scroll-through exit threshold (see below).
        let physical_target = get_physical_body(orbit.target, &q_children, &q_bodies);
        let min_dist = if let Ok(body) = q_bodies.get(physical_target) {
            body.radius_m + SCROLL_EXIT_ALTITUDE_M
        } else if let Ok(sc) = q_sc.get(orbit.target) {
            (sc.hit_radius_m as f64).max(10.0)
        } else {
            10.0 // Generic fallback minimum distance.
        };
        // CELESTIAL target: fly the CAMERA, never the world (doc 47 Phase 6).
        // The camera — it carries the `FloatingOrigin` — migrates onto the
        // focused body's INERTIAL parent grid (2 km cells, real `CellCoord`s:
        // `translation_to_grid` keeps the f32 remainder ≤ ~1.1 km, ULP
        // ~0.1 mm) and is placed at `body + dir·distance` from the STORED
        // grid chain in f64. A drag then moves only the floating origin, and
        // big_space recomputes every GlobalTransform against it atomically
        // inside its own propagation — there is no per-frame world re-pose
        // for other systems to lag behind. (The previous design slid the
        // whole solar tree around a parked camera; at Earth range a drag
        // moved the tree by ~1e6 m per frame, so ANY one-frame-stale writer —
        // mesh rebuild, body spin, LOD tiles, markers — displaced its entity
        // by megameters: "planets jump around when I rotate".)
        if let (Ok(body), Some(pin)) = (q_bodies.get(physical_target), orbital_pin.as_mut()) {
            // SCROLL-THROUGH to the surface: scrolling IN while the commanded
            // arm already sits ON the min-distance floor is an unambiguous
            // "take me down" — leave the orbital view AT THE CURRENT POSE
            // instead of the pose parked on entry. Stamp the pin's anchor
            // with the camera's present world pose and fire the canonical
            // `ReleaseVessel` unwind (the Backspace / Surface-pill path):
            // `on_release_command` drops the body focus and derives the
            // free-flight view from the stamped rotation, then
            // `orbital_exit_restore_system` migrates the camera to the
            // WorldGrid at the stamped position — the descent continues
            // seamlessly in the surface camera.
            if pin.active
                && zoom.delta > 0.0
                && orbit.distance <= min_dist * 1.0005
                // The camera EASES toward the commanded arm — a fast scroll-in
                // parks the arm on the floor while the camera is still
                // hundreds of km up. Exiting then strands free flight at that
                // altitude (where surface-relative orientation used to
                // disengage → world-levelled camera → "the moon is in the
                // corner"). Hold the exit until the camera has ARRIVED near
                // the floor; extra detents while easing are consumed as
                // ordinary (no-op) zoom and the next one exits on arrival.
                && (sample.cam_root - sample.target_pos).length() <= min_dist * 1.02
                && child_of.parent() != root_grid
            {
                pin.anchor_world = sample.cam_root;
                // Keep the heading but LEVEL OUT against the LOCAL radial up:
                // the orbital attitude looks at the body centre (nadir-ish),
                // which reads upside-down as a surface view. A mild downward
                // pitch below the LOCAL horizon puts the ground at the bottom
                // of the screen and the sky above. (World-Y levelling only
                // worked directly above the site — exiting anywhere else on
                // the globe put "the Moon on top": +Y is not up there.)
                let up_l = (sample.cam_root - sample.target_pos)
                    .normalize_or_zero()
                    .as_vec3();
                let fwd_cur = sample.cam_rot_root * Vec3::NEG_Z;
                let heading = (fwd_cur - up_l * fwd_cur.dot(up_l))
                    .try_normalize()
                    .unwrap_or_else(|| up_l.any_orthonormal_vector());
                // ~31° below the local horizon (cos/sin of the old -0.55 rad).
                let dir = (heading * 0.852 - up_l * 0.522).normalize();
                pin.anchor_rotation = Transform::IDENTITY.looking_to(dir, up_l).rotation;
                zoom.delta = 0.0;
                commands.trigger(ReleaseVessel { target: avatar_ent });
                info!("ORBITAL SCROLL-THROUGH: exiting to surface at current pose");
                continue;
            }
            apply_scroll_zoom(&mut orbit.distance, &mut zoom.delta, ZOOM_SENSITIVITY, min_dist, 1.0e11);
            let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
            let dir = rotation.mul_vec3(Vec3::Z).as_dvec3();

            // Park the surface pose on mode entry — the exit paths
            // (`orbital_exit_restore_system`, `SetCamera`) restore it.
            let (anchor_world, anchor_rotation) = if pin.active {
                (pin.anchor_world, pin.anchor_rotation)
            } else {
                (sample.cam_root, tf.rotation)
            };

            // Host grid: nearest grid ancestor of the body that is NOT the
            // body's own (spinning) reference frame — Earth/Moon focus rides
            // the inertial EMB grid, simple planets ride the Solar Grid.
            // `pose_ent` is the direct child of the host along that walk; its
            // stored (CellCoord, Transform) IS the body's pose in the host.
            let mut pose_ent = physical_target;
            let mut host = None;
            for _ in 0..MAX_HIERARCHY_WALK_DEPTH {
                let Ok(child_of_walk) = q_parents.get(pose_ent) else { break };
                let parent = child_of_walk.parent();
                if q_grids.contains(parent) {
                    let own_spinning_frame = q_frames_ids
                        .get(parent)
                        .is_ok_and(|f| f.ephemeris_id == body.ephemeris_id)
                        && q_parents
                            .get(parent)
                            .is_ok_and(|pp| q_grids.contains(pp.parent()));
                    if !own_spinning_frame {
                        host = Some(parent);
                        break;
                    }
                }
                pose_ent = parent;
            }
            let Some(host_grid) = host else { continue };
            let Ok(host_ref) = q_grids.get(host_grid) else { continue };
            let Ok((p_cell, p_tf)) = q_spatial.get(pose_ent) else { continue };

            // The camera pose is expressed DIRECTLY in the host grid's axes:
            // inertial grids carry identity rotation, so host +Y is the
            // engine/ecliptic NORTH pole. (The previous code referenced the
            // WORLD axes and counter-rotated by the stored chain — i.e. the
            // site pin's ENU `align`. At a south-polar site the ENU up is
            // inertially SOUTH, so the Moon rendered south-up with the
            // moonbase at the TOP of the disk; and because the site frame
            // spins with the body, the yaw reference slowly crept. Bodies now
            // always render north-up, wherever the site anchor is.)
            let edge = host_ref.cell_edge_length() as f64;
            let body_local =
                bevy::math::DVec3::new(p_cell.x as f64, p_cell.y as f64, p_cell.z as f64) * edge
                    + p_tf.translation.as_dvec3();

            // Ease the arm LENGTH toward the commanded distance, mirroring the
            // non-celestial branch ("zoom glides instead of snapping"). Without
            // this, every wheel detent teleported the camera by the full step —
            // MEGAMETERS at body range — in one frame: the whole scene jerked
            // (jump probe: identical 1.2e6 m displacement on every landmark
            // including WorldGrid in a single frame), globe LOD churned tiles
            // mid-flight (surface blinking in/out during zoom), and the stale
            // near plane sliced into the globe. Snap on first placement after
            // the grid migration (arrival is an intentional teleport); once the
            // arm converges the writes below go byte-identical again, so a
            // parked view still lets big_space's change-gated propagation skip.
            let arm_len = if child_of.parent() != host_grid {
                orbit.distance
            } else {
                let cam_local =
                    bevy::math::DVec3::new(cell.x as f64, cell.y as f64, cell.z as f64) * edge
                        + tf.translation.as_dvec3();
                let current_len = (cam_local - body_local).length();
                let err = (orbit.distance - current_len).abs();
                if err < orbit.distance * 5e-4 {
                    orbit.distance
                } else {
                    let damping = orbit.damping.unwrap_or(defaults.damping);
                    // Half the surface-orbit rate: at body range each easing
                    // step is megameters, and the orbital views run heavy
                    // (LOD re-tessellation), so frames are long — the doubled
                    // time-constant keeps per-frame steps small enough to
                    // read as a glide instead of a stutter.
                    let alpha =
                        (1.0 - (-0.5 * defaults.position_rate * (1.0 - damping) * dt).exp()) as f64;
                    current_len + (orbit.distance - current_len) * alpha
                }
            };
            let desired_local = body_local + dir * arm_len;
            let (new_cell, new_translation) = host_ref.translation_to_grid(desired_local);
            let local_rot = rotation;

            let next = lunco_celestial::OrbitalViewPin {
                active: true,
                body: body.ephemeris_id,
                dir,
                distance: orbit.distance,
                anchor_world,
                anchor_rotation,
            };
            // Guarded write: consumers (scene-hide, gravity hold) change-gate.
            if pin.active != next.active
                || pin.body != next.body
                || pin.dir != next.dir
                || pin.distance != next.distance
            {
                **pin = next;
            }

            if child_of.parent() != host_grid {
                migrate_to_grid(
                    &mut commands,
                    avatar_ent,
                    host_grid,
                    new_cell,
                    Transform::from_translation(new_translation).with_rotation(local_rot),
                );
                commands.entity(avatar_ent).try_insert(OrbitalViewCamera);
            } else {
                // Guarded writes: a parked view stays byte-identical, so
                // big_space's change-gated propagation can skip cleanly.
                if *cell != new_cell { *cell = new_cell; }
                if tf.translation != new_translation { tf.translation = new_translation; }
                if tf.rotation != local_rot { tf.rotation = local_rot; }
            }
            continue;
        }
        // Non-celestial target (or no celestial plugin): leaving a previous
        // orbital view first restores the parked surface pose —
        // `orbital_exit_restore_system` migrates the camera home next frame,
        // then the generic path below proceeds from the parked state.
        if let Some(pin) = orbital_pin.as_mut() {
            if pin.active {
                pin.active = false;
                continue;
            }
        }
        let current_grid = child_of.parent();

        // Anchor the camera to the root grid, preserving its CURRENT absolute
        // position (don't snap to the target body).
        if current_grid != root_grid {
            let (new_cell, new_translation) = root_ref.translation_to_grid(sample.cam_root);
            let local_tf = Transform::from_translation(new_translation).with_rotation(tf.rotation);
            migrate_to_grid(&mut commands, avatar_ent, root_grid, new_cell, local_tf);
            // Migration is deferred; next frame `child_of` resolves to the root grid.
            continue;
        }

        // Camera is on the root grid. Its own root-frame position comes from
        // exact cell + local-transform math; the target position is the
        // First-schedule snapshot (one frame of target drift — metres — is a
        // constant smooth lag, unlike deriving it from the CURRENT camera
        // position, which fed the camera's own motion back into the target).
        let grid_ref = root_ref;
        let cam_abs = grid_ref.grid_position_double(&cell, &tf);
        let target_pos = sample.target_pos;

        // Multiplicative zoom: proportional to current distance using exponential scaling.
        // Scroll up (delta > 0) -> zoom in. Scroll down (delta < 0) -> zoom out.
        apply_scroll_zoom(&mut orbit.distance, &mut zoom.delta, ZOOM_SENSITIVITY, min_dist, 1.0e11);

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
        let current_len = cam_abs.distance(target_pos);
        let final_len = if current_len < 1e-3 {
            desired_len
        } else {
            let alpha = (1.0 - (-defaults.position_rate * (1.0 - damping) * dt).exp()) as f64;
            current_len + (desired_len - current_len) * alpha
        };
        let next_pos = target_pos + dir * final_len;

        // HOLD when nothing meaningful changed. The camera translation is a
        // single-cell f32 at up to ~4e8 m (32 m ULP): rewriting it every
        // frame while chasing a slowly-drifting target makes the position
        // snap between representable values — nearby orbit lines visibly
        // wobble in parallax ("orbit jitters"). A parked camera is EXACT.
        // The target drifts past a frozen camera at metres per frame (its
        // motion relative the anchored site), which takes minutes to reach a
        // visible angle; the re-write triggers again on any user input or
        // once the arm error passes the dead band.
        let view_changed = orbit.yaw != *last_pose.0 || orbit.pitch != *last_pose.1;
        let arm_err = (current_len - orbit.distance).abs();
        let settled = arm_err < orbit.distance * 5e-3 && (desired_len - orbit.distance).abs() < orbit.distance * 5e-3;
        if settled && !view_changed {
            *log_countdown = log_countdown.saturating_sub(1);
            continue;
        }
        *last_pose.0 = orbit.yaw;
        *last_pose.1 = orbit.pitch;

        // NOTE: below the grid's switching threshold (1e10 m in the sandbox)
        // this keeps the camera in cell (0,0,0) with the full translation in
        // f32 — the SAME single-cell convention every other entity in the app
        // uses. An experiment splitting the camera into real 2000 m cells
        // exercised big_space's origin-cell-crossing machinery, which the
        // rest of the app has never run under: stale change-gated GTs missed
        // the per-frame 2000 m origin shifts and local geometry (the ground
        // plane) flashed into the orbital view. Do NOT re-split without doing
        // it for the whole world (doc 44's celestial-space split).
        let (new_cell, new_tf) = grid_ref.translation_to_grid(next_pos);
        *cell = new_cell;
        tf.translation = new_tf;

        // Apply rotation directly (no look_at — that clobbered yaw/pitch).
        tf.rotation = rotation;

        // Arm telemetry: convergence toward the commanded distance is the
        // invariant every focus bug so far has violated. This is developer
        // telemetry, not user-facing status — at `info!` the `far_off` branch
        // fired EVERY frame for the whole approach (60 lines/s of 200-char
        // lines). `debug!` keeps it available behind `RUST_LOG` and off the
        // default console; the countdown still rate-limits the converged case.
        let far_off = current_len > orbit.distance * 1.5;
        if far_off || *log_countdown == 0 {
            *log_countdown = 240;
            debug!(
                "ORBIT: arm {:.4e}→{:.4e} (cmd {:.3e}) cell=({},{},{}) tf=({:.1},{:.1},{:.1}) tgt=({:.4e},{:.4e},{:.4e}) next=({:.4e},{:.4e},{:.4e}) yaw={:.2} pitch={:.2}",
                current_len, final_len, orbit.distance,
                cell.x, cell.y, cell.z,
                tf.translation.x, tf.translation.y, tf.translation.z,
                target_pos.x, target_pos.y, target_pos.z,
                next_pos.x, next_pos.y, next_pos.z,
                orbit.yaw, orbit.pitch,
            );
        }
        *log_countdown = log_countdown.saturating_sub(1);
    }
}

/// Strip interactive camera-mode components off a cinematically-locked camera.
///
/// [`lunco_core::CinematicCameraLock`] marks a camera whose pose is owned by an
/// authored driver (a USD camera path). The mode components arrive anyway: the
/// avatar SPAWNS with `FreeFlightCamera`, and `ReleaseVessel` re-inserts it —
/// both legitimately, both unaware of the lock. The `Without<…>` guards on the
/// mode systems already keep them from writing the locked `Transform`; this
/// removes the stale components too, so mode-transition systems (which key on
/// component presence, not on writes) don't reason about a camera they don't
/// own. Runs whenever a locked avatar still carries a mode component; steady
/// state is an empty query.
fn strip_camera_modes_from_locked(
    mut commands: Commands,
    q: Query<
        Entity,
        (
            With<lunco_core::CinematicCameraLock>,
            Or<(
                With<FreeFlightCamera>,
                With<SpringArmCamera>,
                With<OrbitCamera>,
                With<SurfaceCamera>,
                With<FrameBlend>,
            )>,
        ),
    >,
) {
    for e in q.iter() {
        info!("[avatar] {e:?} is cinematically locked — stripping interactive camera modes");
        commands
            .entity(e)
            .remove::<FreeFlightCamera>()
            .remove::<SpringArmCamera>()
            .remove::<OrbitCamera>()
            .remove::<SurfaceCamera>()
            .remove::<FrameBlend>()
            .remove::<avian3d::prelude::TranslationInterpolation>()
            .remove::<avian3d::prelude::RotationInterpolation>();
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
    mut q_avatar: Query<(
        &mut Transform,
        &mut FreeFlightCamera,
        &CellCoord,
        &ChildOf,
        Option<&SurfaceRelativeMode>,
    ), (With<Avatar>, Without<FrameBlend>, Without<OrbitCamera>, Without<lunco_core::CinematicCameraLock>)>,
    q_grids: Query<&Grid>,
    q_world: Query<(), With<lunco_core::WorldGrid>>,
    q_site: Query<&lunco_celestial::GeodeticAnchor, With<lunco_celestial::SiteAnchor>>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    _gravity_field: Res<LocalGravityField>,
) {
    let site_center = site_body_center(&q_site, &q_bodies).map(|(_, _, c)| c);
    for (mut tf, mut ff, cell, child_of, surface_mode) in q_avatar.iter_mut() {
        let rot = if surface_mode.is_some() {
            // "Up" under the camera — body-centre aware (see `surface_up`).
            let up_v = if let Ok(grid) = q_grids.get(child_of.0) {
                surface_up(
                    grid.grid_position_double(cell, &tf),
                    q_world.contains(child_of.0),
                    site_center,
                )
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
        ),
        (
            With<Avatar>,
            Or<(With<FreeFlightCamera>, With<SurfaceCamera>)>,
            Without<OrbitCamera>,
            Without<FrameBlend>,
        ),
    >,
    q_grids: Query<&Grid>,
    q_world: Query<(), With<lunco_core::WorldGrid>>,
    q_site: Query<&lunco_celestial::GeodeticAnchor, With<lunco_celestial::SiteAnchor>>,
    q_bodies: Query<(Entity, &CelestialBody)>,
) {
    // Only meaningful on a site-anchored scene whose solar hierarchy is up.
    let Some((body_ent, radius_m, center)) = site_body_center(&q_site, &q_bodies) else {
        return;
    };
    for (avatar_ent, mut tf, mut cell, child_of, mut zoom, cam) in q_avatar.iter_mut() {
        if zoom.delta == 0.0 {
            continue;
        }
        // Only the active render camera transits (scenes carry inactive
        // Avatar-tagged spawn cameras — same guard as `on_focus_command`).
        if !cam.is_some_and(|c| c.is_active) {
            zoom.delta = 0.0;
            continue;
        }
        // The altitude math below is in the SITE (WorldGrid) frame. Right
        // after an orbital scroll-through the avatar is still parented to the
        // body's grid for one frame (until `orbital_exit_restore_system`
        // migrates it back) — running there turned one detent into an ~84 km
        // step that flung the camera across the globe. Leave the delta
        // unconsumed; it applies next frame once the restore lands.
        if !q_world.contains(child_of.parent()) {
            continue;
        }
        let Ok(grid) = q_grids.get(child_of.parent()) else {
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
            .clamp(0.75, 1.25);
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
            commands
                .entity(avatar_ent)
                .remove::<SpringArmCamera>()
                .remove::<FreeFlightCamera>()
                .remove::<FrameBlend>()
                .remove::<SurfaceCamera>()
                .remove::<SurfaceRelativeMode>()
                .remove::<GravityBody>()
                .remove::<OrbitFrameSample>()
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
    mut q_avatar: Query<(
        &mut Transform,
        &SurfaceCamera,
        &CellCoord,
        &ChildOf,
    ), (With<Avatar>, Without<FrameBlend>, Without<lunco_core::CinematicCameraLock>)>,
    q_grids: Query<&Grid>,
    q_world: Query<(), With<lunco_core::WorldGrid>>,
    q_site: Query<&lunco_celestial::GeodeticAnchor, With<lunco_celestial::SiteAnchor>>,
    q_bodies: Query<(Entity, &CelestialBody)>,
) {
    let site_center = site_body_center(&q_site, &q_bodies).map(|(_, _, c)| c);
    for (mut tf, cam, cell, child_of) in q_avatar.iter_mut() {
        let Ok(grid) = q_grids.get(child_of.0) else { continue };

        // Surface normal under the camera — body-centre aware (see `surface_up`).
        let up = surface_up(
            grid.grid_position_double(cell, &tf),
            q_world.contains(child_of.0),
            site_center,
        );

        // Rebuild the rotation from scratch each frame from heading + pitch
        // around the surface normal (local north = world-Y onto the tangent
        // plane, Z near the poles). No incremental rotations -> zero roll drift.
        tf.rotation = tangent_frame(up, cam.heading, cam.pitch);
    }
}

// ─── Locomotion ──────────────────────────────────────────────────────────────

/// Kinematic actuator for the avatar — the port-driven analog of a rover's
/// `apply_drive_mix`. Reads the avatar's FSW command surface (`forward`/`side`/`up`,
/// written through the shared `SetPorts` path by `drive_from_bindings`) and
/// translates the avatar entity in absolute coordinates. No forces (a free-fly
/// observer has no physics) — this is the whole "mechanism" for the avatar.
///
/// Only active with a `FreeFlightCamera`/`SurfaceCamera`, or when CTRL is held while
/// possessing a vessel (a momentary free-flight overlay). `Shift` boosts speed ×10.
/// In surface mode, `up` uses the radial direction so movement follows the tangent
/// plane. Runs in PostUpdate at render rate on wall-clock time, so the ghost camera
/// keeps moving even when the sim's virtual clock is paused/slowed.
fn apply_fly(
    mut q_avatar: Query<(
        Entity,
        &mut Transform,
        &mut CellCoord,
        &ChildOf,
        &lunco_fsw::FlightSoftware,
        Has<FreeFlightCamera>,
        Has<SurfaceCamera>,
        Option<&SurfaceRelativeMode>,
    ), (With<Avatar>, Without<lunco_core::CinematicCameraLock>)>,
    q_grids: Query<&Grid>,
    q_world: Query<(), With<lunco_core::WorldGrid>>,
    q_site: Query<&lunco_celestial::GeodeticAnchor, With<lunco_celestial::SiteAnchor>>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    keys: Res<ButtonInput<KeyCode>>,
    // The INTERACTION clock (wall-rooted): the avatar keeps flying while the sim is
    // paused, because pausing the simulation is not supposed to paralyse the user. Runs
    // at render rate in `PostUpdate` — no lockstep needed, free-flight follows nothing.
    time: lunco_time::InteractionTime,
    mut commands: Commands,
) {
    let site_center = site_body_center(&q_site, &q_bodies).map(|(_, _, c)| c);
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let boost = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) { 10.0 } else { 1.0 };

    for (entity, mut tf, mut cell, child_of, fsw, has_freeflight, has_surface_camera, surface_mode) in q_avatar.iter_mut() {
        let Ok(grid) = q_grids.get(child_of.0) else { continue };
        let current_pos = grid.grid_position_double(&cell, &tf);

        // Only move if we have a camera mode or CTRL-overlay.
        if !has_freeflight && !has_surface_camera && !ctrl_pressed { continue; }

        // Command inputs off the FSW surface (each −1..=1 from the ControlBinding),
        // then boosted. When free (no ControllerLink) `drive_from_bindings` writes
        // these; while possessing they stay 0 (control is redirected to the vessel).
        let forward = (fsw.cmd("forward") * boost) as f32;
        let side = (fsw.cmd("side") * boost) as f32;
        let elevation = (fsw.cmd("up") * boost) as f32;
        if forward.abs() < 0.01 && side.abs() < 0.01 && elevation.abs() < 0.01 { continue; }

        // Actively moving → cancel any idle auto-action.
        commands.entity(entity).remove::<lunco_core::ActiveAction>();

        // In surface mode, "up" = radial direction from the body centre
        // (body-centre aware — see `surface_up`); else world Y.
        let up_dir = if surface_mode.is_some() {
            surface_up(current_pos, q_world.contains(child_of.0), site_center)
        } else {
            Vec3::Y
        };

        let mut move_vec = Vec3::ZERO;
        move_vec += *tf.forward() * forward;
        move_vec += *tf.right() * side;
        move_vec += up_dir * elevation;

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
    let pointer_captured = egui_focus.wants_pointer
        || waypoint_menu_open.map(|m| m.0).unwrap_or(false);

    for (entity, intent_state, mut analog) in q_avatar.iter_mut() {
        let mut delta = Vec2::ZERO;
        let mut mouse_moved = false;
        if !pointer_captured {
            let d = intent_state.axis_pair(&UserIntent::Look);
            if d.length_squared() > 0.00001 { delta = d * 10.0; mouse_moved = true; }
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

/// Mouse-wheel → per-avatar [`CameraZoomInput`], sourced from the `UserIntent::Zoom`
/// axis and gated on egui pointer capture.
///
/// This is the single, unified zoom path: it replaces the two bespoke egui
/// `CameraScroll` bridges (which read `raw_scroll_delta` gated on
/// `wants_pointer_input()`). The `Zoom` axis is already in the shared
/// `InputMap<UserIntent>` (`MouseScrollAxis::Y`), so wheel input flows through the
/// same intent vocabulary as everything else; we accumulate it per-avatar for the
/// active camera behavior to consume + reset. Zeroed while egui holds the pointer
/// so scrolling a panel/scrollarea doesn't zoom the scene.
fn collect_camera_zoom(
    egui_focus: Res<lunco_core::EguiFocus>,
    mut q_avatar: Query<(Entity, &IntentState, &mut CameraZoomInput), With<Avatar>>,
    mut commands: Commands,
) {
    if egui_focus.wants_pointer {
        return;
    }
    for (entity, intent_state, mut zoom) in q_avatar.iter_mut() {
        let d = intent_state.value(&UserIntent::Zoom);
        if d.abs() > f32::EPSILON {
            zoom.delta += d;
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
    mut q_tf: Query<(&mut Transform, &CellCoord, &ChildOf), (With<Avatar>, Without<FrameBlend>, Without<lunco_core::CinematicCameraLock>)>,
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

fn avatar_global_hotkeys(
    q_avatar: Query<(&IntentState, Option<&ControllerLink>), With<Avatar>>,
    mut transport: Option<ResMut<TimeTransport>>,
    keyboard: Res<ButtonInput<KeyCode>>,
) {
    for (intent_state, opt_link) in q_avatar.iter() {
        let mut toggle = intent_state.just_pressed(&UserIntent::Pause);

        // Also if we are in Avatar mode (not possessing a vessel, i.e., no ControllerLink),
        // let's make a press on Space pause/unpause.
        if opt_link.is_none() && keyboard.just_pressed(KeyCode::Space) {
            toggle = true;
        }

        if toggle {
            if let Some(transport) = transport.as_deref_mut() {
                transport.mode = match transport.mode {
                    TransportMode::Playing => TransportMode::Paused,
                    TransportMode::Paused => TransportMode::Playing,
                };
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
    // Driven by bevy_picking: a global `On<Pointer<Click>>` observer. The
    // egui-vs-scene guard is `EguiFocus.wants_pointer` (via `scene_click_ray`) —
    // a global flag, fed by the workbench's egui-authoritative `pointer_over_scene`
    // signal, so a click on any real chrome is stood down here even though this
    // global observer can fire on a scene entity behind the panel.
    mut click: On<bevy::picking::events::Pointer<bevy::picking::events::Click>>,
    keys: Res<ButtonInput<KeyCode>>,
    camera_q: Query<(&Camera, &GlobalTransform, Entity), With<Avatar>>,
    egui_focus: Res<lunco_core::EguiFocus>,
    drag_mode_active: Res<lunco_core::DragModeActive>,
    spawn_tool_active: Res<lunco_core::SpawnToolActive>,
    terrain_tool_active: Res<lunco_core::TerrainToolActive>,
    waypoint_tool_active: Res<lunco_core::WaypointToolActive>,
    mut commands: Commands,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>,
    q_spacecraft: Query<(Entity, &GlobalTransform, &Spacecraft)>,
    q_vessel: Query<Entity, Controllable>,
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_parents: Query<&ChildOf>,
    q_ground: Query<Entity, With<lunco_core::Ground>>,
) {
    use bevy::picking::pointer::PointerButton;
    // Left button only.
    if click.button != PointerButton::Primary { return; }
    // Shift+click is reserved for entity selection / gizmo multi-select in
    // lunco-sandbox-edit (`on_scene_click_select`, the other global
    // `Pointer<Click>` observer). A plain left-click possesses/follows/focuses;
    // a Shift+click never does. This modifier split is what keeps the two
    // observers from both acting on a single click.
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) { return; }
    // Alt-click is likewise reserved for the editor.
    if keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]) { return; }
    // Ctrl-click appends a patrol checkpoint (`on_scene_click_checkpoint`, the
    // third global `Pointer<Click>` observer). Both observers see the same click
    // — `propagate(false)` stops bubbling, not sibling observers — so without
    // this guard every checkpoint placement would ALSO possess/follow whatever
    // the ray hit, yanking the camera onto the terrain.
    if keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]) { return; }
    // Mid-drag on a transform gizmo: don't flip the camera under the user.
    if drag_mode_active.active { return; }
    // Spawn placement tool armed: clicks place objects, don't possess.
    if spawn_tool_active.0 { return; }
    // Terrain brush armed: clicks sculpt the terrain, don't possess.
    if terrain_tool_active.0 { return; }
    // Waypoint Move/Insert armed: that click places the waypoint, don't possess.
    if waypoint_tool_active.0 { return; }

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
    let Some((camera, cam_gtf, avatar_entity)) = camera_q.iter().next() else { return; };
    let Some(ray) = lunco_core::scene_click_ray(&egui_focus, camera, cam_gtf, click.pointer_location.position) else { return; };

    // The mesh the pick resolved to (rover, prop, ground, …); resolve to its
    // clickable root. `hit.depth` is the along-ray distance to compare against
    // the analytic spheres below.
    // Depth is recorded for ANY real mesh hit, clickable or not. Occlusion is a
    // geometric fact, not a property of being click-targetable: the terrain has no
    // `SelectableRoot`, but it is still solid, and a click on it must still shadow
    // the analytic spheres below. Coupling the two (recording `depth` only when a
    // root was found) left `min_t = INFINITY` on every ground click, so the Earth
    // hit-sphere — which a camera standing on the surface ALWAYS intersects —
    // passed `t < min_t` and the click "leaked" through the ground into a
    // `FocusTarget` on the planet.
    let mut nearest_clickable: Option<Entity> = None;
    let mut min_t = if click.hit.position.is_some() { click.hit.depth } else { f32::INFINITY };

    if let Some(root) = find_clickable_from_hit(click.entity, &q_parents, &q_selectable, &q_ground) {
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
        commands.trigger(PossessVessel { avatar: Some(avatar_entity), target, bind_camera: true });
    } else if let Some(target) = nearest_clickable {
        if q_vessel.get(target).is_ok() {
            commands.trigger(PossessVessel { avatar: Some(avatar_entity), target, bind_camera: true });
        } else {
            commands.trigger(FollowTarget { avatar: Some(avatar_entity), target });
        }
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
    q_avatar: Query<(Entity, &IntentState), (With<Avatar>, Or<(With<ControllerLink>, With<SpringArmCamera>, With<OrbitCamera>)>)>,
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

/// Releases possession of a vessel.
///
/// Keeps the camera at its current position — no jarring teleport.
/// Switches to `FreeFlightCamera` mode with the current orientation preserved.
#[on_command(ReleaseVessel)]
fn on_release_command(
    trigger: On<ReleaseVessel>,
    mut commands: Commands,
    q_avatar: Query<(&Transform, Option<&ControllerLink>, Option<&SurfaceRelativeMode>), With<Avatar>>,
    guard: Res<lunco_core::SyncApplyGuard>,
    mut orbital_pin: Option<ResMut<lunco_celestial::OrbitalViewPin>>,
    q_site: Query<&lunco_celestial::GeodeticAnchor, With<lunco_celestial::SiteAnchor>>,
    q_bodies: Query<(Entity, &CelestialBody)>,
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
    // Leaving an orbital view: deactivate the mode. The camera flew to the
    // focused body; `orbital_exit_restore_system` migrates it back to the
    // parked surface pose (`pin.anchor_world`/`anchor_rotation`) next frame.
    let mut restored_from_orbit = None;
    if let Some(pin) = orbital_pin.as_mut() {
        if pin.active {
            pin.active = false;
            // The live orbital rotation is in the HOST grid's rotated axes
            // (`inv_chain × view_rot`); the euler below must match the parked
            // world-axes pose the exit restore reinstates, or the first mouse
            // move snaps the view to a stale orientation.
            restored_from_orbit = Some((pin.anchor_world, pin.anchor_rotation));
        }
    }
    let cmd = trigger.event();
    let avatar_ent = cmd.target;
    let (yaw, pitch, opt_link, is_surface) = if let Ok((tf, link, surface)) = q_avatar.get(avatar_ent) {
        let rot = restored_from_orbit.map(|(_, r)| r).unwrap_or(tf.rotation);
        let (y, p, _) = rot.to_euler(EulerRot::YXZ);
        (y, p, link, surface.is_some())
    } else { (0.0, 0.0, None, false) };

    // Hard stop the rover upon disengaging control: zero throttle/steer, full brake.
    if let Some(link) = opt_link {
        commands.trigger(lunco_cosim::SetPorts {
            target: link.vessel_entity,
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
    commands.entity(avatar_ent)
        .remove::<ControllerLink>()
        .remove::<SpringArmCamera>()
        .remove::<OrbitCamera>()
        .remove::<OrbitFrameSample>()
        .remove::<SunlitArrival>()
        .remove::<RadialArrival>()
        .remove::<FrameBlend>()
        // Defensive: strip the fixed-step easing a SpringArmCamera used to carry back
        // when the camera was written in `FixedPostUpdate`. Cameras now run in the
        // interaction cadence and are never given it, but an entity restored from an
        // older scene/save could still have it, and it would fight the camera writer.
        .remove::<avian3d::prelude::TranslationInterpolation>()
        .remove::<avian3d::prelude::RotationInterpolation>();

    // In surface mode, use SurfaceCamera (recomputed from scratch each frame);
    // otherwise use FreeFlightCamera (incremental euler angles).
    if is_surface {
        commands.entity(avatar_ent).try_insert(SurfaceCamera {
            heading: yaw, // approximate mapping from euler yaw
            pitch,
        });
    } else if let (Some((anchor_world, rot)), Some((_, _, center))) =
        (restored_from_orbit, site_body_center(&q_site, &q_bodies))
    {
        // Descending out of an orbital view on a site-anchored scene resumes
        // in SURFACE-RELATIVE mode: the body stays at the bottom of the
        // screen (local "down" = gravity) and WASD follows the local tangent
        // frame — a world-euler FreeFlightCamera only behaves that way
        // directly over the site. Decompose the parked attitude against the
        // LOCAL up with the SAME reference `tangent_frame` uses, so the
        // resume doesn't visibly rotate the view.
        let up = (anchor_world - center).normalize_or_zero().as_vec3();
        let fwd = rot * Vec3::NEG_Z;
        let ref_dir = if up.dot(Vec3::Y).abs() < 0.9 { Vec3::Y } else { Vec3::Z };
        let east = up.cross(ref_dir).normalize();
        let north = east.cross(up).normalize();
        let heading = fwd.dot(east).atan2(fwd.dot(north));
        let s_pitch = fwd.dot(up).clamp(-1.0, 1.0).asin();
        commands
            .entity(avatar_ent)
            .try_insert((SurfaceCamera { heading, pitch: s_pitch }, SurfaceRelativeMode));
    } else {
        commands.entity(avatar_ent).try_insert(FreeFlightCamera {
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
#[on_command(PossessVessel)]
fn on_possess_command(
    trigger: On<PossessVessel>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf, Option<&ControllerLink>), With<Avatar>>,
    q_spatial_abs: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_vessel: Query<&lunco_fsw::FlightSoftware>,
    q_vessel_gravity: Query<&GravityBody>,
    q_follow: Query<&lunco_core::CameraFollow>,
    guard: Res<lunco_core::SyncApplyGuard>,
    registry: Res<lunco_core::SessionRegistry>,
    rbac: Res<lunco_core::session::SessionRbac>,
    session: Res<lunco_core::LocalSession>,
    q_owned: Query<&lunco_core::GlobalEntityId>,
    mut authority: Option<ResMut<lunco_core::markers::FlightAuthority>>,
) {
    let cmd = trigger.event();
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
    let resolved = cmd.avatar
        .and_then(|a| q_avatar.get(a).ok())
        .or_else(|| q_avatar.iter().next());
    let Some((avatar_ent, cam_tf, cam_cell, _child_of, existing_link)) = resolved else { return; };

    // Idempotent: already controlling this exact target — no-op.
    if let Some(link) = existing_link {
        if link.vessel_entity == cmd.target { return; }
    }

    // Camera-less possession. The authority claim — what actually flips the
    // vessel's `piloted` gate — already ran in `record_possession_authority`,
    // and the caller has declared the VIEW is not ours to touch: a recording
    // scenario drives the vessel through ports while an authored camera path
    // owns the camera, and the chase-camera bind below would land on that very
    // path-driven avatar and steal the shot. The `ControllerLink` is NOT camera
    // work though — it is the control/telemetry binding the vessel HUD keys on
    // (`draw_rover_hud` shows exactly when the avatar carries one) — so it is
    // still bound before bailing.
    if !cmd.bind_camera {
        commands
            .entity(avatar_ent)
            .try_insert(ControllerLink { vessel_entity: cmd.target });
        return;
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

    // Snap to vessel immediately. Orbit/Chase preserve the current look angles so
    // possession doesn't jerk the view; Heading adopts the fixed rover start pose.
    let (current_yaw, current_pitch, _) = cam_tf.rotation.to_euler(EulerRot::YXZ);
    let (init_yaw, init_pitch) = match follow {
        CameraFollow::Heading => (end_yaw, end_pitch),
        _ => (current_yaw, current_pitch),
    };
    let final_rot = Quat::from_euler(EulerRot::YXZ, init_yaw, init_pitch, 0.0);
    let final_offset = final_rot.mul_vec3(Vec3::Z).as_dvec3() * end_distance;
    let final_abs_pos = target_abs + final_offset + Vec3::Y.as_dvec3() * end_vert_off as f64;

    // Migrate to target grid immediately
    migrate_avatar_to_target_grid(
        &mut commands, avatar_ent, target_grid, final_abs_pos, final_rot,
        &q_grids, &q_parents, &q_spatial_abs,
    );

    // The controller link goes on the **avatar** (it carries the shared
    // `ActionState<UserIntent>` that `drive_from_bindings` reads); the intent→port
    // `ControlBinding` lives on the **vessel** as its own property, authored purely
    // from USD (a `Controls` child scope referencing a shared profile in
    // `control_profiles.usda`). There is NO Rust topology default: a vessel is
    // drivable iff its USD carries that scope. `drive_from_bindings` reads the
    // binding off the vessel and skips any vessel that has none, so possession is a
    // pure camera+link bind here.
    commands
        .entity(avatar_ent)
        .try_insert(ControllerLink { vessel_entity: cmd.target });

    // Detect if target is a surface vehicle (has GravityBody) and propagate surface mode.
    let is_surface_vehicle = q_vessel_gravity.get(cmd.target).is_ok();

    // One follow solver for all three modes (`spring_arm_system`, FixedPostUpdate
    // + interpolation): they differ ONLY in the derived attitude. `OrbitCamera`
    // is NOT used here — it is the celestial orbital-view solver; reusing it for a
    // fast-flying vessel was the source of the "jitters when flying fast" (a
    // render-rate solve against a frame-stale target sample). Strip the celestial
    // orbit component in case a prior focus left it on.
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
        CF::Heading => (FollowAttitude::Heading, q_vessel.contains(cmd.target), Some(0.05)),
    };
    let mut cmd_ent = commands.entity(avatar_ent);
    cmd_ent
        .remove::<OrbitCamera>()
        .try_insert((
            SpringArmCamera {
                target: cmd.target,
                distance: end_distance,
                yaw: init_yaw,
                pitch: init_pitch,
                damping,
                vertical_offset: end_vert_off,
                track_heading,
                attitude,
            },
            // The follow camera runs in `FixedPostUpdate`; ease its Transform between
            // fixed samples so it renders in lockstep with the followed body (same
            // avian mechanism the body uses) instead of staircasing at 60 Hz.
            avian3d::prelude::TranslationInterpolation,
            avian3d::prelude::RotationInterpolation,
        ));
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
#[on_command(FollowTarget)]
fn on_follow_command(
    trigger: On<FollowTarget>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf, Option<&SpringArmCamera>), With<Avatar>>,
    q_spatial_abs: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_vessel: Query<Entity, Controllable>,
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

    // Drop the controller link — follow ≠ possess (the vessel keeps its own
    // `ControlBinding`).
    let mut cmd_ent = commands.entity(avatar_ent);
    cmd_ent
        .remove::<ControllerLink>()
        .remove::<FreeFlightCamera>()
        .remove::<SurfaceCamera>()
        .remove::<OrbitCamera>()
        .remove::<FrameBlend>()
        .try_insert((
            SpringArmCamera {
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
            },
            // Fixed-step follow: ease the camera in lockstep with the body (see the
            // possess path and `spring_arm_system`).
            avian3d::prelude::TranslationInterpolation,
            avian3d::prelude::RotationInterpolation,
        ));

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
/// — root-grid anchoring, cell-split, position easing — is owned by
/// `orbit_system`, which runs at a fixed schedule point on frame-consistent
/// transforms. (An earlier version teleported the avatar here through
/// `world_position_seeded`, which drops the site-anchored solar grids'
/// rotations — landing the camera on a phantom point.)
#[on_command(FocusTarget)]
fn on_focus_command(
    trigger: On<FocusTarget>,
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform, Option<&Camera>, Option<&OrbitCamera>), With<Avatar>>,
    q_bodies: Query<&CelestialBody>,
    q_sc: Query<&Spacecraft>,
    q_children: Query<&Children>,
) {
    let cmd = trigger.event();
    // Prefer the avatar carrying the ACTIVE render camera when the command
    // doesn't name one (API/rhai path) — scenes can contain Avatar-tagged
    // prims (spawn points, `is_active: false` spawn cameras) that must not
    // steal the focus.
    let resolved = cmd.avatar
        .and_then(|a| q_avatar.get(a).ok())
        .or_else(|| q_avatar.iter().find(|(_, _, cam, _)| cam.is_some_and(|c| c.is_active)))
        .or_else(|| q_avatar.iter().next());
    let Some((avatar_ent, cam_tf, _, current_orbit)) = resolved else { return; };

    // Compute distance based on target type.
    let mut distance = 20.0;
    let physical_target = get_physical_body(cmd.target, &q_children, &q_bodies);
    let is_body = q_bodies.contains(physical_target);

    // Already orbiting this very body (clicking the focused globe, re-clicking
    // its view pill): a repeat focus must be a NO-OP. Re-running the swap
    // re-arms `SunlitArrival`, which snaps the camera back to the arrival
    // pose — "I jump to the original position".
    if let Some(orbit) = current_orbit {
        if get_physical_body(orbit.target, &q_children, &q_bodies) == physical_target {
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
        // Any previous sample belongs to the previous target; drop it so
        // orbit_system idles until `sample_orbit_frame` refreshes it.
        .remove::<OrbitFrameSample>()
        .try_insert(OrbitCamera {
            target: cmd.target,
            distance,
            yaw,
            pitch,
            damping: None,
            vertical_offset: 0.0,
        });
    // Celestial bodies: aim the arrival at the sunlit side (resolved in
    // `First` by `sample_orbit_frame`, where GTs are frame-consistent).
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
/// camera has NO mode component by design (the janitor strips them), and
/// without the guard this system re-inserted `FreeFlightCamera` every frame
/// while the janitor removed it every frame — a permanent insert/remove war
/// (measured: 8 590 strip rounds in one 63 s recording) whose per-frame
/// archetype churn on the camera entity destabilised scene bring-up
/// (terrain/sky visuals raced and lost) and multiplied run time.
fn avatar_init_system(
    mut commands: Commands,
    q_avatar: Query<(Entity, &Transform), (With<Avatar>, Without<SpringArmCamera>, Without<OrbitCamera>, Without<FreeFlightCamera>, Without<FrameBlend>, Without<lunco_core::CinematicCameraLock>)>,
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
    mut q_camera: Query<(&mut Projection, &GlobalTransform), (With<Camera>, With<AdaptiveNearPlane>)>,
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
                if near_edge < min_dist { min_dist = near_edge; }
                if far_edge > max_far { max_far = far_edge; }
            }
            let (near, far) = if max_far <= 0.0 {
                // No `CelestialBody` contributed (flat sandbox scene, or the
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
                // Headroom is the LARGER of 20 km and 50% of the distance:
                //
                // * 20 km — `min_dist` measures to the body's REFERENCE sphere,
                //   but terrain (and the camera standing on it) can sit
                //   kilometres above it — Shackleton ridge is ~1.2 km up, which
                //   would otherwise clip the ground on close approach.
                // * 50% — this system runs BEFORE TransformSystems::Propagate,
                //   so both GTs are one frame stale, and while zoom-easing at
                //   body range the camera approaches by MEGAMETERS per frame —
                //   far beyond a fixed 20 km. A stale `near` past the true
                //   surface slices a cap off the globe: the "black circle
                //   inside Earth while changing distance". Half-distance
                //   headroom absorbs any single-frame approach ≤50% while
                //   keeping the surface near the depth-precision peak
                //   (reverse-Z cares about the near/dist RATIO, and 0.5 is
                //   still 4 orders of magnitude better than the old 100 m pin).
                (
                    ((min_dist - 20_000.0).min(min_dist * 0.5).max(0.1)) as f32,
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
/// The camera is parented to the Body's Grid (inertial anchor), NOT the Body
/// itself. `SurfaceCamera` rebuilds world-space rotation every frame from
/// `LocalGravityField.local_up`, so the camera stays surface-relative without
/// inheriting the Body's rotation. `FloatingOrigin` must be on a Grid.
#[on_command(TeleportToSurface)]
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
#[on_command(LeaveSurface)]
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
        .try_insert(OrbitCamera {
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
        Entity, &GlobalTransform,
        Option<&GravityBody>, Option<&SurfaceRelativeMode>,
        Option<&FreeFlightCamera>, Option<&SurfaceCamera>,
    ), (With<Avatar>, Without<OrbitCamera>)>,
    q_globals: Query<&GlobalTransform>,
    q_bodies: Query<&CelestialBody>,
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
    let Some((avatar_ent, cam_gt, maybe_gb, maybe_mode, maybe_ff, maybe_sc)) = q_avatar.iter().next() else { return };

    // Altitude from a same-instant GlobalTransform DELTA: its LENGTH is
    // convention-independent (whatever origin/phase big_space is in cancels
    // in the difference). The previous `world_position_seeded` sum drops the
    // site-anchored solar grids' rotations, so the "body position" it
    // produced was rotated away from where the body actually is — altitude
    // came out as garbage and the mode (and camera style) flapped.
    //
    // ENGAGE only for avatars explicitly bound to a body (`GravityBody`,
    // set by TeleportToSurface). Site-anchored scenes put every free camera
    // within `engage_altitude` of the Moon by construction; auto-swapping
    // FreeFlight→SurfaceCamera there hijacks scripted/API camera placement
    // one frame after it lands (the garbage altitude used to keep this
    // branch dead in those scenes — keep the previous effective behavior).
    // The `field.body_entity` fallback still serves DISENGAGE below.
    let engage_body = maybe_gb.map(|gb| gb.body_entity);
    let disengage_body = engage_body.or(field.body_entity);
    let altitude_to = |b: Entity| {
        Some((q_globals.get(b).ok()?, q_bodies.get(b).ok()?)).map(|(body_gt, body)| {
            (body_gt.translation() - cam_gt.translation()).length() as f64 - body.radius_m
        })
    };
    let engage_altitude_m = engage_body.and_then(altitude_to).unwrap_or(f64::MAX);
    let altitude = disengage_body.and_then(altitude_to).unwrap_or(f64::MAX);

    let in_surface_mode = maybe_mode.is_some();

    // Site-anchored scenes NEVER altitude-disengage: the user's frame of
    // reference is the anchor body at every height below the orbital handover
    // ("the planetary body always at the bottom of the screen, following the
    // direction of gravity"). Falling back to the world-euler FreeFlight
    // camera up there levels the view to world +Y instead of the local up —
    // the tilted-horizon / "moon in the corner" report. Beyond ~50 km the
    // scroll transit hands the camera to the orbital mode anyway.
    let site_anchored = !q_site.is_empty();

    if in_surface_mode && altitude > thresholds.disengage_altitude && !site_anchored {
        // Too high → exit surface mode. Swap SurfaceCamera → FreeFlightCamera.
        commands.entity(avatar_ent).remove::<SurfaceRelativeMode>();
        if let Some(sc) = maybe_sc {
            // Note: heading→yaw is approximate (different reference frames)
            // but provides a reasonable starting orientation.
            commands.entity(avatar_ent)
                .remove::<SurfaceCamera>()
                .try_insert(FreeFlightCamera {
                    yaw: sc.heading,
                    pitch: sc.pitch,
                    damping: None,
                });
        }
    } else if !in_surface_mode && engage_altitude_m < thresholds.engage_altitude {
        // Low enough and explicitly bound to a body → enter surface mode.
        let has_body = maybe_gb.is_some();
        if has_body {
            commands.entity(avatar_ent).try_insert(SurfaceRelativeMode);
            // Swap FreeFlightCamera → SurfaceCamera.
            if let Some(ff) = maybe_ff {
                commands.entity(avatar_ent)
                    .remove::<FreeFlightCamera>()
                    .try_insert(SurfaceCamera {
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
    commands.trigger(lunco_core::TelemetryEvent {
        name: "cmd:PossessVessel".into(),
        source: 0,
        severity: lunco_core::Severity::Info,
        data: lunco_core::TelemetryValue::String("PossessVessel".into()),
        timestamp: 0.0,
    });
}

#[on_command(UpdateProfile)]
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
    let kind = if cmd.kind.is_empty() { "info" } else { cmd.kind.as_str() }.to_string();
    info!("[notify:{kind}] {}", cmd.text);
    notes.toasts.push(Toast { text: cmd.text.clone(), kind, remaining: secs });
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

/// Diagnostic read-out of every possessable vessel's *control authority* state —
/// the chain that decides whether the stick actually flies it:
/// `GlobalEntityId` (needed for ownership + the model's `piloted` sensor),
/// `ControlBinding` (intent→port map from the USD `Controls` scope), and whether
/// the `SessionRegistry` currently records an owner (⇒ `piloted = 1`). Logs one
/// `[inspect]` line per vessel at INFO. API-driven: `{"command":"InspectVessels"}`.
#[lunco_core::Command(default)]
pub struct InspectVessels {}

#[on_command(InspectVessels)]
fn on_inspect_vessels(_t: On<InspectVessels>, mut commands: Commands) {
    commands.queue(|world: &mut World| {
        // Collect first so the &mut World query borrow ends before the immutable
        // per-entity component reads below.
        let mut q = world.query_filtered::<Entity, bevy::prelude::Or<(
            bevy::prelude::With<lunco_fsw::FlightSoftware>,
            bevy::prelude::With<lunco_cosim::SimComponent>,
        )>>();
        let ents: Vec<Entity> = q.iter(world).collect();
        info!("[inspect] {} possessable vessel(s)", ents.len());
        for e in ents {
            let name = world.get::<Name>(e).map(|n| n.as_str().to_string()).unwrap_or_default();
            let gid = world.get::<lunco_core::GlobalEntityId>(e).map(|g| g.get());
            let has_fsw = world.get::<lunco_fsw::FlightSoftware>(e).is_some();
            let has_sim = world.get::<lunco_cosim::SimComponent>(e).is_some();
            let has_sel = world.get::<lunco_core::SelectableRoot>(e).is_some();
            let binding = world.get::<lunco_core::ControlBinding>(e).map(|b| {
                let ports: Vec<&str> = b.ports().collect();
                (b.binds.len(), ports.join(","))
            });
            let owner = gid.and_then(|g| {
                world.get_resource::<lunco_core::SessionRegistry>().and_then(|r| r.owner_of(g))
            });
            info!(
                "[inspect] {e:?} name={name:?} gid={gid:?} fsw={has_fsw} sim={has_sim} \
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
    on_surface_teleport_command,
    on_leave_surface_command,
    on_possess_command,
    on_release_command,
    on_focus_command,
    on_follow_command,
    on_update_profile,
    on_inspect_vessels
);
