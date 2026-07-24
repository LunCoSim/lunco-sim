//! Incremental change consumption (E2): apply `UsdChange` deltas to the live
//! world entity-by-entity instead of full-reloading the scene on every edit.
//!
//! `UsdDocument` records granular deltas (`document::UsdChange`): a **move** is
//! `InfoOnly{path, "xformOp:translate"}` (cheap — just an entity transform),
//! while spawns/removes/renames are `Resync` and a wholesale replace is
//! `FullReload`. The doc-backed refresh system (`sync_twin_overlays`) used to
//! full-reload the *whole* scene on any generation bump — so dragging a gizmo
//! re-instantiated every rover + terrain every frame of the drag.
//!
//! E2-1 makes that system change-aware via the helpers here: classify the
//! changes since the last-synced generation, apply transform-only edits in
//! place (no reload), and fall back to the structural reload **only** when a
//! spawn/remove/rename (or any non-transform edit) appears. Incremental
//! spawn/despawn (E2-2/E2-3) will later replace that structural fallback too.

use bevy::prelude::*;
use lunco_autopilot::usd_tree::{BehaviorXml, BehaviorXmlPath};
use lunco_usd_bevy::{UsdPrimPath, UsdRead, UsdStageAsset};
use openusd::sdf::Path as SdfPath;

/// The attribute a move edit (`UsdOp::SetTranslate`) records as `InfoOnly`.
const TRANSLATE_ATTR: &str = "xformOp:translate";

/// The attribute a rotate edit (`UsdOp::SetRotate`) records as `InfoOnly`.
const ROTATE_ATTR: &str = "xformOp:rotateXYZ";

// Edit → live-stage projection is now **op-driven** (author-once): the twin
// projection (`twin_projection::sync_twin_overlays`) replays the document's typed
// ops directly onto the `CanonicalStage`, so the old change-ring *classification*
// (`ChangeBatch` / `classify_changes_since`) that re-derived deltas from the
// composed state was removed. This module keeps the read-side of the bridge
// (`project_stage_changes` and its helpers), which drains the openusd sink.

/// The live entity projecting `path` in the scene scoped to `stage_handle_id`,
/// if one exists.
fn find_live_entity(
    world: &mut World,
    stage_handle_id: AssetId<UsdStageAsset>,
    path: &str,
) -> Option<Entity> {
    let mut q = world.query::<(Entity, &UsdPrimPath)>();
    q.iter(world)
        .find(|(_, upp)| upp.stage_handle.id() == stage_handle_id && upp.path == *path)
        .map(|(e, _)| e)
}

