//! Implementation of the local presentation embodiment and interaction surface.
//!
//! This crate defines the high-level avatar behavior around an [Embodiment]
//! entity, which handles control authority and scene interaction. A successful
//! camera-bound possession emits a typed camera transaction; subject binding,
//! follow, focus/return transactions, and interactive-camera initialization
//! are supplied by `lunco-avatar-camera`. Semantic input is projected by
//! `lunco-avatar-input`. The camera architecture uses
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
//! Camera transitions use explicit mode transactions: orbit entry stores the
//! exact return pose, while the camera realization retains each user-controlled
//! orbital pose by stable body identity. Follow/surface commands install one
//! authoritative mode and its frame through the camera contracts.

use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lunco_avatar_camera_core::{CurrentRegionArrival, OrbitUserInput, OrbitViewHistory};
use lunco_camera_core::FocusTarget;
use lunco_camera_core::{
    AdaptiveNearPlane, CameraUpdateSet, FreeFlightCamera, OrbitCamera, SpringArmCamera,
    SurfaceRelativeMode,
};
use lunco_celestial::{CelestialBody, Spacecraft};
use lunco_control_core::{
    AcquireControl, ControlLink, IntentAnalogState, IntentState, ReleaseControlSource, UserIntent,
};
use lunco_core::{on_command, register_commands};
use lunco_core_session::commands::UpdateProfile;
use lunco_core_session::{LocalSession, NetworkRole, SessionProfiles};
use lunco_embodiment_core::roles::{Embodiment, LocalEmbodiment};
use lunco_notifications_core::{ScreenNotifications, ShowNotification, Toast};
use lunco_settings::{AppSettingsExt, ProfileSettings};
use lunco_usd_bevy_scene::{is_preview_only, is_preview_only_entity, UsdPreviewOnly, UsdPrimPath};

// Render-bound screenshots and deterministic offline recording are owned by
// `lunco-capture`; this crate remains responsible for camera intent,
// possession, and interaction, without linking the render-world readback pipeline.

// ─── Behavior Components ─────────────────────────────────────────────────────

// ─── Plugin ──────────────────────────────────────────────────────────────────

/// Plugin for managing local avatar logic, input processing, and possession.
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
/// camera transaction event that composes it.
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
    q_avatar: Query<(Entity, &ControlLink), (With<Embodiment>, With<LocalEmbodiment>)>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    mut commands: Commands,
) {
    if !matches!(*role, lunco_core_session::NetworkRole::Client) {
        return;
    }
    for (avatar, link) in q_avatar.iter() {
        let Ok(gid) = q_gid.get(link.target) else {
            commands.trigger(ReleaseControlSource { source: avatar });
            continue;
        };
        if registry.owner_of(gid.get()) != Some(session.0) {
            commands.trigger(ReleaseControlSource { source: avatar });
        }
    }
}

