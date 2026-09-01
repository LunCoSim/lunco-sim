//! Place-waypoint intent + primary pointer action — drop a mission waypoint by
//! **authoring a USD prim**.
//!
//! (Design: `docs/architecture/waypoints-in-usd.md`.)
//!
//! A document-backed waypoint is an ordinary prim referencing
//! `vessels/markers/waypoint.usda`, and the vessel's BT.CPP mission
//! (the `info:sourceCode` of its `LunCoProgramAPI "Mission"` child) gains a `drive_to`
//! leaf that names it by path. Both edits go
//! through the one authoring funnel, [`ApplyUsdOp`] — so the waypoint is journaled,
//! undoable, persisted to `.usda`, and replicated exactly like every other prim, with
//! no new command verb.
//! Runtime-only rovers use the existing `AddRuntimeWaypoint` behavior command because
//! they have no document to author; that path shares the same route projection and
//! marker asset.
//!
//! Everything else about a waypoint is therefore already implemented, by code that
//! knows nothing about waypoints:
//!
//! - **Move it** — it is selectable, so the ordinary transform gizmo drags it, and
//!   `lunco_autopilot::usd_tree` recompiles the route when it moves.
//! - **Delete it** — the ordinary Delete key removes the prim.
//! - **Undo** — the document's typed inverse ops.
//! - **Inspect it** — its attributes are ordinary prim parameters.
//!
//! That is the whole point of putting it in USD: the feature mostly stops existing.

use bevy::math::DVec3;
use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_autopilot::usd_tree::{
    append_waypoint_leaf, catmull_rom_path, insert_waypoint_after, remove_waypoint_leaf,
    set_route_smooth, set_waypoint_dwell, BehaviorXml, ReachedWaypoints, TargetBindings,
};
use lunco_controller::{ControllerLink, SimulatedIntents};
use lunco_core::commands::SessionId;
use lunco_core::session::SessionRegistry;
use lunco_core::{
    Avatar, EguiFocus, GlobalEntityId, InputPorts, IntentState, LocalAvatar, SceneViewport,
    SpawnToolActive, TerrainToolActive, TheLocalAvatar, UserIntent,
};
use lunco_doc_bevy::DocumentRegistry;
use lunco_render::{PbrLook, SceneCamera, SurfaceAlpha};
use lunco_scene_commands::runtime_waypoint::runtime_waypoint_key;
use lunco_scene_commands::runtime_waypoint::RuntimeWaypointBinding;
use lunco_scene_commands::SelectedEntities;
use lunco_usd::commands::{ApplyUsdOp, ApplyUsdOps};
use lunco_usd::document::UsdDocument;
use lunco_usd::document::{
    waypoint_billboard_ops, LayerId, UsdOp, WAYPOINT_MARKER_ASSET, WAYPOINT_MISSION_PROGRAM,
    WAYPOINT_ROUTE_SCOPE,
};
use lunco_usd_bevy::{CanonicalStages, SdfPath, UsdPrimPath, UsdRead};

use super::authoring_paths::{join_prim, prim_exists};
use crate::surface_pick::{
    cursor_surface_hit, SurfacePickPolicy, EDITOR_PLACEMENT_RAY_MAX_DISTANCE,
};

fn report_waypoint_failure(commands: &mut Commands, message: impl Into<String>) {
    let message = message.into();
    warn!("[waypoint] {message}");
    lunco_core::trigger_error(commands, "waypoint-edit-failed", message);
}

/// Return the mounted scene root represented by a vessel prim path.
///
/// A waypoint must be authored below the vessel's actual composed root. The
/// prim path is the only identity that survives references and Twin
/// recomposition; a document-layer `defaultPrim` or `/` is not a valid
/// substitute for malformed runtime identity.
fn vessel_root_path(path: &str) -> Option<String> {
    path.split('/')
        .find(|component| !component.is_empty())
        .map(|component| format!("/{component}"))
}

/// Track context menu state for right-clicking waypoints.
#[derive(Resource, Default)]
pub struct WaypointContextMenuState {
    /// The authored waypoint MARKER prim entity.
    pub entity: Option<Entity>,
    pub position: Vec2,
    pub just_opened: bool,
    /// Dwell (seconds) edit buffer, seeded from the leg when the menu opens so the
    /// DragValue shows the authored value instead of snapping back each frame.
    pub dwell: f64,
}

/// What a pending "click the ground" placement will do to the route.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PlacementMode {
    /// Repoint the named leg at the clicked spot (Move).
    Move,
    /// Insert a NEW leg directly after the named one, at the clicked spot.
    InsertAfter,
}

/// A waypoint edit waiting on a ground click.
///
/// Move and insert are addressed by document + prim path, never by a vessel
/// entity. Append resolves the selected or possessed vessel when the click lands,
/// so a scene recompose cannot leave an armed tool holding a stale entity.
#[derive(Debug)]
pub enum PendingPlacement {
    /// An edit armed by a waypoint context menu.
    Route {
        /// The document that owns the marker and mission.
        doc: lunco_doc::DocumentId,
        /// The marker prim path whose leg is edited.
        coord_key: String,
        /// The edit operation.
        mode: PlacementMode,
    },
    /// Append a route member to the selected or possessed vessel.
    Append,
}

/// Armed "click the ground to place" mode. While `Some`, the next scene click is
/// consumed by [`on_scene_click_place_waypoint`] instead of possessing/selecting —
/// `sync_waypoint_tool_active` mirrors this into [`lunco_core::WaypointToolActive`],
/// which is what the possession/selection observers actually honour (every global
/// `Pointer<Click>` observer sees the same click; `propagate(false)` stops bubbling,
/// not siblings).
#[derive(Resource, Default)]
pub struct WaypointPlacement(pub Option<PendingPlacement>);

/// Identity of one pointer click as seen by the global picking observer.
///
/// Streamed terrain can be reported by both the analytic backend and a resident
/// mesh backend in the same frame. `bevy_picking` dispatches that one pointer
/// action to each eligible target, but waypoint authoring is one global
/// operation rather than a per-target operation. Deduplicate at this operation's
/// owner so every click path remains the normal `Pointer<Click>` path.
#[derive(Clone, Copy, Debug, PartialEq)]
struct WaypointClickFingerprint {
    frame_time: f64,
    pointer: bevy::picking::pointer::PointerId,
    position: Vec2,
    count: u8,
    duration: std::time::Duration,
}

fn duplicate_waypoint_click(
    last: &mut Option<WaypointClickFingerprint>,
    frame_time: f64,
    click: &Pointer<Click>,
) -> bool {
    let fingerprint = WaypointClickFingerprint {
        frame_time,
        pointer: click.pointer_id,
        position: click.pointer_location.position,
        count: click.count,
        duration: click.duration,
    };
    let duplicate = *last == Some(fingerprint);
    *last = Some(fingerprint);
    duplicate
}

#[derive(Resource, Default)]
pub(crate) struct WaypointClickDedup(Option<WaypointClickFingerprint>);

impl WaypointClickDedup {
    pub(crate) fn clear(&mut self) {
        self.0 = None;
    }
}

/// Arm route-member placement from the Spawn palette's semantic Waypoint entry.
/// The ground-click handler selects authored USD or runtime patrol ownership at
/// the same boundary used by Alt+LMB.
#[derive(Event)]
pub(crate) struct AppendWaypointPlacementRequested;

pub(crate) fn on_append_waypoint_placement_requested(
    _trigger: On<AppendWaypointPlacementRequested>,
    mut placement: ResMut<WaypointPlacement>,
) {
    placement.0 = Some(PendingPlacement::Append);
}

/// Mirror [`WaypointPlacement`] into the shared `WaypointToolActive` gate so the
/// avatar-possession and entity-selection observers stand down while a placement is
/// armed. Same pattern as the spawn and terrain tools.
pub fn sync_waypoint_tool_active(
    placement: Res<WaypointPlacement>,
    mut active: ResMut<lunco_core::WaypointToolActive>,
) {
    let want = placement.0.is_some();
    if active.0 != want {
        active.0 = want;
    }
}

/// Arm-mode affordance: a crosshair cursor while a placement is pending, and Esc to
/// cancel it. What the click will DO is explained by the menu buttons' hover tooltips
/// — once armed the crosshair alone carries it, so no text follows the cursor around.
///
/// The cursor goes through `ctx.set_cursor_icon` — egui is the single source of truth
/// and bevy_egui translates its output to the window's `CursorIcon` with its own
/// change detection, so this costs nothing per frame and never fights egui's own hover
/// cursors. (Writing `CursorIcon` on the window directly would mean re-asserting it
/// every frame to beat bevy_egui's write, dirtying the component forever.)
pub fn handle_waypoint_placement_mode(
    mut contexts: bevy_egui::EguiContexts,
    placement: Res<WaypointPlacement>,
) {
    if placement.0.is_none() {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
}

/// Back out of ANY in-flight waypoint edit: an armed Move/Insert placement, or the
/// open context menu.
///
/// A real command, so cancelling is one verb for every waypoint mode (not a special
/// case bolted onto Move) and is reachable from rhai/the API like anything else —
/// rather than each mode sniffing a raw key for itself.
#[Command]
pub struct CancelWaypointEdit {}

#[on_command(CancelWaypointEdit)]
fn on_cancel_waypoint_edit(
    _trigger: On<CancelWaypointEdit>,
    mut placement: ResMut<WaypointPlacement>,
    mut menu_state: ResMut<WaypointContextMenuState>,
    mut menu_open: ResMut<lunco_core::WaypointMenuOpen>,
) {
    if let Some(p) = placement.0.take() {
        match p {
            PendingPlacement::Route {
                mode, coord_key, ..
            } => info!("[waypoint] cancelled {:?} of '{}'", mode, coord_key),
            PendingPlacement::Append => info!("[waypoint] cancelled append placement"),
        }
    }
    menu_state.entity = None;
    menu_open.0 = false;
}

/// Route the `Cancel` INTENT to [`CancelWaypointEdit`].
///
/// Reads the intent, never the raw key — so Esc/Backspace come from the DATA keymap
/// (`assets/config/keybindings.json`) and a rebind just works, exactly like
/// `avatar_escape_possession` does for releasing possession.
///
/// Only fires when there is actually something to cancel. `Cancel` is layered
/// innermost-first: with a waypoint edit up it closes that (and
/// `avatar_escape_possession` stands down via the shared gates); with nothing up it
/// falls through to releasing possession as before.
pub fn cancel_waypoint_edit_on_intent(
    cancel: lunco_core::CancelIntent,
    placement: Res<WaypointPlacement>,
    menu_state: Res<WaypointContextMenuState>,
    mut commands: Commands,
) {
    if placement.0.is_none() && menu_state.entity.is_none() {
        return;
    }
    if cancel.just_pressed() {
        commands.trigger(CancelWaypointEdit {});
    }
}

/// Document resolution bundle for waypoint systems. Bundled into one [`SystemParam`]
/// to stay under Bevy's 16-argument system limit.
#[derive(bevy::ecs::system::SystemParam)]
pub struct WaypointDocContext<'w> {
    pub usd_registry: Res<'w, DocumentRegistry<UsdDocument>>,
    pub backed: Res<'w, lunco_usd::twin_projection::DocBackedTwinScenes>,
    pub asset_server: Res<'w, AssetServer>,
}

impl<'w> WaypointDocContext<'w> {
    pub fn resolve_document(
        &self,
        stage_handle: &Handle<lunco_usd_bevy::UsdStageAsset>,
    ) -> Option<lunco_doc::DocumentId> {
        lunco_usd::twin_projection::scene_document_for(
            &self.backed,
            &self.asset_server,
            stage_handle.id(),
        )
    }
}

/// Click-ray inputs bundled into one [`SystemParam`] so the observer stays under
/// Bevy's 16-argument limit.
#[derive(bevy::ecs::system::SystemParam)]
pub struct WaypointClickFrame<'w, 's> {
    pub viewport: Res<'w, SceneViewport>,
    dedup: ResMut<'w, WaypointClickDedup>,
    time: Res<'w, Time>,
    pub cameras: Query<
        'w,
        's,
        (
            &'static Camera,
            &'static GlobalTransform,
            &'static bevy::camera::RenderTarget,
        ),
        (With<Camera3d>, With<SceneCamera>),
    >,
    pub q_parents: Query<'w, 's, &'static ChildOf>,
    /// Terrain collider tiles are a streamed physics approximation of the DEM.
    /// They are useful for body contact, but they are not authoritative for
    /// waypoint authoring when the retained analytic surface covers the click.
    pub terrain_colliders: Query<
        'w,
        's,
        Entity,
        Or<(
            With<lunco_terrain_surface::ColliderTileOf>,
            With<lunco_terrain_surface::DemHeightField>,
        )>,
    >,
}

