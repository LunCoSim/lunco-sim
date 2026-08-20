//! Place-waypoint intent + primary pointer action — drop a mission waypoint by
//! **authoring a USD prim**.
//!
//! (Design: `docs/architecture/waypoints-in-usd.md`.)
//!
//! There is no checkpoint domain. A waypoint is an ordinary prim referencing
//! `vessels/markers/waypoint.usda`, and the vessel's BT.CPP mission
//! (the `info:sourceCode` of its `LunCoProgramAPI "Mission"` child) gains a `drive_to`
//! leaf that names it by path. Both edits go
//! through the one authoring funnel, [`ApplyUsdOp`] — so the waypoint is journaled,
//! undoable, persisted to `.usda`, and replicated exactly like every other prim, with
//! no new command verb.
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
    route_is_smooth, set_route_smooth, set_waypoint_dwell, BehaviorXml, ReachedWaypoints,
    TargetBindings,
};
use lunco_controller::{ControllerLink, SimulatedIntents};
use lunco_core::commands::SessionId;
use lunco_core::session::SessionRegistry;
use lunco_core::{
    paths::prim_path_matches, Avatar, EguiFocus, GlobalEntityId, InputPorts, IntentState,
    SpawnToolActive, TerrainToolActive, UserIntent,
};
use lunco_doc_bevy::DocumentRegistry;
use lunco_render::{PbrLook, SceneCamera, SurfaceAlpha};
use lunco_scene_commands::runtime_waypoint::RuntimeWaypointBinding;
use lunco_usd::commands::{ApplyUsdOp, ApplyUsdOps};
use lunco_usd::document::UsdDocument;
use lunco_usd::document::{
    LayerId, UsdOp, WAYPOINT_MARKER_ASSET, WAYPOINT_MISSION_PROGRAM, WAYPOINT_ROUTE_SCOPE,
};
use lunco_usd_bevy::{CanonicalStages, SdfPath, UsdPrimPath, UsdRead};
use serde_json::Value;

use lunco_scene_commands::runtime_waypoint::runtime_waypoint_key;
use lunco_scene_commands::SelectedEntities;

fn report_waypoint_failure(commands: &mut Commands, message: impl Into<String>) {
    let message = message.into();
    warn!("[waypoint] {message}");
    lunco_core::trigger_error(commands, "waypoint-edit-failed", message);
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

/// A waypoint edit waiting on a ground click, armed from the context menu.
///
/// Addressed by DOCUMENT + PRIM PATH, never by the vessel's `Entity`. Authoring the
/// arming edit can recompose the stage, which despawns and respawns the vessel — a
/// captured `Entity` is stale by the time the ground click lands, and the placement
/// then failed silently ("vessel has no BehaviorXml/UsdPrimPath"). The path is stable
/// across recomposition, so the owning vessel is re-resolved at click time from the
/// mission that names this waypoint (see [`vessel_for_target`]).
#[derive(Debug)]
pub struct PendingPlacement {
    /// The document that owns the marker (and the mission that names it).
    pub doc: lunco_doc::DocumentId,
    /// The waypoint MARKER prim path: the leg to move, or the leg to insert after.
    pub coord_key: String,
    pub mode: PlacementMode,
}

/// Armed "click the ground to place" mode. While `Some`, the next scene click is
/// consumed by [`on_scene_click_place_waypoint`] instead of possessing/selecting —
/// `sync_waypoint_tool_active` mirrors this into [`lunco_core::WaypointToolActive`],
/// which is what the possession/selection observers actually honour (every global
/// `Pointer<Click>` observer sees the same click; `propagate(false)` stops bubbling,
/// not siblings).
#[derive(Resource, Default)]
pub struct WaypointPlacement(pub Option<PendingPlacement>);

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
        info!("[waypoint] cancelled {:?} of '{}'", p.mode, p.coord_key);
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
    pub q_autopilots: Query<'w, 's, &'static lunco_autopilot::Autopilot>,
}

/// Resolve the pointer to a point on the ground in **WORLD** (grid-absolute) space —
/// the one spelling of "where did the user click?" for the waypoint editor, shared by
/// the intent-driven waypoint drop and the Move / Insert-after placement click.
///
/// Casts through the active camera against BOTH the DEM oracle (ground truth over open
/// terrain, where the band-limited collider ring rounds a crater bowl) and the physics
/// colliders (structures/props), taking the nearer hit — the same pairing
/// `spawn::on_scene_click_spawn` uses. [`GridSpatialQuery`] converts the winning
/// render-frame hit to grid-absolute world coordinates.
fn pick_ground_world(
    frame: &WaypointClickFrame,
    surface: &lunco_terrain_surface::GridSurfaceQuery,
    raycaster: &lunco_physics::GridSpatialQuery,
    egui_focus: &EguiFocus,
    pointer: Vec2,
) -> Option<DVec3> {
    let (camera, cam_gtf, _) = frame.cameras.iter().find(|(camera, _, target)| {
        camera.is_active && matches!(target, bevy::camera::RenderTarget::Window(_))
    })?;
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
    let dir = direction.as_dvec3();
    let phys_hit = raycaster
        .cast_ray_grid(
            origin,
            direction,
            1.0e6,
            false,
            &avian3d::prelude::SpatialQueryFilter::default(),
        )
        // Collider tiles are band-limited, streamed approximations.  A tile
        // hit must never replace the DEM's absolute surface in authored
        // waypoint coordinates; static props remain eligible physics hits.
        .filter(|hit| !frame.terrain_colliders.contains(hit.entity));
    let phys = phys_hit.map(|hit| hit.distance);
    let terr = surface.raycast(origin, direction, 1.0e6);
    select_ground_point(origin.0, dir, phys, terr)
}

/// Choose the nearest authored placement surface after streamed terrain
/// colliders have been removed from the physics candidate set.
///
/// Keeping this small decision pure makes the precedence contract explicit:
/// the analytic DEM is the terrain authority, while a real physics prop that
/// lies above it remains selectable.
fn select_ground_point(
    origin: DVec3,
    direction: DVec3,
    physics_distance: Option<f64>,
    terrain: Option<lunco_terrain_surface::surface_query::SurfaceHit>,
) -> Option<DVec3> {
    match (physics_distance, terrain) {
        (Some(pd), Some(hit)) => Some(if hit.distance <= pd {
            hit.point.0
        } else {
            origin + direction * pd
        }),
        (Some(pd), None) => Some(origin + direction * pd),
        (None, Some(hit)) => Some(hit.point.0),
        (None, None) => None,
    }
}

