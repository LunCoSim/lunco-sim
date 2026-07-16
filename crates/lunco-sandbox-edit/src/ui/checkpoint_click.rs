//! Ctrl+LMB — drop a mission waypoint by **authoring a USD prim**.
//!
//! There is no checkpoint domain. A waypoint is an ordinary prim referencing
//! `vessels/markers/waypoint.usda`, and the vessel's BT.CPP mission
//! (`lunco:behavior`) gains a `drive_to` leaf that names it by path. Both edits go
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

use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::math::DVec3;
use serde_json::Value;
use lunco_render::{PbrLook, SurfaceAlpha};
use std::collections::HashMap;
use bevy_egui::egui;
use lunco_autopilot::usd_tree::{append_waypoint_leaf, remove_waypoint_leaf, BehaviorXml, TargetBindings};
use lunco_controller::ControllerLink;
use lunco_core::{Avatar, EguiFocus, SpawnToolActive, TerrainToolActive, GlobalEntityId};
use lunco_core::session::SessionRegistry;
use lunco_core::commands::SessionId;
use lunco_usd::document::{LayerId, UsdOp};
use lunco_usd::registry::UsdDocumentRegistry;
use lunco_usd::commands::ApplyUsdOp;
use lunco_usd_bevy::UsdPrimPath;

use crate::spawn::{terrain_ray_hit, TerrainOracles};
use crate::SelectedEntities;


/// Scope the authored waypoints are parented under, beneath the stage's default prim.
/// A route lives in WORLD space, so it is deliberately NOT a child of the vessel —
/// parented under the rover, the waypoints would ride along as it drives.
const BEHAVIORS_SCOPE: &str = "Behaviors";

/// Track context menu state for right-clicking waypoints.
#[derive(Resource, Default)]
pub struct WaypointContextMenuState {
    pub entity: Option<Entity>,
    pub position: Vec2,
    pub just_opened: bool,
}