/// Projection bridge (Step 1): drain every live [`CanonicalStage`]'s change-sink
/// inbox and reconcile the ECS scene off the **live composed stage** — the read
/// counterpart to authoring onto the stage. This is what turns the openusd
/// `StageSink` into the world's projection engine: each committed edit's
/// `resynced` paths spawn/despawn subtrees and its `info_only` paths update
/// transforms in place — no flatten, no whole-scene reload.
///
/// Exclusive: reconcile mutates arbitrary entities and the `!Send` stage lives
/// as a `NonSend` resource. The stage can't be *held* across the ECS mutation
/// (it aliases the world), so each delta is read under a **short** immutable
/// borrow — prim existence for the structural pass, the composed translate for
/// transforms — and then applied. The spawn path re-reads the stage through the
/// `on_usd_prim_added` observer, which finds it still present in
/// [`CanonicalStages`] (we never remove it).
///
/// [`CanonicalStage`]: lunco_usd_bevy::CanonicalStage
/// [`CanonicalStages`]: lunco_usd_bevy::CanonicalStages
pub(crate) fn project_stage_changes(world: &mut World) {
    use lunco_usd_bevy::CanonicalStages;

    if world.get_non_send::<CanonicalStages>().is_none() {
        return;
    }
    // Phase 1: drain the sink inboxes (owned + `Send`), releasing the borrow.
    let batches = world.non_send_mut::<CanonicalStages>().drain_all_changes();
    if batches.is_empty() {
        return;
    }

    for (id, changes) in batches {
        // Merge this stage's committed changes into one resync / info-only set.
        let mut resynced: Vec<String> = Vec::new();
        let mut info_only: Vec<String> = Vec::new();
        for c in changes {
            resynced.extend(c.resynced.iter().map(|p| p.to_string()));
            info_only.extend(c.info_only.iter().map(|p| p.to_string()));
        }
        resynced.sort();
        resynced.dedup();
        info_only.sort();
        info_only.dedup();

        apply_translates_live(world, id, &info_only);
        apply_rotates_live(world, id, &info_only);
        // A `DomeLight`'s attributes (its HDRI, intensity, skybox flag) are not
        // transforms, so neither of the above sees them. Without this, a
        // `SetDomeLight` on an already-live dome would journal and save but
        // leave the rendered sky untouched. Runs before the general refresh
        // below, which then skips domes — this path is the cheaper one (it keeps
        // the projected cubemap when only the brightness moved).
        refresh_domes_live(world, id, &info_only);
        // EVERYTHING ELSE. Any other authored attribute — a colour, a material
        // input, a light's intensity, a radius, `visibility` — re-projects here,
        // so a live edit shows up without reloading the scene.
        refresh_edited_prims_live(world, id, &info_only);
        reconcile_structural_live(world, id, &resynced);
    }

    // Connections are derived from native `connectionPaths` by
    // `lunco_usd_sim::cosim::rewire_usd_connections`. Prim spawn/despawn triggers
    // that system directly (change-detection); a `connectionPaths` **edit** on an
    // already-spawned prim is neither — so mark the wiring dirty whenever a drain
    // occurred, letting the rewire re-derive off the live stage. This is the
    // op-driven, journaled, distributed path for live connection edits.
    if let Some(mut dirty) = world.get_resource_mut::<lunco_usd_sim::cosim::WiringDirty>() {
        dirty.0 = true;
    }
}

/// Apply the composed `xformOp:translate` of each `path` to its live entity,
/// read from the **live [`CanonicalStage`]** (not the flatten) under a short
/// immutable borrow. Shared by the sink bridge and the doc-diff refresh so both
/// read one source.
///
/// [`CanonicalStage`]: lunco_usd_bevy::CanonicalStage
pub(crate) fn apply_translates_live(
    world: &mut World,
    id: AssetId<UsdStageAsset>,
    paths: &[String],
) {
    use lunco_usd_bevy::CanonicalStages;
    if paths.is_empty() {
        return;
    }
    // Read every translate under one short borrow of the `!Send` stage, then
    // release it before mutating the world.
    let translates: Vec<(String, Vec3)> = {
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return;
        };
        let Some(cs) = stages.get(id) else { return };
        let view = cs.view();
        paths
            .iter()
            .filter_map(|p| {
                let sp = SdfPath::new(p).ok()?;
                lunco_usd_bevy::get_attribute_as_vec3(&view, &sp, TRANSLATE_ATTR)
                    .map(|v| (p.clone(), v))
            })
            .collect()
    };
    for (path, v) in translates {
        let Some(entity) = find_live_entity(world, id, &path) else {
            continue;
        };
        seat_authored_translate(world, entity, v);
    }
}

/// Seat one authored `xformOp:translate` onto a live entity.
///
/// An authored translate on a GRID-DIRECT prim is **grid-absolute**: the spawn
/// path plants the whole value at cell 0 and lets big_space re-split it into
/// `(cell, remainder)`. Writing it straight into `Transform` — as this did — left
/// the entity's existing `CellCoord` standing, so the prim landed at
/// `authored + cell × edge`: a live edit to anything outside the origin cell
/// threw it 2 km per cell across the moonbase. Re-splitting the same way spawn's
/// value gets re-split makes re-applying an unchanged translate a no-op instead
/// of a jump.
///
/// A prim with no parent `Grid` (nested under a referenced scene) has no cell,
/// and its authored value IS the parent-local transform.
fn seat_authored_translate(world: &mut World, entity: Entity, v: Vec3) {
    let parent_grid = world
        .get::<bevy::prelude::ChildOf>(entity)
        .map(|c| c.parent())
        .and_then(|p| world.get::<big_space::prelude::Grid>(p))
        .cloned();
    match parent_grid {
        Some(grid) => {
            let (cell, local) = grid.translation_to_grid(v.as_dvec3());
            let mut e = world.entity_mut(entity);
            if let Some(mut tf) = e.get_mut::<Transform>() {
                tf.translation = local;
            }
            e.insert(cell);
        }
        None => {
            if let Some(mut tf) = world.entity_mut(entity).get_mut::<Transform>() {
                tf.translation = v;
            }
        }
    }
}