impl Plugin for LunCoAvatarPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_embodiment_core::roles::EmbodimentCorePlugin>() {
            app.add_plugins(lunco_embodiment_core::roles::EmbodimentCorePlugin);
        }
        if !app.is_plugin_added::<lunco_input_core::InputBindingsPlugin>() {
            app.add_plugins(lunco_input_core::InputBindingsPlugin);
        }
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
        app.init_resource::<lunco_interaction_core::SpawnToolActive>();
        app.init_resource::<lunco_interaction_core::TerrainToolActive>();
        app.init_resource::<lunco_interaction_core::ArmedScriptTool>();
        app.init_resource::<lunco_interaction_core::SceneInteractionMode>();
        app.add_observer(avatar_raycast_possession);
        // Native avatar construction receives the resolved command policy;
        // composed USD avatars receive the same policy from their `Controls` scope.
        app.add_observer(demote_former_avatar);
        // Register all commands (generated by register_commands! macro at module scope)
        register_all_commands(app);

        // Possession / follow commands cross the wire (a client takes control of
        // the host's authoritative rover, then drives it), and the wire apply path
        // looks them up by reflected short type-path — so the type MUST be in the
        // registry. They used to be wired observer-by-hand + type-by-hand, and when
        // the second half was forgotten the host logged "unknown command type
        // 'AcquireControl'", never recorded the client's ownership, and rejected
        // every subsequent SetPorts as unauthorized (the "client rover won't move"
        // bug). `register_commands!` now does both halves in one step, so the two
        // can't drift apart again.
        app.register_type::<AdaptiveNearPlane>()
            .register_type::<SurfaceRelativeMode>();

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
            use lunco_telemetry_core::ScriptEventAppExt;
            app.project_events::<KeyboardInput, _>(|e| {
                e.state
                    .is_pressed()
                    .then(|| lunco_telemetry_core::TelemetryEvent {
                        name: format!("key:{:?}", e.key_code),
                        source: 0, // raw input — no emitting entity
                        severity: lunco_telemetry_core::Severity::Info,
                        data: lunco_telemetry_core::TelemetryValue::Bool(true),
                        timestamp: 0.0,
                    })
            });
        }

        app.add_systems(
            Update,
            (enforce_ownership, sync_profile, tick_notifications),
        );
        app.configure_sets(
            lunco_time::InteractionSchedule,
            // Between restore and record: start from the authoritative stepped pose
            // (never from the previous frame's render interpolation). This keeps the
            // locomotion writer's `pos += vel·dt` on the authoritative pose and lets the
            // step's final pose be snapshotted for the render-rate ease.
            CameraUpdateSet
                .after(lunco_time::InteractionRestoreSet)
                .after(lunco_control_core::InteractionControlSet)
                .before(lunco_time::InteractionRecordSet),
        );
        // Camera-mode transitions are registered by the camera realization;
        // this package retains only the avatar authority and interaction paths.
    }
}

/// Local avatars are command endpoints with an authored-equivalent
/// `ControlBinding` and `InputPorts` surface. The shared controller translates
/// intents into ports, and the flight realization consumes only those ports.
fn demote_former_avatar(trigger: On<Remove, LocalEmbodiment>, mut commands: Commands) {
    let entity = trigger.entity;
    // Retirement is a presentation contract consumed by the one viewport
    // reconciler. Do not write Camera::is_active here: avatar role lifecycle
    // and viewport activation are separate ownership boundaries.
    commands.entity(entity).try_remove::<(
        Embodiment,
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

// ─── Raycasting ──────────────────────────────────────────────────────────────

/// Resolves a picked vehicle part to its authored vehicle control root.
///
/// `SelectableRoot` is an editor boundary, and every independently simulated
/// wheel may carry it. [`lunco_port_core::InputPorts`] is the public interface:
/// its nonempty vocabulary is the input surface a session may own. A
/// [`lunco_control_core::ControlBinding`] or [`lunco_core::MobilityRoot`] identifies the
/// authored vehicle boundary, which takes precedence over nested component
/// endpoints. An [`Embodiment`] endpoint is excluded even when it carries its own
/// movement ports; walking past one to this owner makes a click on a vehicle
/// part possess the vehicle rather than the avatar.
fn find_control_owner_from_hit(
    mut entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_input_ports: &Query<&lunco_port_core::InputPorts, Without<Embodiment>>,
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
    q_input_ports: &Query<&lunco_port_core::InputPorts, Without<Embodiment>>,
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
/// | opened input-port surface   | `AcquireControl`  |
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
    mode: Res<'w, lunco_interaction_core::SceneInteractionMode>,
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
        (With<Embodiment>, With<LocalEmbodiment>),
    >,
    scene_interaction: SceneInteractionGate,
    drag_mode_active: Res<lunco_interaction_core::DragModeActive>,
    spawn_tool_active: Res<lunco_interaction_core::SpawnToolActive>,
    terrain_tool_active: Res<lunco_interaction_core::TerrainToolActive>,
    armed_script_tool: Res<lunco_interaction_core::ArmedScriptTool>,
    mut commands: Commands,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>,
    q_spacecraft: Query<(Entity, &GlobalTransform, &Spacecraft)>,
    q_input_ports: Query<&lunco_port_core::InputPorts, Without<Embodiment>>,
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
    // re-trigger `AcquireControl`/`FocusTarget` for every ancestor in the chain
    // (we must not gate this on a *mesh* hit being found, the earlier bug).
    click.propagate(false);

    // Shared egui-vs-scene guard + camera ray (replaces the old
    // `hit.position.is_none()` chrome check). Returns `None` on an egui-chrome
    // click; the ray drives the analytic hit-sphere tests (celestial bodies /
    // spacecraft, which have no pickable mesh) alongside the mesh pick.
    let Some(ray) = lunco_viewport_core::scene_click_ray(
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
            camera: Some(avatar_entity),
            target,
        });
    } else if let Some(target) = spacecraft_hit {
        commands.trigger(AcquireControl {
            source: Some(avatar_entity),
            target,
            bind_camera: true,
        });
    } else if let Some(target) = control_target {
        commands.trigger(AcquireControl {
            source: Some(avatar_entity),
            target,
            bind_camera: true,
        });
    }
}

