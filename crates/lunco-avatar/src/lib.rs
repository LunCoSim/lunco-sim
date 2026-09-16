//! Implementation of the local presentation embodiment and interaction surface.
//!
//! This crate defines the [Avatar] entity, which handles camera input,
//! focus transitions, and vessel possession. The camera architecture uses
//! composable behavior components (`SpringArmCamera`, `OrbitCamera`, `FreeFlightCamera`) rather
//! than a monolithic state machine, enabling modular frame-aware operation
//! and explicit transitions between reference frames.
//!
//! # Architecture
//!
//! Each camera behavior is its own component with a dedicated system:
//! - **`SpringArmCamera`**: Chase camera locked to a vessel's heading (rovers, astronauts).
//! - **`OrbitCamera`**: Survey camera locked to the ecliptic/stars (realized by
//!   `lunco-avatar-camera` for avatar celestial targets).
//! - **`FreeFlightCamera`**: Free-moving camera in absolute coordinates (ghost/drone view).
//!
//! Transitions use explicit camera-mode transactions: orbit entry stores the
//! exact return pose, while the avatar retains each user-controlled orbital
//! pose by stable body identity. Follow/surface commands install one
//! authoritative mode and its frame. The avatar camera adapter owns the
//! orbital BigSpace placement; this crate owns the avatar-side transition.

use avian3d::prelude::{
    Collider, MoveAndSlide, MoveAndSlideConfig, MoveAndSlideHitResponse, SpatialQueryFilter,
};
use bevy::ecs::{lifecycle::HookContext, world::DeferredWorld};
use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use leafwing_input_manager::prelude::*;
use lunco_avatar_camera_core::{
    CurrentRegionArrival, OrbitPose, OrbitReturnBehavior, OrbitUserInput, OrbitViewHistory,
    OrbitViewReturn, RadialArrival, CAMERA_ZOOM_SENSITIVITY, SURFACE_ORBIT_HANDOFF_ALTITUDE_M,
};
use lunco_avatar_core::commands::{
    FocusTarget, FollowTarget, PossessVessel, ReleaseVessel, ReturnFromOrbit,
};
use lunco_avatar_core::lifecycle::AvatarSceneHandoffSet;
use lunco_avatar_core::notifications::{ScreenNotifications, ShowNotification, Toast};
use lunco_avatar_core::roles::{Avatar, LocalAvatar};
use lunco_avatar_policy::{
    avatar_soil_collision_policy, AvatarCollisionSettings, AvatarSoilCollisionPolicy,
};
use lunco_camera_core::{
    math::{camera_move_direction, surface_camera_angles, surface_camera_rotation, zoom_factor},
    AdaptiveNearPlane, CameraUpdateSet, CameraZoomInput, FollowAttitude, FreeFlightCamera,
    FreeFlightSettings, OrbitCamera, SpringArmCamera, SurfaceCamera, SurfaceRelativeMode,
};
use lunco_camera_runtime::{body_orbit_look_scale, CameraInputSettings};
use lunco_control_core::{IntentAnalogState, IntentState, UserIntent};
use lunco_core::{on_command, register_commands, CelestialBody, Spacecraft};
use lunco_core_session::commands::UpdateProfile;
use lunco_core_session::{LocalSession, NetworkRole, SessionProfiles};
use lunco_cosim_core::ControlLink;
use lunco_input_core::InputBindingsSettings;
/// Capability test for "**accepts commands**": carries an authored intent→port
/// binding (`ControlBinding`, from its USD `Controls` scope) or a Modelica actuation
/// backend (`SimComponent`).
///
/// This is not the possession predicate. Possession validates a writable
/// [`lunco_port_core::InputPorts`] endpoint that is not an [`Avatar`], then the
/// authority layer (`SessionRegistry::may_possess` / `PossessionPolicy`) decides
/// who may hold it. This alias answers only whether a target accepts commands,
/// and is used for one presentation decision: whether a heading-follow camera
/// should track the target's yaw (a thing that steers has a meaningful heading;
/// a prop tumbles).
type Controllable = bevy::prelude::Or<(
    bevy::prelude::With<lunco_control_core::ControlBinding>,
    bevy::prelude::With<lunco_cosim_core::SimComponent>,
)>;
use lunco_celestial_spatial::{
    gravity_up_in_grid, surface_axes_for_grid_position, surface_axes_in_grid, LeaveSurface,
    LocalGravityField, TeleportToSurface,
};
use lunco_environment::{GravityBody, GravityProvider};
use lunco_settings::{AppSettingsExt, ProfileSettings};
use lunco_spatial::attach::migrate_to_grid;
use lunco_time::{SetTimeTransport, TimeTransport, TransportMode, WorldTime};
use lunco_usd_bevy_scene::{is_preview_only, is_preview_only_entity, UsdPreviewOnly, UsdPrimPath};

mod camera;
mod input;
use camera::freeflight_scroll_transit_system;
use input::{
    avatar_behavior_input_system, avatar_global_hotkeys, capture_avatar_intent, collect_camera_zoom,
};
#[cfg(test)]
use input::{look_angles, normalized_scroll_delta};

// Render-bound screenshots and deterministic offline recording are owned by
// `lunco-capture`; this crate remains responsible for camera intent,
// possession, and interaction, without linking the render-world readback pipeline.

fn report_avatar_policy_error(error: &str, last_error: &mut Option<String>) {
    if last_error.as_deref() != Some(error) {
        warn!("[avatar] collision policy unavailable: {error}");
        *last_error = Some(error.to_string());
    }
}

// ─── Behavior Components ─────────────────────────────────────────────────────

/// Select the pose for a surface-to-orbit scroll entry. The first entry has no
/// avatar-owned orbital presentation to restore and must derive its arm from
/// the live surface position; a later entry reuses the settled body pose.
fn scroll_entry_orbit_camera(
    target: Entity,
    body: &CelestialBody,
    radius_m: f64,
    history: Option<&OrbitViewHistory>,
) -> (OrbitCamera, bool) {
    let saved_pose = history.and_then(|history| history.pose(body.ephemeris_id));
    let camera = OrbitCamera {
        target,
        distance: saved_pose.map_or(radius_m * 3.0, |pose| pose.distance()),
        yaw: saved_pose.map_or(0.0, |pose| pose.yaw()),
        pitch: saved_pose.map_or(0.0, |pose| pose.pitch()),
        damping: saved_pose.and_then(|pose| pose.damping()),
        vertical_offset: saved_pose.map_or(0.0, |pose| pose.vertical_offset()),
    };
    (camera, saved_pose.is_none())
}

fn remember_user_orbit_pose(
    history: &mut OrbitViewHistory,
    camera: &OrbitCamera,
    q_bodies: &Query<&CelestialBody>,
) {
    let Ok(body) = q_bodies.get(camera.target) else {
        return;
    };
    remember_orbit_pose_for_body(history, camera, body.ephemeris_id);
}

fn remember_orbit_pose_for_body(
    history: &mut OrbitViewHistory,
    camera: &OrbitCamera,
    body_id: i32,
) {
    let Some(pose) = OrbitPose::from_camera(camera) else {
        warn!(
            target: "avatar",
            body = body_id,
            "discarding non-finite user orbit pose"
        );
        return;
    };
    history.remember(body_id, pose);
}

/// Retain a user-controlled orbit pose at the single ownership boundary where
/// an orbit mode ends. Every transition that removes or replaces
/// `OrbitCamera` therefore shares the same history write; no command path can
/// accidentally forget one exit route.
fn remember_orbit_camera_on_remove(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    if world.get::<OrbitUserInput>(entity).is_none() {
        return;
    }
    let Some(camera) = world.get::<OrbitCamera>(entity).cloned() else {
        return;
    };
    let Some(body_id) = world
        .get::<CelestialBody>(camera.target)
        .map(|body| body.ephemeris_id)
    else {
        return;
    };
    let Some(mut history) = world.get_mut::<OrbitViewHistory>(entity) else {
        return;
    };
    remember_orbit_pose_for_body(&mut history, &camera, body_id);
    world.commands().entity(entity).remove::<OrbitUserInput>();
}

fn register_orbit_history_hook(app: &mut App) {
    app.world_mut()
        .register_component_hooks::<OrbitCamera>()
        .on_remove(remember_orbit_camera_on_remove);
}

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

/// Plugin for managing local embodiment logic, input processing, and possession.
pub struct LunCoAvatarPlugin;

fn trigger_vessel_hard_stop(commands: &mut Commands, vessel_entity: Entity) {
    commands.trigger(lunco_cosim_core::commands::ReleaseControl {
        target: vessel_entity,
    });
}

fn stop_previous_vessel(
    commands: &mut Commands,
    previous: Option<Entity>,
    released: &[u64],
    q_owned: &Query<&lunco_core::GlobalEntityId>,
) {
    if let Some(entity) = previous {
        let old_gid = q_owned.get(entity).ok().map(|gid| gid.get());
        if old_gid.is_none_or(|gid| !released.contains(&gid)) {
            trigger_vessel_hard_stop(commands, entity);
        }
    }
}

/// Commit one possession to the authoritative registry after the command has
/// validated its endpoint and any requested local binding. The generic session
/// transition owns the authority table; this observer owns the avatar link and
/// camera transaction that compose it.
fn commit_possession_authority(
    commands: &mut Commands,
    authority: &mut PossessionAuthority,
    target: Entity,
) -> Option<Vec<u64>> {
    let origin = authority.guard.0.unwrap_or(authority.session.0);
    let target_gid = authority.q_owned.get(target).ok().map(|gid| gid.get());
    // Clients keep the host's table as the authority and only use the shared
    // table as the authority and only use the shared predicate for optimistic
    // local binding.
    if matches!(*authority.role, lunco_core_session::NetworkRole::Client) {
        return Some(Vec::new());
    }

    let change = if let Some(gid) = target_gid {
        match lunco_core_session::claim_control(
            &mut authority.registry,
            &authority.rbac,
            origin,
            gid,
        ) {
            Ok(change) => change,
            Err(error) => {
                warn!("[auth] session {origin} claim for entity {gid} refused: {error}");
                return None;
            }
        }
    } else {
        lunco_core_session::release_control(&mut authority.registry, origin)
    };
    let released = change.released.clone();
    commands.trigger(change);
    Some(released)
}

/// Scene-owned USD prims are the identity boundary for vessel claims. Clear
/// their session ownership while the outgoing projection still exists, before
/// Bevy applies the deferred despawns. The replacement scene may reuse the
/// same deterministic global ids without inheriting authority from the prior
/// scene.
fn clear_scene_possession_claims(
    mut registry: ResMut<lunco_core_session::SessionRegistry>,
    q_scene_prims: Query<(&lunco_core::GlobalEntityId, &UsdPrimPath)>,
) {
    let mut released = 0;
    for (gid, _) in q_scene_prims.iter() {
        if registry.clear_gid(gid.get()).is_some() {
            released += 1;
        }
    }
    if released > 0 {
        info!("[auth] scene teardown released {released} USD vessel claim(s)");
    }
}

/// Client-side correction: drop control of any vessel the synced ownership table
/// no longer attributes to us (we lost a possession race, or the host force-
/// released us). Keeps "only one owner" true even when an optimistic local bind
/// raced another client. A missing target identity is stale scene state and is
/// released through the same command path.
fn enforce_ownership(
    role: Res<lunco_core_session::NetworkRole>,
    registry: Res<lunco_core_session::SessionRegistry>,
    session: Res<lunco_core_session::LocalSession>,
    q_avatar: Query<(Entity, &ControlLink), (With<Avatar>, With<LocalAvatar>)>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    mut commands: Commands,
) {
    if !matches!(*role, lunco_core_session::NetworkRole::Client) {
        return;
    }
    for (avatar, link) in q_avatar.iter() {
        let Ok(gid) = q_gid.get(link.target) else {
            commands.trigger(ReleaseVessel { target: avatar });
            continue;
        };
        if registry.owner_of(gid.get()) != Some(session.0) {
            commands.trigger(ReleaseVessel { target: avatar });
        }
    }
}