#[cfg(test)]
mod translate_seat_tests {
    use super::*;
    use big_space::prelude::{CellCoord, Grid};

    const EDGE: f32 = 2000.0;

    fn grid_world() -> (World, Entity) {
        let mut world = World::new();
        let grid = world
            .spawn((Grid::new(EDGE, 0.0), CellCoord::ZERO, Transform::default()))
            .id();
        (world, grid)
    }

    /// The authored value is grid-absolute, so seating it must reassemble to that
    /// value — cell AND remainder both written. Pinned outside cell 0, where the
    /// bug was invisible.
    #[test]
    fn authored_translate_is_split_across_cells() {
        let (mut world, grid) = grid_world();
        // Sitting at grid-absolute y = 3947 (cell 2, local -53).
        let prim = world
            .spawn((
                CellCoord::new(0, 2, 0),
                Transform::from_translation(Vec3::new(0.0, -53.0, 0.0)),
                ChildOf(grid),
            ))
            .id();

        seat_authored_translate(&mut world, prim, Vec3::new(0.0, 4047.0, 0.0));

        let cell = world.get::<CellCoord>(prim).copied().unwrap();
        let tf = world.get::<Transform>(prim).copied().unwrap();
        let landed = cell.y as f32 * EDGE + tf.translation.y;
        assert!(
            (landed - 4047.0).abs() < 1e-2,
            "reassembled {landed} != 4047"
        );
        assert!(
            tf.translation.y.abs() < EDGE,
            "local {} must be a remainder, not the absolute",
            tf.translation.y
        );
    }

    /// Re-applying the translate the prim is ALREADY at must not move it. This is
    /// the round-trip that matters in practice: the gizmo authors grid-absolute on
    /// drag-end and this path immediately re-consumes it. Before the fix that
    /// round-trip was a `cell × edge` jump — the disappearing solar panel.
    #[test]
    fn re_seating_the_current_position_does_not_move_the_prim() {
        let (mut world, grid) = grid_world();
        let prim = world
            .spawn((
                CellCoord::new(1, 2, -1),
                Transform::from_translation(Vec3::new(10.0, -53.0, 4.0)),
                ChildOf(grid),
            ))
            .id();
        // What the gizmo would author for this prim: cell × edge + local.
        let authored = Vec3::new(1.0 * EDGE + 10.0, 2.0 * EDGE - 53.0, -1.0 * EDGE + 4.0);

        seat_authored_translate(&mut world, prim, authored);

        let cell = world.get::<CellCoord>(prim).copied().unwrap();
        let tf = world.get::<Transform>(prim).copied().unwrap();
        let reassembled = Vec3::new(
            cell.x as f32 * EDGE + tf.translation.x,
            cell.y as f32 * EDGE + tf.translation.y,
            cell.z as f32 * EDGE + tf.translation.z,
        );
        assert!(
            (reassembled - authored).length() < 1e-2,
            "prim moved: {reassembled:?} != {authored:?}"
        );
    }

    /// No parent grid ⇒ no cell to write, and the authored value is already local.
    #[test]
    fn a_nested_prim_keeps_its_parent_local_translate() {
        let mut world = World::new();
        let parent = world.spawn(Transform::default()).id();
        let nested = world.spawn((Transform::default(), ChildOf(parent))).id();

        seat_authored_translate(&mut world, nested, Vec3::new(1.0, 2.0, 3.0));

        assert_eq!(
            world.get::<Transform>(nested).unwrap().translation,
            Vec3::new(1.0, 2.0, 3.0)
        );
        assert!(world.get::<CellCoord>(nested).is_none());
    }
}

