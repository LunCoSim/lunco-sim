//! Alt+LMB — drop a mission waypoint by **authoring a USD prim**.
//!
//! (The gesture moved from Ctrl to Alt; `docs/waypoints-in-usd-design.md` and
//! `docs/sandbox-control-ui-plan.md` still say Ctrl. The code below is the truth.)
//!
//! There is no checkpoint domain. A waypoint is an ordinary prim referencing
//! `vessels/markers/waypoint.usda`, and the vessel's BT.CPP mission
//! (the `info:sourceCode` of its `LunCoProgram "Mission"` child) gains a `drive_to`
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
    append_waypoint_leaf, catmull_rom_path, format_coord_target, insert_waypoint_after,
    remove_waypoint_leaf, route_is_smooth, set_route_smooth, set_waypoint_dwell,
    set_waypoint_target, BehaviorXml, ReachedWaypoints, TargetBindings,
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

use crate::spawn::{terrain_ray_hit, TerrainOracles};
use crate::SelectedEntities;

/// Scope the authored waypoints are parented under, beneath the stage's default prim.
/// A route lives in WORLD space, so it is deliberately NOT a child of the vessel —
/// parented under the rover, the waypoints would ride along as it drives.
const BEHAVIORS_SCOPE: &str = "Behaviors";

/// Name of the `LunCoProgram` child that carries a vessel's mission tree.
const MISSION_PROGRAM: &str = "Mission";

