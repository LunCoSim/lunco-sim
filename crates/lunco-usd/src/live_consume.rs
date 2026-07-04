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
use lunco_doc::DocumentId;
use lunco_usd_bevy::{UsdPrimPath, UsdRead, UsdStageAsset};
use openusd::sdf::Path as SdfPath;

use crate::document::UsdChange;
use crate::registry::UsdDocumentRegistry;

/// The attribute a move edit (`UsdOp::SetTranslate`) records as `InfoOnly`.
const TRANSLATE_ATTR: &str = "xformOp:translate";

/// Classification of a document's changes since a generation.
pub(crate) struct ChangeBatch {
    /// Prim paths that got an incremental transform-only edit (`InfoOnly`
    /// `xformOp:translate`) — applied in place, no reload.
    pub translate_paths: Vec<String>,
    /// `(prim_path, attr_name)` for a **non-translate** attribute edit (`InfoOnly`
    /// for some other attribute — material color, roughness, size, …). Authored
    /// onto the live stage and the prim's visual refreshed in place — the
    /// sink-driven successor to the reload that used to re-read these.
    pub attr_paths: Vec<(String, String)>,
    /// Prim paths that got a structural `Resync` (spawn / remove / rename of a
    /// concrete prim) — reconciled one subtree at a time by
    /// [`reconcile_structural_live`] (E2-2/E2-3), never re-instantiating siblings.
    pub resync_paths: Vec<String>,
    /// Whether any change needs structural work: a `Resync`, a `FullReload`, an
    /// `InfoOnly` for some other attribute, or a change-ring overflow.
    pub needs_structural: bool,
    /// Whether the structural work must be a *whole-scene* rebuild rather than a
    /// per-prim reconcile: a `FullReload` (source replaced / Save-As), a
    /// whole-stage `Resync { path: "/" }`, or a change-ring overflow (we can't
    /// prove [`resync_paths`](Self::resync_paths) is the complete delta set).
    pub full_reload: bool,
}

/// Classify `doc`'s changes in `(since, cur_gen]`. Conservative: if the change
/// ring dropped entries (more commits than its capacity since `since`), force a
/// structural reload rather than silently miss deltas. `None` if `doc` is gone.
pub(crate) fn classify_changes_since(
    registry: &UsdDocumentRegistry,
    doc: DocumentId,
    since: u64,
    cur_gen: u64,
) -> Option<ChangeBatch> {
    let host = registry.host(doc)?;
    let mut translate_paths = Vec::new();
    let mut attr_paths = Vec::new();
    let mut resync_paths = Vec::new();
    let mut needs_structural = false;
    let mut full_reload = false;
    let mut count = 0u64;
    for (_g, change) in host.document().changes_since(since) {
        count += 1;
        match change {
            UsdChange::InfoOnly { path, attr } if attr == TRANSLATE_ATTR => {
                translate_paths.push(path.clone());
            }
            // A whole-stage resync can't be reconciled per-prim.
            UsdChange::Resync { path } if path == "/" => {
                needs_structural = true;
                full_reload = true;
            }
            UsdChange::Resync { path } => {
                resync_paths.push(path.clone());
                needs_structural = true;
            }
            UsdChange::FullReload => {
                needs_structural = true;
                full_reload = true;
            }
            // Any other (non-translate) attribute edit: author onto the live
            // stage + refresh the prim's visual — no reload.
            UsdChange::InfoOnly { path, attr } => attr_paths.push((path.clone(), attr.clone())),
        }
    }
    // Generations are consecutive (one per commit), so we expect exactly
    // `cur_gen - since` deltas; fewer means the ring overflowed → we can't trust
    // `resync_paths` to be complete, so fall back to a whole-scene rebuild.
    if count < cur_gen.saturating_sub(since) {
        needs_structural = true;
        full_reload = true;
    }
    Some(ChangeBatch { translate_paths, attr_paths, resync_paths, needs_structural, full_reload })
}

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

    if world.get_non_send_resource::<CanonicalStages>().is_none() {
        return;
    }
    // Phase 1: drain the sink inboxes (owned + `Send`), releasing the borrow.
    let batches = world.non_send_resource_mut::<CanonicalStages>().drain_all_changes();
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
        reconcile_structural_live(world, id, &resynced);
    }
}