/// Apply the composed `xformOp:rotateXYZ` (Euler XYZ, **degrees**) of each
/// `path` to its live entity — the rotation counterpart of
/// [`apply_translates_live`], and read the same way (one short borrow of the
/// `!Send` stage, released before the world is mutated).
///
/// Without this, [`UsdOp::SetRotate`](crate::document::UsdOp::SetRotate) authored,
/// journaled and saved, but nothing moved until the scene was reloaded — it is
/// how a `DomeLight`'s environment is spun, and how the sun is aimed.
pub(crate) fn apply_rotates_live(world: &mut World, id: AssetId<UsdStageAsset>, paths: &[String]) {
    use lunco_usd_bevy::CanonicalStages;
    if paths.is_empty() {
        return;
    }
    let rotations: Vec<(String, Quat)> = {
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return;
        };
        let Some(cs) = stages.get(id) else { return };
        let view = cs.view();
        paths
            .iter()
            .filter_map(|p| {
                let sp = SdfPath::new(p).ok()?;
                lunco_usd_bevy::get_attribute_as_vec3(&view, &sp, ROTATE_ATTR)
                    // Degrees → quat, via the canonical converter, so the Euler
                    // order lives in exactly one place.
                    .map(|deg| (p.clone(), lunco_usd_bevy::euler_xyz_deg_to_quat(deg)))
            })
            .collect()
    };
    for (path, q) in rotations {
        if let Some(entity) = find_live_entity(world, id, &path) {
            if let Some(mut tf) = world.entity_mut(entity).get_mut::<Transform>() {
                tf.rotation = q;
            }
        }
    }
}

/// Re-read every changed `DomeLight` prim and push its authored state back onto
/// the live entity (`lunco_usd_bevy::dome`). The HDRI, its tint/intensity and
/// the skybox toggle are plain attributes, so only this sees them move.
pub(crate) fn refresh_domes_live(world: &mut World, id: AssetId<UsdStageAsset>, paths: &[String]) {
    use lunco_usd_bevy::{CanonicalStages, dome};
    if paths.is_empty() {
        return;
    }
    // `AssetServer` is a cheap handle-clone; taking it now keeps the world free
    // to be borrowed mutably below.
    let Some(asset_server) = world.get_resource::<AssetServer>().cloned() else {
        return;
    };

    // Re-read the intent under one short borrow of the `!Send` stage.
    let domes: Vec<(String, Option<dome::UsdDomeEnvironment>, f32)> = {
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return;
        };
        let Some(cs) = stages.get(id) else { return };
        let view = cs.view();
        paths
            .iter()
            .filter_map(|p| {
                let sp = SdfPath::new(p).ok()?;
                if view.type_name(&sp).as_deref() != Some("DomeLight") {
                    return None;
                }
                let env = dome::read_dome_environment(&view, &sp, &asset_server, id);
                // The fallback if the author dropped the texture: a bare dome is
                // a scalar ambient.
                let ambient = view.real_f32(&sp, "inputs:intensity").unwrap_or(0.0);
                Some((p.clone(), env, ambient))
            })
            .collect()
    };

    for (path, env, ambient) in domes {
        if let Some(entity) = find_live_entity(world, id, &path) {
            dome::refresh_dome_entity(world, entity, env, ambient);
        }
    }
}