impl WaypointClickFrame<'_, '_> {
    fn is_duplicate(&mut self, click: &Pointer<Click>) -> bool {
        duplicate_waypoint_click(&mut self.dedup.0, self.time.elapsed_secs_f64(), click)
    }
}

/// Vessel-side queries bundled separately from the click-ray inputs so the
/// waypoint click observer stays within Bevy's system-parameter limit. The bundle also
/// makes the authored-document and runtime-only target surfaces explicit in one
/// place.
#[derive(bevy::ecs::system::SystemParam)]
pub struct WaypointVesselQueries<'w, 's> {
    pub q_prim: Query<'w, 's, &'static UsdPrimPath>,
    pub q_xml: Query<'w, 's, (Entity, &'static BehaviorXml)>,
    // The free Avatar also has an InputPorts surface for flight. It is not a
    // mission vessel, so never choose it as the implicit waypoint target when
    // no vessel is possessed/selected.
    pub q_inputs: Query<'w, 's, Entity, (With<InputPorts>, Without<Avatar>)>,
}

/// Resolve the pointer to a point on the ground in **WORLD** (grid-absolute) space —
/// the one spelling of "where did the user click?" for the waypoint editor, shared by
/// the intent-driven waypoint drop and the Move / Insert-after placement click.
///
/// Casts through the active camera against BOTH the DEM oracle (ground truth over open
/// terrain, where the band-limited collider ring rounds a crater bowl) and the physics
/// colliders (structures/props), taking the nearer hit. The shared surface resolver
/// owns the frame conversion and precedence policy used by spawn as well.
fn pick_ground_world(
    frame: &WaypointClickFrame,
    surface: &lunco_terrain_surface::GridSurfaceQuery,
    raycaster: &lunco_physics::GridSpatialQuery,
    egui_focus: &EguiFocus,
    pointer: Vec2,
) -> Option<DVec3> {
    let camera_entity = frame.viewport.active_camera?;
    let (camera, cam_gtf, target) = frame.cameras.get(camera_entity).ok()?;
    if !camera.is_active || !matches!(target, bevy::camera::RenderTarget::Window(_)) {
        return None;
    }
    let ray = lunco_core::scene_click_ray(egui_focus, camera, cam_gtf, pointer)?;
    // ONE frame crossing, at the ray. Everything after this is grid-absolute —
    // the frame a waypoint prim is authored in. Converting the HIT instead of the
    // ORIGIN (as this used to) means the analytic surface was marched in the
    // render frame, so at an elevated site the oracle never answered and only
    // physics colliders — the ring around the rover — could place a waypoint.
    let (origin, direction) = surface.ray_to_grid(
        lunco_core::coords::RenderPos(ray.origin.as_dvec3()),
        ray.direction,
    )?;
    cursor_surface_hit(
        surface,
        raycaster,
        origin,
        direction,
        EDITOR_PLACEMENT_RAY_MAX_DISTANCE,
        SurfacePickPolicy::Nearest,
        |entity| !frame.terrain_colliders.contains(entity),
    )
    .map(|hit| hit.point.0)
}

/// Global `Pointer<Click>` observer: the `PlaceWaypoint` input intent paired with
/// the primary pointer action drops a waypoint prim for the selected vessel and
/// appends the matching `drive_to` leaf to its mission. The Alt binding lives in
/// `assets/config/keybindings.json`; this handler never inspects a raw key.
///
/// Stands down when the spawn / terrain-sculpt tool is armed, and when egui owns the
/// pointer (the authoritative gate). The semantic waypoint intent is excluded from
/// the possession observer, so this click does not also possess or follow the hit.
pub fn on_scene_click_waypoint(
    mut click: On<Pointer<Click>>,
    egui_focus: Res<EguiFocus>,
    spawn_tool: Res<SpawnToolActive>,
    terrain_tool: Res<TerrainToolActive>,
    selected: Res<SelectedEntities>,
    local_avatar: Res<TheLocalAvatar>,
    avatars: Query<(Entity, &IntentState), (With<Avatar>, With<LocalAvatar>)>,
    simulated_intents: Option<Res<SimulatedIntents>>,
    q_link: Query<&ControllerLink>,
    mut placement: ResMut<WaypointPlacement>,
    mut frame: WaypointClickFrame,
    vessels: WaypointVesselQueries,
    surface: lunco_terrain_surface::GridSurfaceQuery,
    raycaster: lunco_physics::GridSpatialQuery,
    doc_ctx: WaypointDocContext,
    canonical: NonSend<CanonicalStages>,

    mut commands: Commands,
) {
    if egui_focus.wants_pointer {
        return;
    }
    if spawn_tool.0 || terrain_tool.0 {
        return;
    }
    if click.button != PointerButton::Primary {
        return;
    }
    if frame.is_duplicate(&click) {
        return;
    }
    let append_armed = matches!(placement.0.as_ref(), Some(PendingPlacement::Append));
    // Move / Insert-after owns the next click. Do not let an incidental Alt key
    // (or another pointer hit for the same click) create a competing append.
    if placement.0.is_some() && !append_armed {
        return;
    }
    // A plain primary click possesses/selects; the semantic PlaceWaypoint intent
    // paired with it drops a waypoint. The Spawn palette's Append mode uses the
    // same authoring path without requiring a keyboard modifier.
    let place_waypoint_held = local_avatar
        .0
        .and_then(|entity| avatars.get(entity).ok())
        .is_some_and(|(avatar, intents)| {
            intents.pressed(&UserIntent::PlaceWaypoint)
                || simulated_intents.as_ref().is_some_and(|sim| {
                    sim.0
                        .get(&avatar)
                        .is_some_and(|set| set.contains(&UserIntent::PlaceWaypoint))
                })
        });
    if !append_armed && !place_waypoint_held {
        return;
    }

    // Now that this is a waypoint-intent click, stop propagation.
    click.propagate(false);

    // The local control link is authoritative when it exists; otherwise the
    // explicit scene selection is the only valid authoring target. There is no
    // entity-order choice for a waypoint operation.
    let possessed_vessel = local_avatar
        .0
        .and_then(|avatar| q_link.get(avatar).ok().map(|link| link.vessel_entity));
    let raw_vessel = possessed_vessel.or_else(|| selected.primary());
    let Some(mut vessel) = raw_vessel else {
        report_waypoint_failure(
            &mut commands,
            "PlaceWaypoint requires a possessed or explicitly selected vessel",
        );
        return;
    };

    // If selected entity is a sub-part, climb parents to find the vessel's
    // command surface and USD root. A mission is not required here: a spawned
    // asset may be runtime-only and still has a fully valid InputPorts surface.
    if vessels.q_prim.get(vessel).is_err() || vessels.q_inputs.get(vessel).is_err() {
        let mut curr = vessel;
        for _ in 0..16 {
            if vessels.q_prim.get(curr).is_ok() && vessels.q_inputs.get(curr).is_ok() {
                vessel = curr;
                break;
            }
            if let Ok(parent) = frame.q_parents.get(curr) {
                curr = parent.parent();
            } else {
                break;
            }
        }
    }

    let Ok(vessel_prim) = vessels.q_prim.get(vessel) else {
        report_waypoint_failure(
            &mut commands,
            format!("Target vessel {vessel:?} is not a USD prim; its mission cannot be authored"),
        );
        return;
    };

    // ── Find the document that OWNS this vessel ──────────────────────────────
    let Some(hit) = pick_ground_world(
        &frame,
        &surface,
        &raycaster,
        &egui_focus,
        click.pointer_location.position,
    ) else {
        info!("[waypoint] click ignored: no ray / no ground under the cursor");
        return;
    };

    if append_armed {
        placement.0.take();
    }

    info!("[waypoint] dropping waypoint at {:?}", hit);

    // A runtime-spawned asset is not backed by an authored scene document. Route
    // it through the live behaviour-spec seam; never guess the active document.
    let doc = doc_ctx.resolve_document(&vessel_prim.stage_handle);
    let host = doc.and_then(|id| doc_ctx.usd_registry.host(id));
    if host.is_none() {
        commands.trigger(lunco_scene_commands::runtime_waypoint::AddRuntimeWaypoint {
            target: vessel,
            position: hit.to_array(),
        });
        info!(
            "[waypoint] runtime-only rover {:?} received a live waypoint command; no authored USD document was modified",
            vessel
        );
        return;
    }
    let doc = doc.expect("host implies an owning document");
    let host = host.expect("host checked above");

    // ── Where the pin goes ────────────────────────────────────────────────────
    // The root comes from the vessel's OWN prim path: the first path component
    // is the scene's default prim (e.g. "/Traverse" for traverse.usda). This is
    // more robust than reading defaultPrim from the document layer, which may
    // differ when the vessel is composed from a referenced twin scene.
    let Some(root) = vessel_root_path(&vessel_prim.path) else {
        report_waypoint_failure(
            &mut commands,
            format!(
                "Vessel prim path {:?} has no mounted scene root",
                vessel_prim.path
            ),
        );
        return;
    };
    // ── The MARKER is an authored prim ────────────────────────────────────────
    // Not a Rust-built sphere: `vessels/markers/waypoint.usda` already defines
    // the dome, its livery and its arrival trigger zone. Referencing it means
    // one marker implementation for scene-authored and click-dropped waypoints
    // alike — the two used to be different objects that only looked alike, and
    // the Rust one drew itself in the vessel's hull colour.
    let (marker_path, mut ops) = author_marker_ops(host, &root, hit, &canonical, vessel_prim);

    // ── The mission's topology ────────────────────────────────────────────────
    // Append the leaf FIRST: if the tree is a shape the editor must not restructure,
    // bail out. The leaf targets the marker PRIM, so the mission and the map
    // refer to the same object — a coordinate string could drift from the pin.
    let current = vessels.q_xml.get(vessel).ok().map(|(_, x)| x.0.as_str());
    let xml = match append_waypoint_leaf(current, &marker_path) {
        Ok(xml) => xml,
        Err(err) => {
            report_waypoint_failure(&mut commands, format!("Could not add waypoint: {err}"));
            return;
        }
    };

    // ── Author one coherent USD intent ──────────────────────────────────────
    // The live projector owns ECS components. The editor only submits the complete
    // authored change set, so it cannot briefly install a half-built mission or
    // overwrite the composed BehaviorXml with a second, out-of-band value.
    let mission = join_prim(&vessel_prim.path, WAYPOINT_MISSION_PROGRAM);
    let mission_exists = canonical
        .get(vessel_prim.stage_handle.id())
        .zip(SdfPath::new(&mission).ok())
        .is_some_and(|(stage, mission)| stage.view().has_prim(&mission));
    let (mission, mission_ops) =
        ensure_mission_program_ops(host, &vessel_prim.path, mission_exists);
    ops.extend(mission_ops);
    ops.push(UsdOp::SetAttribute {
        edit_target: LayerId::root(),
        path: mission.clone(),
        name: "info:sourceCode".to_string(),
        type_name: "string".to_string(),
        value: xml,
    });
    info!(
        "[waypoint] writing to doc {:?}, mission prim {:?}",
        doc, mission
    );
    commands.trigger(ApplyUsdOps {
        doc,
        parent_gen: None,
        label: "Create waypoint mission edit".to_string(),
        ops,
    });
}

/// Global `Pointer<Click>` observer: right-click a waypoint sphere to open its menu.
///
/// Targets the authored marker PRIM — the pick lands on its `Dome` mesh, so walk
/// up to whichever ancestor is a waypoint of some mission.
pub fn on_scene_right_click_waypoint(
    mut click: On<Pointer<Click>>,
    egui_focus: Res<EguiFocus>,
    q_prim: Query<&UsdPrimPath>,
    q_vessels: Query<(Entity, &BehaviorXml, Option<&ReachedWaypoints>)>,
    q_parents: Query<&ChildOf>,
    q_xml: Query<&BehaviorXml>,
    mut menu_state: ResMut<WaypointContextMenuState>,
) {
    if egui_focus.wants_pointer {
        return;
    }
    if click.button != PointerButton::Secondary {
        return;
    }

    let mut entity = click.entity;
    // 16 levels, the same cap the rest of the editor walks with: the pick lands on a
    // leaf mesh of the referenced marker asset, whose depth is authored content.
    for _ in 0..16 {
        if let Some((vessel, target, _, _)) = resolve_marker(entity, &q_prim, &q_vessels) {
            click.propagate(false);
            menu_state.entity = Some(entity);
            menu_state.position = click.pointer_location.position;
            menu_state.just_opened = true;
            // Seed the dwell buffer from the authored leg.
            menu_state.dwell = q_xml
                .get(vessel)
                .ok()
                .and_then(|x| lunco_autopilot::usd_tree::waypoint_dwell(&x.0, &target))
                .unwrap_or(0.0);
            return;
        }
        let Ok(parent) = q_parents.get(entity) else {
            break;
        };
        entity = parent.parent();
    }
    // Right-clicking something that is not a waypoint is ordinary, but a right-click
    // on a waypoint that resolves to no route is the failure mode worth naming: the
    // menu simply never appears, which reads as "right-click does nothing".
    if let Ok(prim) = q_prim.get(click.entity) {
        debug!(
            "[waypoint] right-click on '{}' is not a waypoint of any current mission",
            prim.path
        );
    }
}

/// Global `Pointer<Click>` observer: consume the next scene click to place a waypoint
/// when a Move / Insert-after is armed from the context menu.
///
/// The possession and selection observers stand down via `WaypointToolActive` (see
/// [`WaypointPlacement`]), so this click only moves the waypoint.
pub fn on_scene_click_place_waypoint(
    mut click: On<Pointer<Click>>,
    egui_focus: Res<EguiFocus>,
    mut placement: ResMut<WaypointPlacement>,
    frame: WaypointClickFrame,
    surface: lunco_terrain_surface::GridSurfaceQuery,
    raycaster: lunco_physics::GridSpatialQuery,
    q_vessel: Query<(Entity, &BehaviorXml, &UsdPrimPath)>,
    doc_ctx: WaypointDocContext,
    canonical: NonSend<CanonicalStages>,
    mut commands: Commands,
) {
    if click.button != PointerButton::Primary
        || !matches!(placement.0.as_ref(), Some(PendingPlacement::Route { .. }))
    {
        return;
    }
    if egui_focus.wants_pointer {
        info!("[waypoint] placement: ignoring click, egui owns the pointer (menu?)");
        return; // clicking the menu itself, not the ground
    }
    click.propagate(false);
    let Some(pending @ PendingPlacement::Route { .. }) = placement.0.take() else {
        unreachable!("route placement was checked immediately before consumption");
    };
    let PendingPlacement::Route {
        doc,
        coord_key,
        mode,
    } = pending
    else {
        unreachable!("append placement is handled by on_scene_click_waypoint");
    };
    info!(
        "[waypoint] placement: consuming click for {:?} of '{}'",
        mode, coord_key
    );

    let Some(world) = pick_ground_world(
        &frame,
        &surface,
        &raycaster,
        &egui_focus,
        click.pointer_location.position,
    ) else {
        info!("[waypoint] placement cancelled: no ground under the cursor");
        return;
    };
    // MOVE repositions the MARKER, and touches the mission not at all: the leg
    // targets the prim by path, and the prim's pose is the prim's business. The
    // coordinate-in-the-XML spelling had to rewrite the whole mission to drag a
    // pin, which is why a move could reorder or lose a leg.
    //
    // It therefore needs NOTHING but the marker path and its document — no vessel
    // lookup, so a stage recomposition between arming the move and clicking the
    // ground (which respawns the vessel entity) cannot strand it.
    if mode == PlacementMode::Move {
        info!("[waypoint] Move → {} to {:?}", coord_key, world);
        commands.trigger(ApplyUsdOp {
            doc,
            parent_gen: None,
            op: UsdOp::SetTranslate {
                edit_target: LayerId::root(),
                path: coord_key,
                value: [world.x, world.y, world.z],
            },
        });
        return;
    }

    // INSERT-AFTER edits the mission, so it does need the vessel — resolved HERE,
    // from the route that names this waypoint, rather than from an entity captured
    // when the menu was open.
    let Some((_vessel, xml, vessel_prim)) = vessel_for_target(&q_vessel, &coord_key) else {
        report_waypoint_failure(
            &mut commands,
            format!(
                "No unique vessel mission refers to '{}'; waypoint insertion was refused",
                coord_key
            ),
        );
        return;
    };
    let Some(root) = vessel_root_path(&vessel_prim.path) else {
        report_waypoint_failure(
            &mut commands,
            format!(
                "Vessel prim path {:?} has no mounted scene root",
                vessel_prim.path
            ),
        );
        return;
    };
    let Some(host) = doc_ctx.usd_registry.host(doc) else {
        report_waypoint_failure(
            &mut commands,
            format!("No USD authoring host exists for document {doc:?}"),
        );
        return;
    };
    let (new_target, mut ops) = author_marker_ops(host, &root, world, &canonical, vessel_prim);
    let edited = insert_waypoint_after(&xml.0, &coord_key, &new_target);
    match edited {
        Ok(new_xml) => {
            info!("[waypoint] {:?} → {}", mode, new_target);
            ops.push(UsdOp::SetAttribute {
                edit_target: LayerId::root(),
                // Editing an EXISTING tree, so the program prim is already there —
                // the XML above was read back off it.
                path: join_prim(&vessel_prim.path, WAYPOINT_MISSION_PROGRAM),
                name: "info:sourceCode".to_string(),
                type_name: "string".to_string(),
                value: new_xml,
            });
            commands.trigger(ApplyUsdOps {
                doc,
                parent_gen: None,
                label: "Insert waypoint".to_string(),
                ops,
            });
        }
        Err(err) => report_waypoint_failure(&mut commands, format!("Placement failed: {err}")),
    }
}

/// Draw the right-clicked waypoint's context menu (an egui `Area`).
///
/// Every action edits the vessel's mission `info:sourceCode` XML through the one authoring
/// funnel ([`ApplyUsdOp`]), so each is journaled, undoable, saved and replicated like
/// any other prim edit — `Move`/`Insert after` just defer the edit until the follow-up
/// ground click ([`on_scene_click_place_waypoint`]).
///
/// `Smooth path` is route-level (it lives on the patrol's `Sequence`, not on one
/// waypoint), so it is shown here as the natural place the user is already looking.
pub fn draw_waypoint_context_menu(
    mut contexts: bevy_egui::EguiContexts,
    mut menu_state: ResMut<WaypointContextMenuState>,
    mut placement: ResMut<WaypointPlacement>,
    mut menu_open: ResMut<lunco_core::WaypointMenuOpen>,
    q_prim: Query<&UsdPrimPath>,
    q_markers: Query<(Entity, &BehaviorXml, Option<&ReachedWaypoints>)>,
    q_vessel: Query<(&BehaviorXml, &UsdPrimPath)>,
    doc_ctx: WaypointDocContext,
    mut commands: Commands,
) {
    let Some(vis_entity) = menu_state.entity else {
        if menu_open.0 {
            menu_open.0 = false; // release the camera
        }
        return;
    };
    // The marker can vanish under the menu (route edited elsewhere) — close, don't panic.
    let Some((marker_vessel, marker_target, marker_index, marker_passed)) =
        resolve_marker(vis_entity, &q_prim, &q_markers)
    else {
        menu_state.entity = None;
        menu_open.0 = false;
        return;
    };
    let Ok((xml, vessel_prim)) = q_vessel.get(marker_vessel) else {
        menu_state.entity = None;
        menu_open.0 = false;
        return;
    };
    // Hold the camera still for as long as the menu is up.
    menu_open.0 = true;
    // The document that owns THIS route — resolved from the vessel's own stage, the
    // same way every other waypoint path resolves it. (Reading the workspace's active
    // document instead put the edit on whatever the user last opened, which is not
    // the route's document once a twin scene is mounted.)
    let Some(doc) = doc_ctx.resolve_document(&vessel_prim.stage_handle) else {
        menu_state.entity = None;
        menu_open.0 = false;
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else { return };

    // The pointer position is window-relative, but egui lays out from the context's
    // content rect — which is NOT the window origin when the scene viewport sits in a
    // dock leaf. Without this offset the menu is placed off under the chrome and looks
    // like it never opened.
    let origin = ctx.content_rect().min.to_vec2();
    let pos = egui::pos2(menu_state.position.x, menu_state.position.y) + origin;
    let mut open = true;
    // Buffer the dwell outside the closure: the closure needs `&mut` to it while
    // `menu_state` is still read afterwards.
    let mut dwell = menu_state.dwell;
    let mut smooth = lunco_autopilot::usd_tree::authored_route_metadata(&xml.0)
        .is_ok_and(|metadata| metadata.smooth);
    let mut edited: Option<String> = None;
    // The marker prim a Delete must also un-author, alongside its mission leg.
    let mut deleted_marker: Option<String> = None;

    let response = egui::Area::new(egui::Id::new("waypoint_context_menu"))
        .fixed_pos(pos)
        .order(egui::Order::Foreground)
        .constrain(true) // never let it spill off-screen near the window edge
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_width(190.0);
                ui.label(egui::RichText::new(format!("Waypoint {}", marker_index + 1)).strong());
                if marker_passed {
                    ui.label(egui::RichText::new("visited (this session)").weak().small());
                }
                ui.separator();

                if ui
                    .button("✋  Move")
                    .on_hover_text(
                        "Then click the ground to put this waypoint there  ·  Esc to cancel",
                    )
                    .clicked()
                {
                    placement.0 = Some(PendingPlacement::Route {
                        doc,
                        coord_key: marker_target.clone(),
                        mode: PlacementMode::Move,
                    });
                    open = false;
                }
                if ui
                    .button("➕  Insert after")
                    .on_hover_text(
                        "Then click the ground to add the next waypoint right after this one  ·  \
                         Esc to cancel",
                    )
                    .clicked()
                {
                    info!("[waypoint] armed Insert-after of '{}'", marker_target);
                    placement.0 = Some(PendingPlacement::Route {
                        doc,
                        coord_key: marker_target.clone(),
                        mode: PlacementMode::InsertAfter,
                    });
                    open = false;
                }
                if lunco_workbench::icon_text_button(
                    ui,
                    lunco_workbench::UiIcon::Delete,
                    "Delete",
                    "Delete this waypoint",
                )
                .clicked()
                {
                    match remove_waypoint_leaf(&xml.0, &marker_target) {
                        Ok(new_xml) => {
                            edited = Some(new_xml);
                            // Delete must take the PIN as well as the leg. Since the
                            // marker became an authored prim it is an object in its
                            // own right, not a visual derived from the XML — so
                            // dropping the leg alone left the dome standing on the
                            // map, belonging to no route. That reads as "delete did
                            // nothing".
                            deleted_marker = Some(marker_target.clone());
                        }
                        Err(err) => {
                            report_waypoint_failure(&mut commands, format!("Delete failed: {err}"))
                        }
                    }
                    open = false;
                }

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Dwell");
                    let resp = ui.add(
                        egui::DragValue::new(&mut dwell)
                            .range(0.0..=600.0)
                            .speed(0.25)
                            .suffix(" s"),
                    );
                    if resp.changed() {
                        match set_waypoint_dwell(&xml.0, &marker_target, dwell) {
                            Ok(new_xml) => edited = Some(new_xml),
                            Err(err) => report_waypoint_failure(
                                &mut commands,
                                format!("Dwell update failed: {err}"),
                            ),
                        }
                    }
                })
                .response
                .on_hover_text("Seconds the rover holds here before departing (0 = none)");