impl Plugin for LunCoAvatarPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_avatar_core::roles::AvatarCorePlugin>() {
            app.add_plugins(lunco_avatar_core::roles::AvatarCorePlugin);
        }
        if !app.is_plugin_added::<lunco_camera_runtime::CameraRuntimePlugin>() {
            app.add_plugins(lunco_camera_runtime::CameraRuntimePlugin);
        }
        register_orbit_history_hook(app);
        if !app.is_plugin_added::<lunco_input_core::InputBindingsPlugin>() {
            app.add_plugins(lunco_input_core::InputBindingsPlugin);
        }
        app.init_resource::<AvatarCollisionSettings>()
            .init_resource::<SurfaceModeThreshold>();
        app.configure_sets(Update, AvatarSceneHandoffSet);
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
        lunco_control_core::ensure_control_plugin(app);
        // Possession and release commands own both authority bookkeeping and
        // local binding, so the registry and `ControlLink` commit together.
        app.add_systems(lunco_core::SceneTeardown, clear_scene_possession_claims);
        // Scene-click possession/follow/focus is now bevy_picking-driven: a
        // global `Pointer<Click>` observer (egui occlusion handled by the
        // framework), replacing the old `ScenePointer`-gated Update system.
        //
        // The observer reads two click-arbitration resources — `DragModeActive`
        // (gizmo drag in progress) and `SpawnToolActive` (click-to-place armed).
        // Both are normally owned by the scene-edit core, but the
        // observer lives here and fires on the FIRST pointer event, so a binary
        // that uses the avatar without the editor (luncosim) would panic on the
        // missing `Res`. Guarantee them here — `init_resource` is idempotent, so
        // a host that inserts its own (luncosim) keeps that value.
        app.init_resource::<lunco_interaction_core::DragModeActive>();
        app.init_resource::<lunco_core::SpawnToolActive>();
        app.init_resource::<lunco_core::TerrainToolActive>();
        app.init_resource::<lunco_core::ArmedScriptTool>();
        app.init_resource::<lunco_core::SceneInteractionMode>();
        app.add_observer(avatar_raycast_possession);
        // Native avatar construction receives the resolved command policy;
        // composed USD avatars receive the same policy from their `Controls` scope.
        app.add_observer(demote_former_avatar);
        app.add_observer(clear_orbit_view_history_on_twin_closed);
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
        app.register_type::<AdaptiveNearPlane>()
            .register_type::<SurfaceRelativeMode>()
            .register_type::<SurfaceModeThreshold>()
            .register_type::<AvatarCollisionSettings>();

        app.register_settings_section::<ProfileSettings>();
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
        // USD projection and celestial projection publish scene entities in
        // Update. The camera subsystem owns the complete handoff after those
        // publishers have committed their deferred components.
        app.add_systems(
            Update,
            (
                capture_site_camera_pose.run_if(site_camera_capture_changed),
                bind_local_avatar_to_site_grid.run_if(avatar_site_handoff_changed),
            )
                .chain()
                .in_set(AvatarSceneHandoffSet),
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

        // Incremental camera modes are stepped at a constant 60 Hz and eased by
        // `InteractionEased`.  Surface mode is derived directly from its gravity
        // frame and is therefore a direct single-writer mode. The chase camera is different: it follows the body's
        // final render pose and therefore runs once at render cadence below.  Keeping
        // it out of this schedule prevents two independent interpolation phases from
        // fighting over the same camera Transform.
        // The generic camera runtime owns rebranching and pose writers. Avatar
        // contributes only the source-specific transit and locomotion edges
        // around the shared camera set.
        app.add_systems(
            lunco_time::InteractionSchedule,
            (freeflight_scroll_transit_system,)
                .chain()
                .before(CameraUpdateSet),
        );
        app.add_systems(
            lunco_time::InteractionSchedule,
            apply_fly.after(CameraUpdateSet),
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
            CameraUpdateSet
                .after(lunco_time::InteractionRestoreSet)
                .after(lunco_controller::InteractionControlSet)
                .before(lunco_time::InteractionRecordSet),
        );
        // Source-specific camera realizations are installed by their focused
        // camera packages. This crate retains the transition and input edges;
        // the application composition root installs their writers explicitly.
        // Every avatar gets easing only for incremental stepped camera modes.
        // Surface mode derives a complete local pose from gravity and spring-arm
        // mode follows the final rendered body pose; neither may have a second
        // Transform writer.
        app.add_systems(Update, sync_avatar_easing);

        // Camera drag and avatar camera-mode transitions remain here. Focused
        // camera packages realize source-specific spatial placement after the
        // avatar has selected a mode and target.
    }
}

/// Pose captured while a local avatar is still attached to the loader's world
/// Grid. The value is local to the authored site frame, so it remains valid
/// when the site root becomes or is already a BigSpace Grid.
#[derive(Component, Clone, Copy, Debug)]
struct PendingSiteCameraPose {
    site_root: Entity,
    position: DVec3,
    rotation: DQuat,
}

fn site_camera_capture_changed(
    q_site: Query<(), With<lunco_celestial::SiteAnchor>>,
    q_avatar: Query<
        (),
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<PendingSiteCameraPose>,
            Or<(
                Changed<Avatar>,
                Changed<LocalAvatar>,
                Changed<ChildOf>,
                Changed<Transform>,
            )>,
        ),
    >,
) -> bool {
    !q_site.is_empty() && !q_avatar.is_empty()
}

/// Capture an authored local-camera pose before the binder migrates the avatar
/// into the site frame. The shared common-grid conversion also handles a USD
/// avatar projected after the site root has already moved beneath a celestial
/// surface Grid.
fn capture_site_camera_pose(
    q_site: Query<Entity, With<lunco_celestial::SiteAnchor>>,
    q_avatar: Query<
        (Entity, &ChildOf, Option<&PendingSiteCameraPose>),
        (With<Avatar>, With<LocalAvatar>),
    >,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_world_grid: Query<(), With<lunco_spatial::WorldGrid>>,
    mut commands: Commands,
) {
    let Ok(site_root) = q_site.single() else {
        return;
    };

    for (avatar, child_of, pending) in &q_avatar {
        if pending.is_some() {
            continue;
        }
        let current_parent = child_of.parent();
        if q_grids.get(current_parent).is_ok() && q_world_grid.get(current_parent).is_err() {
            continue;
        }
        let pose = lunco_spatial::coords::common_grid_poses(
            avatar, site_root, &q_parents, &q_grids, &q_spatial,
        )
        .map(
            |(_, avatar_position, avatar_rotation, site_position, site_rotation)| {
                let inverse_site_rotation = site_rotation.inverse();
                (
                    inverse_site_rotation * (avatar_position - site_position),
                    (inverse_site_rotation * avatar_rotation).normalize(),
                )
            },
        );
        let Some((avatar_position, avatar_rotation)) = pose else {
            warn!(
                ?avatar,
                ?site_root,
                "local avatar cannot be composed with the authored site root"
            );
            continue;
        };
        commands.entity(avatar).try_insert(PendingSiteCameraPose {
            site_root,
            position: avatar_position,
            rotation: avatar_rotation.normalize(),
        });
        info!(
            ?avatar,
            ?site_root,
            "captured local avatar pose for site camera handoff"
        );
    }
}

/// Run the startup camera handoff when either side of the scene/camera
/// boundary changes. This keeps the steady-state path out of the frame loop;
/// scene replacement and a newly projected local avatar are the only events
/// that can require this binding.
fn avatar_site_handoff_changed(
    q_site: Query<(), (With<lunco_celestial::SiteAnchor>, Changed<Grid>)>,
    q_avatar: Query<
        (),
        (
            With<Avatar>,
            With<LocalAvatar>,
            Or<(
                Added<LocalAvatar>,
                Changed<ChildOf>,
                Added<PendingSiteCameraPose>,
            )>,
        ),
    >,
) -> bool {
    !q_site.is_empty() || !q_avatar.is_empty()
}

/// Mount a loader-created local avatar into the authored site's Grid.
///
/// USD projection initially has no celestial knowledge and therefore places
/// the avatar under the persistent world shell. Once celestial placement has
/// made the authored site root a Grid, the camera subsystem converts the
/// avatar pose through the shared BigSpace coordinate helpers and atomically
/// re-parents it. A camera already mounted in another valid Grid is left to
/// its owning camera mode (for example, orbital view).
fn bind_local_avatar_to_site_grid(
    q_site: Query<(Entity, &lunco_celestial::GeodeticAnchor), With<lunco_celestial::SiteAnchor>>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_avatar: Query<(Entity, &ChildOf, Option<&GravityBody>), (With<Avatar>, With<LocalAvatar>)>,
    q_pending: Query<&PendingSiteCameraPose>,
    q_grids: Query<&Grid>,
    q_world_grid: Query<(), With<lunco_spatial::WorldGrid>>,
    mut commands: Commands,
) {
    let Ok((site_root, anchor)) = q_site.single() else {
        return;
    };
    let Some((body_entity, _)) = q_bodies
        .iter()
        .find(|(_, body)| body.ephemeris_id == anchor.body)
    else {
        return;
    };
    let Ok(site_grid) = q_grids.get(site_root) else {
        return;
    };

    for (avatar, child_of, gravity_body) in &q_avatar {
        let current_parent = child_of.parent();
        if current_parent == site_root {
            if gravity_body.is_none_or(|binding| binding.body_entity != body_entity) {
                commands
                    .entity(avatar)
                    .try_insert(GravityBody { body_entity });
            }
            commands
                .entity(avatar)
                .try_remove::<PendingSiteCameraPose>();
            continue;
        }

        // A valid non-world Grid is already owned by an explicit camera mode.
        // The startup binder must not reclaim orbital or target-relative views.
        if q_grids.get(current_parent).is_ok() && q_world_grid.get(current_parent).is_err() {
            continue;
        }

        let Ok(pending) = q_pending.get(avatar) else {
            warn!(
                ?avatar,
                ?site_root,
                "local avatar has no pre-mount pose for the authored site Grid"
            );
            continue;
        };
        if pending.site_root != site_root {
            warn!(
                ?avatar,
                ?site_root,
                pending_site = ?pending.site_root,
                "local avatar site-camera handoff targets a different scene"
            );
            continue;
        }
        let site_position = pending.position;
        let site_rotation = pending.rotation;
        let (cell, translation) = site_grid.translation_to_grid(site_position);
        migrate_to_grid(
            &mut commands,
            avatar,
            site_root,
            cell,
            Transform::from_translation(translation).with_rotation(site_rotation.as_quat()),
        );
        commands
            .entity(avatar)
            .try_insert(GravityBody { body_entity })
            .try_remove::<PendingSiteCameraPose>();
        info!(
            ?avatar,
            ?site_root,
            "local avatar camera mounted in the authored site Grid"
        );
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
            Has<lunco_camera_core::CameraPoseLock>,
        ),
        (With<Avatar>, With<LocalAvatar>),
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
            With<LocalAvatar>,
            Without<lunco_camera_core::CameraPoseLock>,
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
/// [`lunco_control_core::EguiFocus`] is published each frame by `lunco-workbench` from
/// the primary egui context's `wants_keyboard_input()`. On a headless binary
/// nothing writes it, so it stays default (`false`) and the gate is always open.
/// One-frame latency (the flag reflects the previous egui pass) is imperceptible
/// for held input.
fn scene_keyboard_active(focus: Res<lunco_control_core::EguiFocus>) -> bool {
    !focus.wants_keyboard
}

/// Local avatars are command endpoints with an authored-equivalent
/// `ControlBinding` and `InputPorts` surface. The shared controller translates
/// intents into ports, and the flight realization consumes only those ports.
fn demote_former_avatar(trigger: On<Remove, LocalAvatar>, mut commands: Commands) {
    let entity = trigger.entity;
    // Retirement is a presentation contract consumed by the one viewport
    // reconciler. Do not write Camera::is_active here: avatar role lifecycle
    // and viewport activation are separate ownership boundaries.
    commands.entity(entity).try_remove::<(
        Avatar,
        FreeFlightCamera,
        OrbitCamera,
        SpringArmCamera,
        SurfaceRelativeMode,
        OrbitViewHistory,
        OrbitUserInput,
        CurrentRegionArrival,
        ControlLink,
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
    // A host-created camera that is not a USD prim is outside a scene load's
    // `despawn` sweep. Deactivation above closes the ownership transition
    // while the replacement's `Camera3d`/`Projection` is attached by the
    // deferred `SceneCamera` binder.
    //
    // `Camera`/`Camera3d` are deliberately left in place: stripping `Camera` from a
    // live, already-extracted window camera orphans its render-world view and crashes
    // `prepare_lights` on the cascade unwrap (see this function's docs). Removing only
    // the intent marker retires the camera from selection while leaving the render
    // world intact. The shared viewport reconciler still sees every window
    // `Camera3d`, so it deactivates this orphan in the same PostUpdate pass.
    commands
        .entity(entity)
        .try_insert(lunco_render::CameraRetiring)
        .try_remove::<lunco_render::SceneCamera>();
}

/// Retire per-avatar orbital presentation state with the active Twin. A
/// non-active Twin closing must not disturb the live camera's history.
fn clear_orbit_view_history_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    mut commands: Commands,
    q_avatar: Query<Entity, With<OrbitViewHistory>>,
) {
    if !trigger.event().was_active {
        return;
    }
    for entity in &q_avatar {
        commands
            .entity(entity)
            .remove::<(OrbitViewHistory, OrbitUserInput, CurrentRegionArrival)>();
    }
}

/// The body explicitly authored by the loaded site.
fn site_body(
    q_site: &Query<&lunco_celestial::GeodeticAnchor, With<lunco_celestial::SiteAnchor>>,
    q_bodies: &Query<(Entity, &CelestialBody)>,
) -> Option<(Entity, f64)> {
    let anchor = q_site.single().ok()?;
    let (ent, body) = q_bodies
        .iter()
        .find(|(_, b)| b.ephemeris_id == anchor.body)?;
    Some((ent, body.radius_m))
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

/// Migrate the avatar to a target's Grid, placing it at a pose already expressed
/// in that Grid's local frame.
///
/// This is intentionally a local-frame boundary. A possession/follow target is
/// resolved with [`lunco_spatial::coords::grid_relative_pose`], so the camera is
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

// ─── Locomotion ──────────────────────────────────────────────────────────────

/// Move the avatar's capsule in the active Avian frame, then return the
/// resulting position to the avatar's source Grid.
///
/// The avatar itself is not a dynamic rigid body: it is a client-local camera
/// embodiment. `MoveAndSlide` is therefore the correct kinematic boundary. It
/// uses the same projected USD colliders as the physics solver, while the
/// canonical BigSpace transform helpers keep both the query origin and the
/// result in `ActivePhysicsFrame` even when the camera Grid is nested or
/// rotated. This path does not alter Avian's fixed-tick substep count.
fn move_avatar_with_collision(
    avatar: Entity,
    source_grid: Entity,
    cell: &CellCoord,
    transform: &Transform,
    desired_delta: DVec3,
    up_direction: Vec3,
    delta_time: std::time::Duration,
    active_frame: Option<Entity>,
    move_and_slide: Option<&MoveAndSlide<'_, '_>>,
    collision_settings: &AvatarCollisionSettings,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
) -> Option<DVec3> {
    let move_and_slide = move_and_slide?;
    let active_frame = active_frame?;
    let delta_secs = delta_time.as_secs_f64();
    if !delta_secs.is_finite() || delta_secs <= 0.0 {
        return None;
    }
    if !collision_settings.radius_m.is_finite()
        || !collision_settings.capsule_length_m.is_finite()
        || collision_settings.radius_m <= 0.0
        || collision_settings.capsule_length_m < 0.0
    {
        return None;
    }

    let source_grid_ref = q_grids.get(source_grid).ok()?;
    let source_position = source_grid_ref.grid_position_double(cell, transform);
    if !source_position.is_finite() || !desired_delta.is_finite() || !up_direction.is_finite() {
        return None;
    }
    let source_to_physics = lunco_spatial::coords::grid_transform_between_grids(
        source_grid,
        active_frame,
        q_parents,
        q_grids,
        q_spatial,
    )?;
    let physics_to_source = lunco_spatial::coords::grid_transform_between_grids(
        active_frame,
        source_grid,
        q_parents,
        q_grids,
        q_spatial,
    )?;
    let physics_position = source_to_physics.transform_position(source_position);
    let physics_delta = source_to_physics.transform_vector(desired_delta);
    let physics_up = source_to_physics
        .transform_vector(up_direction.as_dvec3())
        .normalize_or(DVec3::Y);
    if !physics_position.is_finite()
        || !physics_delta.is_finite()
        || !physics_up.is_finite()
        || physics_up.length_squared() <= f64::EPSILON
    {
        return None;
    }

    // The camera may pitch, but the avatar's body stays upright in the local
    // surface/world-up direction. A capsule has no meaningful yaw, so this
    // shortest-arc rotation is the complete shape orientation contract.
    let shape_rotation = DQuat::from_rotation_arc(DVec3::Y, physics_up);
    let velocity = physics_delta / delta_secs;
    let shape = Collider::capsule(
        collision_settings.radius_m,
        collision_settings.capsule_length_m,
    );
    let mut filter = SpatialQueryFilter::from_excluded_entities([avatar]);
    filter.mask = avian3d::prelude::LayerMask(!lunco_core::NON_PHYSICAL_QUERY_LAYERS);
    let output = move_and_slide.move_and_slide(
        &shape,
        physics_position,
        shape_rotation,
        velocity,
        delta_time,
        &MoveAndSlideConfig::default(),
        &filter,
        |_| MoveAndSlideHitResponse::Accept,
    );
    if !output.position.is_finite() {
        return None;
    }
    let result = physics_to_source.transform_position(output.position);
    result.is_finite().then_some(result)
}

/// Apply one complete Grid-absolute position without duplicating the
/// `CellCoord`/local-Transform split at each avatar movement entry point.
fn write_avatar_grid_position(
    grid: &Grid,
    cell: &mut CellCoord,
    transform: &mut Transform,
    position: DVec3,
) {
    let (new_cell, new_transform) = grid.translation_to_grid(position);
    if *cell != new_cell {
        *cell = new_cell;
    }
    if transform.translation != new_transform {
        transform.translation = new_transform;
    }
}

/// Kinematic realization for the avatar's authored flight controller. Reads the
/// avatar's FSW input ports
/// (`forward`/`side`/`up`,
/// written through the shared `SetPorts` path by `drive_from_bindings`) and
/// translates the avatar entity through Avian's kinematic move-and-slide
/// controller unless the active Twin explicitly opts into traversal.
///
/// Only active with a `FreeFlightCamera`/`SurfaceCamera`, or when CTRL is held while
/// possessing a vessel (a momentary free-flight overlay). The controller writes the
/// normalized `speed_boost` command beside the movement axes, so the modifier and
/// direction are consumed as one command frame.
/// Q/E elevation follows world up in free flight and gravity up in surface mode.
/// Runs in the interaction cadence at wall-clock time, so the local camera
/// keeps moving even when the sim's virtual clock is paused/slowed.
fn apply_fly(
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &ChildOf,
            &lunco_port_core::InputPorts,
            &FreeFlightSettings,
            Has<FreeFlightCamera>,
            Has<SurfaceCamera>,
            Option<&SurfaceRelativeMode>,
        ),
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    gravity: Res<LocalGravityField>,
    keys: Res<ButtonInput<KeyCode>>,
    // The INTERACTION clock (wall-rooted): the avatar keeps flying while the sim is
    // paused, because pausing the simulation is not supposed to paralyse the user.
    time: Res<Time>,
    drag_mode: Option<Res<lunco_interaction_core::DragModeActive>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    move_and_slide: Option<MoveAndSlide<'_, '_>>,
    collision_settings: Res<AvatarCollisionSettings>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut policy_error: Local<Option<String>>,
) {
    if drag_mode.is_some_and(|drag| drag.active) {
        return;
    }
    let policy = match avatar_soil_collision_policy(workspace.as_deref()) {
        Ok(policy) => policy,
        Err(error) => {
            report_avatar_policy_error(&error, &mut policy_error);
            return;
        }
    };
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    for (
        entity,
        mut tf,
        mut cell,
        child_of,
        inputs,
        flight_settings,
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
        // `ControlBinding`). When free (no ControlLink)
        // `drive_from_bindings` writes these; while possessing they stay 0 (control is
        // redirected to the vessel).
        let forward = inputs.cmd("forward") as f32;
        let side = inputs.cmd("side") as f32;
        let elevation = inputs.cmd("up") as f32;
        let deadzone = flight_settings.input_deadzone as f32;
        let boost = if inputs.cmd("speed_boost") > flight_settings.boost_threshold {
            flight_settings.boost_multiplier
        } else {
            1.0
        };
        if forward.abs() < deadzone && side.abs() < deadzone && elevation.abs() < deadzone {
            continue;
        }

        // Q/E are vertical movement relative to the current world/surface, not
        // the camera's pitched up vector. A camera-relative elevation basis can
        // cancel W/S's horizontal component at a particular pitch (most visibly
        // Q+W), making a valid diagonal look stationary.
        let up_dir = if surface_mode.is_some() {
            let Some(up) =
                gravity_up_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial)
            else {
                continue;
            };
            up
        } else {
            Vec3::Y
        };
        let move_vec = camera_move_direction(&tf, forward, side, elevation, up_dir);

        // Authored flight speed × the real frame delta.
        // Normalize the combined direction BEFORE applying the speed. Scaling
        // the inputs first would make the unit-vector cap erase the boost.
        let desired_delta =
            move_vec.as_dvec3() * flight_settings.speed_mps * boost * time.delta_secs_f64();
        let next_pos = if policy == AvatarSoilCollisionPolicy::ThroughSoilAllowed {
            current_pos + desired_delta
        } else {
            let Some(next_pos) = move_avatar_with_collision(
                entity,
                child_of.parent(),
                &cell,
                &tf,
                desired_delta,
                up_dir,
                time.delta(),
                active_frame.as_deref().map(|frame| frame.0),
                move_and_slide.as_ref(),
                &collision_settings,
                &q_parents,
                &q_grids,
                &q_spatial,
            ) else {
                warn_once!("[avatar] safe collision movement unavailable; movement held");
                continue;
            };
            next_pos
        };
        write_avatar_grid_position(grid, &mut cell, &mut tf, next_pos);
    }
}