/// Global `Pointer<Click>` observer: Ctrl+LMB drops a waypoint prim for the selected
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
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    terrains: TerrainOracles,
    raycaster: avian3d::prelude::SpatialQuery,
    q_prim: Query<&UsdPrimPath>,
    q_xml: Query<&BehaviorXml>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,

    mut commands: Commands,
) {
    if egui_focus.wants_pointer {
        info!("[waypoint] click ignored: egui wants pointer");
        return;
    }
    if spawn_tool.0 || terrain_tool.0 {
        info!("[waypoint] click ignored: spawn_tool={} terrain_tool={}", spawn_tool.0, terrain_tool.0);
        return;
    }
    if click.button != PointerButton::Primary {
        return;
    }
    // Alt+LMB only — a plain click possesses, Shift+click selects, Alt+click drops waypoints
    if !(keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight)) {
        return;
    }

    // Now that we are sure this is a Alt+LMB click meant for a waypoint, stop propagation.
    click.propagate(false);

    // Default to the possessed vessel first, then fall back to the selected one
    let possessed_vessel = avatars.iter().next().and_then(|av| q_link.get(av).ok().map(|link| link.vessel_entity));
    let Some(vessel) = possessed_vessel.or_else(|| selected.primary()) else {
        info!("[waypoint] click ignored: no vessel possessed and no vessel selected");
        return;
    };
    let Ok(vessel_prim) = q_prim.get(vessel) else {
        warn!("[waypoint] target vessel {:?} is not a USD prim; cannot author a mission for it", vessel);
        return;
    };
    let Some(workspace) = workspace else {
        info!("[waypoint] click ignored: no workspace resource");
        return;
    };
    let Some(doc) = workspace.0.active_document.or_else(|| {
        let fallback = usd_registry.ids().next();
        info!("[waypoint] fallback: resolved active document from USD registry: {:?}", fallback);
        fallback
    }) else {
        info!("[waypoint] click ignored: no active document and USD registry is empty");
        return;
    };
    let Some(host) = usd_registry.host(doc) else {
        info!("[waypoint] click ignored: no USD host for active document");
        return;
    };

    // Ground-truth terrain hit (the DEM oracle, not the band-limited collider ring) —
    // the same path `spawn::on_scene_click_spawn` uses.
    let Some((camera, cam_gtf)) = cameras.iter().find(|(c, _)| c.is_active) else {
        info!("[waypoint] click ignored: no active camera");
        return;
    };
    let Some(ray) = lunco_core::scene_click_ray(
        &egui_focus,
        camera,
        cam_gtf,
        click.pointer_location.position,
    ) else {
        info!("[waypoint] click ignored: scene_click_ray failed");
        return;
    };
    
    let origin = ray.origin.as_dvec3();
    let dir = ray.direction.as_dvec3();
    let phys = raycaster
        .cast_ray(origin, ray.direction, 1.0e6, false, &avian3d::prelude::SpatialQueryFilter::default())
        .map(|h| h.distance);
    let terr = terrain_ray_hit(&terrains, origin, dir, 1.0e6);
    
    let hit = match (phys, terr) {
        (Some(pd), Some((td, tp))) => {
            if td <= pd { tp } else { origin + dir * pd }
        }
        (Some(pd), None) => origin + dir * pd,
        (None, Some((_, tp))) => tp,
        (None, None) => {
            info!("[waypoint] click ignored: raycast hit neither physics nor terrain");
            return;
        }
    };
    
    info!("[waypoint] dropping waypoint at {:?}", hit);

    // ── Where the pin goes ────────────────────────────────────────────────────
    let root = lunco_usd_bevy::layer_default_prim(host.document().data())
        .map(|p| format!("/{p}"))
        .unwrap_or_else(|| "/".to_string());
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
    let current = q_xml.get(vessel).ok().map(|x| x.0.as_str());
    let wp_coord_str = format!("{:.6};{:.6};{:.6}", hit.x, hit.y, hit.z);
    let xml = match append_waypoint_leaf(current, &wp_coord_str) {
        Ok(xml) => xml,
        Err(err) => {
            warn!("[waypoint] not adding a checkpoint: {err}");
            return;
        }
    };

    // ── Author: only update the mission that names it ────────────────────────
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetAttribute {
            edit_target: LayerId::runtime(),
            path: vessel_prim.path.clone(),
            name: "lunco:behavior".to_string(),
            type_name: "string".to_string(),
            value: xml,
        },
    });
}

/// Global `Pointer<Click>` observer: Right click on a waypoint opens its context menu.
pub fn on_scene_right_click_waypoint(
    mut click: On<Pointer<Click>>,
    egui_focus: Res<EguiFocus>,
    q_prim: Query<&UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    mut menu_state: ResMut<WaypointContextMenuState>,
) {
    if egui_focus.wants_pointer {
        return;
    }
    if click.button != PointerButton::Secondary {
        return;
    }

    let mut entity = click.entity;
    loop {
        if let Ok(prim_path) = q_prim.get(entity) {
            if prim_path.path.contains("/Behaviors/") && prim_path.path.contains("_wp") {
                click.propagate(false);
                menu_state.entity = Some(entity);
                menu_state.position = click.pointer_location.position;
                menu_state.just_opened = true;
                break;
            }
        }
        if let Ok(parent) = q_parents.get(entity) {
            entity = parent.parent();
        } else {
            break;
        }
    }
}