                ui.separator();
                if ui
                    .checkbox(&mut smooth, "Smooth path (spline)")
                    .on_hover_text(
                        "Whole route: arc through the waypoints on a Catmull-Rom curve \
                         instead of driving straight leg-to-leg",
                    )
                    .changed()
                {
                    match set_route_smooth(&xml.0, smooth) {
                        Ok(new_xml) => edited = Some(new_xml),
                        Err(err) => report_waypoint_failure(
                            &mut commands,
                            format!("Path smoothing update failed: {err}"),
                        ),
                    }
                }
            });
        });

    menu_state.dwell = dwell;

    if let Some(value) = edited {
        commands.trigger(ApplyUsdOp {
            doc,
            parent_gen: None,
            op: UsdOp::SetAttribute {
                edit_target: LayerId::root(),
                path: join_prim(&vessel_prim.path, WAYPOINT_MISSION_PROGRAM),
                name: "info:sourceCode".to_string(),
                type_name: "string".to_string(),
                value,
            },
        });
    }

    // Deactivate the pin itself, AFTER the mission no longer references it. A
    // marker is a purely-visual prim (a non-colliding translucent dome + an
    // overlap-only Sensor — never a rigid body), so this `SetActive` reconciles
    // INCREMENTALLY: `op_needs_rebuild` carves marker paths (`/<vessel>/Route/
    // W<n>`) out of the rebuild set, and the live `author_active` +
    // `refresh_prim_subtree` drops the pin's visual subtree. No scene reload.
    // `RemovePrim` is still NOT used: a route waypoint is normally authored in
    // the scene/variant layer while interactive edits target the runtime
    // overlay, so `RemovePrim` (which can only remove a spec authored by that
    // same layer) left the original marker composed. `active = false` is the
    // authoritative stronger opinion: it hides the authored prim and its
    // subtree, is undoable, and does not mutate the source scene merely to
    // satisfy a runtime delete.
    if let Some(marker_path) = deleted_marker {
        info!("[waypoint] deactivating marker prim {marker_path}");
        commands.trigger(ApplyUsdOp {
            doc,
            parent_gen: None,
            op: UsdOp::SetActive {
                edit_target: LayerId::root(),
                path: marker_path,
                active: false,
            },
        });
    }

    // Dismiss on a LEFT click outside — never on "any click". The menu is opened BY a
    // right-click and the camera is driven by the mouse, so closing on any click let
    // the very release that opened it (and any stray right-drag) slam it shut the
    // moment it appeared. Keyboard dismissal is NOT handled here: it comes through the
    // `Cancel` intent → `CancelWaypointEdit` command (`cancel_waypoint_edit_on_intent`),
    // so every waypoint mode backs out the same way.
    if menu_state.just_opened {
        menu_state.just_opened = false;
    } else if ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary))
        && !response.response.hovered()
    {
        open = false;
    }

    if !open {
        menu_state.entity = None;
        menu_open.0 = false;
    }
}

