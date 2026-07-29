//! Alt+LMB — drop a mission waypoint by **authoring a USD prim**.
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
use lunco_controller::ControllerLink;
use lunco_core::commands::SessionId;
use lunco_core::session::SessionRegistry;
use lunco_core::{Avatar, EguiFocus, GlobalEntityId, SpawnToolActive, TerrainToolActive};
use lunco_doc_bevy::DocumentRegistry;
use lunco_render::{PbrLook, SurfaceAlpha};
use lunco_usd::commands::ApplyUsdOp;
use lunco_usd::document::UsdDocument;
use lunco_usd::document::{LayerId, UsdOp};
use lunco_usd_bevy::UsdPrimPath;
use serde_json::Value;

use crate::SelectedEntities;

/// Scope the authored waypoints are parented under, beneath the stage's default prim.
/// A route lives in WORLD space, so it is deliberately NOT a child of the vessel —
/// parented under the rover, the waypoints would ride along as it drives.
const BEHAVIORS_SCOPE: &str = "Behaviors";
/// Scope holding a scene's waypoint MARKER prims. Scene-authored routes already
/// use this name (`/Traverse/Route/W0`), so a dropped waypoint joins the same
/// scope rather than inventing a parallel one.
const ROUTE_SCOPE: &str = "Route";
/// The authored marker. A dropped waypoint REFERENCES this asset instead of
/// having its geometry rebuilt in Rust: colour, opacity, emission, dome and
/// trigger zone are authored content, and there is exactly one of them.
const WAYPOINT_ASSET: &str = "lunco://vessels/markers/waypoint.usda";

/// Name of the `LunCoProgramAPI` child that carries a vessel's mission tree.
const MISSION_PROGRAM: &str = "Mission";

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
    pub workspace: Option<Res<'w, lunco_workspace::WorkspaceResource>>,
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
        .or_else(|| self.workspace.as_ref().and_then(|w| w.0.active_document))
        .or_else(|| self.usd_registry.ids().next())
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
        With<Camera3d>,
    >,
    pub q_parents: Query<'w, 's, &'static ChildOf>,
}

/// Resolve the pointer to a point on the ground in **WORLD** (grid-absolute) space —
/// the one spelling of "where did the user click?" for the waypoint editor, shared by
/// the Alt+LMB drop and the Move / Insert-after placement click.
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
    let origin = surface.to_grid(lunco_core::coords::RenderPos(ray.origin.as_dvec3()))?;
    let dir = ray.direction.as_dvec3();
    let phys = raycaster
        .cast_ray_grid(
            origin,
            ray.direction,
            1.0e6,
            false,
            &avian3d::prelude::SpatialQueryFilter::default(),
        )
        .map(|h| h.distance);
    let terr = surface.raycast(origin, ray.direction, 1.0e6);
    match (phys, terr) {
        (Some(pd), Some(hit)) => Some(if hit.distance <= pd {
            hit.point.0
        } else {
            origin.0 + dir * pd
        }),
        (Some(pd), None) => Some(origin.0 + dir * pd),
        (None, Some(hit)) => Some(hit.point.0),
        (None, None) => None,
    }
}