/// System to draw context menu popup using egui Area.
pub fn draw_waypoint_context_menu(
    mut contexts: bevy_egui::EguiContexts,
    mut menu_state: ResMut<WaypointContextMenuState>,
    q_prim: Query<&UsdPrimPath>,
    q_vessels_xml: Query<(Entity, &BehaviorXml, &UsdPrimPath)>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut commands: Commands,
) {
    let Some(entity) = menu_state.entity else {
        return;
    };

    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let pos = menu_state.position;
    let mut open = true;

    let response = egui::Area::new(egui::Id::new("waypoint_context_menu"))
        .fixed_pos(egui::pos2(pos.x, pos.y))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_width(120.0);

                if let Ok(prim_path) = q_prim.get(entity) {
                    let name = prim_path.path.rsplit('/').next().unwrap_or("Waypoint");
                    ui.label(egui::RichText::new(name).strong());
                    ui.separator();
                }

                if ui.button("❌ Delete").clicked() {
                    if let Ok(prim_path) = q_prim.get(entity) {
                        if let Some(workspace) = &workspace {
                            if let Some(doc) = workspace.0.active_document {
                                // 1. Delete the waypoint prim in USD using RemovePrim
                                commands.trigger(ApplyUsdOp {
                                    doc,
                                    op: UsdOp::RemovePrim {
                                        edit_target: LayerId::runtime(),
                                        path: prim_path.path.clone(),
                                    },
                                });

                                // 2. Remove it from any vessel's BehaviorXml
                                for (_vessel, xml, vessel_prim) in q_vessels_xml.iter() {
                                    if xml.0.contains(&prim_path.path) {
                                        if let Ok(new_xml) = remove_waypoint_leaf(&xml.0, &prim_path.path) {
                                            commands.trigger(ApplyUsdOp {
                                                doc,
                                                op: UsdOp::SetAttribute {
                                                    edit_target: LayerId::runtime(),
                                                    path: vessel_prim.path.clone(),
                                                    name: "lunco:behavior".to_string(),
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
                    open = false;
                }

                if ui.button("Cancel").clicked() {
                    open = false;
                }
            });
        });

    if menu_state.just_opened {
        menu_state.just_opened = false;
    } else if ctx.input(|i| i.pointer.any_click()) && !response.response.hovered() {
        open = false;
    }

    if !open {
        menu_state.entity = None;
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
            if let Some(pos) = lunco_core::coords::world_position(entity, q_parents, q_grids, q_spatial) {
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
    /// The index of this waypoint in the patrol sequence.
    pub index: usize,
    /// Absolute world position of the waypoint.
    pub position: DVec3,
}

/// System that spawns and updates local visual-only translucent green spheres
/// for all coordinate-based waypoints stored in vessels' BehaviorXml.
/// This prevents polluting the USD stage with waypoint prims.
pub fn sync_waypoint_visuals(
    q_vessels: Query<(Entity, &BehaviorXml)>,
    q_visuals: Query<(Entity, &WaypointVisual)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<(Entity, &big_space::prelude::Grid)>,
    q_grids_only: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::grid::cell::CellCoord>, &Transform)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    // 1. Gather all desired coordinate-based waypoints from all vessels.
    let mut desired: Vec<((Entity, usize), DVec3)> = Vec::new();
    for (vessel, xml) in q_vessels.iter() {
        let Ok(value) = lunco_autopilot::btcpp_xml::xml_to_value(&xml.0) else { continue; };
        let mut targets = Vec::new();
        collect_targets(&value, &mut targets);

        let mut idx = 0;
        for target in targets {
            let parts: Vec<&str> = target.split(';').collect();
            if parts.len() == 3 {
                if let (Ok(x), Ok(y), Ok(z)) = (
                    parts[0].trim().parse::<f64>(),
                    parts[1].trim().parse::<f64>(),
                    parts[2].trim().parse::<f64>(),
                ) {
                    desired.push(((vessel, idx), DVec3::new(x, y, z)));
                    idx += 1;
                }
            }
        }
    }

    // 2. Identify existing visuals.
    let mut existing: std::collections::HashMap<(Entity, usize), Entity> = std::collections::HashMap::new();
    for (entity, visual) in q_visuals.iter() {
        existing.insert((visual.vessel, visual.index), entity);
    }

    // Get active grid for placing visuals.
    let Some((grid_entity, grid)) = q_grids.iter().next() else { return; };
    let grid_world = lunco_core::coords::world_position(grid_entity, &q_parents, &q_grids_only, &q_spatial)
        .unwrap_or(DVec3::ZERO);

    // 3. Spawn or update desired visuals.
    for ((vessel, index), pos) in desired {
        let (cell, local_pos) = lunco_core::coords::world_to_grid_local(pos, grid_world, grid);

        if let Some(entity) = existing.remove(&(vessel, index)) {
            // Update position of existing visual
            commands.entity(entity).insert((
                cell,
                Transform::from_translation(local_pos),
            ));
        } else {
            // Spawn new visual: a translucent dome sphere matching the USD model
            commands.spawn((
                Mesh3d(meshes.add(Sphere::new(2.5).mesh().ico(5).unwrap())),
                PbrLook {
                    base_color: LinearRgba::new(0.2, 0.95, 0.5, 0.28),
                    emissive: LinearRgba::new(0.12, 0.85, 0.42, 1.0),
                    alpha: SurfaceAlpha::Blend,
                    unlit: true,
                    ..default()
                },
                cell,
                Transform::from_translation(local_pos),
                GlobalTransform::default(),
                ChildOf(grid_entity),
                WaypointVisual { vessel, index, position: pos },
            ));
        }
    }

    // 4. Despawn any orphaned visuals.
    for (_, entity) in existing {
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
    mut egui_ctx: bevy_egui::EguiContexts,
) {
    // Prefer the avatar camera (the one the player looks through); fall back
    // to the first active Camera3d if no avatar is spawned yet.
    let cam_result = q_avatar_cam
        .iter()
        .next()
        .and_then(|av| q_camera.get(av).ok())
        .or_else(|| q_camera.iter().find(|(_, cam, _)| cam.is_active));
    let Some((cam_entity, camera, cam_gtf)) = cam_result else { return };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let origin = ctx.content_rect().min.to_vec2();

    // Camera world position for distance-based sizing.
    let cam_world = lunco_core::coords::world_position(cam_entity, &q_parents, &q_grids, &q_spatial)
        .unwrap_or(bevy::math::DVec3::ZERO);

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("waypoint_overlay"),
    ));

    let primary_selected = selected.primary();

    for (vessel, xml, bindings) in q_vessels.iter() {
        let empty_bindings = TargetBindings::default();
        let bindings = bindings.unwrap_or(&empty_bindings);
        let is_selected = Some(vessel) == primary_selected;

        let line_color = if is_selected {
            egui::Color32::from_rgb(51, 242, 128) // bright green
        } else {
            egui::Color32::from_rgb(102, 128, 102) // dim green
        };
        let label_color = egui::Color32::WHITE;

        let wp_positions = get_waypoint_positions(&xml.0, bindings, &q_parents, &q_grids, &q_spatial);

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

            let Ok(viewport) = camera.world_to_viewport(cam_gtf, world_f32) else { continue };
            let screen = egui::pos2(viewport.x, viewport.y) + origin;

            wp_screens.push(WpScreen { screen, index: i, distance });
        }

        // Draw connecting lines between consecutive waypoints.
        let stroke = egui::Stroke::new(2.0, line_color);
        for window in wp_screens.windows(2) {
            painter.line_segment([window[0].screen, window[1].screen], stroke);
        }

        // For patrol loops, connect last to first.
        if xml.0.contains("forever") && wp_screens.len() > 2 {
            if let (Some(first), Some(last)) = (wp_screens.first(), wp_screens.last()) {
                painter.line_segment([last.screen, first.screen], stroke);
            }
        }

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
                label_color.r(), label_color.g(), label_color.b(), alpha,
            );

            let galley = painter.layout_no_wrap(text, font, tc);
            let size = galley.size();
            let top_left = wp.screen - egui::vec2(size.x * 0.5, size.y + 8.0);

            let bg = egui::Rect::from_min_size(top_left, size).expand2(egui::vec2(4.0, 2.0));
            painter.rect_filled(bg, 3.0, egui::Color32::from_black_alpha((180.0 * fade) as u8));
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

/// Whether `path` is already authored in either layer of the document.
fn prim_exists(host: &lunco_doc::DocumentHost<lunco_usd::document::UsdDocument>, path: &str) -> bool {
    let Ok(sdf) = lunco_usd_bevy::SdfPath::new(path) else { return false };
    host.document().data().spec(&sdf).is_some()
        || host.document().runtime_data().spec(&sdf).is_some()
}

/// Component that marks reached waypoints to prevent double deletion.
#[derive(Component)]
pub struct WaypointReached;

/// System that checks if a vessel is close to any of its waypoints.
/// If so, deletes the waypoint from USD stage (if path-based) or updates the behavior tree XML (for both).
pub fn delete_reached_waypoints(
    q_vessels: Query<(Entity, &BehaviorXml, Option<&TargetBindings>, &UsdPrimPath)>,
    q_waypoints: Query<(Entity, &UsdPrimPath), Without<WaypointReached>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::grid::cell::CellCoord>, &Transform)>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut commands: Commands,
) {
    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else { return };

    for (vessel, xml, bindings, vessel_path) in q_vessels.iter() {
        let Some(vessel_pos) = lunco_core::coords::world_position(vessel, &q_parents, &q_grids, &q_spatial) else {
            continue;
        };

        // Parse all targets from behavior XML
        let Ok(value) = lunco_autopilot::btcpp_xml::xml_to_value(&xml.0) else { continue; };
        let mut targets = Vec::new();
        collect_targets(&value, &mut targets);

        for target in &targets {
            // 1. Try coordinate parsing
            let parts: Vec<&str> = target.split(';').collect();
            if parts.len() == 3 {
                if let (Ok(x), Ok(y), Ok(z)) = (
                    parts[0].trim().parse::<f64>(),
                    parts[1].trim().parse::<f64>(),
                    parts[2].trim().parse::<f64>(),
                ) {
                    let wp_pos = DVec3::new(x, y, z);
                    let distance = (wp_pos - vessel_pos).length();
                    if distance < 4.0 {
                        info!("Waypoint coordinate reached: deleting {}", target);
                        if let Ok(new_xml) = remove_waypoint_leaf(&xml.0, target) {
                            commands.trigger(ApplyUsdOp {
                                doc,
                                op: UsdOp::SetAttribute {
                                    edit_target: LayerId::runtime(),
                                    path: vessel_path.path.clone(),
                                    name: "lunco:behavior".to_string(),
                                    type_name: "string".to_string(),
                                    value: new_xml,
                                },
                            });
                        }
                    }
                    continue;
                }
            }

            // 2. Try path resolution
            if let Some(bindings) = bindings {
                if let Some(&wp_entity) = bindings.0.get(target) {
                    if let Ok((entity, prim_path)) = q_waypoints.get(wp_entity) {
                        let Some(wp_pos) = lunco_core::coords::world_position(entity, &q_parents, &q_grids, &q_spatial) else {
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
                                commands.trigger(ApplyUsdOp {
                                    doc,
                                    op: UsdOp::SetAttribute {
                                        edit_target: LayerId::runtime(),
                                        path: vessel_path.path.clone(),
                                        name: "lunco:behavior".to_string(),
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

use lunco_core::{register_commands, Command, on_command};

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
    q_gid: Query<&GlobalEntityId>,
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

        commands.trigger(lunco_autopilot::EngageAutopilot {
            vessel,
            index: 0,
            throttle: 0.5,
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

        commands.trigger(lunco_autopilot::EngageAutopilot {
            vessel,
            index: 0,
            throttle: 0.5,
            spec_json,
        });
    }
}

register_commands!(on_start_autopilot, on_toggle_autopilot);