/// Global `Pointer<Click>` observer: the `PlaceWaypoint` input intent paired with
/// the primary pointer action drops a waypoint prim for the selected vessel and
/// appends the matching `drive_to` leaf to its mission. The Alt binding lives in
/// `assets/config/keybindings.json`; this handler never inspects a raw key.
///
/// Stands down when the spawn / terrain-sculpt tool is armed, and when egui owns the
/// pointer (the authoritative gate). The semantic waypoint intent is excluded from
/// the possession observer, so this click does not also possess or follow the hit.
pub fn on_scene_click_checkpoint(
    mut click: On<Pointer<Click>>,
    egui_focus: Res<EguiFocus>,
    spawn_tool: Res<SpawnToolActive>,
    terrain_tool: Res<TerrainToolActive>,
    selected: Res<SelectedEntities>,
    avatars: Query<(Entity, &IntentState), With<Avatar>>,
    simulated_intents: Option<Res<SimulatedIntents>>,
    q_link: Query<&ControllerLink>,
    frame: WaypointClickFrame,
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
    // A plain primary click possesses/selects; the semantic PlaceWaypoint intent
    // paired with it drops a waypoint. The input map supplies that intent, so
    // rebinding the gesture changes this path without editor-only key handling.
    let place_waypoint_held = avatars.iter().any(|(avatar, intents)| {
        intents.pressed(&UserIntent::PlaceWaypoint)
            || simulated_intents.as_ref().is_some_and(|sim| {
                sim.0
                    .get(&avatar)
                    .is_some_and(|set| set.contains(&UserIntent::PlaceWaypoint))
            })
    });
    if !place_waypoint_held {
        return;
    }

    // Now that this is a waypoint-intent click, stop propagation.
    click.propagate(false);

    // Default to the possessed vessel first, then fall back to the selected one, then fall back to the first vessel with a mission tree in the scene
    let possessed_vessel = avatars
        .iter()
        .next()
        .and_then(|(av, _)| q_link.get(av).ok().map(|link| link.vessel_entity));
    let raw_vessel = possessed_vessel
        .or_else(|| selected.primary())
        .or_else(|| vessels.q_autopilots.iter().map(|ap| ap.vessel).next())
        .or_else(|| vessels.q_inputs.iter().next())
        .or_else(|| vessels.q_xml.iter().next().map(|(e, _)| e));
    let Some(mut vessel) = raw_vessel else {
        info!("[waypoint] click ignored: no vessel found in scene");
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
    let root = vessel_prim
        .path
        .split('/')
        .nth(1) // first non-empty component after the leading '/'
        .map(|p| format!("/{p}"))
        .unwrap_or_else(|| {
            lunco_usd_bevy::layer_default_prim(host.document().data())
                .map(|p| format!("/{p}"))
                .unwrap_or_else(|| "/".to_string())
        });
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
    if placement.0.is_none() || click.button != PointerButton::Primary {
        return;
    }
    if egui_focus.wants_pointer {
        info!("[waypoint] placement: ignoring click, egui owns the pointer (menu?)");
        return; // clicking the menu itself, not the ground
    }
    click.propagate(false);
    let Some(pending) = placement.0.take() else {
        return;
    };
    info!(
        "[waypoint] placement: consuming click for {:?} of '{}'",
        pending.mode, pending.coord_key
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
    let doc = pending.doc;

    // MOVE repositions the MARKER, and touches the mission not at all: the leg
    // targets the prim by path, and the prim's pose is the prim's business. The
    // coordinate-in-the-XML spelling had to rewrite the whole mission to drag a
    // pin, which is why a move could reorder or lose a leg.
    //
    // It therefore needs NOTHING but the marker path and its document — no vessel
    // lookup, so a stage recomposition between arming the move and clicking the
    // ground (which respawns the vessel entity) cannot strand it.
    if pending.mode == PlacementMode::Move {
        info!("[waypoint] Move → {} to {:?}", pending.coord_key, world);
        commands.trigger(ApplyUsdOp {
            doc,
            op: UsdOp::SetTranslate {
                edit_target: LayerId::root(),
                path: pending.coord_key,
                value: [world.x, world.y, world.z],
            },
        });
        return;
    }

    // INSERT-AFTER edits the mission, so it does need the vessel — resolved HERE,
    // from the route that names this waypoint, rather than from an entity captured
    // when the menu was open.
    let Some((_vessel, xml, vessel_prim)) = vessel_for_target(&q_vessel, &pending.coord_key) else {
        report_waypoint_failure(
            &mut commands,
            format!("No vessel mission refers to '{}'", pending.coord_key),
        );
        return;
    };
    let root = vessel_prim
        .path
        .split('/')
        .nth(1)
        .map(|p| format!("/{p}"))
        .unwrap_or_else(|| "/".to_string());
    let Some(host) = doc_ctx.usd_registry.host(doc) else {
        report_waypoint_failure(
            &mut commands,
            format!("No USD authoring host exists for document {doc:?}"),
        );
        return;
    };
    let (new_target, mut ops) = author_marker_ops(host, &root, world, &canonical, vessel_prim);
    let edited = insert_waypoint_after(&xml.0, &pending.coord_key, &new_target);
    match edited {
        Ok(new_xml) => {
            info!("[waypoint] {:?} → {}", pending.mode, new_target);
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
    // like it never opened. Same correction `draw_waypoint_overlay` applies.
    let origin = ctx.content_rect().min.to_vec2();
    let pos = egui::pos2(menu_state.position.x, menu_state.position.y) + origin;
    let mut open = true;
    // Buffer the dwell outside the closure: the closure needs `&mut` to it while
    // `menu_state` is still read afterwards.
    let mut dwell = menu_state.dwell;
    let mut smooth = route_is_smooth(&xml.0);
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
                    placement.0 = Some(PendingPlacement {
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
                    placement.0 = Some(PendingPlacement {
                        doc,
                        coord_key: marker_target.clone(),
                        mode: PlacementMode::InsertAfter,
                    });
                    open = false;
                }
                if ui.button("❌  Delete").clicked() {
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

fn get_waypoint_positions(
    xml: &str,
    bindings: &TargetBindings,
    poses: &lunco_physics::SimulationPoseQuery,
) -> Vec<(String, DVec3)> {
    let Ok(value) = lunco_autopilot::btcpp_xml::xml_to_value(xml) else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    collect_targets(&value, &mut targets);

    let mut positions = Vec::new();
    for t in targets {
        // 1. Try to parse as "x;y;z" coordinate triple
        let parts: Vec<&str> = t.split(';').collect();
        if parts.len() == 3 {
            if let (Ok(x), Ok(y), Ok(z)) = (
                parts[0].trim().parse::<f64>(),
                parts[1].trim().parse::<f64>(),
                parts[2].trim().parse::<f64>(),
            ) {
                positions.push((t, DVec3::new(x, y, z)));
                continue;
            }
        }
        // 2. Try to resolve as USD prim path
        if let Some(&entity) = bindings.0.get(&t) {
            if let Some(pos) = poses.position(entity) {
                positions.push((t, pos.0));
            }
        }
    }
    positions
}

fn get_runtime_waypoint_positions(spec: &lunco_autopilot::AutopilotBehaviorSpec) -> Vec<DVec3> {
    spec.patrol_waypoints()
        .unwrap_or_default()
        .iter()
        .map(|waypoint| DVec3::new(waypoint.pos[0], waypoint.pos[1], waypoint.pos[2]))
        .collect()
}

fn route_targets(
    xml: Option<&BehaviorXml>,
    spec: Option<&lunco_autopilot::AutopilotBehaviorSpec>,
) -> Vec<String> {
    if let Some(xml) = xml {
        let Ok(value) = lunco_autopilot::btcpp_xml::xml_to_value(&xml.0) else {
            return Vec::new();
        };
        let mut targets = Vec::new();
        collect_targets(&value, &mut targets);
        if !targets.is_empty() {
            return targets;
        }
    }
    spec.and_then(|spec| spec.patrol_waypoints())
        .map(|waypoints| (0..waypoints.len()).map(runtime_waypoint_key).collect())
        .unwrap_or_default()
}

fn route_loops(
    xml: Option<&BehaviorXml>,
    spec: Option<&lunco_autopilot::AutopilotBehaviorSpec>,
) -> bool {
    if let Some(xml) = xml {
        // XML is the authored route and therefore owns topology when both the
        // authored tree and its derived runtime spec are present.
        return lunco_autopilot::btcpp_xml::xml_to_value(&xml.0)
            .ok()
            .map(|value| authored_route_loops(&value))
            .unwrap_or(false);
    }

    spec.is_some_and(|spec| match &spec.0 {
        lunco_autopilot::BehaviorSpec::Forever { .. }
        | lunco_autopilot::BehaviorSpec::Patrol { .. } => true,
        lunco_autopilot::BehaviorSpec::Repeat { times, .. } => *times > 1,
        _ => false,
    })
}

fn authored_route_loops(value: &Value) -> bool {
    match value.get("kind").and_then(Value::as_str) {
        Some("forever" | "patrol") => true,
        Some("repeat") => value
            .get("times")
            .and_then(Value::as_u64)
            .is_some_and(|times| times > 1),
        _ => false,
    }
}

fn runtime_marker_is_visited(
    reached: Option<&std::collections::HashSet<String>>,
    index: usize,
) -> bool {
    reached.is_some_and(|reached| reached.contains(&runtime_waypoint_key(index)))
}

/// Runtime visual progress for one route. Only the collision-backed set is
/// evidence that a waypoint was visited. The behavior cursor identifies the
/// active leg, but never changes visited state.
#[derive(Clone, Debug, Default)]
struct RouteVisualState {
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
        .map(|target| {
            reached
                .is_some_and(|reached| reached.0.iter().any(|done| prim_path_matches(done, target)))
        })
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
            // authored target again.
            .or_else(|| looping.then_some(cursor_index).flatten())
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

fn collect_targets(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            if let Some(Value::String(s)) = map.get("target") {
                out.push(s.clone());
            }
            for child in map.values() {
                collect_targets(child, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_targets(i, out)),
        _ => {}
    }
}

/// Single egui overlay that draws both waypoint labels (numbers) and route
/// lines in screen space.
///
/// Uses [`lunco_physics::SimulationPoseQuery`] for authoritative body-fixed
/// positions: f64 Avian poses for physical targets and composed BigSpace poses
/// for non-physical markers.
pub fn draw_waypoint_overlay(
    q_vessels: Query<
        (
            Entity,
            Option<&BehaviorXml>,
            Option<&lunco_autopilot::AutopilotBehaviorSpec>,
            Option<&TargetBindings>,
            Option<&ReachedWaypoints>,
        ),
        With<UsdPrimPath>,
    >,
    selected: Res<SelectedEntities>,
    q_camera: Query<(Entity, &Camera, &GlobalTransform), (With<Camera3d>, With<SceneCamera>)>,
    q_avatar_cam: Query<Entity, With<Avatar>>,
    q_autopilots: Query<(
        &lunco_autopilot::Autopilot,
        Option<&lunco_autopilot::AutopilotBehavior>,
        Option<&lunco_autopilot::AutopilotExecutionState>,
    )>,
    q_parents: Query<&ChildOf>,
    poses: lunco_physics::SimulationPoseQuery,
    q_link: Query<&ControllerLink>,
    scene_viewport: Option<Res<lunco_core::SceneViewport>>,
    panel_rects: Option<Res<lunco_workbench::PanelRects>>,
    mut egui_ctx: bevy_egui::EguiContexts,
    theme: Option<Res<lunco_theme::Theme>>,
) {
    if scene_viewport.is_some_and(|viewport| !viewport.visible) {
        return;
    }
    let theme = theme
        .map(|t| t.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);
    // Prefer the avatar camera (the one the player looks through); fall back
    // to the first active Camera3d if no avatar is spawned yet.
    let cam_result = q_avatar_cam
        .iter()
        .next()
        .and_then(|av| q_camera.get(av).ok())
        .or_else(|| q_camera.iter().find(|(_, cam, _)| cam.is_active));
    let Some((cam_entity, camera, cam_gtf)) = cam_result else {
        return;
    };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let origin = ctx.content_rect().min.to_vec2();
    let clip_rect = panel_rects
        .as_ref()
        .and_then(|rects| rects.egui_rect(lunco_workbench::VIEWPORT_PANEL_ID, ctx))
        .unwrap_or_else(|| ctx.content_rect());

    // Camera world position for distance-based sizing.
    let Some(cam_world) = poses.position(cam_entity).map(|p| p.0) else {
        return;
    };

    // Append to the root background before WorkbenchRenderSet so labels are
    // deterministically below every normal egui window and dock panel.
    let painter = ctx
        .layer_painter(egui::LayerId::background())
        .with_clip_rect(clip_rect);

    let vessel_entities: std::collections::HashSet<Entity> =
        q_vessels.iter().map(|(entity, ..)| entity).collect();
    let primary_selected = route_owner(selected.primary(), &q_parents, &vessel_entities);
    let possessed_vessel = route_owner(
        q_avatar_cam
            .iter()
            .next()
            .and_then(|av| q_link.get(av).ok().map(|link| link.vessel_entity)),
        &q_parents,
        &vessel_entities,
    );

    for (vessel, xml, spec, bindings, reached) in q_vessels.iter() {
        let empty_bindings = TargetBindings::default();
        let bindings = bindings.unwrap_or(&empty_bindings);

        // EVERY route is labelled, not just the focused vessel's. A waypoint is an
        // object in the scene you edit by right-clicking it, so hiding the numbers
        // (and, next door, the ribbon) until the vessel was possessed or selected
        // made the whole waypoint UI look like it only worked while driving.
        // Focus now only DIMS: the route you are working on stays the loud one.
        let is_possessed = Some(vessel) == possessed_vessel;
        let is_selected = Some(vessel) == primary_selected;
        let focused = is_possessed || is_selected;

        let label_color = theme.tokens.text;

        let targets = route_targets(xml, spec);
        let authored_route = xml.is_some() && !targets.is_empty();
        let looping = route_loops(xml, spec);
        let wp_positions = if authored_route {
            get_waypoint_positions(&xml.expect("authored route has XML").0, bindings, &poses)
        } else {
            spec.map(get_runtime_waypoint_positions)
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .map(|(index, position)| (runtime_waypoint_key(index), position))
                .collect()
        };
        let (cursor, completed) = route_execution(vessel, &q_autopilots);
        let progress = route_visual_state(&targets, reached, cursor, completed, looping);

        // Collect screen-space points for each waypoint that is in front of the camera.
        struct WpScreen {
            screen: egui::Pos2,
            index: usize,
            distance: f64,
            visited: bool,
        }
        let mut wp_screens: Vec<WpScreen> = Vec::with_capacity(wp_positions.len());

        for (i, (target, wp_world)) in wp_positions.into_iter().enumerate() {
            let distance = (wp_world - cam_world).length();

            // Convert to camera-relative Vec3 for projection.
            let cam_relative = (wp_world - cam_world).as_vec3();
            let world_f32 = cam_gtf.translation() + cam_relative;

            let Ok(viewport) = camera.world_to_viewport(cam_gtf, world_f32) else {
                continue;
            };
            let screen = egui::pos2(viewport.x, viewport.y) + origin;
            let visited = progress.visited.get(i).copied().unwrap_or_else(|| {
                reached.is_some_and(|reached| {
                    reached
                        .0
                        .iter()
                        .any(|done| prim_path_matches(done, &target))
                })
            });

            wp_screens.push(WpScreen {
                screen,
                index: i,
                distance,
                visited,
            });
        }

        // NOTE: the route LINE is not drawn here. A screen-space overlay stroke has no
        // depth, so it painted straight over terrain and over other waypoints and read
        // as a buggy, overlapping gizmo. The path is real 3D geometry instead — see
        // `sync_waypoint_path_mesh`, which builds a ground-hugging ribbon that occludes
        // correctly. Only the NUMBER labels stay in egui, where screen-space is right.
        // Draw labels above each waypoint.
        for wp in &wp_screens {
            let scale = (30.0 / wp.distance.max(1.0) as f32).clamp(0.4, 2.5);
            let font_size = (18.0 * scale).max(8.0);

            let fade = if wp.distance < 30.0 {
                1.0f32
            } else {
                (1.0 - ((wp.distance as f32 - 30.0) / 200.0)).clamp(0.1, 1.0)
            };
            // Another vessel's route is context, not the subject — same labels, quieter.
            let fade = if focused { fade } else { fade * 0.5 };

            let alpha = (255.0 * fade) as u8;
            let text = if wp.visited {
                format!("✓{}", wp.index + 1)
            } else {
                format!("{}", wp.index + 1)
            };
            let font = egui::FontId::proportional(font_size);
            let label = if wp.visited {
                theme.tokens.success
            } else {
                label_color
            };
            let tc = egui::Color32::from_rgba_unmultiplied(label.r(), label.g(), label.b(), alpha);

            let galley = painter.layout_no_wrap(text, font, tc);
            let size = galley.size();
            let top_left = wp.screen - egui::vec2(size.x * 0.5, size.y + 8.0);

            let bg = egui::Rect::from_min_size(top_left, size).expand2(egui::vec2(4.0, 2.0));
            let backdrop = theme.tokens.overlay_backdrop;
            painter.rect_filled(
                bg,
                3.0,
                egui::Color32::from_rgba_unmultiplied(
                    backdrop.r(),
                    backdrop.g(),
                    backdrop.b(),
                    (f32::from(backdrop.a()) * fade) as u8,
                ),
            );
            painter.galley(top_left, galley, tc);
        }
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
        let Ok(value) = lunco_autopilot::btcpp_xml::xml_to_value(&xml.0) else {
            continue;
        };
        let mut targets = Vec::new();
        collect_targets(&value, &mut targets);
        if let Some(index) = targets.iter().position(|t| t == path) {
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
    q_vessel.iter().find(|(_, xml, _)| {
        let Ok(value) = lunco_autopilot::btcpp_xml::xml_to_value(&xml.0) else {
            return false;
        };
        let mut targets = Vec::new();
        collect_targets(&value, &mut targets);
        targets.iter().any(|t| t == target)
    })
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
    // `AddPrim` on an existing prim is a rejection, not a merge.
    if !composed_prim_exists(canonical, vessel_prim, &route_scope)
        && !prim_exists(host, &route_scope)
    {
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
    // The picked point is grid-absolute, the frame authored translates are in
    // (`persist_move_to_runtime_layer` writes a world position straight into
    // `SetTranslate`).
    ops.push(UsdOp::SetTranslate {
        edit_target: LayerId::root(),
        path: marker_path.clone(),
        value: [at.x, at.y, at.z],
    });
    (marker_path, ops)
}

/// Join a parent prim path and a child name, handling the stage root (`"/"`).
fn join_prim(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
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
/// layer. Traverse authors its mission inside the selected site variant; an
/// authored-layer lookup cannot see that composed prim and used to create a
/// duplicate Mission on the first waypoint click, forcing a rover re-projection.
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

/// Whether `path` is already authored in either layer of the document.
fn prim_exists(
    host: &lunco_doc::DocumentHost<lunco_usd::document::UsdDocument>,
    path: &str,
) -> bool {
    let Ok(sdf) = lunco_usd_bevy::SdfPath::new(path) else {
        return false;
    };
    host.document().data().spec(&sdf).is_some()
        || host.document().runtime_data().spec(&sdf).is_some()
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
    const DRIVE: [lunco_core::UserIntent; 7] = [
        MoveForward,
        MoveBackward,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        Thrust,
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
    avatars: Query<(Entity, &IntentState), With<Avatar>>,
    q_link: Query<&ControllerLink>,
    mut commands: Commands,
) {
    if egui_focus.wants_keyboard {
        return;
    }
    if let Some((avatar, _)) = avatars
        .iter()
        .find(|(_, intent)| intent.just_pressed(&UserIntent::Action))
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
        Has<lunco_autopilot::usd_tree::BehaviorXml>,
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
            .is_ok_and(|(has_xml, spec)| has_authored_movement_route(has_xml, spec));
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
        Has<lunco_autopilot::usd_tree::BehaviorXml>,
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
            .is_ok_and(|(has_xml, spec)| has_authored_movement_route(has_xml, spec));
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
    has_xml: bool,
    spec: Option<&lunco_autopilot::AutopilotBehaviorSpec>,
) -> bool {
    spec.map_or(has_xml, |spec| spec.0.has_motion())
}

register_commands!(
    on_start_autopilot,
    on_toggle_autopilot,
    on_cancel_waypoint_edit
);

// ── Route ribbon (real 3D geometry, not a screen-space overlay) ───────────────

/// Half-width (world units) of the route ribbon — a thin drawn line, not a road.
const PATH_HALF_WIDTH: f32 = 0.12;
/// Centre height of the route ribbon above sampled terrain. This is the centre of the authored waypoint
/// dome (`vessels/markers/waypoint.usda`): route connections meet a waypoint in
/// its middle instead of visually entering through the buried lower hemisphere.
/// The ribbon still samples every DEM height, so this is a constant clearance,
/// not a straight chord that can cut through a ridge or crater.
const WAYPOINT_CONNECTION_HEIGHT: f32 = 2.5;
/// Resample spacing for a `smooth` route's ribbon. Matches the autopilot's own
/// resampling, so the drawn curve IS the driven curve.
const PATH_SPACING: f64 = 2.0;

/// A vessel's route ribbon. `signature` is what the mesh was built from, so the
/// (relatively expensive) rebuild only happens when the route or active leg changes —
/// not every frame.
///
/// A route draws as two roles: the complete green waypoint-to-waypoint path and a
/// blue rover-to-next-waypoint cue. The latter changes when `ReachedWaypoints`
/// advances.
#[derive(Component)]
pub struct WaypointPathMesh {
    pub vessel: Entity,
    pub signature: u64,
    pub part: PathPart,
}

/// Which half of a route a ribbon draws.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PathPart {
    /// Complete waypoint-to-waypoint route, including visited and future legs.
    Driven,
    /// Current rover-to-next-waypoint leg.
    Remaining,
}

/// Cheap change-signature for a route: its ordered coord keys + smooth flag +
/// which points are already visited (visited legs drop out of the curve), plus
/// the rover pose used as the live start of the remaining leg.
fn route_signature(
    targets: &[String],
    smooth: bool,
    progress: &RouteVisualState,
    rover_pos: Option<DVec3>,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    smooth.hash(&mut h);
    for (index, t) in targets.iter().enumerate() {
        t.hash(&mut h);
        progress
            .visited
            .get(index)
            .copied()
            .unwrap_or(false)
            .hash(&mut h);
    }
    progress.active_index.hash(&mut h);
    // The live endpoint is physics state, not a cache bucket. Quantising this
    // position made the line visibly snap every 0.1 m. Exact bits let an
    // unchanged fixed-step pose skip work while every real solve is rendered;
    // the mesh asset is updated in place by `sync_waypoint_path_mesh`.
    if let Some(pos) = rover_pos {
        for component in [pos.x, pos.y, pos.z] {
            component.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

/// Split route geometry into its two visual roles: the complete green
/// waypoint-to-waypoint route and the single active blue rover-to-next segment.
/// Keeping this pure makes the visited-state transition testable without a
/// renderer or a Bevy world.
fn route_ribbon_points(
    points: &[(DVec3, bool)],
    rover_pos: Option<DVec3>,
    active_index: Option<usize>,
) -> (Vec<DVec3>, Vec<DVec3>) {
    let green = points.iter().map(|(point, _)| *point).collect();
    let next = active_index
        .filter(|&index| index < points.len())
        .or_else(|| points.iter().position(|(_, visited)| !visited));
    let blue = next
        .map(|index| {
            vec![
                rover_pos.unwrap_or_else(|| points[index.saturating_sub(1)].0),
                points[index].0,
            ]
        })
        .unwrap_or_default();
    (green, blue)
}

/// Build a flat ground-hugging ribbon through `points`, with vertices expressed
/// relative to `anchor` (the entity's own origin) so f32 vertex precision stays
/// tight regardless of how far the route sits from the world origin.
fn build_ribbon_mesh(points: &[DVec3], anchor: DVec3, first_height_offset: f32) -> Option<Mesh> {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};
    let n = points.len();
    if n < 2 {
        return None;
    }
    let mut pos: Vec<[f32; 3]> = Vec::with_capacity(n * 2);
    let mut nrm: Vec<[f32; 3]> = Vec::with_capacity(n * 2);
    let mut uv: Vec<[f32; 2]> = Vec::with_capacity(n * 2);
    for i in 0..n {
        // Central-difference tangent, flattened to the ground plane so the ribbon
        // stays level across slopes instead of twisting.
        let prev = points[i.saturating_sub(1)];
        let next = points[(i + 1).min(n - 1)];
        let mut tan = next - prev;
        tan.y = 0.0;
        let tan = if tan.length_squared() < 1e-9 {
            DVec3::Z
        } else {
            tan.normalize()
        };
        let mut right = tan.cross(DVec3::Y);
        if right.length_squared() < 1e-9 {
            right = DVec3::X;
        }
        let right = right.normalize() * PATH_HALF_WIDTH as f64;
        // The live leg starts at the rover body's authored root (its centre),
        // while waypoint pins meet at their visible dome centres. Applying the
        // waypoint lift to point zero made the line appear to leave the mast.
        let height_offset = if i == 0 {
            first_height_offset
        } else {
            WAYPOINT_CONNECTION_HEIGHT
        };
        let base = (points[i] - anchor).as_vec3() + Vec3::Y * height_offset;
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

/// Raise interpolated route samples to the analytic surface when a smooth leg
/// crosses relief between two authored waypoints.  Waypoint endpoints are
/// placed on the ground, but Catmull–Rom samples can dip through a crater rim
/// or ridge; the ribbon must follow the same height oracle as placement and
/// terrain rendering rather than assuming endpoint heights describe the whole
/// segment.
fn follow_surface(points: &mut [DVec3], surface: &lunco_terrain_surface::GridSurfaceQuery) {
    for point in points {
        if let Some(ground) = surface.height_at(lunco_core::coords::GridPos(*point)) {
            point.y = point.y.max(ground);
        }
    }
}

/// Spawn/refresh each vessel's route ribbon as REAL scene geometry.
///
/// This replaces the old egui screen-space line stroke, which had no depth and so
/// drew over terrain and over other waypoints (the "gizmos overlap and are buggy"
/// problem). A mesh in the world occludes properly and hugs the ground.
///
/// A `smooth` route is sampled with the SAME Catmull-Rom the autopilot resamples for
/// driving ([`catmull_rom_path`]), so the ribbon you see is literally the path the
/// rover follows. Visited legs drop out of the curve, exactly as they do for driving.
pub fn sync_waypoint_path_mesh(
    q_vessels: Query<(
        Entity,
        Option<&BehaviorXml>,
        Option<&lunco_autopilot::AutopilotBehaviorSpec>,
        Option<&TargetBindings>,
        Option<&ReachedWaypoints>,
    )>,
    selected: Res<SelectedEntities>,
    q_avatar: Query<&ControllerLink, With<Avatar>>,
    q_autopilots: Query<(
        &lunco_autopilot::Autopilot,
        Option<&lunco_autopilot::AutopilotBehavior>,
        Option<&lunco_autopilot::AutopilotExecutionState>,
    )>,
    q_paths: Query<(Entity, &WaypointPathMesh, &Mesh3d)>,
    q_parents: Query<&ChildOf>,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    q_grids_only: Query<&big_space::prelude::Grid>,
    mut spatial: ParamSet<(
        Query<(Option<&big_space::grid::cell::CellCoord>, &Transform)>,
        Query<(
            Entity,
            &lunco_usd_sim::marker::WaypointMarker,
            &mut big_space::grid::cell::CellCoord,
            &mut Transform,
        )>,
        Query<Entity, With<lunco_usd_sim::marker::WaypointMarker>>,
    )>,
    surface: lunco_terrain_surface::GridSurfaceQuery,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    // Waypoint prims author a planimetric position; their height is terrain
    // presentation, not a second hand-maintained elevation. Keep the marker
    // root on the same oracle used by the route ribbon and the collider. The
    // dome/zone are authored above that root, so moving the root fixes both the
    // visible pin and its trigger without changing mission coordinates.
    let grid_entity = active_frame.0;
    let Ok(grid) = q_grids_only.get(grid_entity) else {
        warn_once!(
            ?grid_entity,
            "waypoint route active physics frame is not a BigSpace Grid"
        );
        return;
    };

    // Waypoint transforms are grid-local, while the surface oracle returns a
    // grid-absolute elevation. Compute the absolute target first, then split it
    // back into the marker's CellCoord + local Transform. Writing an absolute
    // elevation directly into Transform was the reason markers stayed below the
    // rendered terrain at the lunar site.
    let waypoint_updates = if surface.has_terrain() {
        let waypoint_entities: Vec<Entity> = spatial.p2().iter().collect();
        let q_spatial = spatial.p0();
        waypoint_entities
            .into_iter()
            .filter_map(|entity| {
                let (position, _) = lunco_core::coords::grid_relative_pose(
                    entity,
                    grid_entity,
                    &q_parents,
                    &q_grids_only,
                    &q_spatial,
                )?;
                let ground = surface.height_at(lunco_core::coords::GridPos(position))?;
                let target = lunco_core::coords::GridPos(DVec3::new(
                    position.x,
                    // The marker asset owns the visible dome's +2.5 m centre
                    // transform. Keep its root at terrain level; applying the
                    // connection height here as well would double-lift routes.
                    ground, position.z,
                ));
                Some((entity, grid.translation_to_grid(target.0)))
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    for (entity, (cell, local)) in waypoint_updates {
        if let Ok((_, _, mut marker_cell, mut transform)) = spatial.p1().get_mut(entity) {
            *marker_cell = cell;
            transform.translation = local;
        }
    }
    let q_spatial = spatial.p0();
    // Existing ribbons, keyed by (vessel, part).
    let mut existing: std::collections::HashMap<(Entity, PathPart), (Entity, u64, Handle<Mesh>)> =
        std::collections::HashMap::new();
    for (e, path, mesh) in q_paths.iter() {
        existing.insert(
            (path.vessel, path.part),
            (e, path.signature, mesh.0.clone()),
        );
    }

    let vessel_entities: std::collections::HashSet<Entity> =
        q_vessels.iter().map(|(entity, ..)| entity).collect();
    let selected_vessel = route_owner(selected.primary(), &q_parents, &vessel_entities);
    let possessed_vessel = route_owner(
        q_avatar.iter().next().map(|link| link.vessel_entity),
        &q_parents,
        &vessel_entities,
    );
    for (vessel, xml, spec, bindings, reached) in q_vessels.iter() {
        // Every route draws. Focus (possessed / selected) only decides how loud it is
        // — a route that vanishes unless you possess its vessel cannot be clicked on,
        // and the waypoint editor is right-click-on-the-pin.
        let focused = Some(vessel) == selected_vessel || Some(vessel) == possessed_vessel;
        let (targets, authored_points, smooth, closed) = if let Some(xml) = xml {
            let Ok(value) = lunco_autopilot::btcpp_xml::xml_to_value(&xml.0) else {
                continue;
            };
            let mut targets = Vec::new();
            collect_targets(&value, &mut targets);
            if !targets.is_empty() {
                (
                    targets,
                    None,
                    route_is_smooth(&xml.0),
                    route_loops(Some(xml), spec),
                )
            } else if let Some(spec) = spec {
                let Some(waypoints) = spec.patrol_waypoints() else {
                    continue;
                };
                let points = waypoints
                    .iter()
                    .map(|waypoint| DVec3::new(waypoint.pos[0], waypoint.pos[1], waypoint.pos[2]))
                    .collect::<Vec<_>>();
                let targets = (0..points.len())
                    .map(runtime_waypoint_key)
                    .collect::<Vec<_>>();
                let closed = route_loops(None, Some(spec));
                (targets, Some(points), false, closed)
            } else {
                continue;
            }
        } else if let Some(spec) = spec {
            let Some(waypoints) = spec.patrol_waypoints() else {
                continue;
            };
            let points = waypoints
                .iter()
                .map(|waypoint| DVec3::new(waypoint.pos[0], waypoint.pos[1], waypoint.pos[2]))
                .collect::<Vec<_>>();
            let targets = (0..points.len())
                .map(runtime_waypoint_key)
                .collect::<Vec<_>>();
            let closed = route_loops(None, Some(spec));
            (targets, Some(points), false, closed)
        } else {
            continue;
        };
        let (cursor, completed) = route_execution(vessel, &q_autopilots);
        let progress = route_visual_state(&targets, reached, cursor, completed, closed);
        let rover_pos = lunco_core::coords::grid_relative_pose(
            vessel,
            grid_entity,
            &q_parents,
            &q_grids_only,
            &q_spatial,
        )
        .map(|(position, _)| position);
        // Focus is part of the signature: it changes the ribbon's colour, so a
        // selection change has to rebuild it (the mesh is only rebuilt when this
        // number moves).
        // Terrain may finish streaming after the route binding.  Include its
        // availability in the change key so a ribbon first created during the
        // loading frame is rebuilt once the analytic surface can clamp its
        // interpolated samples.
        // All control points, in order, each tagged with whether it's been driven.
        let pts: Vec<(DVec3, bool)> = if let Some(points) = authored_points {
            points
                .into_iter()
                .enumerate()
                .map(|(index, point)| {
                    (point, progress.visited.get(index).copied().unwrap_or(false))
                })
                .collect()
        } else {
            targets
                .iter()
                .filter_map(|t| {
                    let pos = bindings.and_then(|b| b.0.get(t)).and_then(|&entity| {
                        lunco_core::coords::grid_relative_pose(
                            entity,
                            grid_entity,
                            &q_parents,
                            &q_grids_only,
                            &q_spatial,
                        )
                        .map(|(position, _)| position)
                    });
                    let index = targets.iter().position(|target| target == t);
                    pos.map(|p| {
                        (
                            p,
                            index
                                .and_then(|index| progress.visited.get(index).copied())
                                .unwrap_or(false),
                        )
                    })
                })
                .collect()
        };
        // The complete waypoint-to-waypoint route is green; a separate blue
        // segment overlays only rover → next unresolved waypoint.
        let (green_points, blue_points) =
            route_ribbon_points(&pts, rover_pos, progress.active_index);
        let closed = closed && pts.len() > 2;

        for part in [PathPart::Driven, PathPart::Remaining] {
            // The complete route is independent of rover motion. Only the live
            // blue leg includes the exact solved rover pose in its revision.
            let live_start = (part == PathPart::Remaining).then_some(rover_pos).flatten();
            let signature = route_signature(&targets, smooth, &progress, live_start)
                ^ (focused as u64)
                ^ ((surface.has_terrain() as u64) << 1);
            let slice: Vec<DVec3> = match part {
                // Green is the complete authored/runtime route, including legs
                // already visited and legs still ahead.
                PathPart::Driven => green_points.clone(),
                // Blue is only the currently active leg. It must not redraw a
                // previous green connection, otherwise the blue pass wins in
                // depth/order and makes visited legs look unvisited.
                PathPart::Remaining => blue_points.clone(),
            };

            let key = (vessel, part);
            // Unchanged → leave this ribbon alone.
            if let Some((_, sig, _)) = existing.get(&key) {
                if *sig == signature {
                    existing.remove(&key);
                    continue;
                }
            }
            let previous = existing.remove(&key);
            if slice.len() < 2 {
                if let Some((old, _, _)) = previous {
                    commands.entity(old).try_despawn();
                }
                continue;
            }

            // A looping patrol's waypoint-to-waypoint route closes in green;
            // the active blue leg remains a single rover→next segment.
            let close_this = closed && part == PathPart::Driven;
            let mut path = if smooth {
                catmull_rom_path(&slice, close_this, PATH_SPACING)
            } else {
                slice.clone()
            };
            if close_this {
                if let Some(first) = path.first().copied() {
                    path.push(first); // seal the loop
                }
            }

            // The route's control points are ground-authored, but the smooth
            // interpolation between them is not.  Clamp every generated sample
            // against the composed DEM before the ribbon receives its visual
            // clearance, so crater crossings cannot draw through the ground.
            follow_surface(&mut path, &surface);

            let anchor = path[0];
            let first_height_offset = if part == PathPart::Remaining {
                0.0
            } else {
                WAYPOINT_CONNECTION_HEIGHT
            };
            let Some(mesh) = build_ribbon_mesh(&path, anchor, first_height_offset) else {
                continue;
            };
            // The complete route stays VISIBLE in green; the active blue leg is
            // rendered separately so visited state cannot be hidden by overlap.
            let (base_color, emissive) = match part {
                PathPart::Driven => (
                    // Green = the waypoint-to-waypoint mission connections.
                    LinearRgba::new(0.18, 0.72, 0.38, 0.38),
                    LinearRgba::new(0.08, 0.55, 0.24, 1.0),
                ),
                PathPart::Remaining => (
                    // Blue = the live commanded leg from the rover centre.
                    LinearRgba::new(0.12, 0.45, 0.95, 0.62),
                    LinearRgba::new(0.06, 0.30, 0.85, 1.0),
                ),
            };
            // Unfocused vessel: same ribbon, held back — visible enough to right-click
            // a pin on it, quiet enough not to compete with the route being edited.
            let (base_color, emissive) = if focused {
                (base_color, emissive)
            } else {
                (
                    LinearRgba::new(
                        base_color.red,
                        base_color.green,
                        base_color.blue,
                        base_color.alpha * 0.45,
                    ),
                    LinearRgba::new(
                        emissive.red * 0.35,
                        emissive.green * 0.35,
                        emissive.blue * 0.35,
                        1.0,
                    ),
                )
            };
            let (cell, local) = grid.translation_to_grid(anchor);
            let look = PbrLook {
                base_color,
                emissive,
                alpha: SurfaceAlpha::Blend,
                unlit: true,
                // The route is an editor annotation, not scenery: a translucent
                // unlit ribbon must not darken the terrain it lies on. This is the
                // INTENT — `NotShadowCaster` is derived from it by the render
                // bridge, which removes any hand-inserted one on every rebind.
                no_shadow_cast: true,
                ..default()
            };
            let path_component = WaypointPathMesh {
                vessel,
                signature,
                part,
            };
            if let Some((entity, _, handle)) = previous {
                *meshes
                    .get_mut(&handle)
                    .expect("a route entity's strong Mesh3d handle must stay resident") = mesh;
                commands.entity(entity).try_insert((
                    look,
                    cell,
                    Transform::from_translation(local),
                    path_component,
                ));
            } else {
                commands.spawn((
                    Mesh3d(meshes.add(mesh)),
                    look,
                    cell,
                    Transform::from_translation(local),
                    GlobalTransform::default(),
                    ChildOf(grid_entity),
                    path_component,
                ));
            }
        }
    }

    // Vessels/parts that no longer have a route.
    for (_, (entity, _, _)) in existing {
        commands.entity(entity).try_despawn();
    }
}

/// Snapshot the authored dome look before tinting it, so a session-only visual
/// state never becomes an authored USD change and can be restored on reload.
#[derive(Component, Clone, Debug)]
pub(crate) struct WaypointVisualBase(PbrLook);

/// Resolve the session-only appearance of a waypoint from its authored look and
/// live visit state. Keeping this decision pure makes the visible visited-state
/// contract testable without a renderer or a spawned USD subtree.
fn waypoint_look_for_visit(base: &PbrLook, visited: bool) -> PbrLook {
    if !visited {
        return base.clone();
    }

    let mut target = base.clone();
    target.base_color = LinearRgba::new(0.38, 0.38, 0.38, target.base_color.alpha);
    target.emissive = LinearRgba::new(0.10, 0.10, 0.10, target.emissive.alpha);
    target.unshared = true;
    target
}

/// Tint visited waypoint domes from the live route state. The marker geometry and
/// its authored material remain in USD; only the resolved render intent is changed
/// for this session. `unshared` is required because the tint is animated state and
/// must not be put through the shared material cache.
pub(crate) fn sync_waypoint_marker_visuals(
    q_vessels: Query<(
        Entity,
        Option<&BehaviorXml>,
        Option<&lunco_autopilot::AutopilotBehaviorSpec>,
        Option<&ReachedWaypoints>,
    )>,
    q_autopilots: Query<(
        &lunco_autopilot::Autopilot,
        Option<&lunco_autopilot::AutopilotBehavior>,
        Option<&lunco_autopilot::AutopilotExecutionState>,
    )>,
    q_markers: Query<
        (Entity, &UsdPrimPath, Option<&RuntimeWaypointBinding>),
        With<lunco_usd_sim::marker::WaypointMarker>,
    >,
    mut q_looks: Query<(
        Entity,
        &UsdPrimPath,
        &mut PbrLook,
        Option<&WaypointVisualBase>,
    )>,
    q_parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let mut routes = std::collections::HashMap::new();
    let mut runtime_reached = std::collections::HashMap::new();
    for (vessel, xml, spec, reached) in q_vessels.iter() {
        if let Some(reached) = reached {
            runtime_reached.insert(vessel, reached);
        }
        let targets = route_targets(xml, spec);
        if targets.is_empty() {
            continue;
        }
        let (cursor, completed) = route_execution(vessel, &q_autopilots);
        routes.insert(
            vessel,
            (
                targets.clone(),
                route_visual_state(&targets, reached, cursor, completed, route_loops(xml, spec)),
            ),
        );
    }

    let mut marker_visits = std::collections::HashMap::new();
    for (marker, path, binding) in q_markers.iter() {
        let visited = binding
            .and_then(|binding| {
                runtime_reached
                    .get(&binding.vessel)
                    .map(|reached| runtime_marker_is_visited(Some(&reached.0), binding.index))
            })
            .or_else(|| {
                // Prefer an exact authored target before the relative/full-path
                // fallback. This keeps a marker attached to its own route when
                // two vessels use similarly named waypoints.
                if let Some(visited) = routes.iter().find_map(|(_, (targets, state))| {
                    targets
                        .iter()
                        .position(|target| target == &path.path)
                        .and_then(|index| state.visited.get(index).copied())
                }) {
                    return Some(visited);
                }
                routes.iter().find_map(|(_, (targets, state))| {
                    targets
                        .iter()
                        .position(|target| prim_path_matches(target, &path.path))
                        .and_then(|index| state.visited.get(index).copied())
                })
            });
        if let Some(visited) = visited {
            marker_visits.insert(marker, visited);
        }
    }

    for (entity, path, mut look, base) in q_looks.iter_mut() {
        if !path.path.ends_with("/Dome") {
            continue;
        }
        let mut current = entity;
        let mut marker = None;
        for _ in 0..32 {
            if marker_visits.contains_key(&current) {
                marker = Some(current);
                break;
            }
            let Ok(parent) = q_parents.get(current) else {
                break;
            };
            current = parent.parent();
        }
        let Some(visited) = marker.and_then(|marker| marker_visits.get(&marker).copied()) else {
            continue;
        };

        let mut target = if let Some(base) = base {
            base.0.clone()
        } else {
            let authored = look.clone();
            commands
                .entity(entity)
                .try_insert(WaypointVisualBase(authored.clone()));
            authored
        };
        target = waypoint_look_for_visit(&target, visited);
        if *look != target {
            *look = target;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        has_authored_movement_route, route_loops, route_ribbon_points, route_signature,
        route_visual_state, runtime_waypoint_key, select_ground_point, BehaviorXml,
        ReachedWaypoints, WAYPOINT_MARKER_ASSET,
    };
    use bevy::math::DVec3;
    use bevy::prelude::{Entity, LinearRgba};
    use lunco_autopilot::{
        btcpp_xml::value_to_xml, AutopilotBehaviorSpec, BehaviorSpec, PatrolWaypoint,
    };
    use lunco_core::paths::prim_path_matches;

    #[test]
    fn analytic_surface_remains_authoritative_when_streamed_terrain_hit_is_removed() {
        let terrain = lunco_terrain_surface::surface_query::SurfaceHit {
            point: lunco_core::coords::GridPos(DVec3::new(0.0, -100.0, 0.0)),
            frame: Entity::PLACEHOLDER,
            distance: 100.0,
            terrain: Entity::PLACEHOLDER,
        };

        let point = select_ground_point(DVec3::ZERO, DVec3::NEG_Y, None, Some(terrain))
            .expect("the DEM hit is a valid placement point");

        assert_eq!(point, terrain.point.0);
    }
    use lunco_render::PbrLook;
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
        assert_eq!((speed, radius, dwell), (0.6, 3.0, 0.0));

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
    fn pending_authored_xml_is_a_route_until_its_spec_is_derived() {
        assert!(has_authored_movement_route(true, None));
        assert!(!has_authored_movement_route(
            false,
            Some(&AutopilotBehaviorSpec::new(BehaviorSpec::Brake))
        ));
        assert!(has_authored_movement_route(
            false,
            Some(&AutopilotBehaviorSpec::new(BehaviorSpec::DriveTo {
                target: [1.0, 0.0, 0.0],
                speed: 0.5,
                radius: 1.0,
            }))
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
    fn route_ribbon_keeps_connections_green_and_advances_blue_leg() {
        let w0 = DVec3::new(0.0, 0.0, 0.0);
        let w1 = DVec3::new(10.0, 0.0, 0.0);
        let w2 = DVec3::new(20.0, 0.0, 0.0);
        let rover = DVec3::new(2.0, 0.0, 0.0);

        let (green_before, blue_before) = route_ribbon_points(
            &[(w0, false), (w1, false), (w2, false)],
            Some(rover),
            Some(0),
        );
        assert_eq!(green_before, vec![w0, w1, w2]);
        assert_eq!(blue_before, vec![rover, w0]);

        let (green_after, blue_after) = route_ribbon_points(
            &[(w0, true), (w1, false), (w2, false)],
            Some(rover),
            Some(1),
        );
        assert_eq!(green_after, vec![w0, w1, w2]);
        assert_eq!(blue_after, vec![rover, w1]);
    }

    #[test]
    fn live_route_revision_preserves_sub_decimetre_physics_motion() {
        let targets = vec!["/Route/W0".to_string()];
        let state = route_visual_state(&targets, None, Some(0), false, false);
        let first = route_signature(
            &targets,
            false,
            &state,
            Some(DVec3::new(10.001, -1_900.0, 0.0)),
        );
        let second = route_signature(
            &targets,
            false,
            &state,
            Some(DVec3::new(10.002, -1_900.0, 0.0)),
        );
        assert_ne!(
            first, second,
            "the live endpoint must not snap through a distance bucket"
        );
    }

    #[test]
    fn route_ribbon_ignores_an_stale_cursor_after_route_resolution() {
        let points = [
            (DVec3::ZERO, true),
            (DVec3::X * 10.0, false),
            (DVec3::X * 20.0, false),
        ];
        let (_, blue) = route_ribbon_points(&points, Some(DVec3::Y), Some(99));
        assert_eq!(blue, vec![DVec3::Y, DVec3::X * 10.0]);
    }

    #[test]
    fn route_ribbon_has_no_blue_leg_when_a_one_way_route_is_done() {
        let points = [(DVec3::ZERO, true), (DVec3::X * 10.0, true)];
        let (_, blue) = route_ribbon_points(&points, Some(DVec3::Y), None);
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
        assert!(!route_loops(Some(&sequence), None));
        assert!(route_loops(Some(&forever), None));
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
        assert!(!route_loops(None, Some(&one_way)));
        assert!(route_loops(None, Some(&patrol)));
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
    fn appended_runtime_waypoint_does_not_reuse_reached_index() {
        let reached = std::collections::HashSet::from([runtime_waypoint_key(0)]);

        assert!(super::runtime_marker_is_visited(Some(&reached), 0));
        assert!(
            !super::runtime_marker_is_visited(Some(&reached), 1),
            "a newly appended marker must remain green after waypoint zero was reached"
        );
    }

    #[test]
    fn waypoint_path_matching_requires_a_prim_boundary() {
        assert!(prim_path_matches("/World/Route/W0", "/Route/W0"));
        assert!(!prim_path_matches("/World/Route/W01", "/Route/W0"));
    }

    #[test]
    fn visited_waypoint_look_is_gray_and_private() {
        let mut authored = PbrLook::matte(LinearRgba::new(0.12, 0.72, 0.34, 0.8));
        authored.emissive = LinearRgba::new(0.02, 0.3, 0.08, 1.0);

        let visited = super::waypoint_look_for_visit(&authored, true);

        assert_eq!(visited.base_color.red, 0.38);
        assert_eq!(visited.base_color.green, 0.38);
        assert_eq!(visited.base_color.blue, 0.38);
        assert_eq!(visited.base_color.alpha, authored.base_color.alpha);
        assert_eq!(visited.emissive.red, 0.10);
        assert_eq!(visited.emissive.green, 0.10);
        assert_eq!(visited.emissive.blue, 0.10);
        assert!(
            visited.unshared,
            "animated visit state must not share authored materials"
        );
    }

    #[test]
    fn unvisited_waypoint_look_preserves_authored_appearance() {
        let mut authored = PbrLook::matte(LinearRgba::new(0.12, 0.72, 0.34, 0.8));
        authored.emissive = LinearRgba::new(0.02, 0.3, 0.08, 1.0);

        assert_eq!(
            super::waypoint_look_for_visit(&authored, false),
            authored,
            "unvisited markers must keep the authored look"
        );
    }
}