/// Global `Pointer<Click>` observer: Alt+LMB drops a waypoint prim for the selected
/// vessel and appends the matching `drive_to` leaf to its mission.
///
/// Stands down when the spawn / terrain-sculpt tool is armed, and when egui owns the
/// pointer (the authoritative gate). Ctrl is excluded from the possession observer, so
/// a checkpoint click does not also possess or follow what the ray hit.
pub fn on_scene_click_checkpoint(
    mut click: On<Pointer<Click>>,
    keys: Res<ButtonInput<KeyCode>>,
    egui_focus: Res<EguiFocus>,
    spawn_tool: Res<SpawnToolActive>,
    terrain_tool: Res<TerrainToolActive>,
    selected: Res<SelectedEntities>,
    avatars: Query<Entity, With<Avatar>>,
    q_link: Query<&ControllerLink>,
    frame: WaypointClickFrame,
    surface: lunco_terrain_surface::GridSurfaceQuery,
    raycaster: lunco_physics::GridSpatialQuery,
    q_prim: Query<&UsdPrimPath>,
    q_xml: Query<(Entity, &BehaviorXml)>,
    doc_ctx: WaypointDocContext,

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
    // Alt+LMB only — a plain click possesses, Shift+click selects, Alt+click drops waypoints
    if !keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]) {
        return;
    }

    // Now that we are sure this is a Alt+LMB click meant for a waypoint, stop propagation.
    click.propagate(false);

    // Default to the possessed vessel first, then fall back to the selected one, then fall back to the first vessel with a mission tree in the scene
    let possessed_vessel = avatars
        .iter()
        .next()
        .and_then(|av| q_link.get(av).ok().map(|link| link.vessel_entity));
    let raw_vessel = possessed_vessel
        .or_else(|| selected.primary())
        .or_else(|| q_xml.iter().next().map(|(e, _)| e));
    let Some(mut vessel) = raw_vessel else {
        info!("[waypoint] click ignored: no vessel found in scene");
        return;
    };

    // If selected entity is a sub-part, climb parents to find the vessel prim carrying BehaviorXml or UsdPrimPath
    if q_prim.get(vessel).is_err() || (q_xml.get(vessel).is_err() && possessed_vessel.is_none()) {
        let mut curr = vessel;
        for _ in 0..16 {
            if q_xml.get(curr).is_ok() {
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

    let Ok(vessel_prim) = q_prim.get(vessel) else {
        warn!(
            "[waypoint] target vessel {:?} is not a USD prim; cannot author a mission for it",
            vessel
        );
        return;
    };

    // ── Find the document that OWNS this vessel ──────────────────────────────
    let Some(doc) = doc_ctx.resolve_document(&vessel_prim.stage_handle) else {
        info!("[waypoint] click ignored: no document found for vessel");
        return;
    };
    let Some(host) = doc_ctx.usd_registry.host(doc) else {
        info!(
            "[waypoint] click ignored: no USD host for document {:?}",
            doc
        );
        return;
    };

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
    let scope_path = join_prim(&root, BEHAVIORS_SCOPE);

    // Create the `Behaviors` scope on first use. `AddPrim` on an existing prim is a
    // rejection, not a merge, so only author it when it is genuinely absent.
    let scope_exists = prim_exists(host, &scope_path);
    if !scope_exists {
        commands.trigger(ApplyUsdOp {
            doc,
            op: UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path: root.clone(),
                name: BEHAVIORS_SCOPE.to_string(),
                type_name: Some("Scope".to_string()),
                reference: None,
            },
        });
    }

    // ── The MARKER is an authored prim ────────────────────────────────────────
    // Not a Rust-built sphere: `vessels/markers/waypoint.usda` already defines
    // the dome, its livery and its arrival trigger zone. Referencing it means
    // one marker implementation for scene-authored and click-dropped waypoints
    // alike — the two used to be different objects that only looked alike, and
    // the Rust one drew itself in the vessel's hull colour.
    let marker_path = author_marker_prim(&mut commands, host, doc, &root, hit);

    // ── The mission's topology ────────────────────────────────────────────────
    // Append the leaf FIRST: if the tree is a shape the editor must not restructure,
    // bail out. The leaf targets the marker PRIM, so the mission and the map
    // refer to the same object — a coordinate string could drift from the pin.
    let current = q_xml.get(vessel).ok().map(|(_, x)| x.0.as_str());
    let wp_coord_str = marker_path.clone();
    let xml = match append_waypoint_leaf(current, &wp_coord_str) {
        Ok(xml) => xml,
        Err(err) => {
            warn!("[waypoint] not adding a checkpoint: {err}");
            return;
        }
    };

    // ── Author: update ECS component immediately and persist to USD document ─
    commands.entity(vessel).insert(BehaviorXml(xml.clone()));
    let mission = ensure_mission_program(&mut commands, host, doc, &vessel_prim.path);
    info!(
        "[waypoint] writing to doc {:?}, mission prim {:?}",
        doc, mission
    );
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetAttribute {
            edit_target: LayerId::runtime(),
            path: mission,
            name: "info:sourceCode".to_string(),
            type_name: "string".to_string(),
            value: xml,
        },
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
                edit_target: LayerId::runtime(),
                path: pending.coord_key.clone(),
                value: [world.x, world.y, world.z],
            },
        });
        return;
    }

    // INSERT-AFTER edits the mission, so it does need the vessel — resolved HERE,
    // from the route that names this waypoint, rather than from an entity captured
    // when the menu was open.
    let Some((vessel, xml, vessel_prim)) = vessel_for_target(&q_vessel, &pending.coord_key) else {
        warn!(
            "[waypoint] placement failed: no vessel's mission names '{}'",
            pending.coord_key
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
        warn!("[waypoint] placement failed: no USD host for document {doc:?}");
        return;
    };
    let new_target = author_marker_prim(&mut commands, host, doc, &root, world);
    let edited = insert_waypoint_after(&xml.0, &pending.coord_key, &new_target);
    match edited {
        Ok(new_xml) => {
            info!("[waypoint] {:?} → {}", pending.mode, new_target);
            commands.entity(vessel).insert(BehaviorXml(new_xml.clone()));
            commands.trigger(ApplyUsdOp {
                doc,
                op: UsdOp::SetAttribute {
                    edit_target: LayerId::runtime(),
                    // Editing an EXISTING tree, so the program prim is already there —
                    // the XML above was read back off it.
                    path: join_prim(&vessel_prim.path, MISSION_PROGRAM),
                    name: "info:sourceCode".to_string(),
                    type_name: "string".to_string(),
                    value: new_xml,
                },
            });
        }
        Err(err) => warn!("[waypoint] placement failed: {err}"),
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
                        Err(err) => warn!("[waypoint] delete failed: {err}"),
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
                            Err(err) => warn!("[waypoint] dwell failed: {err}"),
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
                        Err(err) => warn!("[waypoint] smooth toggle failed: {err}"),
                    }
                }
            });
        });

    menu_state.dwell = dwell;

    if let Some(value) = edited {
        // UPDATE THE LIVE COMPONENT TOO, not only the document.
        //
        // `compile_behavior_xml` watches `Changed<BehaviorXml>`; that component IS
        // the running mission. Authoring `info:sourceCode` alone only edits the
        // document, and the ECS does not learn about it until the USD round-trip
        // projects back — so Delete, Dwell and Smooth all appeared to do nothing:
        // the rover kept driving the tree it already had.
        //
        // The drop (`on_scene_click_checkpoint`) and Insert-after
        // (`on_scene_click_place_waypoint`) paths have always written both. This
        // menu was the one edit surface that wrote only one, which is why it was
        // the one that did not work.
        commands
            .entity(marker_vessel)
            .insert(BehaviorXml(value.clone()));
        commands.trigger(ApplyUsdOp {
            doc,
            op: UsdOp::SetAttribute {
                edit_target: LayerId::runtime(),
                path: join_prim(&vessel_prim.path, MISSION_PROGRAM),
                name: "info:sourceCode".to_string(),
                type_name: "string".to_string(),
                value,
            },
        });
    }

    // Un-author the pin itself, AFTER the mission no longer references it — the
    // leg is what makes a marker a waypoint, so dropping it first means the prim
    // is never briefly a waypoint of a route that no longer names it.
    if let Some(marker_path) = deleted_marker {
        info!("[waypoint] deleting marker prim {marker_path}");
        commands.trigger(ApplyUsdOp {
            doc,
            op: UsdOp::RemovePrim {
                edit_target: LayerId::runtime(),
                path: marker_path,
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
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&big_space::prelude::Grid>,
    q_spatial: &Query<(Option<&big_space::grid::cell::CellCoord>, &Transform)>,
) -> Vec<DVec3> {
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
                positions.push(DVec3::new(x, y, z));
                continue;
            }
        }
        // 2. Try to resolve as USD prim path
        if let Some(&entity) = bindings.0.get(&t) {
            if let Some(pos) =
                lunco_core::coords::world_position(entity, q_parents, q_grids, q_spatial)
            {
                positions.push(pos.0);
            }
        }
    }
    positions
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
/// Uses [`lunco_core::coords::world_position`] for high-precision positions
/// that work correctly for all entities in the big_space hierarchy.
pub fn draw_waypoint_overlay(
    q_vessels: Query<(Entity, &BehaviorXml, Option<&TargetBindings>), With<UsdPrimPath>>,
    selected: Res<SelectedEntities>,
    q_camera: Query<(Entity, &Camera, &GlobalTransform), With<Camera3d>>,
    q_avatar_cam: Query<Entity, With<Avatar>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::grid::cell::CellCoord>, &Transform)>,
    q_link: Query<&ControllerLink>,
    mut egui_ctx: bevy_egui::EguiContexts,
    theme: Option<Res<lunco_theme::Theme>>,
) {
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

    // Camera world position for distance-based sizing.
    let cam_world =
        lunco_core::coords::world_position(cam_entity, &q_parents, &q_grids, &q_spatial)
            .map(|p| p.0)
            .unwrap_or(bevy::math::DVec3::ZERO);

    // Under the chrome, over the 3D — see `billboard_overlay::world_overlay_layer`.
    let painter = ctx.layer_painter(super::billboard_overlay::world_overlay_layer(
        "waypoint_overlay",
    ));

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

    for (vessel, xml, bindings) in q_vessels.iter() {
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

        // TODO(theme): migrate to lunco-theme once the token set covers this.
        // Route-line colour, selected vs unselected vessel. Currently dead (the
        // path is 3D geometry now — see the NOTE below), so the selected/dimmed
        // pair wants deciding alongside whatever replaces it.
        let line_color = if is_selected {
            egui::Color32::from_rgb(51, 242, 128) // bright green
        } else {
            egui::Color32::from_rgb(102, 128, 102) // dim green
        };
        let label_color = theme.tokens.text;

        let wp_positions =
            get_waypoint_positions(&xml.0, bindings, &q_parents, &q_grids, &q_spatial);

        // Collect screen-space points for each waypoint that is in front of the camera.
        struct WpScreen {
            screen: egui::Pos2,
            index: usize,
            distance: f64,
        }
        let mut wp_screens: Vec<WpScreen> = Vec::with_capacity(wp_positions.len());

        for (i, wp_world) in wp_positions.into_iter().enumerate() {
            let distance = (wp_world - cam_world).length();

            // Convert to camera-relative Vec3 for projection.
            let cam_relative = (wp_world - cam_world).as_vec3();
            let world_f32 = cam_gtf.translation() + cam_relative;

            let Ok(viewport) = camera.world_to_viewport(cam_gtf, world_f32) else {
                continue;
            };
            let screen = egui::pos2(viewport.x, viewport.y) + origin;

            wp_screens.push(WpScreen {
                screen,
                index: i,
                distance,
            });
        }

        // NOTE: the route LINE is not drawn here. A screen-space overlay stroke has no
        // depth, so it painted straight over terrain and over other waypoints and read
        // as a buggy, overlapping gizmo. The path is real 3D geometry instead — see
        // `sync_waypoint_path_mesh`, which builds a ground-hugging ribbon that occludes
        // correctly. Only the NUMBER labels stay in egui, where screen-space is right.
        let _ = line_color;

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
            let text = format!("{}", wp.index + 1);
            let font = egui::FontId::proportional(font_size);
            let tc = egui::Color32::from_rgba_unmultiplied(
                label_color.r(),
                label_color.g(),
                label_color.b(),
                alpha,
            );

            let galley = painter.layout_no_wrap(text, font, tc);
            let size = galley.size();
            let top_left = wp.screen - egui::vec2(size.x * 0.5, size.y + 8.0);

            let bg = egui::Rect::from_min_size(top_left, size).expand2(egui::vec2(4.0, 2.0));
            // TODO(theme): migrate to lunco-theme once the token set covers this.
            // Distance-faded chip behind a waypoint number over the 3D scene.
            // `overlay_backdrop` is the nearest token but carries its own fixed
            // alpha, which this needs to modulate per-waypoint.
            painter.rect_filled(
                bg,
                3.0,
                egui::Color32::from_black_alpha((180.0 * fade) as u8),
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
fn author_marker_prim(
    commands: &mut Commands,
    host: &lunco_doc::DocumentHost<lunco_usd::document::UsdDocument>,
    doc: lunco_doc::DocumentId,
    root: &str,
    at: DVec3,
) -> String {
    let route_scope = join_prim(root, ROUTE_SCOPE);
    // `AddPrim` on an existing prim is a rejection, not a merge.
    if !prim_exists(host, &route_scope) {
        commands.trigger(ApplyUsdOp {
            doc,
            op: UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path: root.to_string(),
                name: ROUTE_SCOPE.to_string(),
                type_name: Some("Scope".to_string()),
                reference: None,
            },
        });
    }
    // First free `W<n>` — the name a scene author would have written by hand.
    let marker_name = (0..)
        .map(|n| format!("W{n}"))
        .find(|name| !prim_exists(host, &join_prim(&route_scope, name)))
        .expect("an unbounded search always finds a free name");
    let marker_path = join_prim(&route_scope, &marker_name);
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: route_scope,
            name: marker_name,
            type_name: Some("Xform".to_string()),
            reference: Some(WAYPOINT_ASSET.to_string()),
        },
    });
    // The picked point is grid-absolute, the frame authored translates are in
    // (`persist_move_to_runtime_layer` writes a world position straight into
    // `SetTranslate`).
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetTranslate {
            edit_target: LayerId::runtime(),
            path: marker_path.clone(),
            value: [at.x, at.y, at.z],
        },
    });
    marker_path
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
/// this is the first waypoint — returns the path to author `info:sourceCode` onto.
///
/// The tree is a PROGRAM, not an attribute on the vessel: a mission is bolted on,
/// so it is a child prim that can be deleted to remove the behaviour, and the
/// behaviour engine is chosen by the source's extension exactly as `.mo` and
/// `.rhai` are. `process_usd_sim_prims` reads it back off this child and stamps
/// `BehaviorXml` on the vessel that owns it.
///
/// `AddPrim` on an existing prim is a rejection rather than a merge, so it is only
/// authored when genuinely absent — the same rule the `Behaviors` scope follows.
fn ensure_mission_program(
    commands: &mut Commands,
    host: &lunco_doc::DocumentHost<lunco_usd::document::UsdDocument>,
    doc: lunco_doc::DocumentId,
    vessel_path: &str,
) -> String {
    let path = join_prim(vessel_path, MISSION_PROGRAM);
    if !prim_exists(host, &path) {
        commands.trigger(ApplyUsdOp {
            doc,
            op: UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path: vessel_path.to_string(),
                name: MISSION_PROGRAM.to_string(),
                type_name: Some("Scope".to_string()),
                reference: None,
            },
        });
        commands.trigger(ApplyUsdOp {
            doc,
            op: UsdOp::SetApiSchemas {
                edit_target: LayerId::runtime(),
                path: path.clone(),
                schemas: vec!["LunCoProgramAPI".to_string()],
            },
        });
    }
    path
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