fn runtime_route_loops(spec: &lunco_autopilot::AutopilotBehaviorSpec) -> bool {
    match &spec.0 {
        lunco_autopilot::BehaviorSpec::Forever { .. }
        | lunco_autopilot::BehaviorSpec::Patrol { .. } => true,
        lunco_autopilot::BehaviorSpec::Repeat { times, .. } => *times > 1,
        _ => false,
    }
}

/// Runtime visual progress for one route. Only the collision-backed set is
/// evidence that a waypoint was visited. The behavior cursor identifies the
/// active leg, but never changes visited state.
#[derive(Clone, Debug, Default)]
pub(crate) struct RouteVisualState {
    visited: Vec<bool>,
    active_index: Option<usize>,
}

fn route_visual_state(
    targets: &[String],
    reached: Option<&ReachedWaypoints>,
    cursor: Option<usize>,
    completed: bool,
    looping: bool,
) -> RouteVisualState {
    let visited = targets
        .iter()
        .map(|target| reached.is_some_and(|reached| reached.0.iter().any(|done| done == target)))
        .collect::<Vec<_>>();

    let cursor_index = cursor.and_then(|index| {
        if targets.is_empty() {
            None
        } else if looping {
            Some(index % targets.len())
        } else {
            (index < targets.len()).then_some(index)
        }
    });
    // The compiled USD route may strip reached legs before rebuilding the
    // behaviour tree.  Its cursor is therefore relative to the remaining
    // legs, while `targets` is still the complete authored route used by the
    // editor.  Translate through the unvisited indices before choosing the
    // visual blue leg; otherwise cursor 0 points back at W0 after W0 was
    // reached and a newly appended/current waypoint never receives the cue.
    let unvisited_indices: Vec<usize> = visited
        .iter()
        .enumerate()
        .filter_map(|(index, &is_visited)| (!is_visited).then_some(index))
        .collect();
    let active_index = if completed {
        None
    } else {
        cursor_index
            .and_then(|cursor| unvisited_indices.get(cursor).copied())
            .or_else(|| visited.iter().position(|visited| !visited))
            // A looping route can have completed a full authored lap while
            // its runtime cursor has already wrapped. In that state there is
            // no unvisited index to translate, so the cursor is the next
            // authored target again. A completed loop keeps the full authored
            // route visible; it does not require following the rover pose.
            .or_else(|| looping.then_some(0))
    };

    RouteVisualState {
        visited,
        active_index,
    }
}

fn route_execution(
    vessel: Entity,
    q_autopilots: &Query<(
        &lunco_autopilot::Autopilot,
        Option<&lunco_autopilot::AutopilotBehavior>,
        Option<&lunco_autopilot::AutopilotExecutionState>,
    )>,
) -> (Option<usize>, bool) {
    q_autopilots
        .iter()
        .find(|(autopilot, _, _)| autopilot.vessel == vessel)
        .map(|(_, behavior, execution)| {
            (
                behavior.and_then(|behavior| behavior.route_cursor()),
                execution.is_some_and(|state| {
                    matches!(state, lunco_autopilot::AutopilotExecutionState::Completed)
                }),
            )
        })
        .unwrap_or_default()
}

/// Return runtime marker roots in patrol order. A runtime route is only a
/// presentable route once every live waypoint has its explicit binding; using a
/// partial list would put the ribbon and marker state on different target indices.
fn ordered_runtime_marker_entities(
    vessel: Entity,
    count: usize,
    runtime_markers: &std::collections::HashMap<(Entity, usize), Entity>,
) -> Option<Vec<Entity>> {
    (0..count)
        .map(|index| runtime_markers.get(&(vessel, index)).copied())
        .collect()
}
/// One resolved waypoint in the derived route view. The key remains the
/// authored target identity; the position is only a cache of the current ECS
/// projection and is never written back to USD or to the mission XML.
#[derive(Clone, Debug)]
pub(crate) struct RouteVisualTarget {
    pub key: String,
    pub entity: Option<Entity>,
    pub position: DVec3,
    pub visited: bool,
}

/// The one editor-facing route view. It is rebuilt only when an authoritative
/// route, target pose, terrain, frame, progress, or focus input changes. Meshes
/// and marker tinting consume this snapshot instead of interpreting XML or
/// resolving targets independently. Labels are authored on waypoint prims and
/// consumed by the generic billboard renderer.
#[derive(Clone, Debug)]
pub(crate) struct RouteVisualRoute {
    pub targets: Vec<RouteVisualTarget>,
    pub green: Vec<DVec3>,
    pub blue: Vec<DVec3>,
    pub focused: bool,
}

#[derive(Resource, Default)]
pub(crate) struct RouteVisualProjection {
    pub frame: Option<Entity>,
    pub surface: Option<(Entity, u64)>,
    pub revision: u64,
    pub routes: std::collections::HashMap<Entity, RouteVisualRoute>,
}

/// Shared work ticket for the two route producers. It is deliberately a
/// resource rather than a duplicated run condition: `RemovedComponents` has a
/// reader cursor, so evaluating the same condition twice can consume a removal
/// before the snapshot producer sees it.
#[derive(Resource)]
pub(crate) struct RouteProjectionRebuildRequested {
    pub pending: bool,
}

impl Default for RouteProjectionRebuildRequested {
    fn default() -> Self {
        Self { pending: true }
    }
}

/// Resolve a picked sub-part (wheel, hull panel, antenna, ...) to the vehicle
/// whose mission owns the route.  Selection is intentionally granular; routes
/// are intentionally vehicle-level.
fn route_owner(
    entity: Option<Entity>,
    q_parents: &Query<&ChildOf>,
    vessels: &std::collections::HashSet<Entity>,
) -> Option<Entity> {
    let mut current = entity?;
    loop {
        if vessels.contains(&current) {
            return Some(current);
        }
        current = q_parents.get(current).ok()?.parent();
    }
}

/// Resolve a picked marker prim to the route it belongs to: the vessel whose
/// mission targets it, the target string, its position in the route, and whether
/// it has been reached.
///
/// The marker is an authored PRIM, so its identity IS its path — there is no
/// parallel visual entity to key off. `None` when the prim is not a waypoint of
/// any current mission (a random prim was right-clicked).
fn resolve_marker(
    marker: Entity,
    q_prim: &Query<&UsdPrimPath>,
    q_vessels: &Query<(Entity, &BehaviorXml, Option<&ReachedWaypoints>)>,
) -> Option<(Entity, String, usize, bool)> {
    let path = &q_prim.get(marker).ok()?.path;
    for (vessel, xml, reached) in q_vessels.iter() {
        let Ok(metadata) = lunco_autopilot::usd_tree::authored_route_metadata(&xml.0) else {
            continue;
        };
        if let Some(index) = metadata.targets.iter().position(|t| t == path) {
            let passed = reached.map(|r| r.0.contains(path)).unwrap_or(false);
            return Some((vessel, path.clone(), index, passed));
        }
    }
    None
}

/// The vessel whose mission names `target` (a waypoint MARKER prim path).
///
/// Resolution by CONTENT, not by a remembered `Entity`: an armed edit outlives at
/// least one authoring round-trip, and a stage recomposition respawns the vessel in
/// between. The path is what is stable, so it is what the lookup keys on.
fn vessel_for_target<'a>(
    q_vessel: &'a Query<(Entity, &BehaviorXml, &UsdPrimPath)>,
    target: &str,
) -> Option<(Entity, &'a BehaviorXml, &'a UsdPrimPath)> {
    let mut matches = q_vessel.iter().filter(|(_, xml, _)| {
        let Ok(metadata) = lunco_autopilot::usd_tree::authored_route_metadata(&xml.0) else {
            return false;
        };
        metadata.targets.iter().any(|t| t == target)
    });
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

/// Author a waypoint MARKER: a prim referencing `vessels/markers/waypoint.usda`
/// under the scene's `Route` scope, translated to `at`. Returns its path, which
/// is also its identity — the mission targets it by path.
///
/// One implementation for every way a waypoint comes into being (drop,
/// insert-after), so a marker is never half-authored: the geometry, livery and
/// trigger zone all come from the referenced asset.
fn author_marker_ops(
    host: &lunco_doc::DocumentHost<lunco_usd::document::UsdDocument>,
    root: &str,
    at: DVec3,
    canonical: &CanonicalStages,
    vessel_prim: &UsdPrimPath,
) -> (String, Vec<UsdOp>) {
    let route_scope = join_prim(root, WAYPOINT_ROUTE_SCOPE);
    let mut ops = Vec::new();
    // The composed stage can already contain Route from a referenced or variant
    // layer, while this document's root layer has no local parent spec. AddPrim
    // validates the edit target's authored data, so composed existence cannot
    // suppress this required local spec.
    if !prim_exists(host, &route_scope) {
        ops.push(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: root.to_string(),
            name: WAYPOINT_ROUTE_SCOPE.to_string(),
            type_name: Some("Scope".to_string()),
            reference: None,
        });
    }
    // First free `W<n>` — the name a scene author would have written by hand.
    let marker_name = (0..)
        .map(|n| format!("W{n}"))
        .find(|name| {
            let path = join_prim(&route_scope, name);
            // The composed stage can lag while a referenced marker's asset
            // closure is loading. The document is authoritative for names
            // already reserved by an earlier click, so consult both sources.
            !composed_prim_exists(canonical, vessel_prim, &path) && !prim_exists(host, &path)
        })
        .expect("an unbounded search always finds a free name");
    let marker_path = join_prim(&route_scope, &marker_name);
    ops.push(UsdOp::AddPrim {
        edit_target: LayerId::root(),
        parent_path: route_scope,
        name: marker_name,
        type_name: Some("Xform".to_string()),
        reference: Some(WAYPOINT_MARKER_ASSET.to_string()),
    });
    // The picked point is grid-absolute and is the authored placement value for
    // this marker, matching the frame used by the live route projection.
    ops.push(UsdOp::SetTranslate {
        edit_target: LayerId::root(),
        path: marker_path.clone(),
        value: [at.x, at.y, at.z],
    });
    ops.extend(waypoint_billboard_ops(marker_path.clone()));
    (marker_path, ops)
}