/// Apply the composed `xformOp:translate` of each `path` to its live entity,
/// read from the **live [`CanonicalStage`]** (not the flatten) under a short
/// immutable borrow. The canonical counterpart of [`apply_translates`]; shared
/// by the sink bridge and the doc-diff refresh so both read one source.
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
        let Some(stages) = world.get_non_send_resource::<CanonicalStages>() else {
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
            let Some(stages) = world.get_non_send_resource::<CanonicalStages>() else {
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
                    let stages = world.non_send_resource::<CanonicalStages>();
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
    use crate::document::{LayerId, UsdOp};
    use lunco_doc::{Document, DocumentOrigin};

    const TINY: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

    /// A move (`SetTranslate`) classifies as a transform-only change — no
    /// structural reload — while a spawn (`AddPrim`) forces structural.
    #[test]
    fn move_is_transform_only_spawn_is_structural() {
        let mut registry = UsdDocumentRegistry::default();
        let doc = registry.allocate(TINY.to_string(), DocumentOrigin::writable_file("/s.usda"));
        // Seed a prim to move, then record its generation.
        registry
            .host_mut(doc)
            .unwrap()
            .document_mut()
            .apply(UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path: "/World".into(),
                name: "rover_1".into(),
                type_name: Some("Xform".into()),
                reference: None,
            })
            .unwrap();
        let after_spawn = registry.host(doc).unwrap().document().generation();

        // The spawn itself is structural — but a per-prim reconcile, not a
        // whole-scene rebuild: its path is in `resync_paths` and `full_reload`
        // stays clear.
        let b = classify_changes_since(&registry, doc, 0, after_spawn).unwrap();
        assert!(b.needs_structural, "AddPrim is a Resync → structural");
        assert!(!b.full_reload, "a concrete-prim Resync reconciles per-prim");
        assert_eq!(b.resync_paths, vec!["/World/rover_1".to_string()]);

        // A subsequent move is transform-only.
        registry
            .host_mut(doc)
            .unwrap()
            .document_mut()
            .apply(UsdOp::SetTranslate {
                edit_target: LayerId::runtime(),
                path: "/World/rover_1".into(),
                value: [1.0, 2.0, 3.0],
            })
            .unwrap();
        let after_move = registry.host(doc).unwrap().document().generation();

        let b = classify_changes_since(&registry, doc, after_spawn, after_move).unwrap();
        assert!(!b.needs_structural, "SetTranslate is InfoOnly → no reload");
        assert_eq!(b.translate_paths, vec!["/World/rover_1".to_string()]);
    }

    /// A gap larger than the change ring can hold forces a structural reload
    /// (we can't prove we saw every delta).
    #[test]
    fn change_ring_overflow_forces_structural() {
        let mut registry = UsdDocumentRegistry::default();
        let doc = registry.allocate(TINY.to_string(), DocumentOrigin::writable_file("/s.usda"));
        // `since` far below current with no retained changes → count < expected.
        let b = classify_changes_since(&registry, doc, 0, 999).unwrap();
        assert!(b.needs_structural, "missing deltas ⇒ fall back to reload");
        assert!(b.full_reload, "an overflowed ring can't be reconciled per-prim");
    }

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
            .init_non_send_resource::<CanonicalStages>();

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
            .non_send_resource_mut::<CanonicalStages>()
            .get_or_build(id, &recipe)
            .expect("canonical stage builds from the recipe");
        app.world_mut()
            .non_send_resource_mut::<CanonicalStages>()
            .drain_all_changes();

        // The live `/World` scene-root entity the reconcile spawns children under.
        app.world_mut().spawn((
            Name::new("/World"),
            UsdPrimPath { stage_handle: handle.clone(), path: "/World".into() },
            Transform::default(),
        ));

        // Author a child prim ONTO THE LIVE STAGE → its sink records a resync.
        app.world()
            .non_send_resource::<CanonicalStages>()
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
            .non_send_resource::<CanonicalStages>()
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

    /// `RemovePrim` records a `Resync` for the removed path — classified as an
    /// incremental (per-prim) structural change, not a whole-scene rebuild.
    #[test]
    fn remove_is_per_prim_resync() {
        let mut registry = UsdDocumentRegistry::default();
        let doc = registry.allocate(TINY.to_string(), DocumentOrigin::writable_file("/s.usda"));
        registry
            .host_mut(doc)
            .unwrap()
            .document_mut()
            .apply(UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path: "/World".into(),
                name: "rover_1".into(),
                type_name: Some("Xform".into()),
                reference: None,
            })
            .unwrap();
        let after_spawn = registry.host(doc).unwrap().document().generation();
        registry
            .host_mut(doc)
            .unwrap()
            .document_mut()
            .apply(UsdOp::RemovePrim {
                edit_target: LayerId::runtime(),
                path: "/World/rover_1".into(),
            })
            .unwrap();
        let after_remove = registry.host(doc).unwrap().document().generation();

        let b = classify_changes_since(&registry, doc, after_spawn, after_remove).unwrap();
        assert!(b.needs_structural && !b.full_reload);
        assert_eq!(b.resync_paths, vec!["/World/rover_1".to_string()]);
    }
}
