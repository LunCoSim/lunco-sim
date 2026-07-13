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
use lunco_autopilot::usd_tree::{append_waypoint_leaf, BehaviorXml};
use lunco_core::{EguiFocus, SpawnToolActive, TerrainToolActive};
use lunco_usd::document::{LayerId, UsdOp};
use lunco_usd::registry::UsdDocumentRegistry;
use lunco_usd::commands::ApplyUsdOp;
use lunco_usd_bevy::UsdPrimPath;

use crate::spawn::{terrain_ray_hit, TerrainOracles};
use crate::SelectedEntities;

/// The prim the marker asset is referenced from — the pin's visuals ARE the USD
/// scene, not a debug gizmo.
const WAYPOINT_ASSET: &str = "vessels/markers/waypoint.usda";
/// Scope the authored waypoints are parented under, beneath the stage's default prim.
/// A route lives in WORLD space, so it is deliberately NOT a child of the vessel —
/// parented under the rover, the waypoints would ride along as it drives.
const BEHAVIORS_SCOPE: &str = "Behaviors";

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
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    terrains: TerrainOracles,
    q_prim: Query<&UsdPrimPath>,
    q_xml: Query<&BehaviorXml>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    // Monotonic disambiguator for waypoint prim names within a session.
    mut wp_seq: Local<u32>,
    mut commands: Commands,
) {
    click.propagate(false);
    if egui_focus.wants_pointer || spawn_tool.0 || terrain_tool.0 {
        return;
    }
    if click.button != PointerButton::Primary {
        return;
    }
    // Ctrl+LMB only — a plain click possesses, Shift+click selects (the partition
    // documented in `selection.rs`).
    if !(keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight)) {
        return;
    }

    let Some(vessel) = selected.primary() else { return };
    let Ok(vessel_prim) = q_prim.get(vessel) else {
        warn!("[waypoint] selected vessel is not a USD prim; cannot author a mission for it");
        return;
    };
    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else { return };
    let Some(host) = usd_registry.host(doc) else { return };

    // Ground-truth terrain hit (the DEM oracle, not the band-limited collider ring) —
    // the same path `spawn::on_scene_click_spawn` uses.
    let Some((camera, cam_gtf)) = cameras.iter().find(|(c, _)| c.is_active) else { return };
    let Some(ray) = lunco_core::scene_click_ray(
        &egui_focus,
        camera,
        cam_gtf,
        click.pointer_location.position,
    ) else {
        return;
    };
    let Some((_, hit)) = terrain_ray_hit(&terrains, ray.origin.as_dvec3(), ray.direction.as_dvec3(), 1.0e6)
    else {
        return;
    };

    // ── Where the pin goes ────────────────────────────────────────────────────
    let root = lunco_usd_bevy::stage_default_prim(host.document().data())
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

    // A stable, valid USD identifier, scoped by the vessel so two rovers' routes never
    // collide.
    *wp_seq += 1;
    let stem: String = vessel_prim
        .path
        .rsplit('/')
        .next()
        .unwrap_or("vessel")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let name = format!("{stem}_wp{}", *wp_seq);
    let wp_path = join_prim(&scope_path, &name);

    // ── The mission's topology ────────────────────────────────────────────────
    // Append the leaf FIRST: if the tree is a shape the editor must not restructure,
    // bail out before authoring an orphan pin no mission references.
    let current = q_xml.get(vessel).ok().map(|x| x.0.as_str());
    let xml = match append_waypoint_leaf(current, &wp_path) {
        Ok(xml) => xml,
        Err(err) => {
            warn!("[waypoint] not adding a checkpoint: {err}");
            return;
        }
    };

    // ── Author: pin prim, its position, and the mission that names it ─────────
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: scope_path,
            name,
            type_name: None,
            reference: Some(WAYPOINT_ASSET.to_string()),
        },
    });
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetTranslate {
            edit_target: LayerId::runtime(),
            path: wp_path,
            value: [hit.x, hit.y, hit.z],
        },
    });
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