/// Re-project every prim whose attributes were edited — the general live-edit
/// path, so **an edit shows up without reloading the scene**.
///
/// `info_only` carries both the owning prim path and the PROPERTY path naming the
/// changed attribute (pinned by `info_only_reports_both_prim_and_property_paths`).
/// That attribute name is what lets this be precise instead of a reload:
///
/// - **`xformOp:*`** — skipped. [`apply_translates_live`] / [`apply_rotates_live`]
///   already wrote the `Transform` in place. Re-instantiating on a transform edit
///   would rebuild the mesh and re-run the physics observers on every frame of a
///   gizmo drag.
/// - **`Shader` / `Material` prims** — a material edit fans out through
///   `material:binding` to arbitrary meshes elsewhere in the scene, so the prim's
///   own subtree is not enough: refresh the scene's visuals.
/// - **anything else** — re-instantiate just that prim's subtree.
///
/// `DomeLight`s are excluded: [`refresh_domes_live`] already handled them, and it
/// is strictly better (it keeps the projected cubemap when only the intensity or
/// the skybox flag moved, instead of re-projecting a 1024² cubemap per edit).
pub(crate) fn refresh_edited_prims_live(
    world: &mut World,
    id: AssetId<UsdStageAsset>,
    info_only: &[String],
) {
    use lunco_usd_bevy::CanonicalStages;
    if info_only.is_empty() {
        return;
    }

    // Split the property paths into (prim, attr). A prim path carries no `.`,
    // so the ones that do not split are the prim-path half of the same change and
    // are simply skipped here.
    let mut prims: Vec<String> = Vec::new();
    let mut behavior_updates: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    // Wheel/vehicle dynamics edits are claimed by the in-place resync (same
    // shape as the mission `info:sourceCode` special-case below): excluded from the
    // subtree refresh — which would corrupt a spawned wheel — and folded into
    // ONE `resync_wheels_for_stage` call after the loop.
    let mut wheels_dirty = false;
    for p in info_only {
        let Some((prim, attr)) = p.split_once('.') else {
            continue;
        };
        {
            let claimed = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(id))
                .zip(SdfPath::new(prim).ok())
                .is_some_and(|(cs, sp)| {
                    lunco_usd_sim::wheel_params::claims_edit(&cs.view(), &sp, attr)
                });
            if claimed {
                wheels_dirty = true;
                continue;
            }
        }
        // A mission tree is a `LunCoProgramAPI` child carrying `info:sourceCode` (inline
        // XML) or `info:sourceAsset` (`@…btxml@`, with `.xml` accepted for
        // upstream interop); the behaviour engine is picked by the
        // extension, the same rule `.mo` and `.rhai` follow. A live edit to either
        // re-reads the tree from the prim that owns it.
        if attr == "info:sourceCode" || attr == "info:sourceAsset" {
            // Read value under stage borrow
            if let Some(stages) = world.get_non_send::<CanonicalStages>() {
                if let Some(cs) = stages.get(id) {
                    let view = cs.view();
                    if let Ok(sp) = SdfPath::new(prim) {
                        let val = view
                            .scalar::<String>(&sp, "info:sourceCode")
                            .filter(|s| s.trim_start().starts_with('<'));
                        let path_val =
                            lunco_usd_bevy::UsdRead::asset(&view, &sp, "info:sourceAsset")
                                .filter(|s| lunco_core::programs::is_behavior_tree_asset(s));
                        // The tree is authored on the `LunCoProgramAPI` child, but the
                        // VESSEL owns it — `process_usd_sim_prims` inserts `BehaviorXml`
                        // on the parent entity, so a live edit must resolve to the same
                        // one or it would stamp the tree onto the program prim instead.
                        if val.is_some() || path_val.is_some() {
                            if let Some(owner) = sp.parent() {
                                behavior_updates.push((owner.as_str().to_string(), val, path_val));
                            }
                        }
                    }
                }
            }
            continue;
        }
        if attr.starts_with("xformOp:") {
            continue;
        }
        if !prims.iter().any(|s| s == prim) {
            prims.push(prim.to_string());
        }
    }

    if wheels_dirty {
        lunco_usd_sim::wheel_params::resync_wheels_for_stage(world, id);
    }

    // Apply any inline behavior XML or path updates directly to the entity
    for (prim, xml, path) in behavior_updates {
        if let Some(entity) = find_live_entity(world, id, &prim) {
            if let Some(xml_text) = xml {
                world.entity_mut(entity).insert(BehaviorXml(xml_text));
            }
            if let Some(path_text) = path {
                world.entity_mut(entity).insert(BehaviorXmlPath(path_text));
            }
        }
    }

    if prims.is_empty() {
        return;
    }

    // Classify under one short borrow of the `!Send` stage, then release it —
    // the refreshes below mutate the world.
    let mut scene_wide = false;
    let mut subtrees: Vec<String> = Vec::new();
    {
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return;
        };
        let Some(cs) = stages.get(id) else { return };
        let view = cs.view();
        for prim in prims {
            let Ok(sp) = SdfPath::new(&prim) else {
                continue;
            };
            match view.type_name(&sp).as_deref() {
                // Already re-projected, better, by `refresh_domes_live`.
                Some("DomeLight") => {}
                // Material network edits fan out to every prim bound to them.
                Some("Shader") | Some("Material") => scene_wide = true,
                _ => subtrees.push(prim),
            }
        }
    }

    if scene_wide {
        crate::twin_projection::refresh_scene_visuals(world, id);
        return;
    }
    for prim in subtrees {
        crate::twin_projection::refresh_prim_subtree(world, id, &prim);
    }
}

