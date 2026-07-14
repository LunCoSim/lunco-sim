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
        if let Some(entity) = find_live_entity(world, id, &path) {
            if let Some(mut tf) = world.entity_mut(entity).get_mut::<Transform>() {
                tf.translation = v;
            }
        }
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
pub(crate) fn apply_rotates_live(
    world: &mut World,
    id: AssetId<UsdStageAsset>,
    paths: &[String],
) {
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
pub(crate) fn refresh_domes_live(
    world: &mut World,
    id: AssetId<UsdStageAsset>,
    paths: &[String],
) {
    use lunco_usd_bevy::{dome, CanonicalStages};
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
                let ambient = lunco_usd_bevy::get_attribute_as_f32(&view, &sp, "inputs:intensity")
                    .unwrap_or(0.0);
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
    for p in info_only {
        let Some((prim, attr)) = p.split_once('.') else {
            continue;
        };
        if attr.starts_with("xformOp:") {
            continue;
        }
        if !prims.iter().any(|s| s == prim) {
            prims.push(prim.to_string());
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
            let Ok(sp) = SdfPath::new(&prim) else { continue };
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
            .add(UsdStageAsset { recipe: Some(recipe.clone()) });
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
            UsdPrimPath { stage_handle: handle.clone(), path: "/World".into() },
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
            q.iter(world).any(|p| p.stage_handle.id() == id && p.path == "/World/rover")
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
            .add(UsdStageAsset { recipe: Some(recipe.clone()) });
        let id = handle.id();
        app.world_mut()
            .non_send_mut::<CanonicalStages>()
            .get_or_build(id, &recipe)
            .expect("stage builds");

        // Define the prim, then drain so only the ATTRIBUTE edit is observed.
        {
            let stages = app.world().non_send::<CanonicalStages>();
            let stage = stages.get(id).unwrap().stage();
            stage.define_prim("/World/Ball").unwrap().set_type_name("Sphere").unwrap();
        }
        app.world_mut().non_send_mut::<CanonicalStages>().drain_all_changes();

        // Author an attribute value — an "info only" change, no restructuring.
        {
            let stages = app.world().non_send::<CanonicalStages>();
            let stage = stages.get(id).unwrap().stage();
            stage
                .create_attribute("/World/Ball.primvars:displayColor", "color3f[]")
                .unwrap();
        }

        let batches = app.world_mut().non_send_mut::<CanonicalStages>().drain_all_changes();
        let paths: Vec<String> = batches
            .into_iter()
            .flat_map(|(_, cs)| cs)
            .flat_map(|c| c.info_only.iter().map(|p| p.to_string()).collect::<Vec<_>>())
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