/// The API-applied Scope that carries a vessel's mission tree, creating it if
/// this is the first waypoint — returns the path to author `info:sourceCode` onto
/// and the operations needed to create it when absent.
///
/// The tree is a PROGRAM, not an attribute on the vessel: a mission is bolted on,
/// so it is a child prim that can be deleted to remove the behaviour, and the
/// behaviour engine is chosen by the source's extension exactly as `.mo` and
/// `.rhai` are. `process_usd_sim_prims` reads it back off this child and stamps
/// `BehaviorXml` on the vessel that owns it.
///
/// `AddPrim` on an existing prim is a rejection rather than a merge, so it is only
/// authored when genuinely absent.
///
/// `mission_exists` comes from the **live composed stage**, not the document
/// layer. Traverse authors its mission inside the selected site variant; the
/// existing `SetAttribute` document operation creates a local over when the
/// composed prim is absent from the root layer. Do not add a duplicate local
/// Mission spec here merely because the mission came from a referenced layer.
fn ensure_mission_program_ops(
    host: &lunco_doc::DocumentHost<lunco_usd::document::UsdDocument>,
    vessel_path: &str,
    mission_exists: bool,
) -> (String, Vec<UsdOp>) {
    let path = join_prim(vessel_path, WAYPOINT_MISSION_PROGRAM);
    let mut ops = Vec::new();
    if !mission_exists && !prim_exists(host, &path) {
        ops.push(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: vessel_path.to_string(),
            name: WAYPOINT_MISSION_PROGRAM.to_string(),
            type_name: Some("Scope".to_string()),
            reference: None,
        });
        ops.push(UsdOp::SetApiSchemas {
            edit_target: LayerId::root(),
            path: path.clone(),
            schemas: vec!["LunCoProgramAPI".to_string()],
        });
    }
    (path, ops)
}

/// Waypoint creation targets the selected USD variant, so its existence
/// decisions must read the live composed stage. The document's authored layers
/// deliberately retain the variant opinions unflattened and therefore cannot
/// answer whether `/Traverse/Route` or `/Traverse/Route/W0` already exists.
fn composed_prim_exists(
    canonical: &CanonicalStages,
    vessel_prim: &UsdPrimPath,
    path: &str,
) -> bool {
    canonical
        .get(vessel_prim.stage_handle.id())
        .zip(SdfPath::new(path).ok())
        .is_some_and(|(stage, prim)| stage.view().has_prim(&prim))
}

/// the autopilot currently driving it and returns ownership to the local session.
///
/// Without this the autopilot keeps the vessel claimed, so `drive_from_bindings`
/// yields and the player's input is silently swallowed — the rover just keeps driving
/// its route while you press the keys. Taking the wheel is the universal expectation
/// for an autopilot, so it is an implicit disengage rather than a separate hotkey
/// (the canonical Action intent still toggles explicitly). Thrust is included
/// because it is the lander's manual engine command; pressing it must also
/// reclaim a vessel from an autopilot.
///
/// Keyed off the vessel's `ActionState<UserIntent>` — the DATA keymap
/// (`assets/config/keybindings.json`) — not hardcoded WASD, so a rebound control
/// takes over too. Look/Zoom are excluded: moving the camera is not driving.
pub fn manual_input_disengages_autopilot(
    egui_focus: Res<EguiFocus>,
    q_ctrl: Query<(
        &ControllerLink,
        &leafwing_input_manager::prelude::ActionState<lunco_core::UserIntent>,
    )>,
    q_autopilot: Query<&lunco_autopilot::Autopilot>,
    q_gid: Query<&GlobalEntityId>,
    mut registry: ResMut<SessionRegistry>,
    mut commands: Commands,
) {
    if egui_focus.wants_keyboard {
        return; // typing in a panel is not driving
    }
    use lunco_core::UserIntent::*;
    const DRIVE: [lunco_core::UserIntent; 8] = [
        MoveForward,
        MoveBackward,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        Thrust,
        Brake,
    ];

    for (link, intents) in q_ctrl.iter() {
        // Edge-triggered: react to the press, not to every frame it's held, so a held
        // key doesn't re-fire the disengage every tick.
        if !DRIVE.iter().any(|i| intents.just_pressed(i)) {
            continue;
        }
        let vessel = link.vessel_entity;
        if !q_autopilot.iter().any(|ap| ap.vessel == vessel) {
            continue; // nothing driving it; the input is already the player's
        }
        info!("[autopilot] manual drive input — disengaging and handing control back");
        commands.trigger(lunco_autopilot::DisengageAutopilot { vessel });
        // Reclaim ownership for the player, exactly as the Action intent does —
        // otherwise the vessel is left unowned and the input still goes nowhere.
        if let Ok(gid) = q_gid.get(vessel) {
            let _ = registry.claim(SessionId::LOCAL, gid.get());
        }
    }
}

/// Toggle autopilot for the possessed vessel through the canonical Action
/// intent. The default F key is the only autopilot shortcut; vehicle-specific
/// commands such as the rover brake use their own authored intent and never
/// reach this handler.
pub fn handle_autopilot_toggle_intent(
    egui_focus: Res<EguiFocus>,
    local_avatar: Res<TheLocalAvatar>,
    avatars: Query<(Entity, &IntentState), (With<Avatar>, With<LocalAvatar>)>,
    q_link: Query<&ControllerLink>,
    mut commands: Commands,
) {
    if egui_focus.wants_keyboard {
        return;
    }
    if let Some((avatar, _)) = local_avatar
        .0
        .and_then(|entity| avatars.get(entity).ok())
        .filter(|(_, intent)| intent.just_pressed(&UserIntent::Action))
    {
        if let Ok(link) = q_link.get(avatar) {
            commands.trigger(ToggleAutopilot {
                vessel: link.vessel_entity,
            });
        }
    }
}

use lunco_core::{on_command, register_commands, Command};

/// Command to engage autopilot on a vessel.
#[Command]
pub struct StartAutopilot {
    /// The vessel entity to start autopilot on.
    pub vessel: Entity,
}

#[on_command(StartAutopilot)]
fn on_start_autopilot(
    trigger: On<StartAutopilot>,
    q_autopilot: Query<(Entity, &lunco_autopilot::Autopilot)>,
    q_spec: Query<&lunco_autopilot::AutopilotBehaviorSpec>,
    q_route: Query<(
        Option<&lunco_autopilot::usd_tree::BehaviorXml>,
        Option<&lunco_autopilot::AutopilotBehaviorSpec>,
    )>,
    mut registry: ResMut<SessionRegistry>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let vessel = cmd.vessel;
    let autopilot_engaged = q_autopilot.iter().any(|(_, ap)| ap.vessel == vessel);
    if !autopilot_engaged {
        let has_route = q_route
            .get(vessel)
            .is_ok_and(|(xml, spec)| has_authored_movement_route(xml, spec));
        if !has_route {
            info!(
                "[autopilot] start ignored for vessel {:?}: no authored movement route",
                vessel
            );
            commands.trigger(lunco_avatar::ShowNotification {
                text:
                    "No autopilot route is authored for this vessel; manual control remains active."
                        .to_string(),
                kind: "warn".to_string(),
                secs: 3.0,
            });
            return;
        }
        info!("Engaging autopilot on vessel {:?}", vessel);
        let spec_json = if let Ok(spec) = q_spec.get(vessel) {
            spec.to_json().unwrap_or_default()
        } else {
            String::new()
        };
        registry.release_session(SessionId::LOCAL);

        // Throttle 0: engaging runs the vessel's route (`spec_json`), and a vessel
        // with no route HOLDS. A constant setpoint here drove routeless rovers
        // straight off the site.
        commands.trigger(lunco_autopilot::EngageAutopilot {
            vessel,
            index: 0,
            throttle: 0.0,
            spec_json,
        });
    }
}

/// Command to toggle autopilot on/off on a vessel.
#[Command]
pub struct ToggleAutopilot {
    /// The vessel entity to toggle autopilot on/off.
    pub vessel: Entity,
}

#[on_command(ToggleAutopilot)]
fn on_toggle_autopilot(
    trigger: On<ToggleAutopilot>,
    q_autopilot: Query<(Entity, &lunco_autopilot::Autopilot)>,
    q_spec: Query<&lunco_autopilot::AutopilotBehaviorSpec>,
    q_route: Query<(
        Option<&lunco_autopilot::usd_tree::BehaviorXml>,
        Option<&lunco_autopilot::AutopilotBehaviorSpec>,
    )>,
    q_gid: Query<&GlobalEntityId>,
    mut registry: ResMut<SessionRegistry>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let vessel = cmd.vessel;
    let autopilot_engaged = q_autopilot.iter().any(|(_, ap)| ap.vessel == vessel);
    if autopilot_engaged {
        info!("Disengaging autopilot on vessel {:?}", vessel);
        commands.trigger(lunco_autopilot::DisengageAutopilot { vessel });

        if let Ok(gid) = q_gid.get(vessel) {
            let _ = registry.claim(SessionId::LOCAL, gid.get());
        }
    } else {
        let has_route = q_route
            .get(vessel)
            .is_ok_and(|(xml, spec)| has_authored_movement_route(xml, spec));
        if !has_route {
            info!(
                "[autopilot] F ignored for vessel {:?}: no authored movement route",
                vessel
            );
            commands.trigger(lunco_avatar::ShowNotification {
                text:
                    "No autopilot route is authored for this vessel; manual control remains active."
                        .to_string(),
                kind: "warn".to_string(),
                secs: 3.0,
            });
            return;
        }
        info!("Engaging autopilot on vessel {:?}", vessel);
        let spec_json = if let Ok(spec) = q_spec.get(vessel) {
            spec.to_json().unwrap_or_default()
        } else {
            String::new()
        };
        registry.release_session(SessionId::LOCAL);

        // Throttle 0: engaging runs the vessel's route (`spec_json`), and a vessel
        // with no route HOLDS. A constant setpoint here drove routeless rovers
        // straight off the site.
        commands.trigger(lunco_autopilot::EngageAutopilot {
            vessel,
            index: 0,
            throttle: 0.0,
            spec_json,
        });
    }
}

/// A route may be authored as XML before its derived runtime spec is ready.
/// Treat that state as available so a user pressing F during scene startup does
/// not get a false "no route" warning; once a spec exists, its actual movement
/// content is authoritative and an empty/holding tree is still rejected.
fn has_authored_movement_route(
    xml: Option<&lunco_autopilot::usd_tree::BehaviorXml>,
    spec: Option<&lunco_autopilot::AutopilotBehaviorSpec>,
) -> bool {
    if let Some(xml) = xml {
        let Ok(metadata) = lunco_autopilot::usd_tree::authored_route_metadata(&xml.0) else {
            return false;
        };
        return !metadata.targets.is_empty();
    }
    spec.is_some_and(|spec| spec.0.has_motion())
}

register_commands!(
    on_start_autopilot,
    on_toggle_autopilot,
    on_cancel_waypoint_edit
);

// ── Route ribbon (real 3D geometry, not a screen-space overlay) ───────────────

/// Half-width (metres) of the route ribbon — a thin drawn annotation, not a road.
const ROUTE_RIBBON_HALF_WIDTH_M: f32 = 0.12;
/// Separation (metres) between the derived route annotation and the composed
/// terrain surface. This belongs to the annotation renderer, not to the USD
/// waypoint asset: a waypoint's authored visual dimensions must never determine
/// the position of an unrelated derived overlay.
const ROUTE_SURFACE_CLEARANCE_M: f32 = 0.08;
/// Resample spacing for a `smooth` route's ribbon. Matches the autopilot's own
/// resampling, so the drawn curve IS the driven curve.
const ROUTE_SAMPLE_SPACING_M: f64 = 2.0;

/// A vessel's route ribbon. `signature` is what the mesh was built from, so the
/// (relatively expensive) rebuild only happens when the route, surface, or active
/// leg changes — not while the rover moves.
///
/// A route draws as two roles: the complete ordered green waypoint-to-waypoint
/// path and a blue highlight for the current authored leg. The blue highlight
/// changes when `ReachedWaypoints` advances; the moving rover is intentionally
/// not a route projection input.
#[derive(Component)]
pub struct WaypointPathMesh {
    pub vessel: Entity,
    pub signature: u64,
    pub part: PathPart,
}