/// Reconcile the live entities of the scene scoped to `id` against the **live
/// [`CanonicalStage`]** for the structurally-changed `resync_paths`: spawn the
/// added (present in the stage, no live entity), despawn the removed (absent,
/// but a live entity survives). Reads the live stage via short borrows (the
/// `!Send` stage can't be held across the ECS mutation), so the sink bridge and
/// the doc-diff twin refresh share one reconciler and one source.
///
/// `resync_paths` is applied in caller order; the caller sorts parent-before-
/// child so a subtree root spawns first and its `on_usd_prim_added` observer
/// builds the descendants (the per-path idempotency check then no-ops them).
///
/// [`CanonicalStage`]: lunco_usd_bevy::CanonicalStage
pub(crate) fn reconcile_structural_live(
    world: &mut World,
    id: AssetId<UsdStageAsset>,
    resync_paths: &[String],
) {
    use lunco_usd_bevy::CanonicalStages;
    for path in resync_paths {
        let Ok(sp) = SdfPath::new(path) else { continue };
        let exists = {
            let Some(stages) = world.get_non_send::<CanonicalStages>() else {
                return;
            };
            match stages.get(id) {
                Some(cs) => cs.view().has_prim(&sp),
                None => return,
            }
        };
        let live = find_live_entity(world, id, path);
        match (exists, live) {
            (false, Some(entity)) => {
                lunco_usd_sim::cosim::despawn_usd_subtree(world, entity);
            }
            (true, None) => {
                // Pre-read the child's translate under a short borrow; the
                // observer builds the subtree from the still-present stage.
                let tf = {
                    let stages = world.non_send::<CanonicalStages>();
                    stages
                        .get(id)
                        .and_then(|cs| {
                            lunco_usd_bevy::get_attribute_as_vec3(&cs.view(), &sp, TRANSLATE_ATTR)
                        })
                        .map(Transform::from_translation)
                        .unwrap_or_default()
                };
                lunco_usd_sim::cosim::spawn_usd_child_with_translate(world, id, path, tf);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

    /// Step-1 projection bridge, end to end: authoring a prim **onto the live
    /// `CanonicalStage`** fires its openusd change sink, and
    /// [`project_stage_changes`] drains that and spawns the matching ECS entity
    /// off the live stage — no flatten, no whole-scene reload. Removing the prim
    /// despawns it again. This is the read half of "journal → stage → projection".
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn sink_drain_projects_spawn_and_despawn() {
        use bevy::asset::AssetApp;
        use bevy::prelude::*;
        use lunco_usd_bevy::{CanonicalStages, StageRecipe};

        const SCENE: &str =
            "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<UsdStageAsset>()
            .init_non_send::<CanonicalStages>();

        // An asset carrying the ref-less in-memory scene + its build recipe.
        let recipe = StageRecipe::from_source("scene.usda", SCENE);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(UsdStageAsset {
                recipe: Some(recipe.clone()),
            });
        let id = handle.id();

        // Build the live stage on demand, then drain its initial change set so
        // the only deltas we observe are the ones we author below.
        app.world_mut()
            .non_send_mut::<CanonicalStages>()
            .get_or_build(id, &recipe)
            .expect("canonical stage builds from the recipe");
        app.world_mut()
            .non_send_mut::<CanonicalStages>()
            .drain_all_changes();

        // The live `/World` scene-root entity the reconcile spawns children under.
        app.world_mut().spawn((
            Name::new("/World"),
            UsdPrimPath {
                stage_handle: handle.clone(),
                path: "/World".into(),
            },
            Transform::default(),
        ));

        // Author a child prim ONTO THE LIVE STAGE → its sink records a resync.
        app.world()
            .non_send::<CanonicalStages>()
            .get(id)
            .unwrap()
            .stage()
            .define_prim("/World/rover")
            .unwrap()
            .set_type_name("Xform")
            .unwrap();

        // Drain + reconcile: the authored prim projects into a live entity.
        project_stage_changes(app.world_mut());
        let live = |world: &mut World| {
            let mut q = world.query::<&UsdPrimPath>();
            q.iter(world)
                .any(|p| p.stage_handle.id() == id && p.path == "/World/rover")
        };
        assert!(
            live(app.world_mut()),
            "authoring /World/rover onto the live stage must spawn its entity via the sink bridge"
        );

        // Remove it → the sink records a resync for the vanished prim → despawn.
        app.world()
            .non_send::<CanonicalStages>()
            .get(id)
            .unwrap()
            .stage()
            .remove_prim("/World/rover")
            .unwrap();
        project_stage_changes(app.world_mut());
        assert!(
            !live(app.world_mut()),
            "removing the prim from the live stage must despawn its entity"
        );
    }

    /// **The shape of an `info_only` entry.** An attribute edit reports BOTH the
    /// owning prim path (`/World/Ball`) and the property path
    /// (`/World/Ball.primvars:displayColor`).
    ///
    /// Pinned by a test because the live-edit bridge depends on both halves and
    /// they are easy to assume away in either direction:
    /// - the prim path is what [`apply_translates_live`] matches on (drop it and
    ///   gizmo moves stop projecting);
    /// - the property path is what names the CHANGED ATTRIBUTE, which is the only
    ///   way [`refresh_edited_prims_live`] can tell "the colour moved, re-project
    ///   the look" from "it was just a drag, use the cheap transform path".
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn info_only_reports_both_prim_and_property_paths() {
        use bevy::asset::AssetApp;
        use bevy::prelude::*;
        use lunco_usd_bevy::{CanonicalStages, StageRecipe};

        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<UsdStageAsset>()
            .init_non_send::<CanonicalStages>();

        let recipe = StageRecipe::from_source("scene.usda", TINY);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(UsdStageAsset {
                recipe: Some(recipe.clone()),
            });
        let id = handle.id();
        app.world_mut()
            .non_send_mut::<CanonicalStages>()
            .get_or_build(id, &recipe)
            .expect("stage builds");

        // Define the prim, then drain so only the ATTRIBUTE edit is observed.
        {
            let stages = app.world().non_send::<CanonicalStages>();
            let stage = stages.get(id).unwrap().stage();
            stage
                .define_prim("/World/Ball")
                .unwrap()
                .set_type_name("Sphere")
                .unwrap();
        }
        app.world_mut()
            .non_send_mut::<CanonicalStages>()
            .drain_all_changes();

        // Author an attribute value — an "info only" change, no restructuring.
        {
            let stages = app.world().non_send::<CanonicalStages>();
            let stage = stages.get(id).unwrap().stage();
            stage
                .create_attribute("/World/Ball.primvars:displayColor", "color3f[]")
                .unwrap();
        }

        let batches = app
            .world_mut()
            .non_send_mut::<CanonicalStages>()
            .drain_all_changes();
        let paths: Vec<String> = batches
            .into_iter()
            .flat_map(|(_, cs)| cs)
            .flat_map(|c| {
                c.info_only
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
            })
            .collect();

        assert!(
            paths.iter().any(|p| p == "/World/Ball"),
            "info_only must carry the owning PRIM path (what the transform fast \
             path matches on). got: {paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|p| p == "/World/Ball.primvars:displayColor"),
            "info_only must carry the PROPERTY path, naming the changed attribute. \
             got: {paths:?}"
        );
    }
}