/// System that checks if a vessel is close to any of its waypoints.
///
/// Arrival, driven by the marker's OWN trigger zone — never a distance poll.
///
/// `vessels/markers/waypoint.usda` authors a `lunco:triggerZone` sensor around
/// every waypoint, and `lunco-mobility` fires `enter:<zone>` when a body crosses
/// it, carrying the ZONE's gid as the event source and the entrant's as payload.
/// That makes arrival an event about a specific pair of entities: zone names may
/// repeat across a scene without ambiguity, because the zone is identified by
/// entity, not by string.
///
/// Reaching a waypoint **deletes nothing**. Two states are written:
///
/// 1. the vessel's runtime [`ReachedWaypoints`] set — what the compiled tree
///    consumes to advance past a completed leg (so the autopilot skips it), and
/// 2. `lunco:waypoint:reached` on the marker prim, authored into the RUNTIME
///    layer — session state that never reaches the saved `.usda`.
///
/// The mission keeps its leg and the map keeps its pin, so a route can be
/// reviewed, replayed and re-run. The previous implementation swept every
/// vessel × every target every tick, measuring distances, to discover the same
/// fact the physics engine had already reported.
pub fn mark_reached_waypoints_on_zone_enter(
    trigger: On<lunco_core::TelemetryEvent>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_prim: Query<&UsdPrimPath>,
    mut q_vessels: Query<(Entity, &BehaviorXml, Option<&mut ReachedWaypoints>)>,
    doc_ctx: WaypointDocContext,
    mut commands: Commands,
) {
    let event = trigger.event();
    if !event.name.starts_with("enter:") {
        return;
    }
    // The zone entity is the event SOURCE; its marker prim is its parent path.
    let Some(zone) = registry.resolve(&lunco_core::GlobalEntityId::from_raw(event.source)) else {
        return;
    };
    let Ok(zone_prim) = q_prim.get(zone) else {
        return;
    };
    let Some((marker_path, _)) = zone_prim.path.rsplit_once('/') else {
        return;
    };
    let marker_path = marker_path.to_string();

    // Which vessel owns a leg pointing at this marker? A scene can hold several
    // routes, and only the one that authored this target has arrived.
    for (vessel, xml, reached) in q_vessels.iter_mut() {
        let Ok(value) = lunco_autopilot::btcpp_xml::xml_to_value(&xml.0) else {
            continue;
        };
        let mut targets = Vec::new();
        collect_targets(&value, &mut targets);
        if !targets.iter().any(|t| *t == marker_path) {
            continue;
        }
        let known = reached
            .as_ref()
            .map(|r| r.0.contains(&marker_path))
            .unwrap_or(false);
        if known {
            continue;
        }
        info!("[waypoint] reached {marker_path} (zone enter)");
        // Writing the component fires `Changed<ReachedWaypoints>`, which
        // `compile_behavior_xml` watches, so the tree re-strips the completed leg
        // and the rover advances immediately.
        match reached {
            Some(mut r) => {
                r.0.insert(marker_path.clone());
            }
            None => {
                commands.entity(vessel).insert(ReachedWaypoints(
                    std::iter::once(marker_path.clone()).collect(),
                ));
            }
        }
        // Author the same fact onto the marker, in the runtime layer only.
        if let Ok(vessel_prim) = q_prim.get(vessel) {
            if let Some(doc) = doc_ctx.resolve_document(&vessel_prim.stage_handle) {
                commands.trigger(ApplyUsdOp {
                    doc,
                    op: UsdOp::SetAttribute {
                        edit_target: LayerId::runtime(),
                        path: marker_path.clone(),
                        name: "lunco:waypoint:reached".to_string(),
                        type_name: "bool".to_string(),
                        value: "true".to_string(),
                    },
                });
            }
        }
    }
}