// ─── Commands ────────────────────────────────────────────────────────────────

/// Releases possession of a vessel.
///
/// Keeps the camera at its current position — no jarring teleport.
/// Switches to `FreeFlightCamera` mode with the current orientation preserved.
#[on_command(ReleaseControlSource)]
fn on_release_command(
    trigger: On<ReleaseControlSource>,
    mut commands: Commands,
    q_avatar: Query<Option<&ControlLink>, (With<Embodiment>, With<LocalEmbodiment>)>,
    guard: Res<lunco_core_session::SyncApplyGuard>,
    q_owned: Query<&lunco_core::GlobalEntityId>,
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
    if q_avatar.get(cmd.source).is_err() {
        warn!(target = ?cmd.source, "[release] refused: source is not the local embodiment");
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
    let avatar_ent = cmd.source;
    let opt_vessel = q_avatar
        .get(avatar_ent)
        .ok()
        .flatten()
        .map(|link| link.target);

    // Hard stop the rover upon disengaging control: zero throttle/steer, full brake.
    if let Some(vessel_entity) = opt_vessel {
        let old_gid = q_owned.get(vessel_entity).ok().map(|gid| gid.get());
        if old_gid.is_none_or(|gid| !released.contains(&gid)) {
            trigger_vessel_hard_stop(&mut commands, vessel_entity);
        }
    }

    commands.entity(avatar_ent).remove::<ControlLink>();
    commands.trigger(lunco_camera_core::ClearCameraBinding { camera: avatar_ent });
    info!("Released control source {:?}", avatar_ent);
}

fn controller_avatar_state_error(requested: Option<Entity>) -> String {
    match requested {
        Some(entity) => {
            format!("requested avatar {entity:?} has no local controller input state")
        }
        None => "the authoritative LocalEmbodiment has no local controller input state".to_string(),
    }
}

/// Possesses a vessel with an instant camera transition.
#[derive(bevy::ecs::system::SystemParam)]
struct PossessAvatarQueries<'w, 's> {
    controller: Query<
        'w,
        's,
        Option<&'static ControlLink>,
        (With<Embodiment>, With<ActionState<UserIntent>>),
    >,
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

#[on_command(AcquireControl)]
fn on_possess_command(
    trigger: On<AcquireControl>,
    mut commands: Commands,
    possession_avatars: PossessAvatarQueries,
    q_parents: Query<&ChildOf>,
    q_input_ports: Query<&lunco_port_core::InputPorts, Without<Embodiment>>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    mut possession_authority: PossessionAuthority,
    mut authority: Option<ResMut<lunco_core::markers::FlightAuthority>>,
    local_avatar: Option<Res<lunco_embodiment_core::roles::TheLocalEmbodiment>>,
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
        if let Some(requested) = cmd.source {
            let Some(previous) = possession_avatars.controller.get(requested).ok() else {
                warn!(
                    target = ?requested,
                    "[possess] refused: {}",
                    controller_avatar_state_error(Some(requested))
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

    let avatar_ent = match lunco_embodiment_core::roles::resolve_requested_or_local(
        cmd.source,
        local_avatar.as_deref(),
    ) {
        Ok(entity) => entity,
        Err(message) => {
            warn!(target = ?cmd.target, "[possess] refused: {message}");
            return;
        }
    };
    if !possession_avatars.controller.contains(avatar_ent) {
        let message = controller_avatar_state_error(cmd.source);
        warn!(target = ?cmd.target, "[possess] refused: {message}");
        return;
    }
    let previous_vessel = possession_avatars
        .controller
        .get(avatar_ent)
        .ok()
        .flatten()
        .map(|link| link.target);

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
    if cmd.bind_camera {
        commands.trigger(lunco_camera_core::BindCameraTarget {
            camera: avatar_ent,
            target: cmd.target,
        });
    }
}

/// Initializes avatar entities that lack a behavior component.
///

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
                .claim(lunco_command_contracts::SessionId::LOCAL, scene_gid)
                .unwrap();
            registry
                .claim(lunco_command_contracts::SessionId::LOCAL, persistent_gid)
                .unwrap();
        }

        world
            .run_system_once(clear_scene_possession_claims)
            .unwrap();

        let registry = world.resource::<lunco_core_session::SessionRegistry>();
        assert_eq!(registry.owner_of(scene_gid), None);
        assert_eq!(
            registry.owner_of(persistent_gid),
            Some(lunco_command_contracts::SessionId::LOCAL)
        );
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

        // During a scene/perspective handoff the camera-owned `LocalEmbodiment`
        // marker, Transform, and parent can be absent for one lifecycle tick.
        // The semantic controller source is still alive and must be enough for
        // an interactive possession to install its shared link before the
        // camera state is available.
        let avatar = app
            .world_mut()
            .spawn((
                Embodiment,
                ActionState::<lunco_control_core::UserIntent>::default(),
            ))
            .id();
        let rover = app
            .world_mut()
            .spawn(lunco_port_core::InputPorts::new(&["throttle"]))
            .id();

        app.world_mut().trigger(AcquireControl {
            source: Some(avatar),
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
        app.world_mut().trigger(AcquireControl {
            source: None,
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
            app.world_mut().trigger(AcquireControl {
                source: None,
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
            Some(lunco_command_contracts::SessionId::LOCAL),
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
            Query<&lunco_port_core::InputPorts, Without<Embodiment>>,
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
            Query<&lunco_port_core::InputPorts, Without<Embodiment>>,
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
            Query<&lunco_port_core::InputPorts, Without<Embodiment>>,
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
            Query<&lunco_port_core::InputPorts, Without<Embodiment>>,
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
                Embodiment,
                lunco_port_core::InputPorts::new(&["forward"]),
                ChildOf(rover),
            ))
            .id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&lunco_port_core::InputPorts, Without<Embodiment>>,
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
            .spawn((Embodiment, lunco_port_core::InputPorts::new(&["forward"])))
            .id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&lunco_port_core::InputPorts, Without<Embodiment>>,
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
            Query<&lunco_port_core::InputPorts, Without<Embodiment>>,
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

    /// **A retired avatar camera must leave the viewport candidate pool.**
    ///
    /// A host-created camera can outlive a scene load because it is not owned by a
    /// USD prim. When an incoming scene claims `LocalEmbodiment`, this observer adds
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
        app.init_resource::<lunco_embodiment_core::roles::TheLocalEmbodiment>();
        app.add_observer(demote_former_avatar);

        let old = app
            .world_mut()
            .spawn((
                Camera::default(),
                lunco_render::scene_camera_look_with_profile(
                    None,
                    lunco_render::RenderingQuality::Balanced.profile(),
                ),
                lunco_embodiment_core::roles::Embodiment,
                LocalEmbodiment,
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
                lunco_embodiment_core::roles::Embodiment,
                LocalEmbodiment,
            ))
            .id();
        app.update();

        assert_eq!(
            app.world()
                .resource::<lunco_embodiment_core::roles::TheLocalEmbodiment>()
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
    on_possess_command,
    on_release_command,
    on_update_profile,
    on_inspect_vessels
);