/// Which half of a route a ribbon draws.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PathPart {
    /// Complete ordered waypoint-to-waypoint route.
    Future,
    /// Current authored waypoint-to-waypoint leg.
    Remaining,
}

/// Split route geometry into its two visual roles: the complete ordered green
/// waypoint-to-waypoint route and the single active blue authored leg. Visited
/// state advances the blue highlight and marker appearance, but never removes
/// a point from the authored route. Keeping this pure makes the transition
/// testable without a renderer or a Bevy world.
fn route_ribbon_points(
    points: &[(DVec3, bool)],
    active_index: Option<usize>,
) -> (Vec<DVec3>, Vec<DVec3>) {
    let next = active_index
        .filter(|&index| index < points.len())
        .or_else(|| points.iter().position(|(_, visited)| !visited));
    let green = points.iter().map(|(point, _)| *point).collect();
    let blue = next
        .and_then(|index| {
            // The first leg has no previous authored waypoint. The route itself
            // remains green; blue is reserved for highlighting an existing
            // waypoint-to-waypoint leg without following the rover every frame.
            (index > 0).then(|| vec![points[index - 1].0, points[index].0])
        })
        .unwrap_or_default();
    (green, blue)
}

/// Build a surface-separated ribbon through `points`, with vertices expressed
/// relative to `anchor` (the entity's own origin) so f32 vertex precision stays
/// tight regardless of how far the route sits from the world origin.
///
/// `points` are grid-absolute positions returned by the terrain/waypoint
/// projection. The route renderer adds one fixed presentation clearance to every
/// vertex; it never reuses a marker's authored radius or local visual offset.
pub(crate) fn build_ribbon_mesh(
    points: &[DVec3],
    anchor: DVec3,
    half_width: f32,
    surface_clearance: f32,
) -> Option<Mesh> {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};
    let n = points.len();
    if n < 2 {
        return None;
    }
    let mut pos: Vec<[f32; 3]> = Vec::with_capacity(n * 2);
    let mut nrm: Vec<[f32; 3]> = Vec::with_capacity(n * 2);
    let mut uv: Vec<[f32; 2]> = Vec::with_capacity(n * 2);
    let mut previous_right = None;
    for i in 0..n {
        // Use the local route direction, flattened to the ground plane so the
        // ribbon stays level across slopes instead of twisting. At a U-turn
        // (the Apollo 15 W3 → W4 → W3 turnaround), the central difference is
        // zero; keep the incoming direction there instead of inventing a
        // world-axis tangent that breaks the ribbon at the waypoint.
        let tan = route_tangent(points, i);
        let right = route_right(tan, previous_right);
        previous_right = Some(right);
        let right = right * half_width as f64;
        let base = (points[i] - anchor).as_vec3() + Vec3::Y * surface_clearance;
        let r = right.as_vec3();
        pos.push((base - r).to_array());
        pos.push((base + r).to_array());
        nrm.push([0.0, 1.0, 0.0]);
        nrm.push([0.0, 1.0, 0.0]);
        let v = i as f32;
        uv.push([0.0, v]);
        uv.push([1.0, v]);
    }
    let mut idx: Vec<u32> = Vec::with_capacity((n - 1) * 6);
    for i in 0..n - 1 {
        let a = (i * 2) as u32;
        idx.extend_from_slice(&[a, a + 1, a + 2, a + 2, a + 1, a + 3]);
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_indices(Indices::U32(idx));
    Some(mesh)
}

fn route_tangent(points: &[DVec3], index: usize) -> DVec3 {
    debug_assert!(!points.is_empty());
    let current = points[index];
    let prev = points[index.saturating_sub(1)];
    let next = points[(index + 1).min(points.len() - 1)];

    let mut tangent = next - prev;
    tangent.y = 0.0;
    if tangent.length_squared() < 1.0e-9 {
        tangent = current - prev;
        tangent.y = 0.0;
    }
    if tangent.length_squared() < 1.0e-9 {
        tangent = next - current;
        tangent.y = 0.0;
    }
    if tangent.length_squared() < 1.0e-9 {
        DVec3::Z
    } else {
        tangent.normalize()
    }
}

fn route_right(tangent: DVec3, previous_right: Option<DVec3>) -> DVec3 {
    let mut right = tangent.cross(DVec3::Y);
    if right.length_squared() < 1.0e-9 {
        right = DVec3::X;
    }
    let mut right = right.normalize();
    // A route ribbon is an unoriented strip. Keep its lateral frame coherent
    // when traversal reverses at a turnaround; otherwise the outgoing vertex
    // swaps left/right and the two triangles cross.
    if previous_right.is_some_and(|previous| right.dot(previous) < 0.0) {
        right = -right;
    }
    right
}

/// Uniformly sample a polyline in horizontal distance. Endpoints alone form a
/// chord through relief, so straight legs need the same intermediate samples as
/// curved legs before terrain projection.
fn resample_polyline(points: &[DVec3], spacing: f64) -> Vec<DVec3> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let mut out = vec![points[0]];
    for pair in points.windows(2) {
        let a = pair[0];
        let b = pair[1];
        let distance = DVec3::new(b.x - a.x, 0.0, b.z - a.z).length();
        let steps = (distance / spacing.max(1.0e-6)).ceil().max(1.0) as usize;
        for step in 1..=steps {
            let point = a.lerp(b, step as f64 / steps as f64);
            if out
                .last()
                .is_none_or(|last| (*last - point).length_squared() > 1.0e-12)
            {
                out.push(point);
            }
        }
    }
    out
}

fn project_route_to_surface(
    path: &[DVec3],
    surface: &lunco_terrain_surface::GridSurfaceQuery,
    surface_present: bool,
) -> Option<Vec<DVec3>> {
    if !surface_present {
        return Some(path.to_vec());
    }
    path.iter()
        .map(|point| {
            surface
                .height_at(lunco_core::coords::GridPos(*point))
                .map(|height| DVec3::new(point.x, height, point.z))
        })
        .collect()
}

fn route_geometry(
    points: &[DVec3],
    smooth: bool,
    closed: bool,
    surface: &lunco_terrain_surface::GridSurfaceQuery,
    surface_present: bool,
) -> Option<Vec<DVec3>> {
    if points.len() < 2 {
        return None;
    }
    let mut path = if smooth {
        catmull_rom_path(points, closed, ROUTE_SAMPLE_SPACING_M)
    } else {
        points.to_vec()
    };
    if closed {
        if let Some(first) = path.first().copied() {
            path.push(first);
        }
    }
    let path = resample_polyline(&path, ROUTE_SAMPLE_SPACING_M);
    project_route_to_surface(&path, surface, surface_present)
}

/// Arm one shared route rebuild ticket from authoritative source changes. In the
/// steady state this performs only Bevy change detection; it never parses XML or
/// samples terrain. The ticket is consumed by the snapshot producer after marker
/// projection has run, so both producers observe the same change, including a
/// removed behavior component.
pub(crate) fn arm_route_projection_rebuild(
    mut request: ResMut<RouteProjectionRebuildRequested>,
    q_route_inputs: Query<
        (),
        Or<(
            Changed<BehaviorXml>,
            Changed<lunco_autopilot::AutopilotBehaviorSpec>,
            Changed<TargetBindings>,
            Changed<ReachedWaypoints>,
        )>,
    >,
    q_route_poses: Query<
        (),
        (
            Or<(
                Changed<Transform>,
                Changed<big_space::grid::cell::CellCoord>,
            )>,
            Or<(
                With<lunco_usd_sim::marker::WaypointMarker>,
                With<RuntimeWaypointBinding>,
            )>,
            Without<WaypointPathMesh>,
        ),
    >,
    q_surface: Query<
        (),
        Or<(
            Changed<lunco_terrain_surface::DemHeightField>,
            Changed<lunco_terrain_surface::TerrainPoseInPhysicsFrame>,
        )>,
    >,
    mut removed_xml: RemovedComponents<BehaviorXml>,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    selected: Res<SelectedEntities>,
    local_avatar: Res<TheLocalAvatar>,
) {
    if !q_route_inputs.is_empty()
        || !q_route_poses.is_empty()
        || !q_surface.is_empty()
        || removed_xml.read().next().is_some()
        || active_frame.is_changed()
        || selected.is_changed()
        || local_avatar.is_changed()
    {
        request.pending = true;
    }
}

pub(crate) fn route_projection_rebuild_is_pending(
    request: Res<RouteProjectionRebuildRequested>,
) -> bool {
    request.pending
}

/// Project authored waypoint roots onto the active analytic surface.
///
/// Waypoint USD stores the planimetric route identity and authored X/Z pose;
/// the runtime marker root is the presentation projection used by both the
/// dome and its arrival sensor. This is deliberately a separate system from
/// route-mesh generation: marker placement owns marker transforms, while the
/// route projection owns only the transient line view. It is change-gated by
/// the same authoritative inputs and does no work while they are stable.
pub(crate) fn project_waypoint_markers_to_surface(
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_parents: Query<&ChildOf>,
    mut spatial: ParamSet<(
        Query<(Option<&big_space::grid::cell::CellCoord>, &Transform)>,
        Query<(
            Entity,
            &lunco_usd_sim::marker::WaypointMarker,
            &mut big_space::grid::cell::CellCoord,
            &mut Transform,
        )>,
    )>,
    surface: lunco_terrain_surface::GridSurfaceQuery,
) {
    let frame = active_frame.0;
    let Ok(grid) = q_grids.get(frame) else { return };
    if !surface.has_terrain() {
        return;
    }

    let markers = spatial
        .p1()
        .iter()
        .map(|(entity, ..)| entity)
        .collect::<Vec<_>>();
    let updates = {
        let q_spatial = spatial.p0();
        markers
            .into_iter()
            .filter_map(|entity| {
                let (position, _) = lunco_core::coords::grid_relative_pose(
                    entity, frame, &q_parents, &q_grids, &q_spatial,
                )?;
                let ground = surface.height_at(lunco_core::coords::GridPos(position))?;
                Some((
                    entity,
                    grid.translation_to_grid(DVec3::new(position.x, ground, position.z)),
                ))
            })
            .collect::<Vec<_>>()
    };

    let mut q_markers = spatial.p1();
    for (entity, (cell, local)) in updates {
        let Ok((_, _, mut marker_cell, mut transform)) = q_markers.get_mut(entity) else {
            continue;
        };
        if *marker_cell != cell {
            *marker_cell = cell;
        }
        if transform.translation != local {
            transform.translation = local;
        }
    }
}