// ─── Raycasting ──────────────────────────────────────────────────────────────

/// Resolves a picked vehicle part to its authored vehicle control root.
///
/// `SelectableRoot` is an editor boundary, and every independently simulated
/// wheel may carry it. [`lunco_port_core::InputPorts`] is the public interface:
/// its nonempty vocabulary is the input surface a session may own. A
/// [`lunco_control_core::ControlBinding`] or [`lunco_core::MobilityRoot`] identifies the
/// authored vehicle boundary, which takes precedence over nested component
/// endpoints. An [`Avatar`] endpoint is excluded even when it carries its own
/// movement ports; walking past one to this owner makes a click on a vehicle
/// part possess the vehicle rather than the avatar.
fn find_control_owner_from_hit(
    mut entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_input_ports: &Query<&lunco_port_core::InputPorts, Without<Avatar>>,
    q_vehicle_roots: &Query<
        (),
        Or<(
            With<lunco_control_core::ControlBinding>,
            With<lunco_core::MobilityRoot>,
        )>,
    >,
    q_preview_only: &Query<(), With<UsdPreviewOnly>>,
    q_ground: &Query<Entity, With<lunco_core::Ground>>,
) -> Option<Entity> {
    let mut nearest_endpoint = None;
    for _ in 0..lunco_spatial::MAX_HIERARCHY_WALK_DEPTH {
        if q_ground.get(entity).is_ok() {
            return None;
        }
        // A vehicle may contain nested Modelica or actuator input surfaces.
        // Those are valid command endpoints for their own generic APIs, but a
        // scene click on a vehicle part must resolve to the vehicle's authored
        // control root so the camera and controller switch as one unit.
        if is_vessel_control_endpoint(entity, q_input_ports, q_parents, q_preview_only) {
            nearest_endpoint.get_or_insert(entity);
        }
        if q_vehicle_roots.get(entity).is_ok() {
            // A root without its own writable surface is not ready to possess;
            // never fall through to a nested endpoint and split the vehicle's
            // interaction identity.
            return is_vessel_control_endpoint(entity, q_input_ports, q_parents, q_preview_only)
                .then_some(entity);
        }
        if let Ok(parent) = q_parents.get(entity) {
            entity = parent.parent();
        } else {
            break;
        }
    }
    nearest_endpoint
}

/// The possession boundary is a writable command surface owned by a domain
/// entity, not a presentation marker. The local avatar has an `InputPorts`
/// surface too, but that surface drives its free-flight embodiment and must
/// never become the vessel selected by a click or a direct possession command.
fn is_vessel_control_endpoint(
    entity: Entity,
    q_input_ports: &Query<&lunco_port_core::InputPorts, Without<Avatar>>,
    q_child_of: &Query<&ChildOf>,
    q_preview_only: &Query<(), With<UsdPreviewOnly>>,
) -> bool {
    // USD previews intentionally mirror a live vessel's input surface so
    // their Inspector can describe the assembly, but they are not simulation
    // endpoints. The USD-owned root marker is authoritative for every
    // descendant, including a preview vessel whose name matches the live one.
    if is_preview_only(entity, q_child_of, q_preview_only) {
        return false;
    }
    q_input_ports
        .get(entity)
        .is_ok_and(|surface| !surface.values.is_empty())
}

/// Raycasts possession against actual collider geometry.
///
/// Uses Avian3D SpatialQuery to hit real mesh colliders, not invisible spheres.
/// Walks up the parent chain to find the owning vessel control endpoint for
/// possession. An avatar endpoint is never a vessel target.
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
/// which the generic placement tools use) and fold that distance into
/// `min_t` before the sphere loop. That fixes Earth-through-the-ground too, and stops
/// the behaviour depending on whether a terrain happens to be tile-streamed.
///
/// Doing it here means giving this observer a `GridSurfaceQuery`, which pulls
/// `lunco-terrain-surface` into `lunco-avatar`'s dependency set — a call the crate
/// boundary owner should make, not something to slip into a bug fix. Until then:
/// off. Focus is still reachable through the `FocusTarget` command and the
/// `focus_target` API/MCP verb; only the click gesture is suppressed.
const CELESTIAL_CLICK_FOCUS: bool = false;

#[derive(bevy::ecs::system::SystemParam)]
/// Shared scene-click mode and egui gate for the avatar pointer observer.
pub struct SceneInteractionGate<'w> {
    mode: Res<'w, lunco_core::SceneInteractionMode>,
    egui_focus: Res<'w, lunco_control_core::EguiFocus>,
}