/// Grabbing the controls takes the vessel back: any manual DRIVE intent disengages
/// the autopilot currently driving it and returns ownership to the local session.
///
/// Without this the autopilot keeps the vessel claimed, so `drive_from_bindings`
/// yields and the player's input is silently swallowed — the rover just keeps driving
/// its route while you press the keys. Taking the wheel is the universal expectation
/// for an autopilot, so it is an implicit disengage rather than a separate hotkey
/// (KeyF still toggles explicitly).
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
    const DRIVE: [lunco_core::UserIntent; 6] = [
        MoveForward,
        MoveBackward,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
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
        // Reclaim ownership for the player, exactly as the KeyF toggle does —
        // otherwise the vessel is left unowned and the input still goes nowhere.
        if let Ok(gid) = q_gid.get(vessel) {
            let _ = registry.claim(SessionId::LOCAL, gid.get());
        }
    }
}

/// System that toggles autopilot driving state for the possessed vessel on KeyF press.
pub fn handle_autopilot_toggle_hotkey(
    keys: Res<ButtonInput<KeyCode>>,
    egui_focus: Res<EguiFocus>,
    avatars: Query<Entity, With<Avatar>>,
    q_link: Query<&ControllerLink>,
    mut commands: Commands,
) {
    if egui_focus.wants_keyboard {
        return;
    }
    if keys.just_pressed(KeyCode::KeyF) {
        if let Some(av) = avatars.iter().next() {
            if let Ok(link) = q_link.get(av) {
                let vessel = link.vessel_entity;
                commands.trigger(ToggleAutopilot { vessel });
            }
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
    mut registry: ResMut<SessionRegistry>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let vessel = cmd.vessel;
    let autopilot_engaged = q_autopilot.iter().any(|(_, ap)| ap.vessel == vessel);
    if !autopilot_engaged {
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

register_commands!(
    on_start_autopilot,
    on_toggle_autopilot,
    on_cancel_waypoint_edit
);

// ── Route ribbon (real 3D geometry, not a screen-space overlay) ───────────────

/// Half-width (world units) of the route ribbon — a thin drawn line, not a road.
const PATH_HALF_WIDTH: f32 = 0.12;
/// Lift above the sampled path so the ribbon doesn't z-fight the terrain it hugs.
const PATH_LIFT: f32 = 0.12;
/// Resample spacing for a `smooth` route's ribbon. Matches the autopilot's own
/// resampling, so the drawn curve IS the driven curve.
const PATH_SPACING: f64 = 2.0;

/// A vessel's route ribbon. `signature` is what the mesh was built from, so the
/// (relatively expensive) rebuild only happens when the route actually changes —
/// not every frame.
///
/// A route draws as up to TWO ribbons: the already-driven prefix and the remaining
/// path. The driven part is NOT removed — it stays visible in a dimmed colour so the
/// mission reads as a whole — so `part` distinguishes them.
#[derive(Component)]
pub struct WaypointPathMesh {
    pub vessel: Entity,
    pub signature: u64,
    pub part: PathPart,
}

/// Which half of a route a ribbon draws.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PathPart {
    /// Already driven this session — dimmed, but still drawn.
    Driven,
    /// Still to drive — the live route.
    Remaining,
}

/// Cheap change-signature for a route: its ordered coord keys + smooth flag +
/// which points are already visited (visited legs drop out of the curve).
fn route_signature(targets: &[String], smooth: bool, reached: Option<&ReachedWaypoints>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    smooth.hash(&mut h);
    for t in targets {
        t.hash(&mut h);
        reached
            .map(|r| r.0.contains(t))
            .unwrap_or(false)
            .hash(&mut h);
    }
    h.finish()
}

/// Build a flat ground-hugging ribbon through `points`, with vertices expressed
/// relative to `anchor` (the entity's own origin) so f32 vertex precision stays
/// tight regardless of how far the route sits from the world origin.
fn build_ribbon_mesh(points: &[DVec3], anchor: DVec3) -> Option<Mesh> {
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
        let base = (points[i] - anchor).as_vec3() + Vec3::Y * PATH_LIFT;
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
        &BehaviorXml,
        Option<&TargetBindings>,
        Option<&ReachedWaypoints>,
    )>,
    selected: Res<SelectedEntities>,
    q_avatar: Query<&ControllerLink, With<Avatar>>,
    q_paths: Query<(Entity, &WaypointPathMesh)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<(Entity, &big_space::prelude::Grid)>,
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
    let Some((grid_entity, grid)) = q_grids.iter().next() else {
        return;
    };
    let grid_world = {
        let q_spatial = spatial.p0();
        lunco_core::coords::world_position(grid_entity, &q_parents, &q_grids_only, &q_spatial)
            .unwrap_or(lunco_core::coords::GridPos(DVec3::ZERO))
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
                let position = lunco_core::coords::world_position(
                    entity,
                    &q_parents,
                    &q_grids_only,
                    &q_spatial,
                )?;
                let ground = surface.height_at(position)?;
                let target = lunco_core::coords::GridPos(DVec3::new(
                    position.0.x,
                    ground + PATH_LIFT as f64,
                    position.0.z,
                ));
                Some((
                    entity,
                    lunco_core::coords::world_to_grid_local(target, grid_world, grid),
                ))
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
    let mut existing: std::collections::HashMap<(Entity, PathPart), (Entity, u64)> =
        std::collections::HashMap::new();
    for (e, path) in q_paths.iter() {
        existing.insert((path.vessel, path.part), (e, path.signature));
    }

    let vessel_entities: std::collections::HashSet<Entity> =
        q_vessels.iter().map(|(entity, ..)| entity).collect();
    let selected_vessel = route_owner(selected.primary(), &q_parents, &vessel_entities);
    let possessed_vessel = route_owner(
        q_avatar.iter().next().map(|link| link.vessel_entity),
        &q_parents,
        &vessel_entities,
    );
    for (vessel, xml, bindings, reached) in q_vessels.iter() {
        // Every route draws. Focus (possessed / selected) only decides how loud it is
        // — a route that vanishes unless you possess its vessel cannot be clicked on,
        // and the waypoint editor is right-click-on-the-pin.
        let focused = Some(vessel) == selected_vessel || Some(vessel) == possessed_vessel;
        let Ok(value) = lunco_autopilot::btcpp_xml::xml_to_value(&xml.0) else {
            continue;
        };
        let mut targets = Vec::new();
        collect_targets(&value, &mut targets);
        let smooth = route_is_smooth(&xml.0);
        // Focus is part of the signature: it changes the ribbon's colour, so a
        // selection change has to rebuild it (the mesh is only rebuilt when this
        // number moves).
        // Terrain may finish streaming after the route binding.  Include its
        // availability in the change key so a ribbon first created during the
        // loading frame is rebuilt once the analytic surface can clamp its
        // interpolated samples.
        let signature = route_signature(&targets, smooth, reached)
            ^ (focused as u64)
            ^ ((surface.has_terrain() as u64) << 1);

        // All control points, in order, each tagged with whether it's been driven.
        let pts: Vec<(DVec3, bool)> = targets
            .iter()
            .filter_map(|t| {
                let pos = bindings.and_then(|b| b.0.get(t)).and_then(|&entity| {
                    lunco_core::coords::world_position(
                        entity,
                        &q_parents,
                        &q_grids_only,
                        &q_spatial,
                    )
                    .map(|p| p.0)
                });
                pos.map(|p| (p, reached.map(|r| r.0.contains(t)).unwrap_or(false)))
            })
            .collect();
        // The rover consumes the route in order, so the driven part is a prefix. Split
        // there and SHARE the boundary point, so the two ribbons meet with no gap.
        let driven_upto = pts
            .iter()
            .rposition(|(_, done)| *done)
            .map(|i| i + 1)
            .unwrap_or(0);
        let closed = xml.0.contains("forever") && pts.len() > 2;

        for part in [PathPart::Driven, PathPart::Remaining] {
            let slice: Vec<DVec3> = match part {
                // `min(len)` guards the boundary-sharing when everything is driven.
                PathPart::Driven if driven_upto > 0 => pts[..(driven_upto + 1).min(pts.len())]
                    .iter()
                    .map(|(p, _)| *p)
                    .collect(),
                PathPart::Remaining if driven_upto < pts.len() => pts
                    [driven_upto.saturating_sub(1)..]
                    .iter()
                    .map(|(p, _)| *p)
                    .collect(),
                _ => Vec::new(),
            };

            let key = (vessel, part);
            // Unchanged → leave this ribbon alone.
            if let Some(&(_, sig)) = existing.get(&key) {
                if sig == signature {
                    existing.remove(&key);
                    continue;
                }
            }
            if let Some((old, _)) = existing.remove(&key) {
                commands.entity(old).despawn();
            }
            if slice.len() < 2 {
                continue;
            }

            // Only the REMAINING half closes a `forever` loop back to the start —
            // the driven half is an open trail behind the rover.
            let close_this = closed && part == PathPart::Remaining && driven_upto == 0;
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
            let Some(mesh) = build_ribbon_mesh(&path, anchor) else {
                continue;
            };
            // Driven stays VISIBLE, just dimmed — the mission reads as a whole and you
            // can see where the rover has been; only the colour says "done".
            let (base_color, emissive) = match part {
                PathPart::Driven => (
                    LinearRgba::new(0.40, 0.42, 0.40, 0.30),
                    LinearRgba::new(0.22, 0.24, 0.22, 1.0),
                ),
                PathPart::Remaining => (
                    LinearRgba::new(0.15, 0.85, 0.45, 0.55),
                    LinearRgba::new(0.10, 0.70, 0.35, 1.0),
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
            let (cell, local) = lunco_core::coords::world_to_grid_local(
                lunco_core::coords::GridPos(anchor),
                grid_world,
                grid,
            );
            commands.spawn((
                Mesh3d(meshes.add(mesh)),
                PbrLook {
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
                },
                cell,
                Transform::from_translation(local),
                GlobalTransform::default(),
                ChildOf(grid_entity),
                WaypointPathMesh {
                    vessel,
                    signature,
                    part,
                },
            ));
        }
    }

    // Vessels/parts that no longer have a route.
    for (_, (entity, _)) in existing {
        commands.entity(entity).despawn();
    }
}