/// Track context menu state for right-clicking waypoints.
#[derive(Resource, Default)]
pub struct WaypointContextMenuState {
    /// The waypoint VISUAL entity (carries [`WaypointVisual`]), not a prim.
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
#[derive(Debug)]
pub struct PendingPlacement {
    pub vessel: Entity,
    /// The leg to move, or the leg to insert after.
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

/// Grid-frame conversion bundle for the waypoint click observer. Bundled into one
/// [`SystemParam`] so the observer stays under Bevy's 16-argument limit, and so the
/// render→world math lives in one place. `cameras` rides along because it also reads
/// `GlobalTransform` (read-only, no alias with `q_gt`).
#[derive(bevy::ecs::system::SystemParam)]
pub struct WaypointClickFrame<'w, 's> {
    pub cameras: Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<Camera3d>>,
    pub q_parents: Query<'w, 's, &'static ChildOf>,
    pub q_grids: Query<'w, 's, (Entity, &'static big_space::prelude::Grid)>,
    pub q_grids_only: Query<'w, 's, &'static big_space::prelude::Grid>,
    pub q_spatial: Query<
        'w,
        's,
        (
            Option<&'static big_space::grid::cell::CellCoord>,
            &'static Transform,
        ),
    >,
    pub q_gt: Query<'w, 's, &'static GlobalTransform>,
}

impl WaypointClickFrame<'_, '_> {
    /// Convert a point in the RENDER (floating-origin) frame to WORLD (grid-absolute)
    /// space — the exact inverse of `bake_targets`' world→render bake, so a captured
    /// waypoint round-trips to the same target the driver steers toward, even when the
    /// floating origin is far from world zero or rebases mid-mission.
    fn render_to_world(&self, p: DVec3) -> DVec3 {
        let Some((grid_entity, _)) = self.q_grids.iter().next() else {
            return p; // no grid → render and world coincide
        };
        let grid_world = lunco_core::coords::world_position(
            grid_entity,
            &self.q_parents,
            &self.q_grids_only,
            &self.q_spatial,
        )
        .unwrap_or(DVec3::ZERO);
        let grid_floating = self
            .q_gt
            .get(grid_entity)
            .map(|gt| gt.translation())
            .unwrap_or(Vec3::ZERO);
        grid_world + (p - grid_floating.as_dvec3())
    }
}

/// Resolve the pointer to a point on the ground in **WORLD** (grid-absolute) space —
/// the one spelling of "where did the user click?" for the waypoint editor, shared by
/// the Alt+LMB drop and the Move / Insert-after placement click.
///
/// Casts through the active camera against BOTH the DEM oracle (ground truth over open
/// terrain, where the band-limited collider ring rounds a crater bowl) and the physics
/// colliders (structures/props), taking the nearer hit — the same pairing
/// `spawn::on_scene_click_spawn` uses. The hit comes back in the RENDER frame, so it is
/// converted to world via [`WaypointClickFrame::render_to_world`].
fn pick_ground_world(
    frame: &WaypointClickFrame,
    terrains: &TerrainOracles,
    raycaster: &lunco_physics::GridSpatialQuery,
    egui_focus: &EguiFocus,
    pointer: Vec2,
) -> Option<DVec3> {
    let (camera, cam_gtf) = frame.cameras.iter().find(|(c, _)| c.is_active)?;
    let ray = lunco_core::scene_click_ray(egui_focus, camera, cam_gtf, pointer)?;
    let origin = ray.origin.as_dvec3();
    let dir = ray.direction.as_dvec3();
    let phys = raycaster
        .cast_ray_render(
            origin,
            ray.direction,
            1.0e6,
            false,
            &avian3d::prelude::SpatialQueryFilter::default(),
        )
        .map(|h| h.distance);
    let terr = terrain_ray_hit(terrains, origin, dir, 1.0e6);
    let hit_render = match (phys, terr) {
        (Some(pd), Some((td, tp))) => {
            if td <= pd {
                tp
            } else {
                origin + dir * pd
            }
        }
        (Some(pd), None) => origin + dir * pd,
        (None, Some((_, tp))) => tp,
        (None, None) => return None,
    };
    Some(raycaster.to_physics(hit_render))
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
    terrains: TerrainOracles,
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
        &terrains,
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

    // ── The mission's topology ────────────────────────────────────────────────
    // Append the leaf FIRST: if the tree is a shape the editor must not restructure,
    // bail out.
    let current = q_xml.get(vessel).ok().map(|(_, x)| x.0.as_str());
    let wp_coord_str = format_coord_target(hit);
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
/// Targets the coordinate-waypoint VISUALS ([`WaypointVisual`], spawned by
/// `sync_waypoint_visuals`) — the pick can land on the sphere mesh itself, so walk up
/// to whichever ancestor carries the marker.
pub fn on_scene_right_click_waypoint(
    mut click: On<Pointer<Click>>,
    egui_focus: Res<EguiFocus>,
    q_visual: Query<&WaypointVisual>,
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
    for _ in 0..8 {
        if let Ok(visual) = q_visual.get(entity) {
            click.propagate(false);
            menu_state.entity = Some(entity);
            menu_state.position = click.pointer_location.position;
            menu_state.just_opened = true;
            // Seed the dwell buffer from the authored leg.
            menu_state.dwell = q_xml
                .get(visual.vessel)
                .ok()
                .and_then(|x| lunco_autopilot::usd_tree::waypoint_dwell(&x.0, &visual.coord_key))
                .unwrap_or(0.0);
            return;
        }
        let Ok(parent) = q_parents.get(entity) else {
            break;
        };
        entity = parent.parent();
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
    terrains: TerrainOracles,
    raycaster: lunco_physics::GridSpatialQuery,
    q_vessel: Query<(&BehaviorXml, &UsdPrimPath)>,
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
        &terrains,
        &raycaster,
        &egui_focus,
        click.pointer_location.position,
    ) else {
        info!("[waypoint] placement cancelled: no ground under the cursor");
        return;
    };
    let Ok((xml, vessel_prim)) = q_vessel.get(pending.vessel) else {
        warn!(
            "[waypoint] placement failed: vessel {:?} has no BehaviorXml/UsdPrimPath",
            pending.vessel
        );
        return;
    };
    let Some(doc) = doc_ctx.resolve_document(&vessel_prim.stage_handle) else {
        warn!("[waypoint] placement failed: no document found for vessel");
        return;
    };

    let new_target = format_coord_target(world);
    let edited = match pending.mode {
        PlacementMode::Move => set_waypoint_target(&xml.0, &pending.coord_key, &new_target),
        PlacementMode::InsertAfter => {
            insert_waypoint_after(&xml.0, &pending.coord_key, &new_target)
        }
    };
    match edited {
        Ok(new_xml) => {
            info!("[waypoint] {:?} → {}", pending.mode, new_target);
            commands
                .entity(pending.vessel)
                .insert(BehaviorXml(new_xml.clone()));
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
    q_visual: Query<&WaypointVisual>,
    q_vessel: Query<(&BehaviorXml, &UsdPrimPath)>,
    usd_registry: Option<Res<DocumentRegistry<UsdDocument>>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut commands: Commands,
) {
    let Some(vis_entity) = menu_state.entity else {
        if menu_open.0 {
            menu_open.0 = false; // release the camera
        }
        return;
    };
    // The visual can vanish under the menu (route edited elsewhere) — close, don't panic.
    let Ok(visual) = q_visual.get(vis_entity) else {
        menu_state.entity = None;
        menu_open.0 = false;
        return;
    };
    let Ok((xml, vessel_prim)) = q_vessel.get(visual.vessel) else {
        menu_state.entity = None;
        menu_open.0 = false;
        return;
    };
    // Hold the camera still for as long as the menu is up.
    menu_open.0 = true;
    let Some(doc) = workspace
        .and_then(|w| w.0.active_document)
        .or_else(|| usd_registry.as_ref().and_then(|r| r.ids().next()))
    else {
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

    let response = egui::Area::new(egui::Id::new("waypoint_context_menu"))
        .fixed_pos(pos)
        .order(egui::Order::Foreground)
        .constrain(true) // never let it spill off-screen near the window edge
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_width(190.0);
                ui.label(egui::RichText::new(format!("Waypoint {}", visual.index + 1)).strong());
                if visual.passed {
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
                        vessel: visual.vessel,
                        coord_key: visual.coord_key.clone(),
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
                    info!("[waypoint] armed Insert-after of '{}'", visual.coord_key);
                    placement.0 = Some(PendingPlacement {
                        vessel: visual.vessel,
                        coord_key: visual.coord_key.clone(),
                        mode: PlacementMode::InsertAfter,
                    });
                    open = false;
                }
                if ui.button("❌  Delete").clicked() {
                    match remove_waypoint_leaf(&xml.0, &visual.coord_key) {
                        Ok(new_xml) => edited = Some(new_xml),
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
                        match set_waypoint_dwell(&xml.0, &visual.coord_key, dwell) {
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
                positions.push(pos);
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

/// Marker component for local waypoint visual entities.
#[derive(Component)]
pub struct WaypointVisual {
    /// The vessel entity this waypoint is for.
    pub vessel: Entity,
    /// Stable identity: the raw "x;y;z" coordinate string from the BehaviorXml.
    /// Keyed by string rather than sequence index so that removing the first
    /// waypoint (which shifts all subsequent indices down by one) does not
    /// cause every remaining sphere to be despawned and respawned.
    pub coord_key: String,
    /// The index of this waypoint in the patrol sequence (for label display).
    pub index: usize,
    /// Absolute world position of the waypoint.
    pub position: DVec3,
    /// Whether this waypoint has been reached on a previous run.
    /// Passed waypoints render differently (grey) but stay visible.
    pub passed: bool,
}

/// System that spawns and updates local visual-only translucent green spheres
/// for all coordinate-based waypoints stored in vessels' BehaviorXml.
/// This prevents polluting the USD stage with waypoint prims.
pub fn sync_waypoint_visuals(
    q_vessels: Query<(
        Entity,
        &BehaviorXml,
        Option<&TargetBindings>,
        Option<&ReachedWaypoints>,
    )>,
    q_visuals: Query<(Entity, &WaypointVisual)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<(Entity, &big_space::prelude::Grid)>,
    q_grids_only: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::grid::cell::CellCoord>, &Transform)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    // 1. Gather all desired waypoints (coordinates or resolved USD prim paths) from ALL vessels.
    // Key: (vessel, coord_key) → (index, world_pos, passed).
    // coord_key is the raw "x;y;z" string or USD prim path — stable across sequence-index shifts.
    // All vessels' waypoints are shown so the full mission map is visible.
    // `passed` is read from the live-only `ReachedWaypoints` set, never the XML.
    let mut desired: std::collections::HashMap<(Entity, String), (usize, DVec3, bool)> =
        std::collections::HashMap::new();
    for (vessel, xml, bindings, reached) in q_vessels.iter() {
        let Ok(value) = lunco_autopilot::btcpp_xml::xml_to_value(&xml.0) else {
            continue;
        };
        let mut targets = Vec::new();
        collect_targets(&value, &mut targets);
        let mut idx = 0usize;
        for target in &targets {
            let pos = parse_coord_target(target).or_else(|| {
                bindings.and_then(|b| b.0.get(target)).and_then(|&entity| {
                    lunco_core::coords::world_position(
                        entity,
                        &q_parents,
                        &q_grids_only,
                        &q_spatial,
                    )
                })
            });
            let Some(pos) = pos else { continue };
            let passed = reached.map(|r| r.0.contains(target)).unwrap_or(false);
            desired.insert((vessel, target.clone()), (idx, pos, passed));
            idx += 1;
        }
    }

    // 2. Identify existing visuals, keyed by stable (vessel, coord_key).
    // Value is (visual_entity, current_passed_state) so we can re-spawn when colour changes.
    let mut existing: std::collections::HashMap<(Entity, String), (Entity, bool)> =
        std::collections::HashMap::new();
    for (entity, visual) in q_visuals.iter() {
        existing.insert(
            (visual.vessel, visual.coord_key.clone()),
            (entity, visual.passed),
        );
    }

    // Get active grid for placing visuals.
    let Some((grid_entity, grid)) = q_grids.iter().next() else {
        return;
    };
    let grid_world =
        lunco_core::coords::world_position(grid_entity, &q_parents, &q_grids_only, &q_spatial)
            .unwrap_or(DVec3::ZERO);

    // 3. Spawn or update desired visuals.
    for ((vessel, coord_key), (index, pos, passed)) in desired {
        let (cell, local_pos) = lunco_core::coords::world_to_grid_local(pos, grid_world, grid);

        if let Some((entity, existing_passed)) = existing.remove(&(vessel, coord_key.clone())) {
            if existing_passed == passed {
                // Same colour: just update position. `try_insert`, not `insert`: a scene
                // load can despawn this visual between the query snapshot and command
                // application, and a bare `insert` on the dead entity panics the schedule.
                commands
                    .entity(entity)
                    .try_insert((cell, Transform::from_translation(local_pos)));
                continue;
            }
            // Passed state changed (green → grey): despawn and fall through to re-spawn.
            commands.entity(entity).despawn();
        }
        // Colour: green = active target, grey = already visited.
        let (base_color, emissive) = if passed {
            (
                LinearRgba::new(0.45, 0.45, 0.45, 0.18),
                LinearRgba::new(0.3, 0.3, 0.3, 1.0),
            )
        } else {
            (
                LinearRgba::new(0.2, 0.95, 0.5, 0.28),
                LinearRgba::new(0.12, 0.85, 0.42, 1.0),
            )
        };
        commands.spawn((
            Mesh3d(meshes.add(Sphere::new(2.5).mesh().ico(5).unwrap())),
            PbrLook {
                base_color,
                emissive,
                alpha: SurfaceAlpha::Blend,
                unlit: true,
                // Same reason as the route ribbon: an editor pin must not cast a
                // shadow onto the scene it annotates.
                no_shadow_cast: true,
                ..default()
            },
            cell,
            Transform::from_translation(local_pos),
            GlobalTransform::default(),
            ChildOf(grid_entity),
            WaypointVisual {
                vessel,
                coord_key,
                index,
                position: pos,
                passed,
            },
        ));
    }

    // 4. Despawn only the visuals whose coord_key is no longer in the XML.
    for (_, (entity, _)) in existing {
        commands.entity(entity).despawn();
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
            .unwrap_or(bevy::math::DVec3::ZERO);

    // Under the chrome, over the 3D — see `billboard_overlay::world_overlay_layer`.
    let painter = ctx.layer_painter(super::billboard_overlay::world_overlay_layer(
        "waypoint_overlay",
    ));

    let primary_selected = selected.primary();
    let possessed_vessel = q_avatar_cam
        .iter()
        .next()
        .and_then(|av| q_link.get(av).ok().map(|link| link.vessel_entity));

    for (vessel, xml, bindings) in q_vessels.iter() {
        let empty_bindings = TargetBindings::default();
        let bindings = bindings.unwrap_or(&empty_bindings);

        let is_possessed = Some(vessel) == possessed_vessel;
        let is_selected = Some(vessel) == primary_selected;
        if (possessed_vessel.is_some() || primary_selected.is_some())
            && !is_possessed
            && !is_selected
        {
            continue;
        }

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

/// Join a parent prim path and a child name, handling the stage root (`"/"`).
fn join_prim(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// The `LunCoProgram` prim that carries a vessel's mission tree, creating it if
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
                type_name: Some("LunCoProgram".to_string()),
                reference: None,
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

/// Component that marks reached waypoints to prevent double deletion.
#[derive(Component)]
pub struct WaypointReached;

/// How close (world units) the vessel must get for a waypoint to count as reached.
pub const WAYPOINT_ARRIVAL: f64 = 4.0;

/// Parse a `"x;y;z"` coord target. `None` for a prim-path target.
fn parse_coord_target(target: &str) -> Option<DVec3> {
    let p: Vec<&str> = target.split(';').collect();
    if p.len() != 3 {
        return None;
    }
    match (
        p[0].trim().parse(),
        p[1].trim().parse(),
        p[2].trim().parse(),
    ) {
        (Ok(x), Ok(y), Ok(z)) => Some(DVec3::new(x, y, z)),
        _ => None,
    }
}

/// System that checks if a vessel is close to any of its waypoints.
///
/// A **coordinate** waypoint is recorded in the vessel's runtime [`ReachedWaypoints`]
/// set — LIVE-ONLY state that greys the pin and strips the leg from the compiled tree
/// so the rover advances. It is deliberately never written to the XML or USD: the
/// waypoint-drop path re-authors the whole mission `info:sourceCode` string through
/// `ApplyUsdOp`, so a flag living in that XML would get journaled and baked into the
/// saved `.usda` and survive a reload. Keeping it in a component means it simply
/// resets each session.
///
/// A **prim** waypoint (the legacy path-based form) is a real authored prim, so
/// reaching it genuinely deletes it through the one authoring funnel.
pub fn delete_reached_waypoints(
    mut q_vessels: Query<(
        Entity,
        &mut BehaviorXml,
        Option<&TargetBindings>,
        &UsdPrimPath,
        Option<&mut ReachedWaypoints>,
    )>,
    q_waypoints: Query<(Entity, &UsdPrimPath), Without<WaypointReached>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::grid::cell::CellCoord>, &Transform)>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    usd_registry: Option<Res<DocumentRegistry<UsdDocument>>>,
    mut commands: Commands,
) {
    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace
        .0
        .active_document
        .or_else(|| usd_registry.as_ref().and_then(|r| r.ids().next()))
    else {
        return;
    };

    for (vessel, mut xml, bindings, vessel_path, mut reached) in q_vessels.iter_mut() {
        let Some(vessel_pos) =
            lunco_core::coords::world_position(vessel, &q_parents, &q_grids, &q_spatial)
        else {
            continue;
        };

        // Parse all targets from behavior XML
        let Ok(value) = lunco_autopilot::btcpp_xml::xml_to_value(&xml.0) else {
            continue;
        };
        let mut targets = Vec::new();
        collect_targets(&value, &mut targets);

        let mut newly_reached: Vec<String> = Vec::new();
        for target in &targets {
            // 1. Coordinate waypoint → runtime-only reached set.
            if let Some(wp_pos) = parse_coord_target(target) {
                if (wp_pos - vessel_pos).length() < WAYPOINT_ARRIVAL {
                    let known = reached
                        .as_ref()
                        .map(|r| r.0.contains(target))
                        .unwrap_or(false);
                    if !known && !newly_reached.iter().any(|t| t == target) {
                        info!("Waypoint reached (live-only, not persisted): {}", target);
                        newly_reached.push(target.clone());
                    }
                }
                continue;
            }

            // 2. Try path resolution
            if let Some(bindings) = bindings {
                if let Some(&wp_entity) = bindings.0.get(target) {
                    if let Ok((entity, prim_path)) = q_waypoints.get(wp_entity) {
                        let Some(wp_pos) = lunco_core::coords::world_position(
                            entity, &q_parents, &q_grids, &q_spatial,
                        ) else {
                            continue;
                        };
                        let distance = (wp_pos - vessel_pos).length();
                        if distance < 4.0 {
                            info!("Waypoint prim reached: deleting {}", prim_path.path);

                            commands.entity(entity).insert(WaypointReached);

                            commands.trigger(ApplyUsdOp {
                                doc,
                                op: UsdOp::RemovePrim {
                                    edit_target: LayerId::runtime(),
                                    path: prim_path.path.clone(),
                                },
                            });

                            if let Ok(new_xml) = remove_waypoint_leaf(&xml.0, &prim_path.path) {
                                xml.0 = new_xml.clone();
                                commands.trigger(ApplyUsdOp {
                                    doc,
                                    op: UsdOp::SetAttribute {
                                        edit_target: LayerId::runtime(),
                                        path: join_prim(&vessel_path.path, MISSION_PROGRAM),
                                        name: "info:sourceCode".to_string(),
                                        type_name: "string".to_string(),
                                        value: new_xml,
                                    },
                                });
                            }
                        }
                    }
                }
            }
        }

        // Commit this tick's arrivals to the live-only set. Writing the component
        // fires `Changed<ReachedWaypoints>`, which `compile_behavior_xml` watches, so
        // the tree re-strips and the rover advances immediately — with nothing
        // touching the document.
        if !newly_reached.is_empty() {
            match reached.as_mut() {
                Some(r) => r.0.extend(newly_reached),
                None => {
                    commands
                        .entity(vessel)
                        .insert(ReachedWaypoints(newly_reached.into_iter().collect()));
                }
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
    q_paths: Query<(Entity, &WaypointPathMesh)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<(Entity, &big_space::prelude::Grid)>,
    q_grids_only: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::grid::cell::CellCoord>, &Transform)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    let Some((grid_entity, grid)) = q_grids.iter().next() else {
        return;
    };
    let grid_world =
        lunco_core::coords::world_position(grid_entity, &q_parents, &q_grids_only, &q_spatial)
            .unwrap_or(DVec3::ZERO);

    // Existing ribbons, keyed by (vessel, part).
    let mut existing: std::collections::HashMap<(Entity, PathPart), (Entity, u64)> =
        std::collections::HashMap::new();
    for (e, path) in q_paths.iter() {
        existing.insert((path.vessel, path.part), (e, path.signature));
    }

    for (vessel, xml, bindings, reached) in q_vessels.iter() {
        let Ok(value) = lunco_autopilot::btcpp_xml::xml_to_value(&xml.0) else {
            continue;
        };
        let mut targets = Vec::new();
        collect_targets(&value, &mut targets);
        let smooth = route_is_smooth(&xml.0);
        let signature = route_signature(&targets, smooth, reached);

        // All control points, in order, each tagged with whether it's been driven.
        let pts: Vec<(DVec3, bool)> = targets
            .iter()
            .filter_map(|t| {
                let pos = parse_coord_target(t).or_else(|| {
                    bindings.and_then(|b| b.0.get(t)).and_then(|&entity| {
                        lunco_core::coords::world_position(
                            entity,
                            &q_parents,
                            &q_grids_only,
                            &q_spatial,
                        )
                    })
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
            let (cell, local) = lunco_core::coords::world_to_grid_local(anchor, grid_world, grid);
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