pub fn avatar_raycast_possession(
    // Driven by bevy_picking: a global `On<Pointer<Click>>` observer. The
    // egui-vs-scene guard is `EguiFocus.wants_pointer` (via `scene_click_ray`) —
    // a global flag, fed by the workbench's egui-authoritative `pointer_over_scene`
    // signal, so a click on any real chrome is stood down here even though this
    // global observer can fire on a scene entity behind the panel.
    mut click: On<bevy::picking::events::Pointer<bevy::picking::events::Click>>,
    keys: Res<ButtonInput<KeyCode>>,
    camera_q: Query<
        (&Camera, &GlobalTransform, Entity, &IntentState),
        (With<Avatar>, With<LocalAvatar>),
    >,
    scene_interaction: SceneInteractionGate,
    drag_mode_active: Res<lunco_interaction_core::DragModeActive>,
    spawn_tool_active: Res<lunco_core::SpawnToolActive>,
    terrain_tool_active: Res<lunco_core::TerrainToolActive>,
    armed_script_tool: Res<lunco_core::ArmedScriptTool>,
    mut commands: Commands,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>,
    q_spacecraft: Query<(Entity, &GlobalTransform, &Spacecraft)>,
    q_input_ports: Query<&lunco_port_core::InputPorts, Without<Avatar>>,
    q_parents: Query<&ChildOf>,
    q_vehicle_roots: Query<
        (),
        Or<(
            With<lunco_control_core::ControlBinding>,
            With<lunco_core::MobilityRoot>,
        )>,
    >,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    q_ground: Query<Entity, With<lunco_core::Ground>>,
) {
    use bevy::picking::pointer::PointerButton;
    // Left button only.
    if click.button != PointerButton::Primary {
        return;
    }
    // The editor selection observer receives the same global click. Both
    // observers consult the shared mode and modifier intent so one gesture has
    // one owner: View plain clicks possess, while modifiers select/remove.
    let modified = keys.any_pressed([
        KeyCode::AltLeft,
        KeyCode::AltRight,
        KeyCode::ShiftLeft,
        KeyCode::ShiftRight,
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
    ]);
    if !scene_interaction.mode.possession_owns_click(modified) {
        return;
    }
    let Some((camera, cam_gtf, avatar_entity, _intents)) = camera_q.single().ok() else {
        return;
    };
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
        scene_interaction.egui_focus.wants_pointer,
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

    let control_target = find_control_owner_from_hit(
        click.entity,
        &q_parents,
        &q_input_ports,
        &q_vehicle_roots,
        &q_preview_only,
        &q_ground,
    );

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
/// (which strips ControlLink, SpringArm, OrbitCamera, interpolation, and
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
            With<LocalAvatar>,
            Or<(With<ControlLink>, With<SpringArmCamera>, With<OrbitCamera>)>,
        ),
    >,
    cursor_mode: lunco_core::CursorModeActive,
    mut commands: Commands,
) {
    // `Cancel` unwinds the active cursor mode first. While a spawn ghost, terrain
    // brush, or authored script tool owns the pointer, Cancel belongs to that mode,
    // not to possession. With nothing up, Cancel means
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
        .remove::<RadialArrival>()
        .remove::<CurrentRegionArrival>()
        .remove::<OrbitUserInput>();

    match state.behavior() {
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
    if let Some(gravity_body) = state.gravity_body() {
        entity.try_insert(gravity_body);
    } else {
        entity.remove::<GravityBody>();
    }
    if state.surface_relative() {
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
            &mut CameraZoomInput,
            &ChildOf,
            &OrbitViewReturn,
            Option<&OrbitCamera>,
            Option<&mut OrbitViewHistory>,
            Has<OrbitUserInput>,
            Has<lunco_camera_core::CameraPoseLock>,
        ),
        (With<Avatar>, With<LocalAvatar>),
    >,
    q_bodies: Query<&CelestialBody>,
    mut orbital_pin: Option<ResMut<lunco_celestial_spatial::OrbitalViewPin>>,
) {
    let avatar = trigger.event().target;
    let Ok((
        mut transform,
        mut cell,
        mut zoom,
        child_of,
        return_state,
        current_orbit,
        mut orbit_history,
        orbit_user_input,
        cinematic_lock,
    )) = q_avatar.get_mut(avatar)
    else {
        return;
    };
    if cinematic_lock {
        return;
    }
    // Returning is a camera-mode handoff even when initiated by a typed
    // command rather than the wheel. Keep the restored mode from inheriting
    // any input that was already in flight at the handoff boundary.
    zoom.begin_mode_transition(None);
    if orbit_user_input {
        if let (Some(camera), Some(history)) = (current_orbit, orbit_history.as_deref_mut()) {
            remember_user_orbit_pose(history, camera, &q_bodies);
        }
    }
    let return_state = return_state.clone();

    if child_of.parent() == return_state.parent_grid() {
        cell.set_if_neq(return_state.cell());
        transform.set_if_neq(return_state.transform());
    } else {
        migrate_to_grid(
            &mut commands,
            avatar,
            return_state.parent_grid(),
            return_state.cell(),
            return_state.transform(),
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
            Option<&ControlLink>,
            Option<&SurfaceRelativeMode>,
            &ChildOf,
            Option<&OrbitViewReturn>,
            Option<&OrbitCamera>,
            Option<&mut OrbitViewHistory>,
            Has<OrbitUserInput>,
            Has<lunco_camera_core::CameraPoseLock>,
        ),
        (With<Avatar>, With<LocalAvatar>),
    >,
    guard: Res<lunco_core_session::SyncApplyGuard>,
    mut orbital_pin: Option<ResMut<lunco_celestial_spatial::OrbitalViewPin>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_owned: Query<&lunco_core::GlobalEntityId>,
    q_bodies: Query<&CelestialBody>,
    gravity: Res<LocalGravityField>,
    role: Res<lunco_core_session::NetworkRole>,
    mut authority: Option<ResMut<lunco_core::markers::FlightAuthority>>,
    local: Res<lunco_core_session::LocalSession>,
    mut registry: ResMut<lunco_core_session::SessionRegistry>,
) {
    let cmd = trigger.event();
    // A wire-applied release carries the remote client's avatar, which is not a
    // local camera entity. The same command still owns the host-side release and
    // hard-stop transaction.
    if guard.is_from_sync() {
        if !matches!(*role, lunco_core_session::NetworkRole::Client) {
            let origin = guard.0.unwrap_or(local.0);
            let change = lunco_core_session::release_control(&mut registry, origin);
            let released = change.released.clone();
            commands.trigger(change);
            if !released.is_empty() {
                info!(
                    "[auth] session {origin} released {} vessel(s)",
                    released.len()
                );
            }
        }
        return;
    }
    // A local release is meaningful only for the authoritative local avatar.
    // Validate this before freeing the session table so a stale entity cannot
    // release an otherwise valid possession.
    if q_avatar.get(cmd.target).is_err() {
        warn!(target = ?cmd.target, "[release] refused: target is not the local avatar");
        return;
    }
    let released = if matches!(*role, lunco_core_session::NetworkRole::Client) {
        Vec::new()
    } else {
        let change = lunco_core_session::release_control(&mut registry, local.0);
        let freed = change.released.clone();
        commands.trigger(change);
        if !freed.is_empty() {
            info!(
                "[auth] session {} released {} vessel(s)",
                local.0,
                freed.len()
            );
        }
        freed
    };
    // The stick goes back to the guidance law — publish it for the UI that
    // shows WHO is flying (the overlay's AUTO/MANUAL badge).
    if let Some(a) = authority.as_mut() {
        a.piloted = false;
    }
    // Orbital presentation state is global, but the exact return frame belongs
    // to the avatar and is restored below from `OrbitViewReturn`.
    if let Some(pin) = orbital_pin.as_mut() {
        if pin.active {
            pin.active = false;
        }
    }
    let avatar_ent = cmd.target;
    let (yaw, pitch, opt_vessel, is_surface, local_translation, return_state) = if let Ok((
        mut tf,
        mut cell,
        link,
        surface,
        child_of,
        return_state,
        current_orbit,
        mut orbit_history,
        orbit_user_input,
        cinematic_lock,
    )) =
        q_avatar.get_mut(avatar_ent)
    {
        let opt_vessel = link.map(|link| link.target);
        if cinematic_lock {
            if let Some(vessel_entity) = opt_vessel {
                let old_gid = q_owned.get(vessel_entity).ok().map(|gid| gid.get());
                if old_gid.is_none_or(|gid| !released.contains(&gid)) {
                    trigger_vessel_hard_stop(&mut commands, vessel_entity);
                }
            }
            commands.entity(avatar_ent).remove::<ControlLink>();
            return;
        }
        if orbit_user_input {
            if let (Some(camera), Some(history)) = (current_orbit, orbit_history.as_deref_mut()) {
                remember_user_orbit_pose(history, camera, &q_bodies);
            }
        }
        let return_state = return_state.cloned();
        if let Some(state) = &return_state {
            if child_of.parent() == state.parent_grid() {
                cell.set_if_neq(state.cell());
                tf.set_if_neq(state.transform());
            } else {
                migrate_to_grid(
                    &mut commands,
                    avatar_ent,
                    state.parent_grid(),
                    state.cell(),
                    state.transform(),
                );
            }
        }
        let rot = return_state
            .as_ref()
            .map(|state| state.transform().rotation)
            .unwrap_or(tf.rotation);
        let returning_surface = return_state
            .as_ref()
            .is_some_and(|state| matches!(state.behavior(), OrbitReturnBehavior::Surface(_)));
        let (y, p) = if surface.is_some() {
            let axes = surface_axes_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial);
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
                .map(|state| state.transform().translation)
                .unwrap_or(tf.translation),
            return_state,
        )
    } else {
        (0.0, 0.0, None, false, Vec3::ZERO, None)
    };

    // Hard stop the rover upon disengaging control: zero throttle/steer, full brake.
    if let Some(vessel_entity) = opt_vessel {
        let old_gid = q_owned.get(vessel_entity).ok().map(|gid| gid.get());
        if old_gid.is_none_or(|gid| !released.contains(&gid)) {
            trigger_vessel_hard_stop(&mut commands, vessel_entity);
        }
    }

    // Dropping the `ControlLink` stops `drive_from_bindings` (the target keeps
    // its own `ControlBinding` for the next possession).
    commands
        .entity(avatar_ent)
        .remove::<ControlLink>()
        .remove::<SpringArmCamera>()
        .remove::<OrbitCamera>()
        // Release is a mode transition. Clear both stepped camera modes before
        // installing the one selected below; otherwise a stale mode can
        // survive the transition and make both mode systems exclude each
        // other from their queries.
        .remove::<FreeFlightCamera>()
        .remove::<SurfaceCamera>()
        .remove::<OrbitViewReturn>()
        .remove::<RadialArrival>()
        .remove::<CurrentRegionArrival>()
        .remove::<OrbitUserInput>();

    if let Some(state) = return_state {
        let mut entity = commands.entity(avatar_ent);
        match state.behavior().clone() {
            OrbitReturnBehavior::SpringArm(_) => {
                let (yaw, pitch, _) = state.transform().rotation.to_euler(EulerRot::YXZ);
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
        if let Some(gravity_body) = state.gravity_body() {
            entity.try_insert(gravity_body);
        } else {
            entity.remove::<GravityBody>();
        }
        if state.surface_relative() {
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

fn resolve_requested_or_local_avatar(
    requested: Option<Entity>,
    local_avatar: Option<&lunco_avatar_core::roles::TheLocalAvatar>,
) -> Result<Entity, String> {
    match requested {
        Some(entity) => Ok(entity),
        None => local_avatar
            .and_then(|slot| slot.0)
            .ok_or_else(|| "no authoritative LocalAvatar is available".to_string()),
    }
}

fn local_avatar_state_error(requested: Option<Entity>) -> String {
    match requested {
        Some(entity) => {
            format!("requested avatar {entity:?} is not a complete local avatar")
        }
        None => "the authoritative LocalAvatar has no complete camera state".to_string(),
    }
}

fn controller_avatar_state_error(requested: Option<Entity>) -> String {
    match requested {
        Some(entity) => {
            format!("requested avatar {entity:?} has no local controller input state")
        }
        None => "the authoritative LocalAvatar has no local controller input state".to_string(),
    }
}

fn replace_avatar_diagnostic(
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    message: Option<String>,
) {
    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        diagnostics.replace_producer(
            "avatar-camera",
            message.map(|message| lunco_core::RuntimeDiagnostic {
                code: "avatar-camera".to_string(),
                severity: lunco_core::DiagnosticSeverity::Error,
                producer: "avatar-camera".to_string(),
                subject: "LocalAvatar".to_string(),
                message,
            }),
        );
    }
}

/// Possesses a vessel with an instant camera transition.
#[derive(bevy::ecs::system::SystemParam)]
struct PossessAvatarQueries<'w, 's> {
    camera: Query<
        'w,
        's,
        (
            Entity,
            &'static Transform,
            &'static ChildOf,
            Option<&'static ControlLink>,
            Has<lunco_camera_core::CameraPoseLock>,
        ),
        (With<Avatar>, With<LocalAvatar>),
    >,
    controller:
        Query<'w, 's, Option<&'static ControlLink>, (With<Avatar>, With<ActionState<UserIntent>>)>,
}

#[derive(bevy::ecs::system::SystemParam)]
struct PossessionAuthority<'w, 's> {
    role: Res<'w, lunco_core_session::NetworkRole>,
    guard: Res<'w, lunco_core_session::SyncApplyGuard>,
    registry: ResMut<'w, lunco_core_session::SessionRegistry>,
    rbac: Res<'w, lunco_core_session::SessionRbac>,
    session: Res<'w, lunco_core_session::LocalSession>,
    q_owned: Query<'w, 's, &'static lunco_core::GlobalEntityId>,
}

#[on_command(PossessVessel)]
fn on_possess_command(
    trigger: On<PossessVessel>,
    mut commands: Commands,
    possession_avatars: PossessAvatarQueries,
    // Camera readiness and control readiness are separate lifecycle facts. A
    // scene/perspective handoff can temporarily remove `LocalAvatar` (and its
    // camera components) while the avatar still owns its ActionState. The
    // controller link must be able to survive that handoff; otherwise a
    // possession command is accepted and authority is published, but semantic
    // intents have no consumer.
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    // Used ONLY for the heading-follow camera decision below. Possession is
    // gated by a non-avatar public input endpoint, then authority.
    q_vessel: Query<
        (
            Option<&lunco_camera_core::CameraFollow>,
            Option<&GravityBody>,
        ),
        Controllable,
    >,
    q_input_ports: Query<&lunco_port_core::InputPorts, Without<Avatar>>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    mut possession_authority: PossessionAuthority,
    mut authority: Option<ResMut<lunco_core::markers::FlightAuthority>>,
    local_avatar: Option<Res<lunco_avatar_core::roles::TheLocalAvatar>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let cmd = trigger.event();
    if !is_vessel_control_endpoint(cmd.target, &q_input_ports, &q_parents, &q_preview_only) {
        warn!(target = ?cmd.target, "[possess] refused: target exposes no writable input ports");
        return;
    }
    // A wire-applied possession records the remote session on the host but
    // never binds that session to this process's local camera or controller.
    if possession_authority.guard.is_from_sync() {
        if !matches!(
            *possession_authority.role,
            lunco_core_session::NetworkRole::Client
        ) {
            let _ =
                commit_possession_authority(&mut commands, &mut possession_authority, cmd.target);
        }
        return;
    }
    // A headless/direct-control possession deliberately has no camera owner.
    // Return before avatar resolution so `bind_camera = false` never performs
    // an implicit lookup.
    if !cmd.bind_camera {
        if let Some(requested) = cmd.avatar {
            let Some(previous) = possession_avatars.controller.get(requested).ok() else {
                replace_avatar_diagnostic(
                    &mut diagnostics,
                    Some(controller_avatar_state_error(Some(requested))),
                );
                return;
            };
            let Some(released) =
                commit_possession_authority(&mut commands, &mut possession_authority, cmd.target)
            else {
                return;
            };
            stop_previous_vessel(
                &mut commands,
                previous.map(|link| link.target),
                &released,
                &possession_authority.q_owned,
            );
            commands
                .entity(requested)
                .try_insert(ControlLink { target: cmd.target });
            if let Some(a) = authority.as_mut() {
                a.piloted = true;
            }
        } else if commit_possession_authority(&mut commands, &mut possession_authority, cmd.target)
            .is_some()
        {
            if let Some(a) = authority.as_mut() {
                a.piloted = true;
            }
        }
        return;
    }

    let avatar_ent = match resolve_requested_or_local_avatar(cmd.avatar, local_avatar.as_deref()) {
        Ok(entity) => entity,
        Err(message) => {
            warn!(target = ?cmd.target, "[possess] refused: {message}");
            replace_avatar_diagnostic(&mut diagnostics, Some(message));
            return;
        }
    };
    if !possession_avatars.controller.contains(avatar_ent) {
        let message = controller_avatar_state_error(cmd.avatar);
        warn!(target = ?cmd.target, "[possess] refused: {message}");
        replace_avatar_diagnostic(&mut diagnostics, Some(message));
        return;
    }
    let previous_vessel = possession_avatars
        .controller
        .get(avatar_ent)
        .ok()
        .flatten()
        .map(|link| link.target);

    let Ok((avatar_ent, cam_tf, _child_of, existing_link, cinematic_lock)) =
        possession_avatars.camera.get(avatar_ent)
    else {
        let message = local_avatar_state_error(cmd.avatar);
        warn!(target = ?cmd.target, "[possess] camera bind deferred: {message}");
        let Some(released) =
            commit_possession_authority(&mut commands, &mut possession_authority, cmd.target)
        else {
            return;
        };
        stop_previous_vessel(
            &mut commands,
            previous_vessel,
            &released,
            &possession_authority.q_owned,
        );
        commands
            .entity(avatar_ent)
            .try_insert(ControlLink { target: cmd.target });
        if let Some(a) = authority.as_mut() {
            a.piloted = true;
        }
        replace_avatar_diagnostic(&mut diagnostics, Some(message));
        return;
    };

    // A possession command may still establish control, but it cannot replace
    // the pose owner of a cinematic camera. The caller must explicitly release
    // the authored camera path before requesting an interactive camera bind.
    let already_bound = existing_link.is_some_and(|link| link.target == cmd.target);

    if already_bound || cinematic_lock {
        let Some(released) =
            commit_possession_authority(&mut commands, &mut possession_authority, cmd.target)
        else {
            return;
        };
        stop_previous_vessel(
            &mut commands,
            previous_vessel,
            &released,
            &possession_authority.q_owned,
        );
        commands
            .entity(avatar_ent)
            .try_insert(ControlLink { target: cmd.target });
        if let Some(a) = authority.as_mut() {
            a.piloted = true;
        }
        replace_avatar_diagnostic(&mut diagnostics, None);
        return;
    }

    let target_grid = get_grid_for_entity(cmd.target, &q_parents, &q_grids);
    let Some(target_grid_entity) = target_grid else {
        warn!(target = ?cmd.target, "[possess] refused: target has no live Grid frame");
        return;
    };
    let Some((target_local_pos, target_local_rotation)) = lunco_spatial::coords::grid_relative_pose(
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

    let Some(released) =
        commit_possession_authority(&mut commands, &mut possession_authority, cmd.target)
    else {
        return;
    };
    stop_previous_vessel(
        &mut commands,
        previous_vessel,
        &released,
        &possession_authority.q_owned,
    );
    if let Some(a) = authority.as_mut() {
        a.piloted = true;
    }
    commands
        .entity(avatar_ent)
        .try_insert(ControlLink { target: cmd.target });
    replace_avatar_diagnostic(&mut diagnostics, None);

    // Camera-follow mode is authored on the vessel's control profile
    // (`lunco_camera_core::CameraFollow`) — that, not any hardcoded marker, decides
    // whether the camera tracks the body's attitude. `Heading` follows yaw only
    // (surface vehicles); `Orbit` keeps a stable external frame a 6-DOF flyer
    // rotates inside of; `Chase` copies full orientation. A vessel with no
    // authored mode (or no control profile) defaults to `Heading`.
    use lunco_camera_core::CameraFollow;
    let follow = q_vessel
        .get(cmd.target)
        .ok()
        .and_then(|(follow, _)| follow.copied())
        .unwrap_or_default();

    // Per-mode framing. Orbit sits well out (whole vehicle in view); the
    // body-relative modes ride close behind.
    let (end_distance, end_vert_off) = match follow {
        CameraFollow::Orbit => (50.0, 0.0),
        CameraFollow::Chase => (25.0, 3.0),
        CameraFollow::Heading => (15.0, 2.0),
    };
    let end_yaw = 0.0;
    let end_pitch = -0.25;

    let surface_frame = q_vessel
        .get(cmd.target)
        .ok()
        .and_then(|(_, gravity)| gravity)
        .and_then(|gb| {
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

    // The control link goes on the **producer** (it carries the shared
    // `ActionState<UserIntent>` that `drive_from_bindings` reads); the intent→port
    // `ControlBinding` lives on the **vessel** as its own property, authored purely
    // from USD (a `Controls` child scope referencing a shared profile in
    // `control_profiles.usda`). There is NO Rust topology default: a vessel is
    // drivable iff its USD carries that scope. `drive_from_bindings` reads the
    // binding off the vessel and skips any vessel that has none, so possession is a
    // pure camera+link bind here.
    commands
        .entity(avatar_ent)
        .try_insert(ControlLink { target: cmd.target });

    // Detect if target is a surface vehicle (has GravityBody) and propagate surface mode.
    let is_surface_vehicle = q_vessel
        .get(cmd.target)
        .is_ok_and(|(_, gravity)| gravity.is_some());

    // One follow solver serves the vessel follow modes. The spring arm owns the
    // render-rate chase pose; stepped camera modes own their pose through the
    // interaction schedule and `InteractionEased`. They differ only in derived
    // attitude. `OrbitCamera` is NOT used here — it is the celestial orbital-view
    // solver; reusing it for a fast-flying vessel was the source of the old
    // frame-stale target sampling jitter. Strip the celestial orbit component in
    // case a prior focus left it on the avatar.
    use lunco_camera_core::CameraFollow as CF;
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
        if let Ok((_, Some(gb))) = q_vessel.get(cmd.target) {
            cmd_ent.try_insert(*gb);
        }
        cmd_ent.try_insert(SurfaceRelativeMode);
    } else {
        cmd_ent.remove::<SurfaceRelativeMode>();
    }

    commands
        .entity(avatar_ent)
        .remove::<FreeFlightCamera>()
        .remove::<SurfaceCamera>();
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
            Has<lunco_camera_core::CameraPoseLock>,
        ),
        (With<Avatar>, With<LocalAvatar>),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_vessel: Query<Entity, Controllable>,
    q_vessel_gravity: Query<&GravityBody>,
    local_avatar: Option<Res<lunco_avatar_core::roles::TheLocalAvatar>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let cmd = trigger.event();
    let avatar_ent = match resolve_requested_or_local_avatar(cmd.avatar, local_avatar.as_deref()) {
        Ok(entity) => entity,
        Err(message) => {
            warn!(target = ?cmd.target, "[follow] refused: {message}");
            replace_avatar_diagnostic(&mut diagnostics, Some(message));
            return;
        }
    };
    let Ok((avatar_ent, _child_of, existing_spring, cinematic_lock)) = q_avatar.get(avatar_ent)
    else {
        let message = local_avatar_state_error(cmd.avatar);
        warn!(target = ?cmd.target, "[follow] refused: {message}");
        replace_avatar_diagnostic(&mut diagnostics, Some(message));
        return;
    };
    replace_avatar_diagnostic(&mut diagnostics, None);

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
    let Some((target_local_pos, target_local_rotation)) = lunco_spatial::coords::grid_relative_pose(
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
        .remove::<ControlLink>()
        .remove::<FreeFlightCamera>()
        .remove::<SurfaceCamera>()
        .remove::<OrbitCamera>()
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
/// `AvatarCelestialCameraPlugin`, which runs at a fixed schedule point on frame-consistent
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
            Option<&OrbitViewHistory>,
            Has<OrbitUserInput>,
            Option<&OrbitViewReturn>,
            Option<&SpringArmCamera>,
            Option<&SurfaceCamera>,
            Option<&FreeFlightCamera>,
            Option<&GravityBody>,
            Has<SurfaceRelativeMode>,
            Has<lunco_camera_core::CameraPoseLock>,
        ),
        (With<Avatar>, With<LocalAvatar>),
    >,
    mut q_zoom: Query<&mut CameraZoomInput, (With<Avatar>, With<LocalAvatar>)>,
    q_bodies: Query<&CelestialBody>,
    q_body_decls: Query<&lunco_celestial_spatial::CelestialBodyDecl>,
    q_body_entities: Query<(Entity, &CelestialBody)>,
    q_sc: Query<&Spacecraft>,
    q_children: Query<&Children>,
    local_avatar: Option<Res<lunco_avatar_core::roles::TheLocalAvatar>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let cmd = trigger.event();
    let avatar_ent = match resolve_requested_or_local_avatar(cmd.avatar, local_avatar.as_deref()) {
        Ok(entity) => entity,
        Err(message) => {
            warn!(target = ?cmd.target, "[focus] refused: {message}");
            replace_avatar_diagnostic(&mut diagnostics, Some(message));
            return;
        }
    };
    let Ok((
        avatar_ent,
        cam_tf,
        cam_cell,
        cam_parent,
        _,
        current_orbit,
        orbit_history,
        orbit_user_input,
        return_state,
        spring_arm,
        surface_camera,
        freeflight_camera,
        gravity_body,
        surface_relative,
        cinematic_lock,
    )) = q_avatar.get(avatar_ent)
    else {
        let message = local_avatar_state_error(cmd.avatar);
        warn!(target = ?cmd.target, "[focus] refused: {message}");
        replace_avatar_diagnostic(&mut diagnostics, Some(message));
        return;
    };
    replace_avatar_diagnostic(&mut diagnostics, None);

    // Focus is an interactive camera-mode transition. A cinematic path owns
    // this entity's complete pose, so the command has no valid camera-side
    // effect while the lock is present.
    if cinematic_lock {
        return;
    }

    // Compute distance based on target type.
    let mut distance = 20.0;
    let physical_target = resolve_declared_body(cmd.target, &q_body_decls, &q_body_entities)
        .or_else(|| {
            lunco_spatial::find_descendant_or_self(cmd.target, &q_children, &q_body_entities)
        })
        .unwrap_or(cmd.target);
    let is_body = q_bodies.get(physical_target).is_ok();

    // Already orbiting this very body (clicking the focused globe, re-clicking
    // its view pill): a repeat focus must be a NO-OP. Re-running the swap
    // would discard the current interactive pose and restart its arrival.
    if let Some(orbit) = current_orbit {
        if resolve_declared_body(orbit.target, &q_body_decls, &q_body_entities)
            .or_else(|| {
                lunco_spatial::find_descendant_or_self(orbit.target, &q_children, &q_body_entities)
            })
            .unwrap_or(orbit.target)
            == physical_target
        {
            return;
        }
    }

    // Focus is also a camera-mode handoff. Any wheel delta accumulated before
    // the target switch must not be consumed by the newly focused orbit.
    if let Ok(mut zoom) = q_zoom.get_mut(avatar_ent) {
        zoom.begin_mode_transition(None);
    }
    if let Ok(body) = q_bodies.get(physical_target) {
        distance = body.radius_m * 3.0;
    } else if let Ok(sc) = q_sc.get(cmd.target) {
        distance = (sc.hit_radius_m as f64 * 5.0).max(100.0);
    }

    let (yaw, pitch, _) = cam_tf.rotation.to_euler(EulerRot::YXZ);
    let mut next_history = orbit_history.cloned();
    if orbit_user_input {
        if let Some(orbit) = current_orbit {
            let current_target =
                resolve_declared_body(orbit.target, &q_body_decls, &q_body_entities)
                    .or_else(|| {
                        lunco_spatial::find_descendant_or_self(
                            orbit.target,
                            &q_children,
                            &q_body_entities,
                        )
                    })
                    .unwrap_or(orbit.target);
            if let Ok(body) = q_bodies.get(current_target) {
                let history = next_history.get_or_insert_with(OrbitViewHistory::default);
                remember_orbit_pose_for_body(history, orbit, body.ephemeris_id);
            }
        }
    }
    let saved_pose = q_bodies.get(physical_target).ok().and_then(|body| {
        next_history
            .as_ref()
            .and_then(|history| history.pose(body.ephemeris_id))
    });

    let mut ent = commands.entity(avatar_ent);
    if let Some(history) = next_history {
        ent.try_insert(history);
    }
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
        ent.try_insert(OrbitViewReturn::new(
            cam_parent.parent(),
            *cam_cell,
            *cam_tf,
            behavior,
            gravity_body.copied(),
            surface_relative,
        ));
    }
    ent.remove::<SpringArmCamera>()
        .remove::<FreeFlightCamera>()
        // Surface state must go too: the generic surface-camera runtime runs
        // after the celestial orbit writer and would rebuild the rotation as a ground-level
        // tangent frame every frame — the camera orbits the target but looks
        // at the horizon (planet off-screen, view jitters as the arm eases).
        .remove::<SurfaceCamera>()
        .remove::<SurfaceRelativeMode>()
        .remove::<GravityBody>()
        .try_insert(OrbitCamera {
            target: physical_target,
            distance: saved_pose.map_or(distance, |pose| pose.distance()),
            yaw: saved_pose.map_or(yaw, |pose| pose.yaw()),
            pitch: saved_pose.map_or(pitch, |pose| pose.pitch()),
            damping: saved_pose.and_then(|pose| pose.damping()),
            vertical_offset: saved_pose.map_or(0.0, |pose| pose.vertical_offset()),
        });
    ent.remove::<OrbitUserInput>();
    if is_body && saved_pose.is_none() {
        ent.try_insert(CurrentRegionArrival);
    } else {
        ent.remove::<CurrentRegionArrival>();
    }
    info!(
        "FOCUS: avatar={avatar_ent:?} target={:?} (physical {physical_target:?}) body={is_body} distance={:.3e} restored={}",
        cmd.target,
        saved_pose.map_or(distance, |pose| pose.distance()),
        saved_pose.is_some(),
    );
}

/// Initializes avatar entities that lack a behavior component.
///
/// Inserts `FreeFlightCamera` as the default behavior with the entity's
/// current transform orientation.
///
/// `Without<CameraPoseLock>` is load-bearing, not hygiene: a path-driven
/// camera has no interactive mode, and this initializer must never create one
/// after the authored path has claimed pose ownership.
fn avatar_init_system(
    mut commands: Commands,
    q_avatar: Query<
        (
            Entity,
            &Transform,
            Option<&OrbitViewHistory>,
            Option<&InputMap<UserIntent>>,
            Option<&ActionState<UserIntent>>,
        ),
        (
            With<Avatar>,
            With<LocalAvatar>,
            With<Projection>,
            With<lunco_render::SceneCamera>,
            Without<SpringArmCamera>,
            Without<OrbitCamera>,
            Without<FreeFlightCamera>,
            // SurfaceCamera is a complete interactive mode, not an absent
            // behavior component. Without this guard init would reinsert
            // FreeFlightCamera over it on the next Update tick.
            Without<SurfaceCamera>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    q_proj: Query<
        Entity,
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<AdaptiveNearPlane>,
            With<Projection>,
            With<lunco_render::SceneCamera>,
        ),
    >,
    bindings: Res<InputBindingsSettings>,
) {
    for (entity, tf, history, input_map, action_state) in q_avatar.iter() {
        if history.is_none() {
            commands
                .entity(entity)
                .try_insert(OrbitViewHistory::default());
        }

        // An authored standard USD camera already owns its projection, camera
        // presentation profile, exposure, and initial look-at transform. The
        // avatar owner adds only the generic interactive movement substrate;
        // Rhai selects richer behavior through typed camera commands.
        let resolved_input_map = if input_map.is_none() {
            match bindings.input_map() {
                Ok(input_map) => Some(input_map),
                Err(error) => {
                    error!(
                        "avatar {entity:?} has invalid input bindings; refusing interactive initialization: {error}"
                    );
                    continue;
                }
            }
        } else {
            None
        };
        let mut avatar = commands.entity(entity);
        let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
        avatar.try_insert((
            AdaptiveNearPlane,
            IntentAnalogState::default(),
            FreeFlightCamera {
                yaw,
                pitch,
                damping: None,
            },
        ));
        if let Some(input_map) = resolved_input_map {
            avatar.try_insert(input_map);
        }
        if action_state.is_none() {
            avatar.try_insert(ActionState::<UserIntent>::default());
        }
    }
    for entity in q_proj.iter() {
        commands.entity(entity).try_insert(AdaptiveNearPlane);
    }
}

// ─── Surface Teleport Commands ───────────────────────────────────────────────

/// Teleports the avatar to a body's surface.
///
/// The camera is parented to the body's surface Grid, not to the Body entity.
/// That keeps the camera in the same body-fixed BigSpace branch as streamed
/// terrain while `SurfaceCamera` derives its orientation from the canonical
/// body-fixed ENU frame. BigSpace origin ownership remains with the persistent
/// OriginAnchor while this camera is migrated into the body-fixed Grid.
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
            Has<lunco_camera_core::CameraPoseLock>,
        ),
        (With<Avatar>, With<LocalAvatar>),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial_abs: Query<(Option<&CellCoord>, &Transform)>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_globe_lods: Query<&lunco_celestial_spatial::GlobeLod>,
    q_gravity_providers: Query<&GravityProvider>,
    mut field: ResMut<LocalGravityField>,
) {
    let cmd = trigger.event();
    let avatar_ent = cmd.target;

    let body_entity = cmd.body_entity;

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
            lunco_spatial::coords::common_grid_poses(
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
            lunco_spatial::coords::common_grid_poses(
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
        let Some(t) = [-b - root, -b + root].into_iter().find(|t| *t > 0.0) else {
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

        // Parent the camera to the same surface Grid as terrain and rover
        // content. The persistent OriginAnchor tracks the selected camera.
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
            .remove::<SpringArmCamera>();

        // Update LocalGravityField (world-space "up")
        field.body_entity = Some(body_entity);
        field.body_relative_position = surface_body_pos;
        field.local_up = surface_normal;
        field.surface_g = surface_g;
        let Some((_, body_world_rotation)) =
            lunco_spatial::coords::world_pose(body_entity, &q_parents, &q_grids, &q_spatial_abs)
                .ok()
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
/// Spatial placement is owned exclusively by `AvatarCelestialCameraPlugin`, which migrates
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
            Has<lunco_camera_core::CameraPoseLock>,
        ),
        (With<Avatar>, With<LocalAvatar>),
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
            With<LocalAvatar>,
            Without<OrbitCamera>,
            Without<lunco_camera_core::CameraPoseLock>,
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
    // surface mode and inserted a `FreeFlightCamera`. The generic free-flight
    // runtime has no `Without<OrbitCamera>` filter, so it then fought the orbit writer for the
    // Transform every frame — the camera drifted off the site and right-drag
    // flew the view away ("right click moved somewhere else"), while the two
    // writers alternating produced the residual per-frame wobble. An orbital
    // view owns the camera; leave it alone.
    let Some((avatar_ent, transform, child_of, maybe_gb, maybe_mode, maybe_sc, maybe_spring)) =
        q_avatar.single().ok()
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

fn resolve_declared_body(
    target: Entity,
    declarations: &Query<&lunco_celestial_spatial::CelestialBodyDecl>,
    bodies: &Query<(Entity, &CelestialBody)>,
) -> Option<Entity> {
    let decl = declarations.get(target).ok()?;
    bodies
        .iter()
        .find(|(_, body)| body.ephemeris_id == decl.naif)
        .map(|(entity, _)| entity)
}

#[on_command(UpdateProfile)]
fn on_update_profile(
    trigger: On<UpdateProfile>,
    guard: Res<lunco_core_session::SyncApplyGuard>,
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

#[on_command(ShowNotification)]
pub fn on_show_notification(trigger: On<ShowNotification>, mut notes: ResMut<ScreenNotifications>) {
    let cmd = trigger.event();
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
    use bevy::ecs::system::RunSystemOnce;
    use bevy::ecs::system::SystemState;

    #[test]
    fn scene_teardown_clears_claims_for_usd_prims_only() {
        let mut world = World::new();
        world.insert_resource(lunco_core_session::SessionRegistry::default());
        let scene_gid = 41;
        let persistent_gid = 42;
        world.spawn((
            lunco_core::GlobalEntityId::from_raw(scene_gid),
            UsdPrimPath::default(),
        ));
        world.spawn(lunco_core::GlobalEntityId::from_raw(persistent_gid));
        {
            let mut registry = world.resource_mut::<lunco_core_session::SessionRegistry>();
            registry
                .claim(lunco_core::SessionId::LOCAL, scene_gid)
                .unwrap();
            registry
                .claim(lunco_core::SessionId::LOCAL, persistent_gid)
                .unwrap();
        }

        world
            .run_system_once(clear_scene_possession_claims)
            .unwrap();

        let registry = world.resource::<lunco_core_session::SessionRegistry>();
        assert_eq!(registry.owner_of(scene_gid), None);
        assert_eq!(
            registry.owner_of(persistent_gid),
            Some(lunco_core::SessionId::LOCAL)
        );
    }

    #[test]
    fn focus_refuses_an_explicit_non_local_avatar_without_entity_order_fallback() {
        let mut app = App::new();
        app.init_resource::<lunco_core::RuntimeDiagnostics>()
            .add_observer(on_focus_command);

        let requested_avatar = app.world_mut().spawn(Avatar).id();
        let target = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(FocusTarget {
            avatar: Some(requested_avatar),
            target,
        });
        app.world_mut().flush();

        let diagnostics = app.world().resource::<lunco_core::RuntimeDiagnostics>();
        assert_eq!(diagnostics.findings.len(), 1);
        assert_eq!(diagnostics.findings[0].producer, "avatar-camera");
        assert!(diagnostics.findings[0].message.contains("requested avatar"));
        assert!(app.world().get::<OrbitCamera>(requested_avatar).is_none());
    }

    #[test]
    fn possession_binds_controller_during_camera_handoff() {
        let mut app = App::new();
        app.init_resource::<lunco_core_session::SyncApplyGuard>()
            .init_resource::<lunco_core_session::NetworkRole>()
            .init_resource::<lunco_core_session::SessionRegistry>()
            .init_resource::<lunco_core_session::SessionRbac>()
            .init_resource::<lunco_core_session::LocalSession>()
            .add_observer(on_possess_command);

        // During a scene/perspective handoff the camera-owned `LocalAvatar`
        // marker, Transform, and parent can be absent for one lifecycle tick.
        // The semantic controller source is still alive and must be enough for
        // an interactive possession to install its shared link before the
        // camera state is available.
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                ActionState::<lunco_control_core::UserIntent>::default(),
            ))
            .id();
        let rover = app
            .world_mut()
            .spawn(lunco_port_core::InputPorts::new(&["throttle"]))
            .id();

        app.world_mut().trigger(PossessVessel {
            avatar: Some(avatar),
            target: rover,
            bind_camera: true,
        });
        app.world_mut().flush();

        assert_eq!(
            app.world().get::<ControlLink>(avatar).unwrap().target,
            rover,
            "control binding must not wait for camera readiness"
        );
    }

    #[test]
    fn possession_validation_precedes_authority_claim() {
        let mut app = App::new();
        app.init_resource::<lunco_core_session::SyncApplyGuard>()
            .init_resource::<lunco_core_session::NetworkRole>()
            .init_resource::<lunco_core_session::SessionRegistry>()
            .init_resource::<lunco_core_session::SessionRbac>()
            .init_resource::<lunco_core_session::LocalSession>()
            .add_observer(on_possess_command);
        let target = app
            .world_mut()
            .spawn((
                lunco_port_core::InputPorts::new(&["throttle"]),
                lunco_core::GlobalEntityId::from_raw(0xA1),
            ))
            .id();

        // No local avatar exists. The endpoint is valid, but the camera-binding
        // request must fail before it can publish a registry claim.
        app.world_mut().trigger(PossessVessel {
            avatar: None,
            target,
            bind_camera: true,
        });
        app.world_mut().flush();

        assert_eq!(
            app.world()
                .resource::<lunco_core_session::SessionRegistry>()
                .owner_of(0xA1),
            None,
            "invalid local binding must not leave an authority claim"
        );
    }

    #[test]
    fn possession_handoff_keeps_one_claim_and_releases_the_old_target() {
        let mut app = App::new();
        app.init_resource::<lunco_core_session::SyncApplyGuard>()
            .init_resource::<lunco_core_session::NetworkRole>()
            .init_resource::<lunco_core_session::SessionRegistry>()
            .init_resource::<lunco_core_session::SessionRbac>()
            .init_resource::<lunco_core_session::LocalSession>()
            .add_observer(on_possess_command);
        let first = app
            .world_mut()
            .spawn((
                lunco_port_core::InputPorts::new(&["throttle"]),
                lunco_core::GlobalEntityId::from_raw(0xA1),
            ))
            .id();
        let second = app
            .world_mut()
            .spawn((
                lunco_port_core::InputPorts::new(&["throttle"]),
                lunco_core::GlobalEntityId::from_raw(0xB2),
            ))
            .id();

        for target in [first, second] {
            app.world_mut().trigger(PossessVessel {
                avatar: None,
                target,
                bind_camera: false,
            });
            app.world_mut().flush();
        }

        let registry = app
            .world()
            .resource::<lunco_core_session::SessionRegistry>();
        assert_eq!(registry.owner_of(0xA1), None);
        assert_eq!(
            registry.owner_of(0xB2),
            Some(lunco_core::SessionId::LOCAL),
            "handoff leaves the session owning only the selected target"
        );
    }

    #[test]
    fn wheel_click_resolves_to_owning_vehicle_command_root() {
        let mut world = World::new();
        let rover = world
            .spawn((
                lunco_port_core::InputPorts::new(&["drive"]),
                Name::new("Rover"),
            ))
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
            Query<&lunco_port_core::InputPorts, Without<Avatar>>,
            Query<
                (),
                Or<(
                    With<lunco_control_core::ControlBinding>,
                    With<lunco_core::MobilityRoot>,
                )>,
            >,
            Query<(), With<UsdPreviewOnly>>,
            Query<Entity, With<lunco_core::Ground>>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_input_ports, q_vehicle_roots, q_preview_only, q_ground) =
            state.get(&world).unwrap();

        assert_eq!(
            find_control_owner_from_hit(
                wheel_mesh,
                &q_parents,
                &q_input_ports,
                &q_vehicle_roots,
                &q_preview_only,
                &q_ground,
            ),
            Some(rover)
        );
    }

    #[test]
    fn vehicle_click_prefers_root_over_nested_input_endpoint() {
        let mut world = World::new();
        let rover = world
            .spawn((
                lunco_core::MobilityRoot,
                lunco_port_core::InputPorts::new(&["throttle"]),
            ))
            .id();
        let actuator = world
            .spawn((lunco_port_core::InputPorts::new(&["force"]), ChildOf(rover)))
            .id();
        let mesh = world.spawn(ChildOf(actuator)).id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&lunco_port_core::InputPorts, Without<Avatar>>,
            Query<
                (),
                Or<(
                    With<lunco_control_core::ControlBinding>,
                    With<lunco_core::MobilityRoot>,
                )>,
            >,
            Query<(), With<UsdPreviewOnly>>,
            Query<Entity, With<lunco_core::Ground>>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_input_ports, q_vehicle_roots, q_preview_only, q_ground) =
            state.get(&world).unwrap();

        assert_eq!(
            find_control_owner_from_hit(
                mesh,
                &q_parents,
                &q_input_ports,
                &q_vehicle_roots,
                &q_preview_only,
                &q_ground,
            ),
            Some(rover),
            "clicking a vehicle part must possess the vehicle root, not its nested endpoint"
        );
    }

    #[test]
    fn selectable_vehicle_root_remains_a_possession_target() {
        let mut world = World::new();
        let rover = world
            .spawn((
                lunco_core::SelectableRoot,
                lunco_port_core::InputPorts::new(&["drive"]),
            ))
            .id();
        let mesh = world.spawn(ChildOf(rover)).id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&lunco_port_core::InputPorts, Without<Avatar>>,
            Query<
                (),
                Or<(
                    With<lunco_control_core::ControlBinding>,
                    With<lunco_core::MobilityRoot>,
                )>,
            >,
            Query<(), With<UsdPreviewOnly>>,
            Query<Entity, With<lunco_core::Ground>>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_input_ports, q_vehicle_roots, q_preview_only, q_ground) =
            state.get(&world).unwrap();

        assert_eq!(
            find_control_owner_from_hit(
                mesh,
                &q_parents,
                &q_input_ports,
                &q_vehicle_roots,
                &q_preview_only,
                &q_ground,
            ),
            Some(rover)
        );
    }

    #[test]
    fn preview_selection_root_stays_out_of_possession() {
        let mut world = World::new();
        let preview = world
            .spawn((
                UsdPreviewOnly,
                lunco_core::SelectableRoot,
                lunco_port_core::InputPorts::new(&["drive"]),
            ))
            .id();
        let mesh = world.spawn(ChildOf(preview)).id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&lunco_port_core::InputPorts, Without<Avatar>>,
            Query<
                (),
                Or<(
                    With<lunco_control_core::ControlBinding>,
                    With<lunco_core::MobilityRoot>,
                )>,
            >,
            Query<(), With<UsdPreviewOnly>>,
            Query<Entity, With<lunco_core::Ground>>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_input_ports, q_vehicle_roots, q_preview_only, q_ground) =
            state.get(&world).unwrap();

        assert_eq!(
            find_control_owner_from_hit(
                mesh,
                &q_parents,
                &q_input_ports,
                &q_vehicle_roots,
                &q_preview_only,
                &q_ground,
            ),
            None
        );
    }

    #[test]
    fn avatar_endpoint_hit_resolves_to_vehicle_parent_not_avatar() {
        let mut world = World::new();
        let rover = world
            .spawn((
                lunco_port_core::InputPorts::new(&["drive"]),
                Name::new("Rover"),
            ))
            .id();
        let avatar = world
            .spawn((
                Avatar,
                lunco_port_core::InputPorts::new(&["forward"]),
                ChildOf(rover),
            ))
            .id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&lunco_port_core::InputPorts, Without<Avatar>>,
            Query<
                (),
                Or<(
                    With<lunco_control_core::ControlBinding>,
                    With<lunco_core::MobilityRoot>,
                )>,
            >,
            Query<(), With<UsdPreviewOnly>>,
            Query<Entity, With<lunco_core::Ground>>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_input_ports, q_vehicle_roots, q_preview_only, q_ground) =
            state.get(&world).unwrap();

        assert_eq!(
            find_control_owner_from_hit(
                avatar,
                &q_parents,
                &q_input_ports,
                &q_vehicle_roots,
                &q_preview_only,
                &q_ground,
            ),
            Some(rover)
        );
    }

    #[test]
    fn top_level_avatar_endpoint_is_not_a_possession_target() {
        let mut world = World::new();
        let avatar = world
            .spawn((Avatar, lunco_port_core::InputPorts::new(&["forward"])))
            .id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&lunco_port_core::InputPorts, Without<Avatar>>,
            Query<
                (),
                Or<(
                    With<lunco_control_core::ControlBinding>,
                    With<lunco_core::MobilityRoot>,
                )>,
            >,
            Query<(), With<UsdPreviewOnly>>,
            Query<Entity, With<lunco_core::Ground>>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_input_ports, q_vehicle_roots, q_preview_only, q_ground) =
            state.get(&world).unwrap();

        assert_eq!(
            find_control_owner_from_hit(
                avatar,
                &q_parents,
                &q_input_ports,
                &q_vehicle_roots,
                &q_preview_only,
                &q_ground,
            ),
            None
        );
    }

    #[test]
    fn preview_descendant_is_not_a_possession_target() {
        let mut world = World::new();
        let preview_root = world
            .spawn((UsdPreviewOnly, lunco_port_core::InputPorts::new(&["drive"])))
            .id();
        let preview_part = world.spawn(ChildOf(preview_root)).id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&lunco_port_core::InputPorts, Without<Avatar>>,
            Query<
                (),
                Or<(
                    With<lunco_control_core::ControlBinding>,
                    With<lunco_core::MobilityRoot>,
                )>,
            >,
            Query<(), With<UsdPreviewOnly>>,
            Query<Entity, With<lunco_core::Ground>>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_input_ports, q_vehicle_roots, q_preview_only, q_ground) =
            state.get(&world).unwrap();

        assert_eq!(
            find_control_owner_from_hit(
                preview_part,
                &q_parents,
                &q_input_ports,
                &q_vehicle_roots,
                &q_preview_only,
                &q_ground,
            ),
            None
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
    fn orbital_release_restores_pose_and_mode_in_one_transition() {
        let mut app = App::new();
        app.init_resource::<lunco_core_session::SyncApplyGuard>()
            .init_resource::<lunco_core_session::NetworkRole>()
            .init_resource::<lunco_core_session::LocalSession>()
            .init_resource::<lunco_core_session::SessionRegistry>()
            .init_resource::<LocalGravityField>()
            .add_observer(on_release_command);

        let root_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGrid,
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
            ))
            .id();
        let surface_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
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
        app.insert_resource(lunco_celestial_spatial::OrbitalViewPin {
            active: true,
            body: lunco_celestial::ephemeris_id::MOON,
            dir: DVec3::Z,
            distance: 5_000_000.0,
        });
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                LocalAvatar,
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
                OrbitViewReturn::new(
                    surface_grid,
                    return_cell,
                    return_transform,
                    OrbitReturnBehavior::Surface(return_surface.clone()),
                    Some(GravityBody { body_entity: body }),
                    true,
                ),
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
        assert!(
            !world
                .resource::<lunco_celestial_spatial::OrbitalViewPin>()
                .active
        );
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
        app.init_resource::<lunco_core_session::SyncApplyGuard>()
            .init_resource::<LocalGravityField>()
            .init_resource::<lunco_celestial_spatial::OrbitalViewPin>()
            .add_observer(on_focus_command)
            .add_observer(on_return_from_orbit);

        let surface_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
            ))
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
                LocalAvatar,
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
        assert_eq!(first_snapshot.parent_grid(), surface_grid);
        assert_eq!(first_snapshot.cell(), original_cell);
        assert!(matches!(
            first_snapshot.behavior(),
            OrbitReturnBehavior::Surface(_)
        ));
        assert!(
            app.world().get::<CurrentRegionArrival>(avatar).is_some(),
            "surface-to-body focus must resolve the current region instead of using a fixed arrival"
        );

        app.world_mut().trigger(FocusTarget {
            avatar: Some(avatar),
            target: earth,
        });
        app.world_mut().flush();
        let second_snapshot = app.world().get::<OrbitViewReturn>(avatar).unwrap();
        assert_eq!(second_snapshot.parent_grid(), first_snapshot.parent_grid());
        assert_eq!(second_snapshot.cell(), first_snapshot.cell());
        assert!(second_snapshot
            .transform()
            .translation
            .abs_diff_eq(first_snapshot.transform().translation, 1e-6));
        assert_eq!(
            app.world().get::<OrbitCamera>(avatar).unwrap().target,
            earth
        );
        assert!(
            app.world().get::<CurrentRegionArrival>(avatar).is_some(),
            "a body without saved user pose must resolve from the current region"
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
    fn user_orbit_pose_is_restored_per_body_after_switching_targets() {
        let mut app = App::new();
        register_orbit_history_hook(&mut app);
        app.add_observer(on_focus_command);

        let grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
            ))
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
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                LocalAvatar,
                CellCoord::ZERO,
                Transform::from_xyz(0.0, 0.0, 100.0),
                ChildOf(grid),
                OrbitCamera {
                    target: moon,
                    distance: 8_000.0,
                    yaw: 0.7,
                    pitch: -0.3,
                    damping: Some(0.2),
                    vertical_offset: 4.0,
                },
                OrbitViewHistory::default(),
                OrbitUserInput,
            ))
            .id();

        app.world_mut().trigger(FocusTarget {
            avatar: Some(avatar),
            target: earth,
        });
        app.world_mut().flush();

        assert_eq!(
            app.world().get::<OrbitCamera>(avatar).unwrap().target,
            earth,
            "body switch must replace the active orbit target"
        );

        let stored = app
            .world()
            .get::<OrbitViewHistory>(avatar)
            .expect("orbit history remains avatar-owned")
            .pose(lunco_celestial::ephemeris_id::MOON)
            .expect("leaving Moon orbit stores the user-controlled pose");
        assert_eq!(
            stored,
            OrbitPose::from_camera(&OrbitCamera {
                target: moon,
                distance: 8_000.0,
                yaw: 0.7,
                pitch: -0.3,
                damping: Some(0.2),
                vertical_offset: 4.0,
            })
            .unwrap()
        );

        app.world_mut().trigger(FocusTarget {
            avatar: Some(avatar),
            target: moon,
        });
        app.world_mut().flush();
        let restored = app.world().get::<OrbitCamera>(avatar).unwrap();
        assert_eq!(restored.target, moon);
        assert_eq!(restored.yaw, 0.7);
        assert_eq!(restored.pitch, -0.3);
        assert_eq!(restored.distance, 8_000.0);
        assert_eq!(restored.damping, Some(0.2));
        assert_eq!(restored.vertical_offset, 4.0);
        assert!(
            app.world().get::<CurrentRegionArrival>(avatar).is_none(),
            "a saved body pose must not be replaced by a new arrival"
        );
    }

    #[test]
    fn repeated_scroll_entry_restores_saved_body_pose_without_radial_arrival() {
        let body = CelestialBody {
            name: "Moon".into(),
            ephemeris_id: lunco_celestial::ephemeris_id::MOON,
            radius_m: 1_737_400.0,
        };
        let mut history = OrbitViewHistory::default();
        let saved = OrbitCamera {
            target: Entity::PLACEHOLDER,
            distance: 8_000.0,
            yaw: 0.7,
            pitch: -0.3,
            damping: Some(0.2),
            vertical_offset: 4.0,
        };
        remember_orbit_pose_for_body(&mut history, &saved, body.ephemeris_id);

        let (restored, needs_radial_arrival) =
            scroll_entry_orbit_camera(Entity::PLACEHOLDER, &body, body.radius_m, Some(&history));
        assert!(!needs_radial_arrival);
        assert_eq!(restored.target, saved.target);
        assert_eq!(restored.distance, saved.distance);
        assert_eq!(restored.yaw, saved.yaw);
        assert_eq!(restored.pitch, saved.pitch);
        assert_eq!(restored.damping, saved.damping);
        assert_eq!(restored.vertical_offset, saved.vertical_offset);

        let (first_entry, needs_radial_arrival) =
            scroll_entry_orbit_camera(Entity::PLACEHOLDER, &body, body.radius_m, None);
        assert!(needs_radial_arrival);
        assert_eq!(first_entry.distance, body.radius_m * 3.0);
        assert_eq!(first_entry.yaw, 0.0);
        assert_eq!(first_entry.pitch, 0.0);
    }

    #[test]
    fn direct_orbit_mode_removal_records_and_clears_user_pose() {
        let mut app = App::new();
        register_orbit_history_hook(&mut app);

        let body = app
            .world_mut()
            .spawn(CelestialBody {
                name: "Moon".into(),
                ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                radius_m: lunco_celestial::MOON_MEAN_RADIUS_M,
            })
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                LocalAvatar,
                OrbitCamera {
                    target: body,
                    distance: 12_000.0,
                    yaw: 1.1,
                    pitch: -0.25,
                    damping: Some(0.15),
                    vertical_offset: 7.0,
                },
                OrbitViewHistory::default(),
                OrbitUserInput,
            ))
            .id();

        app.world_mut().entity_mut(avatar).remove::<OrbitCamera>();
        app.world_mut().flush();

        let history = app.world().get::<OrbitViewHistory>(avatar).unwrap();
        assert_eq!(
            history.pose(lunco_celestial::ephemeris_id::MOON),
            Some(
                OrbitPose::from_camera(&OrbitCamera {
                    target: body,
                    distance: 12_000.0,
                    yaw: 1.1,
                    pitch: -0.25,
                    damping: Some(0.15),
                    vertical_offset: 7.0,
                })
                .unwrap()
            )
        );
        assert!(app.world().get::<OrbitUserInput>(avatar).is_none());
    }

    #[test]
    fn active_twin_close_clears_orbit_history_without_touching_inactive_close() {
        let mut app = App::new();
        app.add_observer(clear_orbit_view_history_on_twin_closed);
        let avatar = app
            .world_mut()
            .spawn((Avatar, LocalAvatar, OrbitViewHistory::default()))
            .id();

        app.world_mut().trigger(lunco_workspace::TwinClosed {
            twin: lunco_workspace::TwinId::new(1),
            root: std::path::PathBuf::from("/inactive"),
            was_active: false,
        });
        app.world_mut().flush();
        assert!(app.world().get::<OrbitViewHistory>(avatar).is_some());

        app.world_mut().trigger(lunco_workspace::TwinClosed {
            twin: lunco_workspace::TwinId::new(2),
            root: std::path::PathBuf::from("/active"),
            was_active: true,
        });
        app.world_mut().flush();
        assert!(app.world().get::<OrbitViewHistory>(avatar).is_none());
    }

    #[test]
    fn possessed_orbit_view_round_trip_preserves_control_and_spring_arm() {
        let mut app = App::new();
        app.init_resource::<lunco_core_session::SyncApplyGuard>()
            .init_resource::<LocalGravityField>()
            .init_resource::<lunco_celestial_spatial::OrbitalViewPin>()
            .add_observer(on_focus_command)
            .add_observer(on_return_from_orbit);

        let surface_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
            ))
            .id();
        let orbit_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
            ))
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
                LocalAvatar,
                Camera {
                    is_active: true,
                    ..default()
                },
                original_cell,
                original_transform,
                ChildOf(surface_grid),
                original_spring.clone(),
                ControlLink { target: rover },
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
            app.world()
                .get::<OrbitViewReturn>(avatar)
                .unwrap()
                .behavior(),
            OrbitReturnBehavior::SpringArm(_)
        ));
        assert_eq!(
            app.world().get::<ControlLink>(avatar).unwrap().target,
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
            world.get::<ControlLink>(avatar).unwrap().target,
            rover,
            "returning from a presentation view must preserve possession"
        );
        assert!(world.get::<OrbitCamera>(avatar).is_none());
        assert!(world.get::<OrbitViewReturn>(avatar).is_none());
        assert!(
            !world
                .resource::<lunco_celestial_spatial::OrbitalViewPin>()
                .active
        );
    }

    #[test]
    fn local_avatar_mounts_into_ready_site_grid() {
        let mut app = App::new();
        app.insert_resource(lunco_spatial::WorldGridConfig::default());

        let world_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                lunco_spatial::WorldGrid,
            ))
            .id();
        let site = app
            .world_mut()
            .spawn((
                lunco_celestial::SiteAnchor,
                lunco_celestial::GeodeticAnchor {
                    body: lunco_celestial::ephemeris_id::MOON,
                    geodetic: lunco_celestial::Geodetic::new(25.28, 307.60, 0.0),
                },
                CellCoord::ZERO,
                Transform::from_xyz(100.0, 2.0, -50.0),
                ChildOf(world_grid),
            ))
            .id();
        let body = app
            .world_mut()
            .spawn(lunco_celestial::CelestialBody {
                name: "Moon".into(),
                ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                radius_m: lunco_celestial::MOON_MEAN_RADIUS_M,
            })
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                LocalAvatar,
                CellCoord::ZERO,
                Transform::from_xyz(110.0, 4.0, -40.0),
                ChildOf(world_grid),
            ))
            .id();

        app.add_systems(PreUpdate, capture_site_camera_pose);
        app.add_systems(Update, bind_local_avatar_to_site_grid);
        app.update();

        assert!(app.world().get::<PendingSiteCameraPose>(avatar).is_some());
        app.world_mut()
            .entity_mut(site)
            .insert(lunco_spatial::WorldGridConfig::default().grid());
        app.update();

        assert_eq!(app.world().get::<ChildOf>(avatar).unwrap().parent(), site);
        assert_eq!(
            app.world().get::<GravityBody>(avatar).unwrap().body_entity,
            body
        );
        let avatar_transform = app.world().get::<Transform>(avatar).unwrap();
        assert!(avatar_transform
            .translation
            .abs_diff_eq(Vec3::new(10.0, 2.0, 10.0), 1e-5));
    }

    #[test]
    fn late_avatar_projection_is_captured_after_site_grid_creation() {
        let mut app = App::new();
        let world_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                lunco_spatial::WorldGrid,
            ))
            .id();
        let site = app
            .world_mut()
            .spawn((
                lunco_celestial::SiteAnchor,
                lunco_celestial::GeodeticAnchor {
                    body: lunco_celestial::ephemeris_id::MOON,
                    geodetic: lunco_celestial::Geodetic::new(25.28, 307.60, 0.0),
                },
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(world_grid),
            ))
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                LocalAvatar,
                CellCoord::ZERO,
                Transform::from_xyz(45.0, 22.0, 28.0),
                ChildOf(world_grid),
            ))
            .id();

        // The scene frame is already live, but USD projection has only just
        // produced the avatar. Its loader-grid pose is still the authored
        // site-local pose and must be captured before the binder reparents it.
        app.configure_sets(Update, AvatarSceneHandoffSet);
        app.add_systems(
            Update,
            capture_site_camera_pose
                .run_if(site_camera_capture_changed)
                .in_set(AvatarSceneHandoffSet),
        );
        app.update();

        let pending = app
            .world()
            .get::<PendingSiteCameraPose>(avatar)
            .expect("late local avatar must retain its loader-relative pose");
        assert_eq!(pending.site_root, site);
        assert_eq!(pending.position, DVec3::new(45.0, 22.0, 28.0));
        assert_eq!(pending.rotation, DQuat::IDENTITY);
    }

    #[test]
    fn late_avatar_projection_uses_the_mounted_site_frame() {
        let mut app = App::new();
        let world_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                lunco_spatial::WorldGrid,
            ))
            .id();
        let surface_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(world_grid),
            ))
            .id();
        let site = app
            .world_mut()
            .spawn((
                lunco_celestial::SiteAnchor,
                lunco_celestial::GeodeticAnchor {
                    body: lunco_celestial::ephemeris_id::MOON,
                    geodetic: lunco_celestial::Geodetic::new(25.28, 307.60, 0.0),
                },
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::from_xyz(100.0, 0.0, 0.0),
                ChildOf(surface_grid),
            ))
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                LocalAvatar,
                CellCoord::ZERO,
                Transform::from_xyz(110.0, 0.0, 0.0),
                ChildOf(world_grid),
            ))
            .id();

        app.configure_sets(Update, AvatarSceneHandoffSet);
        app.add_systems(
            Update,
            capture_site_camera_pose
                .run_if(site_camera_capture_changed)
                .in_set(AvatarSceneHandoffSet),
        );
        app.update();

        let pending = app
            .world()
            .get::<PendingSiteCameraPose>(avatar)
            .expect("late local avatar must be projected in site coordinates");
        assert_eq!(pending.site_root, site);
        assert_eq!(pending.position, DVec3::new(10.0, 0.0, 0.0));
        assert_eq!(pending.rotation, DQuat::IDENTITY);
    }

    #[test]
    fn deferred_avatar_projection_is_captured_at_the_handoff_boundary() {
        fn project_avatar_once(
            mut commands: Commands,
            q_world_grid: Query<Entity, With<lunco_spatial::WorldGrid>>,
            mut projected: Local<bool>,
        ) {
            if *projected {
                return;
            }
            *projected = true;
            let world_grid = q_world_grid.single().unwrap();
            commands.spawn((
                Avatar,
                LocalAvatar,
                CellCoord::ZERO,
                Transform::from_xyz(45.0, 22.0, 28.0),
                ChildOf(world_grid),
            ));
        }

        let mut app = App::new();
        app.configure_sets(Update, AvatarSceneHandoffSet);
        let world_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                lunco_spatial::WorldGrid,
            ))
            .id();
        app.world_mut().spawn((
            lunco_celestial::SiteAnchor,
            lunco_celestial::GeodeticAnchor {
                body: lunco_celestial::ephemeris_id::MOON,
                geodetic: lunco_celestial::Geodetic::new(25.28, 307.60, 0.0),
            },
            lunco_spatial::WorldGridConfig::default().grid(),
            CellCoord::ZERO,
            Transform::default(),
            ChildOf(world_grid),
        ));
        app.add_systems(
            Update,
            (
                project_avatar_once,
                capture_site_camera_pose.run_if(site_camera_capture_changed),
            )
                .chain()
                .in_set(AvatarSceneHandoffSet),
        );

        app.update();

        let position = {
            let world = app.world_mut();
            let mut query = world.query::<&PendingSiteCameraPose>();
            query.single(world).unwrap().position
        };
        assert_eq!(position, DVec3::new(45.0, 22.0, 28.0));
    }

    #[test]
    fn avatar_init_does_not_reinsert_freeflight_over_surface_camera() {
        let mut app = App::new();
        app.init_resource::<InputBindingsSettings>();
        app.add_systems(Update, avatar_init_system);

        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                LocalAvatar,
                Transform::default(),
                Projection::Perspective(PerspectiveProjection::default()),
                SurfaceCamera {
                    heading: 0.0,
                    pitch: -0.2,
                },
            ))
            .id();

        let camera_less_avatar = app
            .world_mut()
            .spawn((
                Avatar,
                LocalAvatar,
                Transform::default(),
                Projection::Perspective(PerspectiveProjection::default()),
            ))
            .id();

        app.update();

        assert!(app.world().get::<SurfaceCamera>(avatar).is_some());
        assert!(
            app.world().get::<FreeFlightCamera>(avatar).is_none(),
            "camera initialization must not create two mutually-exclusive modes"
        );
        assert!(
            app.world()
                .get::<FreeFlightCamera>(camera_less_avatar)
                .is_none(),
            "camera behavior requires an authored SceneCamera intent"
        );
        assert!(
            app.world()
                .get::<lunco_render::SceneCamera>(camera_less_avatar)
                .is_none(),
            "avatar initialization must not fabricate missing camera intent"
        );
        assert!(
            app.world()
                .get::<AdaptiveNearPlane>(camera_less_avatar)
                .is_none(),
            "camera precision policy requires authored SceneCamera intent"
        );
    }

    #[test]
    fn surface_transition_preserves_possessed_spring_arm_writer() {
        let mut app = App::new();
        register_orbit_history_hook(&mut app);
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
                LocalAvatar,
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
                LocalAvatar,
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
                LocalAvatar,
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
                LocalAvatar,
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
            .spawn((Avatar, LocalAvatar, lunco_camera_core::CameraPoseLock))
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
            "a pose lock must remove stale avatar interpolation history"
        );
    }

    /// **A retired avatar camera must leave the viewport candidate pool.**
    ///
    /// A host-created camera can outlive a scene load because it is not owned by a
    /// USD prim. When an incoming scene claims `LocalAvatar`, this observer adds
    /// the render-owned retirement marker to the former camera and removes its
    /// `SceneCamera` intent marker. The marker hook makes it inactive
    /// synchronously, while camera selection excludes it before the replacement's
    /// render components are attached.
    ///
    /// `Camera` must SURVIVE: stripping it from a live extracted window camera
    /// crashes the render app on the shadow cascade unwrap.
    #[test]
    fn demoted_avatar_camera_stops_being_a_viewport_candidate() {
        let mut app = App::new();
        app.init_resource::<lunco_avatar_core::roles::TheLocalAvatar>();
        app.add_observer(demote_former_avatar);

        let old = app
            .world_mut()
            .spawn((
                Camera::default(),
                lunco_render::scene_camera_look_with_profile(
                    None,
                    lunco_render::RenderingQuality::Balanced.profile(),
                ),
                lunco_avatar_core::roles::Avatar,
                LocalAvatar,
            ))
            .id();
        assert!(
            app.world().get::<lunco_render::SceneCamera>(old).is_some(),
            "precondition: the explicit host camera starts as a viewport candidate"
        );
        assert!(
            app.world().get::<Camera>(old).unwrap().is_active,
            "precondition: the old camera starts active"
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
                lunco_avatar_core::roles::Avatar,
                LocalAvatar,
            ))
            .id();
        app.update();

        assert_eq!(
            app.world()
                .resource::<lunco_avatar_core::roles::TheLocalAvatar>()
                .0,
            Some(new),
            "the incoming camera holds the avatar role"
        );
        assert!(
            app.world().get::<lunco_render::SceneCamera>(old).is_none(),
            "the retired camera must leave the viewport pool, or implicit selection \
             can put it back on screen — the two-camera bug"
        );
        assert!(
            !app.world().get::<Camera>(old).unwrap().is_active,
            "the retired camera must be inactive before its SceneCamera marker is removed"
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
            bevy::prelude::With<lunco_control_core::ControlBinding>,
            bevy::prelude::With<lunco_cosim_core::SimComponent>,
        )>>();
        let ents: Vec<Entity> = q.iter(world).collect();
        let ents: Vec<Entity> = ents
            .into_iter()
            .filter(|entity| !is_preview_only_entity(world, *entity))
            .collect();
        info!("[inspect] {} commandable vessel(s)", ents.len());
        for e in ents {
            let name = world
                .get::<Name>(e)
                .map(|n| n.as_str().to_string())
                .unwrap_or_default();
            let gid = world.get::<lunco_core::GlobalEntityId>(e).map(|g| g.get());
            let has_cmd = world.get::<lunco_port_core::InputPorts>(e).is_some();
            let has_sim = world.get::<lunco_cosim_core::SimComponent>(e).is_some();
            let has_sel = world.get::<lunco_core::SelectableRoot>(e).is_some();
            let binding = world.get::<lunco_control_core::ControlBinding>(e).map(|b| {
                let ports: Vec<&str> = b.ports().collect();
                (b.binds.len(), ports.join(","))
            });
            let owner = gid.and_then(|g| {
                world
                    .get_resource::<lunco_core_session::SessionRegistry>()
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
// LunCoAvatarPlugin::build(). Render-bound capture commands are registered by
// `lunco-capture` with the renderer that owns their implementation.
register_commands!(
    on_show_notification,
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