/// Build one atomic route view for marker tinting and mesh rendering.
/// Authored XML resolves exclusively through the exact `TargetBindings` map;
/// coordinate strings are not a second authored contract. The view deliberately
/// excludes the moving rover pose: route annotations connect authored waypoints,
/// so driving does not cause per-frame route sampling or mesh rebuilds.
pub(crate) fn rebuild_waypoint_route_projection(
    q_vessels: Query<(
        Entity,
        Option<&BehaviorXml>,
        Option<&lunco_autopilot::AutopilotBehaviorSpec>,
        Option<&TargetBindings>,
        Option<&ReachedWaypoints>,
    )>,
    selected: Res<SelectedEntities>,
    local_avatar: Res<TheLocalAvatar>,
    q_avatar: Query<&ControllerLink, (With<Avatar>, With<LocalAvatar>)>,
    q_autopilots: Query<(
        &lunco_autopilot::Autopilot,
        Option<&lunco_autopilot::AutopilotBehavior>,
        Option<&lunco_autopilot::AutopilotExecutionState>,
    )>,
    q_runtime_markers: Query<(Entity, &RuntimeWaypointBinding)>,
    q_parents: Query<&ChildOf>,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::grid::cell::CellCoord>, &Transform)>,
    surface: lunco_terrain_surface::GridSurfaceQuery,
    mut request: ResMut<RouteProjectionRebuildRequested>,
    mut projection: ResMut<RouteVisualProjection>,
) {
    request.pending = false;
    let frame_entity = active_frame.0;
    let Ok(_grid) = q_grids.get(frame_entity) else {
        projection.frame = Some(frame_entity);
        projection.surface = None;
        projection.routes.clear();
        projection.revision = projection.revision.wrapping_add(1);
        return;
    };
    let surface_key = surface.surface_key();
    let surface_present = surface_key.is_some();
    let vessel_entities: std::collections::HashSet<Entity> =
        q_vessels.iter().map(|(entity, ..)| entity).collect();
    let selected_vessel = route_owner(selected.primary(), &q_parents, &vessel_entities);
    let possessed_vessel = route_owner(
        local_avatar
            .0
            .and_then(|entity| q_avatar.get(entity).ok())
            .map(|link| link.vessel_entity),
        &q_parents,
        &vessel_entities,
    );
    let runtime_markers: std::collections::HashMap<(Entity, usize), Entity> = q_runtime_markers
        .iter()
        .map(|(entity, binding)| ((binding.vessel, binding.index), entity))
        .collect();
    let mut routes = std::collections::HashMap::new();

    for (vessel, xml, spec, bindings, reached) in q_vessels.iter() {
        let (targets, points, smooth, closed, entities) = if let Some(xml) = xml {
            // Authored XML is parsed once by the autopilot owner. Its route
            // metadata remains authoritative even while a derived runtime spec
            // is present, and malformed XML is an explicit no-route state.
            let Ok(metadata) = lunco_autopilot::usd_tree::authored_route_metadata(&xml.0) else {
                continue;
            };
            let targets = metadata.targets;
            let Some(bindings) = bindings else { continue };
            if targets.is_empty() {
                continue;
            }
            let Some(resolved) = targets
                .iter()
                .map(|target| {
                    let entity = *bindings.0.get(target)?;
                    let (position, _) = lunco_core::coords::grid_relative_pose(
                        entity,
                        frame_entity,
                        &q_parents,
                        &q_grids,
                        &q_spatial,
                    )?;
                    Some((position, entity))
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            (
                targets,
                resolved.iter().map(|(point, _)| *point).collect::<Vec<_>>(),
                metadata.smooth,
                metadata.loops,
                resolved
                    .into_iter()
                    .map(|(_, entity)| Some(entity))
                    .collect(),
            )
        } else if let Some(spec) = spec {
            let Some(waypoints) = spec.patrol_waypoints() else {
                continue;
            };
            // Runtime waypoint roots are the presentation projection of the same
            // live route. Resolve their current active-frame poses instead of
            // reusing the original command Y coordinate: terrain edits move the
            // marker root while the ribbon is rebuilt from the same points.
            let Some(runtime_entities) =
                ordered_runtime_marker_entities(vessel, waypoints.len(), &runtime_markers)
            else {
                continue;
            };
            let Some(points) = runtime_entities
                .iter()
                .map(|entity| {
                    lunco_core::coords::grid_relative_pose(
                        *entity,
                        frame_entity,
                        &q_parents,
                        &q_grids,
                        &q_spatial,
                    )
                    .map(|(position, _)| position)
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            (
                (0..waypoints.len()).map(runtime_waypoint_key).collect(),
                points,
                false,
                runtime_route_loops(spec),
                runtime_entities.into_iter().map(Some).collect::<Vec<_>>(),
            )
        } else {
            continue;
        };
        let (cursor, completed) = route_execution(vessel, &q_autopilots);
        let progress = route_visual_state(&targets, reached, cursor, completed, closed);
        let points_with_state = points
            .iter()
            .copied()
            .enumerate()
            .map(|(index, point)| (point, progress.visited.get(index).copied().unwrap_or(false)))
            .collect::<Vec<_>>();
        let (green_control, blue_control) =
            route_ribbon_points(&points_with_state, progress.active_index);
        let closed = closed && points.len() > 2;
        let green = route_geometry(&green_control, smooth, closed, &surface, surface_present)
            .unwrap_or_default();
        let blue = route_geometry(&blue_control, false, false, &surface, surface_present)
            .unwrap_or_default();
        let targets = targets
            .into_iter()
            .enumerate()
            .map(|(index, key)| RouteVisualTarget {
                key,
                entity: entities.get(index).copied().flatten(),
                position: points[index],
                visited: progress.visited.get(index).copied().unwrap_or(false),
            })
            .collect();
        routes.insert(
            vessel,
            RouteVisualRoute {
                targets,
                green,
                blue,
                focused: Some(vessel) == selected_vessel || Some(vessel) == possessed_vessel,
            },
        );
    }

    projection.frame = Some(frame_entity);
    projection.surface = surface_key;
    projection.routes = routes;
    projection.revision = projection.revision.wrapping_add(1);
}

fn route_mesh_signature(
    route: &RouteVisualRoute,
    part: PathPart,
    frame: Option<Entity>,
    surface: Option<(Entity, u64)>,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    frame.hash(&mut hash);
    surface.hash(&mut hash);
    part.hash(&mut hash);
    route.focused.hash(&mut hash);
    for target in &route.targets {
        target.key.hash(&mut hash);
        target.entity.hash(&mut hash);
        target.visited.hash(&mut hash);
        target.position.x.to_bits().hash(&mut hash);
        target.position.y.to_bits().hash(&mut hash);
        target.position.z.to_bits().hash(&mut hash);
    }
    let points = match part {
        PathPart::Future => &route.green,
        PathPart::Remaining => &route.blue,
    };
    for point in points {
        point.x.to_bits().hash(&mut hash);
        point.y.to_bits().hash(&mut hash);
        point.z.to_bits().hash(&mut hash);
    }
    hash.finish()
}

fn route_look(part: PathPart, focused: bool) -> PbrLook {
    let (mut base_color, mut emissive) = match part {
        PathPart::Future => (
            LinearRgba::new(0.18, 0.72, 0.38, 0.38),
            LinearRgba::new(0.08, 0.55, 0.24, 1.0),
        ),
        PathPart::Remaining => (
            LinearRgba::new(0.12, 0.45, 0.95, 0.62),
            LinearRgba::new(0.06, 0.30, 0.85, 1.0),
        ),
    };
    if !focused {
        base_color.alpha *= 0.45;
        emissive.red *= 0.35;
        emissive.green *= 0.35;
        emissive.blue *= 0.35;
    }
    PbrLook {
        base_color,
        emissive,
        alpha: SurfaceAlpha::Blend,
        unlit: true,
        double_sided: true,
        no_shadow_cast: true,
        ..default()
    }
}

/// Reconcile only the transient route meshes from the change-built view. It
/// performs no route parsing, binding lookup, or terrain query.
pub(crate) fn sync_route_visual_meshes(
    projection: Res<RouteVisualProjection>,
    q_paths: Query<(Entity, &WaypointPathMesh, &Mesh3d, &ChildOf)>,
    q_grids: Query<&big_space::prelude::Grid>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    let Some(frame_entity) = projection.frame else {
        for (entity, ..) in q_paths.iter() {
            commands.entity(entity).try_despawn();
        }
        return;
    };
    let Ok(grid) = q_grids.get(frame_entity) else {
        for (entity, ..) in q_paths.iter() {
            commands.entity(entity).try_despawn();
        }
        return;
    };
    let mut existing: std::collections::HashMap<
        (Entity, PathPart),
        (Entity, u64, Handle<Mesh>, Entity),
    > = std::collections::HashMap::new();
    for (entity, path, mesh, parent) in q_paths.iter() {
        let key = (path.vessel, path.part);
        let value = (entity, path.signature, mesh.0.clone(), parent.parent());
        // There should be one path per vessel/part. If a prior failed asset
        // replacement left duplicates behind, retire the older one now instead
        // of letting a HashMap silently orphan it from future reconciliation.
        if let Some((stale, ..)) = existing.insert(key, value) {
            commands.entity(stale).try_despawn();
        }
    }
    for (&vessel, route) in &projection.routes {
        for part in [PathPart::Future, PathPart::Remaining] {
            let points = match part {
                PathPart::Future => &route.green,
                PathPart::Remaining => &route.blue,
            };
            let key = (vessel, part);
            let signature = route_mesh_signature(route, part, projection.frame, projection.surface);
            let previous = existing.remove(&key);
            if points.len() < 2 {
                if let Some((entity, _, _, _)) = previous {
                    commands.entity(entity).try_despawn();
                }
                continue;
            }
            if let Some((entity, old_signature, handle, parent)) = previous {
                if old_signature == signature && parent == frame_entity {
                    continue;
                }
                let Some(new_mesh) = build_ribbon_mesh(
                    points,
                    points[0],
                    ROUTE_RIBBON_HALF_WIDTH_M,
                    ROUTE_SURFACE_CLEARANCE_M,
                ) else {
                    commands.entity(entity).try_despawn();
                    continue;
                };
                if parent == frame_entity {
                    if let Some(mut mesh) = meshes.get_mut(&handle) {
                        *mesh = new_mesh;
                        let (cell, local) = grid.translation_to_grid(points[0]);
                        commands.entity(entity).try_insert((
                            route_look(part, route.focused),
                            cell,
                            Transform::from_translation(local),
                            WaypointPathMesh {
                                vessel,
                                signature,
                                part,
                            },
                        ));
                        continue;
                    }
                }
                // The asset may have been removed during a reload, or the path
                // may be attached to an obsolete frame. In either case the old
                // entity must not survive beside the replacement.
                commands.entity(entity).try_despawn();
                let (cell, local) = grid.translation_to_grid(points[0]);
                commands.spawn((
                    Mesh3d(meshes.add(new_mesh)),
                    route_look(part, route.focused),
                    cell,
                    Transform::from_translation(local),
                    GlobalTransform::default(),
                    ChildOf(frame_entity),
                    WaypointPathMesh {
                        vessel,
                        signature,
                        part,
                    },
                ));
                continue;
            }
            let Some(mesh) = build_ribbon_mesh(
                points,
                points[0],
                ROUTE_RIBBON_HALF_WIDTH_M,
                ROUTE_SURFACE_CLEARANCE_M,
            ) else {
                continue;
            };
            let (cell, local) = grid.translation_to_grid(points[0]);
            commands.spawn((
                Mesh3d(meshes.add(mesh)),
                route_look(part, route.focused),
                cell,
                Transform::from_translation(local),
                GlobalTransform::default(),
                ChildOf(frame_entity),
                WaypointPathMesh {
                    vessel,
                    signature,
                    part,
                },
            ));
        }
    }
    for (_, (entity, _, _, _)) in existing {
        commands.entity(entity).try_despawn();
    }
}

/// Route annotations are scene-derived presentation state. Remove them at the
/// scene replacement boundary instead of waiting for the next Update pass to
/// discover that their vessel disappeared.
pub(crate) fn clear_route_visual_projection(
    mut projection: ResMut<RouteVisualProjection>,
    q_paths: Query<Entity, With<WaypointPathMesh>>,
    mut commands: Commands,
) {
    projection.frame = None;
    projection.surface = None;
    projection.routes.clear();
    projection.revision = projection.revision.wrapping_add(1);
    for entity in q_paths.iter() {
        commands.entity(entity).try_despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        has_authored_movement_route, ordered_runtime_marker_entities, resample_polyline,
        route_ribbon_points, route_right, route_tangent, route_visual_state, runtime_route_loops,
        BehaviorXml, ReachedWaypoints, WAYPOINT_MARKER_ASSET,
    };
    use crate::surface_pick::{resolve_cursor_surface, SurfacePickPolicy};
    use bevy::math::DVec3;
    use bevy::prelude::Entity;
    use lunco_autopilot::{
        btcpp_xml::value_to_xml, AutopilotBehaviorSpec, BehaviorSpec, PatrolWaypoint,
    };

    #[test]
    fn analytic_surface_remains_authoritative_when_streamed_terrain_hit_is_removed() {
        let terrain = lunco_terrain_surface::surface_query::SurfaceHit {
            point: lunco_core::coords::GridPos(DVec3::new(0.0, -100.0, 0.0)),
            frame: Entity::PLACEHOLDER,
            distance: 100.0,
            terrain: Entity::PLACEHOLDER,
        };

        let point = resolve_cursor_surface(
            lunco_core::coords::GridPos(DVec3::ZERO),
            DVec3::NEG_Y,
            Some(terrain),
            None,
            None,
            SurfacePickPolicy::Nearest,
        )
        .map(|hit| hit.point.0)
        .expect("the DEM hit is a valid placement point");

        assert_eq!(point, terrain.point.0);
    }
    use lunco_scene_commands::runtime_waypoint::append_runtime_patrol;

    #[test]
    fn runtime_click_creates_and_extends_patrol_without_usd() {
        let first = append_runtime_patrol(None, None, [1.0, 2.0, 3.0]).unwrap();
        let BehaviorSpec::Patrol {
            waypoints,
            speed,
            radius,
            dwell,
        } = first
        else {
            panic!("runtime waypoint must create a patrol");
        };
        assert_eq!(waypoints.len(), 1);
        // Runtime markers carry a 2.5 m authored sensor; the runtime click
        // command uses its 2.0 m arrival radius so the rover must enter the
        // physical trigger rather than merely approach its edge.
        assert_eq!((speed, radius, dwell), (0.6, 2.0, 0.0));

        let current = AutopilotBehaviorSpec::new(BehaviorSpec::Patrol {
            waypoints: vec![PatrolWaypoint::at([1.0, 2.0, 3.0])],
            speed: 0.25,
            radius: 1.5,
            dwell: 2.0,
        });
        let extended = append_runtime_patrol(Some(&current), None, [4.0, 5.0, 6.0]).unwrap();
        let BehaviorSpec::Patrol {
            waypoints,
            speed,
            radius,
            dwell,
        } = extended
        else {
            panic!("runtime waypoint must preserve patrol shape");
        };
        assert_eq!(waypoints.len(), 2);
        assert_eq!((speed, radius, dwell), (0.25, 1.5, 2.0));
    }

    #[test]
    fn runtime_click_does_not_replace_a_different_tree() {
        let current = AutopilotBehaviorSpec::new(BehaviorSpec::Brake);
        let err = append_runtime_patrol(Some(&current), None, [1.0, 0.0, 1.0]).unwrap_err();
        assert!(err.contains("non-patrol"));
    }

    #[test]
    fn runtime_route_requires_complete_marker_bindings_in_order() {
        let vessel = Entity::from_bits(10);
        let first = Entity::from_bits(11);
        let second = Entity::from_bits(12);
        let mut markers = std::collections::HashMap::new();
        markers.insert((vessel, 0), first);
        markers.insert((vessel, 1), second);

        assert_eq!(
            ordered_runtime_marker_entities(vessel, 2, &markers),
            Some(vec![first, second])
        );

        markers.remove(&(vessel, 1));
        assert_eq!(
            ordered_runtime_marker_entities(vessel, 2, &markers),
            None,
            "a partial binding set must not produce mismatched route state"
        );
    }

    #[test]
    fn pending_authored_xml_is_a_route_until_its_spec_is_derived() {
        let xml = BehaviorXml(
            value_to_xml(&serde_json::json!({
                "kind": "sequence",
                "children": [{"kind": "drive_to", "target": "/Route/W0"}]
            }))
            .unwrap(),
        );
        assert!(has_authored_movement_route(Some(&xml), None));
        assert!(!has_authored_movement_route(
            None,
            Some(&AutopilotBehaviorSpec::new(BehaviorSpec::Brake))
        ));
        assert!(has_authored_movement_route(
            None,
            Some(&AutopilotBehaviorSpec::new(BehaviorSpec::DriveTo {
                target: [1.0, 0.0, 0.0],
                speed: 0.5,
                radius: 1.0,
            }))
        ));
    }

    #[test]
    fn malformed_authored_xml_does_not_fall_back_to_a_stale_runtime_route() {
        let malformed = BehaviorXml("<broken".to_string());
        let stale_runtime = AutopilotBehaviorSpec::new(BehaviorSpec::Patrol {
            waypoints: vec![PatrolWaypoint::at([1.0, 0.0, 0.0])],
            speed: 0.5,
            radius: 1.0,
            dwell: 0.0,
        });

        assert!(
            lunco_autopilot::usd_tree::authored_route_metadata(&malformed.0).is_err(),
            "malformed authored data must remain an explicit parse failure"
        );
        assert!(!has_authored_movement_route(
            Some(&malformed),
            Some(&stale_runtime)
        ));
    }

    #[test]
    fn runtime_waypoint_uses_the_catalog_asset_identity() {
        assert_eq!(
            lunco_assets::engine_asset_rel(WAYPOINT_MARKER_ASSET),
            lunco_assets::engine_asset_rel("vessels/markers/waypoint.usda")
        );
    }

    #[test]
    fn route_ribbon_keeps_complete_ordered_route_green_and_advances_blue_leg() {
        let w0 = DVec3::new(0.0, 0.0, 0.0);
        let w1 = DVec3::new(10.0, 0.0, 0.0);
        let w2 = DVec3::new(20.0, 0.0, 0.0);

        let (green_before, blue_before) =
            route_ribbon_points(&[(w0, false), (w1, false), (w2, false)], Some(0));
        assert_eq!(green_before, vec![w0, w1, w2]);
        assert!(blue_before.is_empty());

        let (green_after, blue_after) =
            route_ribbon_points(&[(w0, true), (w1, false), (w2, false)], Some(1));
        assert_eq!(green_after, vec![w0, w1, w2]);
        assert_eq!(blue_after, vec![w0, w1]);
    }

    #[test]
    fn straight_route_is_resampled_before_surface_projection() {
        let points = resample_polyline(&[DVec3::ZERO, DVec3::new(10.0, 100.0, 0.0)], 2.0);

        assert_eq!(points.len(), 6);
        assert_eq!(points.first(), Some(&DVec3::ZERO));
        assert_eq!(points.last(), Some(&DVec3::new(10.0, 100.0, 0.0)));
        assert_eq!(points[3], DVec3::new(6.0, 60.0, 0.0));
    }

    #[test]
    fn turnaround_uses_the_incoming_route_direction_for_ribbon_orientation() {
        let points = [
            DVec3::new(0.0, 10.0, 0.0),
            DVec3::new(10.0, 20.0, 0.0),
            DVec3::new(0.0, 30.0, 0.0),
        ];

        assert_eq!(route_tangent(&points, 1), DVec3::X);

        let rights = points
            .iter()
            .enumerate()
            .scan(None, |previous, (index, _)| {
                let right = route_right(route_tangent(&points, index), *previous);
                *previous = Some(right);
                Some(right)
            })
            .collect::<Vec<_>>();
        assert!(rights[1].dot(rights[2]) > 0.0);
    }

    #[test]
    fn route_target_extraction_only_accepts_navigation_leaves() {
        let xml = BehaviorXml(
            value_to_xml(&serde_json::json!({
                "kind": "sequence",
                "children": [
                    {"kind": "drive_to", "target": "/Route/W0"},
                    {"kind": "run_tool", "target": "camera"}
                ]
            }))
            .unwrap(),
        );

        assert_eq!(
            lunco_autopilot::usd_tree::authored_route_metadata(&xml.0)
                .unwrap()
                .targets,
            vec!["/Route/W0"]
        );
    }

    #[test]
    fn route_ribbon_ignores_an_stale_cursor_after_route_resolution() {
        let points = [
            (DVec3::ZERO, true),
            (DVec3::X * 10.0, false),
            (DVec3::X * 20.0, false),
        ];
        let (_, blue) = route_ribbon_points(&points, Some(99));
        assert_eq!(blue, vec![DVec3::ZERO, DVec3::X * 10.0]);
    }

    #[test]
    fn route_ribbon_has_no_blue_leg_when_a_one_way_route_is_done() {
        let points = [(DVec3::ZERO, true), (DVec3::X * 10.0, true)];
        let (_, blue) = route_ribbon_points(&points, None);
        assert!(blue.is_empty());
    }

    #[test]
    fn first_authored_leg_has_no_blue_segment() {
        let points = [(DVec3::ZERO, false), (DVec3::X * 10.0, false)];
        let (_, blue) = route_ribbon_points(&points, Some(0));
        assert!(blue.is_empty());
    }

    #[test]
    fn route_loop_detection_uses_authored_tree_shape() {
        let sequence = BehaviorXml(
            value_to_xml(&serde_json::json!({
                "kind": "sequence",
                "children": [{"kind": "drive_to", "target": "/Route/W0"}]
            }))
            .unwrap(),
        );
        let forever = BehaviorXml(
            value_to_xml(&serde_json::json!({
                "kind": "forever",
                "child": {
                    "kind": "sequence",
                    "children": [{"kind": "drive_to", "target": "/Route/W0"}]
                }
            }))
            .unwrap(),
        );
        assert!(
            !lunco_autopilot::usd_tree::authored_route_metadata(&sequence.0)
                .unwrap()
                .loops
        );
        assert!(
            lunco_autopilot::usd_tree::authored_route_metadata(&forever.0)
                .unwrap()
                .loops
        );
    }

    #[test]
    fn route_loop_detection_uses_runtime_spec_kind_not_waypoint_count() {
        let one_way = AutopilotBehaviorSpec::new(BehaviorSpec::Sequence {
            children: vec![BehaviorSpec::DriveTo {
                target: [1.0, 0.0, 2.0],
                speed: 0.5,
                radius: 1.0,
            }],
        });
        let patrol = AutopilotBehaviorSpec::new(BehaviorSpec::Patrol {
            waypoints: vec![PatrolWaypoint::at([1.0, 0.0, 2.0])],
            speed: 0.5,
            radius: 1.0,
            dwell: 0.0,
        });
        assert!(!runtime_route_loops(&one_way));
        assert!(runtime_route_loops(&patrol));
    }

    #[test]
    fn route_progress_does_not_mark_unobserved_waypoints_visited() {
        let targets = vec!["/Route/W0".to_string(), "/Route/W1".to_string()];
        let state = route_visual_state(&targets, None, Some(1), false, false);
        assert_eq!(state.visited, vec![false, false]);
        assert_eq!(state.active_index, Some(1));
    }

    #[test]
    fn route_progress_maps_stripped_cursor_to_the_full_authored_route() {
        let targets = vec![
            "/Route/W0".to_string(),
            "/Route/W1".to_string(),
            "/Route/W2".to_string(),
        ];
        let reached = std::collections::HashSet::from(["/Route/W0".to_string()]);
        let state = route_visual_state(
            &targets,
            Some(&ReachedWaypoints(reached)),
            Some(0),
            false,
            false,
        );
        assert_eq!(state.visited, vec![true, false, false]);
        assert_eq!(
            state.active_index,
            Some(1),
            "runtime cursor 0 is the first remaining leg, W1, not authored W0"
        );
    }

    #[test]
    fn route_progress_keeps_the_current_leg_after_all_session_arrivals() {
        let targets = vec!["/Route/W0".to_string(), "/Route/W1".to_string()];
        let mut reached = std::collections::HashSet::new();
        reached.insert("/Route/W0".to_string());
        reached.insert("/Route/W1".to_string());
        let state = route_visual_state(
            &targets,
            Some(&ReachedWaypoints(reached)),
            Some(0),
            false,
            true,
        );
        assert_eq!(state.visited, vec![true, true]);
        assert_eq!(state.active_index, Some(0));
    }

    #[test]
    fn route_progress_stops_the_active_leg_after_a_one_way_route_completes() {
        let targets = vec!["/Route/W0".to_string(), "/Route/W1".to_string()];
        let mut reached = std::collections::HashSet::new();
        reached.insert("/Route/W0".to_string());
        reached.insert("/Route/W1".to_string());
        let state = route_visual_state(
            &targets,
            Some(&ReachedWaypoints(reached)),
            Some(2),
            false,
            false,
        );
        assert_eq!(state.visited, vec![true, true]);
        assert_eq!(state.active_index, None);
    }

    #[test]
    fn route_progress_completion_keeps_unvisited_waypoints_unvisited() {
        let targets = vec!["/Route/W0".to_string(), "/Route/W1".to_string()];
        let mut reached = std::collections::HashSet::new();
        reached.insert("/Route/W0".to_string());
        let state = route_visual_state(
            &targets,
            Some(&ReachedWaypoints(reached)),
            None,
            true,
            false,
        );
        assert_eq!(state.visited, vec![true, false]);
        assert_eq!(state.active_index, None);
    }

    #[test]
    fn waypoint_progress_requires_exact_composed_prim_identity() {
        let targets = vec!["/World/Route/W0".to_string()];
        let reached = ReachedWaypoints(std::collections::HashSet::from(["/Route/W0".to_string()]));

        let progress = route_visual_state(&targets, Some(&reached), Some(0), false, false);

        assert_eq!(progress.visited, vec![false]);
    }
}
