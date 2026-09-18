//! Incremental change consumption (E2): apply `UsdChange` deltas to the live
//! world entity-by-entity instead of full-reloading the scene on every edit.
//!
//! `UsdDocument` records granular deltas (`document::UsdChange`): a transform
//! edit is an `InfoOnly` change for one standard `xformOp:*` channel (cheap —
//! just an entity transform), while spawns/removes/renames are `Resync` and a
//! wholesale replace is `FullReload`. The doc-backed projector routes each
//! delta to the smallest live-stage or ECS update that can express it.
//!
//! The document projector replays typed edits onto the live canonical stage.
//! This read-side bridge drains the stage sink, applies transform and other
//! attribute edits in place, and reconciles structural resyncs incrementally.
//! Coarse document operations use the explicit full-rebuild path in
//! `twin_projection`; ordinary authored route edits use the incremental path.

use bevy::prelude::*;
use lunco_usd_bevy_core::{UsdRead, UsdStageAsset};
use lunco_usd_bevy_scene::{UsdPrimPath, UsdSceneInfoChanged};
use openusd::schemas::lux::tokens as ltok;
use openusd::sdf::Path as SdfPath;
use std::collections::{BTreeMap, HashMap, HashSet};

/// The attribute a move edit (`UsdOp::SetTranslate`) records as `InfoOnly`.
const TRANSLATE_ATTR: &str = "xformOp:translate";

/// The attribute a rotate edit (`UsdOp::SetRotate`) records as `InfoOnly`.
const ROTATE_ATTR: &str = "xformOp:rotateXYZ";
/// The attribute a scale edit (`UsdOp::SetScale`) records as `InfoOnly`.
const SCALE_ATTR: &str = "xformOp:scale";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TransformEditChannels {
    pub(crate) translate: bool,
    pub(crate) rotate: bool,
    pub(crate) scale: bool,
}

/// A document generation whose typed operations have been applied to a live
/// canonical stage and is waiting for the read-side projection boundary.
///
/// Some valid stage edits do not produce a structural/info-only sink notice:
/// a rebuilt stage starts with an empty sink, and an idempotent authored value
/// can be absorbed by the stage without a visible delta. The document sync
/// owner records those stages here so the live projector can still close the
/// document-to-ECS handoff after the stage is ready.
#[derive(Clone, Copy)]
struct PendingStageProjection {
    doc: lunco_doc::DocumentId,
    stage_id: AssetId<UsdStageAsset>,
    generation: u64,
}

#[derive(Resource, Default)]
pub(crate) struct PendingStageProjections {
    stages: HashMap<AssetId<UsdStageAsset>, PendingStageProjection>,
}

pub(crate) fn queue_stage_projection(
    world: &mut World,
    doc: lunco_doc::DocumentId,
    stage_id: AssetId<UsdStageAsset>,
    generation: u64,
) {
    world
        .resource_mut::<PendingStageProjections>()
        .stages
        .insert(
            stage_id,
            PendingStageProjection {
                doc,
                stage_id,
                generation,
            },
        );
}

fn remove_pending_stage_projection(world: &mut World, stage_id: AssetId<UsdStageAsset>) {
    if let Some(mut pending) = world.get_resource_mut::<PendingStageProjections>() {
        pending.stages.remove(&stage_id);
    }
}

impl TransformEditChannels {
    pub(crate) const fn translate() -> Self {
        Self {
            translate: true,
            rotate: false,
            scale: false,
        }
    }

    pub(crate) const fn rotate() -> Self {
        Self {
            translate: false,
            rotate: true,
            scale: false,
        }
    }

    pub(crate) const fn scale() -> Self {
        Self {
            translate: false,
            rotate: false,
            scale: true,
        }
    }

    pub(crate) fn for_attribute(attribute: &str) -> Option<Self> {
        let mut channels = Self::default();
        match attribute {
            TRANSLATE_ATTR => channels.translate = true,
            ROTATE_ATTR => channels.rotate = true,
            SCALE_ATTR => channels.scale = true,
            _ => return None,
        }
        Some(channels)
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.translate |= other.translate;
        self.rotate |= other.rotate;
        self.scale |= other.scale;
    }
}

fn transform_edits(info_only: &[String]) -> HashMap<String, TransformEditChannels> {
    let mut edits: HashMap<String, TransformEditChannels> = HashMap::new();
    for path in info_only {
        let Some((prim, attribute)) = path.split_once('.') else {
            continue;
        };
        let Some(channels) = TransformEditChannels::for_attribute(attribute) else {
            continue;
        };
        edits.entry(prim.to_string()).or_default().merge(channels);
    }
    edits
}

/// Returns whether an authored standard USD `inputs:*` value changed.
///
/// Unconnected scalar inputs are the instance-default boundary for Modelica
/// parameters. They are reported as ordinary info-only edits, but still need
/// to invalidate the USD-derived defaults projection so the owning cosim
/// lifecycle can decide whether a compile-time parameter changed.
fn authored_input_defaults_changed(info_only: &[String]) -> bool {
    info_only.iter().any(|path| {
        path.split_once('.')
            .is_some_and(|(_, attribute)| attribute.starts_with("inputs:"))
    })
}

/// Mark the live model instance owning an authored `inputs:*` edit.
///
/// This publishes only a generic state revision. The authoring projection does
/// not decide whether a backend should recompile, reset, or simply accept a
/// live input; the backend that owns the model decides that from its own state.
fn mark_model_state_revision(world: &mut World, prim_path: &str) {
    let entity = world
        .query::<(Entity, &UsdPrimPath)>()
        .iter(world)
        .find_map(|(entity, path)| (path.path == prim_path).then_some(entity));
    if let Some(entity) = entity {
        let mut revision = world
            .get::<lunco_core::ModelStateRevision>(entity)
            .copied()
            .unwrap_or_default();
        revision.advance();
        world.entity_mut(entity).insert(revision);
    }
}

/// Transform paths explicitly authored by the typed live-stage projector.
///
/// OpenUSD can report a transform author as a resync of an already-live prim,
/// because authoring `xformOpOrder` changes a uniform field. A resync of a
/// descendant can also include its existing ancestors, though. The latter is
/// structural bookkeeping, not a request to overwrite a moving physics body
/// with its authored spawn pose. Keep the typed transform intent beside the
/// sink notice so the structural bridge can distinguish those two cases.
#[derive(Resource, Default)]
pub(crate) struct LiveTransformEditHints {
    paths: HashMap<AssetId<UsdStageAsset>, HashMap<String, TransformEditChannels>>,
}

impl LiveTransformEditHints {
    pub(crate) fn mark(
        &mut self,
        stage: AssetId<UsdStageAsset>,
        path: String,
        channels: TransformEditChannels,
    ) {
        self.paths
            .entry(stage)
            .or_default()
            .entry(path)
            .or_default()
            .merge(channels);
    }

    fn take(&mut self, stage: AssetId<UsdStageAsset>) -> HashMap<String, TransformEditChannels> {
        self.paths.remove(&stage).unwrap_or_default()
    }
}

/// Record a successful typed transform authoring operation. The resource
/// is optional for small headless projection tests that construct only the sink
/// bridge; production installs it with this package's `UsdSceneRuntimePlugin`.
pub(crate) fn mark_live_transform(
    world: &mut World,
    stage: AssetId<UsdStageAsset>,
    path: impl Into<String>,
    channels: TransformEditChannels,
) {
    if let Some(mut hints) = world.get_resource_mut::<LiveTransformEditHints>() {
        hints.mark(stage, path.into(), channels);
    }
}

// Edit → live-stage projection is **op-driven** (author-once): the twin
// projection (`twin_projection::sync_twin_overlays`) replays the document's
// typed ops directly onto the `CanonicalStage`. This module is the read-side
// of that bridge: it drains the OpenUSD sink and projects the committed stage
// changes into ECS.

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

fn find_program_owner(
    world: &mut World,
    stage_id: AssetId<UsdStageAsset>,
    source_path: &str,
) -> Option<Entity> {
    let mut query = world.query::<(Entity, &UsdPrimPath, &lunco_core::ScenarioProgramPrim)>();
    query
        .iter(world)
        .find(|(_, prim, source)| prim.stage_handle.id() == stage_id && source.0 == source_path)
        .map(|(entity, _, _)| entity)
}

fn mark_stage_projected(
    world: &mut World,
    stage_id: AssetId<UsdStageAsset>,
) -> Option<(lunco_doc::DocumentId, u64)> {
    if let Some(mut backed) = world.get_resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>() {
        return backed.mark_stage_projected(stage_id);
    }
    None
}

/// Publish the generic document-to-ECS projection boundary after the stage
/// cursor has been advanced. Authored policies use this event to invalidate
/// cached composed topology when an edit was committed in the same frame that
/// its live entities were still being reconciled.
fn publish_stage_projected(world: &mut World, doc: lunco_doc::DocumentId, generation: u64) {
    let mut data = BTreeMap::new();
    data.insert(
        "doc_id".to_string(),
        lunco_telemetry_core::TelemetryValue::I64(doc.raw() as i64),
    );
    data.insert(
        "generation".to_string(),
        lunco_telemetry_core::TelemetryValue::I64(generation as i64),
    );
    world.trigger(lunco_telemetry_core::TelemetryEvent {
        name: "usd.document.projected".to_string(),
        source: 0,
        severity: lunco_telemetry_core::Severity::Info,
        data: lunco_telemetry_core::TelemetryValue::Map(data),
        timestamp: 0.0,
    });
}

fn publish_pending_stage_projections(world: &mut World) {
    let pending = world
        .get_resource_mut::<PendingStageProjections>()
        .map(|mut pending| std::mem::take(&mut pending.stages))
        .unwrap_or_default();
    for projection in pending.into_values() {
        let projected = world
            .resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
            .mark_document_projected(projection.doc, projection.stage_id, projection.generation);
        if let Some((doc, generation)) = projected {
            publish_stage_projected(world, doc, generation);
        }
    }
}

/// Re-project a live prim when a structural edit adds its simulation schemas.
///
/// A referenced instance may first appear as a typeless root while its layer
/// closure is loading. When the closure lands, the root's composed schema is
/// updated in place and the ordinary child resync does not necessarily include
/// the already-existing root. Keep the schema transition at this bridge, where
/// the live stage, ECS entity, and physics owner meet.
pub(crate) fn reproject_physics_if_needed(
    world: &mut World,
    stage_id: AssetId<UsdStageAsset>,
    path: &str,
) -> bool {
    let Ok(sdf_path) = SdfPath::new(path) else {
        return false;
    };
    let (has_rigid_body_api, has_vehicle_context_api) = world
        .get_non_send::<lunco_usd_bevy_core::canonical::CanonicalStages>()
        .and_then(|stages| stages.get(stage_id))
        .map(|stage| {
            let view = stage.view();
            (
                view.has_api_schema(&sdf_path, openusd::schemas::physics::tokens::API_RIGID_BODY),
                view.has_api_schema(&sdf_path, "PhysxVehicleContextAPI"),
            )
        })
        .unwrap_or_default();
    let Some(entity) = find_live_entity(world, stage_id, path) else {
        return false;
    };

    // OpenUSD reports a descendant AddPrim as a resync of already-existing
    // ancestors. That is structural context, not a schema transition. The
    // one-shot markers are therefore invalidated only when the corresponding
    // projection is actually missing: Avian owns the rigid-body admission,
    // while the sim owner identifies a vehicle context by MobilityRoot.
    let physics_invalidated = has_rigid_body_api
        && lunco_usd_avian_contracts::invalidate_usd_physics_projection(world, entity);
    let live_edit_owners = world
        .get_resource::<lunco_usd_bevy_core::live_edit::UsdLiveEditRegistry>()
        .map(|registry| registry.snapshot())
        .unwrap_or_default();
    let sim_invalidated = has_vehicle_context_api
        && live_edit_owners
            .iter()
            .any(|owner| owner.invalidates_projection(world, entity));
    if !physics_invalidated && !sim_invalidated {
        return false;
    }
    crate::twin_projection::refresh_prim_subtree(world, stage_id, path);
    true
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
/// [`CanonicalStage`]: lunco_usd_bevy_core::canonical::CanonicalStage
/// [`CanonicalStages`]: lunco_usd_bevy_core::canonical::CanonicalStages
pub(crate) fn project_stage_changes(world: &mut World) {
    use lunco_usd_bevy_core::canonical::CanonicalStages;

    if world.get_non_send::<CanonicalStages>().is_none() {
        return;
    }
    // Phase 1: drain the sink inboxes (owned + `Send`), releasing the borrow.
    let batches = world.non_send_mut::<CanonicalStages>().drain_all_changes();
    if batches.is_empty() {
        publish_pending_stage_projections(world);
        return;
    }

    let mut projected_anything = false;
    let mut connection_paths_changed = false;
    let mut input_defaults_changed = false;
    let mut projected_stages = HashSet::new();
    for (id, changes) in batches {
        let authored_transform_edits = world
            .get_resource_mut::<LiveTransformEditHints>()
            .map(|mut hints| hints.take(id))
            .unwrap_or_default();
        // Merge this stage's committed changes into one resync / info-only set.
        let mut resynced: Vec<String> = Vec::new();
        let mut info_only: Vec<String> = Vec::new();
        for c in changes {
            connection_paths_changed |= c.connection_paths_changed;
            resynced.extend(c.resynced.iter().map(|p| p.to_string()));
            info_only.extend(c.info_only.iter().map(|p| p.to_string()));
        }
        resynced.sort();
        resynced.dedup();
        info_only.sort();
        info_only.dedup();
        input_defaults_changed |= authored_input_defaults_changed(&info_only);
        for path in &info_only {
            if let Some((prim_path, attribute)) = path.split_once('.') {
                if attribute.starts_with("inputs:") {
                    mark_model_state_revision(world, prim_path);
                }
            }
        }

        if resynced.is_empty() && info_only.is_empty() && authored_transform_edits.is_empty() {
            if let Some((doc, generation)) = mark_stage_projected(world, id) {
                publish_stage_projected(world, doc, generation);
                remove_pending_stage_projection(world, id);
                projected_stages.insert(id);
            }
            continue;
        }
        projected_anything = true;

        let mut transform_edits = transform_edits(&info_only);
        for (path, channels) in authored_transform_edits {
            transform_edits.entry(path).or_default().merge(channels);
        }
        apply_transform_edits_live(world, id, &transform_edits);
        // Publish all changed prim paths so domain projectors can apply their
        // own cheap in-place updates before the generic subtree refresh below.
        let mut changed_prim_paths: Vec<String> = info_only
            .iter()
            .filter_map(|path| path.split_once('.').map(|(prim, _)| prim.to_string()))
            .collect();
        changed_prim_paths.sort();
        changed_prim_paths.dedup();
        if !changed_prim_paths.is_empty() {
            world.write_message(UsdSceneInfoChanged {
                stage_id: id,
                prim_paths: changed_prim_paths,
            });
        }
        // EVERYTHING ELSE. Any other authored attribute — a colour, a material
        // input, a light's intensity, a radius, `visibility` — re-projects here,
        // so a live edit shows up without reloading the scene.
        refresh_edited_prims_live(world, id, &info_only);
        reconcile_structural_live(world, id, &resynced);
        // The write-side projector has already authored this batch onto the
        // canonical stage. Publish the read-side generation only after this
        // sink batch has been reconciled into the live ECS projection. A query
        // that runs before this boundary receives an explicit "projection is
        // not current" result instead of stale composed data.
        if let Some((doc, generation)) = mark_stage_projected(world, id) {
            publish_stage_projected(world, doc, generation);
            remove_pending_stage_projection(world, id);
            projected_stages.insert(id);
        }
    }

    // A rebuild has already reconciled its full scene, while an idempotent
    // author may have no sink delta at all. Close those pending handoffs after
    // every sink-bearing stage has been handled, preserving one generic event
    // boundary for both projection paths.
    let pending = world
        .get_resource_mut::<PendingStageProjections>()
        .map(|mut pending| std::mem::take(&mut pending.stages))
        .unwrap_or_default();
    for projection in pending.into_values() {
        if projected_stages.contains(&projection.stage_id) {
            continue;
        }
        let projected = world
            .resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
            .mark_document_projected(projection.doc, projection.stage_id, projection.generation);
        if let Some((doc, generation)) = projected {
            publish_stage_projected(world, doc, generation);
        }
    }

    // Connections are derived from native `connectionPaths` by
    // The USD co-simulation wiring owner. Prim spawn/despawn triggers
    // that system directly (change-detection); a `connectionPaths` **edit** on an
    // already-spawned prim is neither — use the typed native-field notice to
    // re-derive off the live stage. Standard authored `inputs:*` edits also
    // publish a backend-neutral `ModelStateRevision`; the owning model adapter
    // decides whether that state needs a rebuild, reset, or only a live update.
    if connection_paths_changed || input_defaults_changed {
        if let Some(mut dirty) = world.get_resource_mut::<lunco_usd_bevy_core::UsdWiringDirty>() {
            dirty.0 = true;
        }
    }
    // Same reason, one level up: a live edit to an already-spawned prim changes
    // the composed stage without spawning or despawning anything, so it raises no
    // ECS-structural signal. Every USD-derived view-model gates on this revision
    // (`lunco_usd_bevy_scene::UsdStageRevision`), so without the bump an edit would
    // never reach the connection canvas or the prim tree.
    if projected_anything {
        if let Some(mut rev) = world.get_resource_mut::<lunco_usd_bevy_scene::UsdStageRevision>() {
            rev.bump();
        }
    }
}

/// Apply the changed composed transform channels to matching live entities,
/// reading from the **live [`CanonicalStage`]** (not the flatten) under one
/// short immutable borrow. The sink bridge applies this before structural
/// reconciliation so every transform channel has one reader and one live
/// projection owner.
///
/// [`CanonicalStage`]: lunco_usd_bevy_core::canonical::CanonicalStage
pub(crate) fn apply_transform_edits_live(
    world: &mut World,
    id: AssetId<UsdStageAsset>,
    edits: &HashMap<String, TransformEditChannels>,
) {
    use lunco_usd_bevy_core::canonical::CanonicalStages;
    if edits.is_empty() {
        return;
    }
    // Read every changed transform under one short borrow of the `!Send` stage,
    // then release it before mutating the world.
    let transforms: Vec<(String, TransformEditChannels, Transform)> = {
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return;
        };
        let Some(cs) = stages.get(id) else { return };
        let view = cs.view();
        edits
            .iter()
            .filter_map(|(path, channels)| {
                let sp = SdfPath::new(path).ok()?;
                match lunco_usd_bevy_core::local_transform_at(&view, &sp, 0.0) {
                    Ok(Some(transform)) => Some((path.clone(), *channels, transform)),
                    Ok(None) => None,
                    Err(error) => {
                        error!("[usd] transform edit rejected for {path}: {error}");
                        None
                    }
                }
            })
            .collect()
    };
    for (path, channels, transform) in transforms {
        let Some(entity) = find_live_entity(world, id, &path) else {
            // Named, because the silent version of this is "the edit journalled and
            // saved but nothing moved": an authored translate for a prim this stage
            // projects no entity for.
            debug!("[usd] transform edit {path}: no live entity on this stage");
            continue;
        };
        if channels.translate {
            seat_authored_translate(world, entity, transform.translation);
        }
        let preview_only =
            channels.scale && lunco_usd_bevy_scene::is_preview_only_entity(world, entity);
        if let Some(mut tf) = world.entity_mut(entity).get_mut::<Transform>() {
            if channels.rotate {
                tf.rotation = transform.rotation;
            }
            // Scale is an authoring-preview capability. Applying it to a live
            // simulation entity would change render scale without updating its
            // Avian collider/body contract, so the preview ownership marker is
            // the explicit admission boundary.
            if preview_only {
                tf.scale = transform.scale;
            }
        }
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
            // NOTE: a prim that carries a rigid body owns its pose in avian's
            // `Position`, and avian syncs `Position` → `Transform`, not the reverse —
            // so for those prims this seat is overwritten on the next physics tick.
            // Re-seating an admitted body remains owned by the USD physics
            // projection, next to the rest of its Avian mapping. Non-body
            // visuals are updated here; a scripted move of a rigid body keeps
            // its pose authority beside that mapping.
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
        let authored = Vec3::new(1.0 * EDGE + 10.0, 2.0 * EDGE - 53.0, -EDGE + 4.0);

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

/// Re-project every prim whose attributes were edited — the general live-edit
/// path, so **an edit shows up without reloading the scene**.
///
/// `info_only` carries both the owning prim path and the PROPERTY path naming the
/// changed attribute (pinned by `info_only_reports_both_prim_and_property_paths`).
/// That attribute name is what lets this be precise instead of a reload:
///
/// - **`xformOp:*`** — skipped. [`apply_transform_edits_live`] already wrote the
///   changed channels in place. Re-instantiating on a transform edit
///   would rebuild the mesh and re-run the physics observers on every frame of a
///   gizmo drag.
/// - **live curve geometry** — skipped. The generic USD visual projector rebuilds
///   the existing curve mesh from the canonical stage revision in place, so a
///   point/topology/width edit does not tear down the entity or its render binding.
/// - **`Shader` / `Material` prims** — a material edit fans out through
///   `material:binding` to arbitrary meshes elsewhere in the scene, so the prim's
///   own subtree is not enough: refresh the scene's visuals.
/// - **anything else** — re-instantiate just that prim's subtree.
///
/// `DomeLight`s are excluded: the light projection consumes
/// [`UsdSceneInfoChanged`] and handles them in place, which keeps the projected
/// cubemap when only the intensity or skybox flag moved instead of re-projecting
/// a 1024² cubemap per edit.
pub(crate) fn refresh_edited_prims_live(
    world: &mut World,
    id: AssetId<UsdStageAsset>,
    info_only: &[String],
) {
    use lunco_usd_bevy_core::canonical::CanonicalStages;
    if info_only.is_empty() {
        return;
    }

    // Split the property paths into (prim, attr). A prim path carries no `.`,
    // so the ones that do not split are the prim-path half of the same change and
    // are simply skipped here.
    let mut prims: Vec<String> = Vec::new();
    let mut program_updates: Vec<(
        Entity,
        Option<lunco_usd_bevy_core::program::ResolvedProgram>,
    )> = Vec::new();
    // Domain-owned in-place edits are excluded from the subtree refresh — which
    // can corrupt a synthesized runtime entity — and folded into one refresh
    // call per claiming owner after the loop.
    let live_edit_owners = world
        .get_resource::<lunco_usd_bevy_core::live_edit::UsdLiveEditRegistry>()
        .map(|registry| registry.snapshot())
        .unwrap_or_default();
    let mut claimed_live_edit_owners = HashSet::new();
    for p in info_only {
        let Some((prim, attr)) = p.split_once('.') else {
            continue;
        };
        {
            let claimed_owner = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(id))
                .zip(SdfPath::new(prim).ok())
                .and_then(|(cs, sp)| {
                    live_edit_owners
                        .iter()
                        .position(|owner| owner.claims_edit(&cs.view(), &sp, attr))
                });
            if let Some(owner) = claimed_owner {
                claimed_live_edit_owners.insert(owner);
                continue;
            }
        }
        // A mission tree is a `LunCoProgramAPI` child. The shared resolver owns
        // the selected source arm and backend; a live edit to either arm or to
        // the selector re-reads the tree from the prim that owns it.
        if matches!(
            attr,
            "info:implementationSource" | "info:sourceCode" | "info:sourceAsset"
        ) {
            let projected_owner = find_program_owner(world, id, prim);
            let authored_generic_program = if projected_owner.is_none() {
                SdfPath::new(prim)
                    .ok()
                    .and_then(|path| {
                        world
                            .get_non_send::<CanonicalStages>()
                            .and_then(|stages| stages.get(id))
                            .and_then(|stage| {
                                let view = stage.view();
                                lunco_usd_bevy_core::program::resolve_program(&view, &path)
                                    .ok()
                                    .filter(|program| {
                                        lunco_usd_bevy_core::program::is_generic_program_backend(
                                            program.backend,
                                        )
                                    })
                            })
                    })
                    .is_some()
            } else {
                false
            };
            if projected_owner.is_none() && !authored_generic_program {
                continue;
            }
            let Ok(sp) = SdfPath::new(prim) else {
                continue;
            };
            let resolved = world
                .get_non_send::<CanonicalStages>()
                .and_then(|stages| stages.get(id))
                .and_then(|stage| {
                    let view = stage.view();
                    match lunco_usd_bevy_core::program::resolve_program(&view, &sp) {
                        Ok(program) => Some(program),
                        Err(issue) => {
                            warn!(
                                "[usd-live] program {} is unresolved at {}: {}",
                                prim, issue.property, issue.message
                            );
                            None
                        }
                    }
                });
            if let Some(owner) = projected_owner {
                program_updates.push((owner, resolved));
            }
            continue;
        }
        if attr.starts_with("xformOp:") {
            continue;
        }
        if curve_geometry_edit(world, id, prim, attr) {
            continue;
        }
        if !prims.iter().any(|s| s == prim) {
            prims.push(prim.to_string());
        }
    }

    for owner in claimed_live_edit_owners {
        live_edit_owners[owner].refresh_stage(world, id);
    }

    for (owner, resolved) in program_updates {
        lunco_usd_bevy_core::program::apply_program_resolution(world, owner, id, resolved);
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
                // Already handled in place by the light projection.
                Some(ltok::T_DOME_LIGHT) => {}
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

/// Whether an info-only edit changes the tessellated geometry of a USD curve.
/// Curve entities stay live so the renderer can replace their mesh asset in
/// place; material, visibility, and other appearance edits still use the normal
/// prim refresh path.
fn curve_geometry_edit(
    world: &mut World,
    stage_id: AssetId<UsdStageAsset>,
    prim: &str,
    attribute: &str,
) -> bool {
    const CURVE_GEOMETRY_ATTRIBUTES: &[&str] = &[
        "points",
        "curveVertexCounts",
        "widths",
        "type",
        "wrap",
        "basis",
        "order",
        "knots",
        "pointWeights",
    ];
    if !CURVE_GEOMETRY_ATTRIBUTES.contains(&attribute) {
        return false;
    }
    let Ok(path) = SdfPath::new(prim) else {
        return false;
    };
    world
        .get_non_send::<lunco_usd_bevy_core::canonical::CanonicalStages>()
        .and_then(|stages| stages.get(stage_id))
        .is_some_and(|stage| {
            matches!(
                stage.view().type_name(&path).as_deref(),
                Some("BasisCurves") | Some("NurbsCurves")
            )
        })
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
/// builds the descendants. The normalized change set and the live-path check
/// are the only admission gates; the child constructor receives its resolved
/// parent and does not perform another duplicate lookup.
///
/// [`CanonicalStage`]: lunco_usd_bevy_core::canonical::CanonicalStage
pub(crate) fn reconcile_structural_live(
    world: &mut World,
    id: AssetId<UsdStageAsset>,
    resync_paths: &[String],
) {
    use lunco_usd_bevy_core::canonical::CanonicalStages;
    for path in resync_paths {
        let Ok(sp) = SdfPath::new(path) else { continue };
        // Program source is projected onto the existing owner, while the
        // program prim itself still follows the normal USD structural path.
        let program_owner = if let Some(owner) = find_program_owner(world, id, path) {
            Some(owner)
        } else {
            sp.parent()
                .and_then(|parent| find_live_entity(world, id, parent.as_str()))
                .filter(|_| {
                    world
                        .get_non_send::<CanonicalStages>()
                        .and_then(|stages| stages.get(id))
                        .is_some_and(|stage| stage.view().has_api_schema(&sp, "LunCoProgramAPI"))
                })
        };
        if let Some(owner) = program_owner {
            lunco_usd_bevy_authored_runtime::refresh_program_owner(world, id, owner);
        }
        let exists = {
            let Some(stages) = world.get_non_send::<CanonicalStages>() else {
                return;
            };
            match stages.get(id) {
                Some(cs) => cs.view().has_prim(&sp) && cs.view().is_active(&sp),
                None => return,
            }
        };
        let live = find_live_entity(world, id, path);
        match (exists, live) {
            (false, Some(entity)) => {
                if let Some(mut pending) =
                    world.get_resource_mut::<crate::twin_projection::PendingInstanceProjections>()
                {
                    pending.remove(id, path);
                }
                crate::scene::despawn_usd_subtree(world, entity);
            }
            (true, None) => {
                let parent_path =
                    path.rsplit_once('/')
                        .map(|(prefix, _)| if prefix.is_empty() { "/" } else { prefix });
                let Some(parent_path) = parent_path else {
                    continue;
                };
                let Some(parent_entity) = find_live_entity(world, id, parent_path) else {
                    // The parent is not projected yet. The next structural
                    // reconcile receives the normalized USD change set and
                    // retries once the parent exists.
                    continue;
                };
                // Pre-read the child's translate under a short borrow; the
                // observer builds the subtree from the still-present stage.
                let tf = {
                    let stages = world.non_send::<CanonicalStages>();
                    let Some(cs) = stages.get(id) else {
                        continue;
                    };
                    match lunco_usd_bevy_core::local_transform_at(&cs.view(), &sp, 0.0) {
                        Ok(Some(transform)) => transform,
                        Ok(None) => Transform::IDENTITY,
                        Err(error) => {
                            error!("[usd] incremental spawn rejected for {path}: {error}");
                            continue;
                        }
                    }
                };
                let instance_projection = world
                    .get_resource_mut::<crate::twin_projection::PendingInstanceProjections>()
                    .and_then(|mut pending| pending.take(id, path));
                let catalog_id = instance_projection.as_ref().and_then(|_| {
                    world
                        .non_send::<CanonicalStages>()
                        .get(id)
                        .and_then(|stage| stage.view().text(&sp, "lunco:catalogId"))
                        .filter(|value| !value.trim().is_empty())
                });
                if let Some(entity) =
                    crate::scene::spawn_usd_child_under_parent(world, parent_entity, path, tf)
                {
                    if let Some(mut projection) = instance_projection {
                        projection.root = Some(entity);
                        world
                            .entity_mut(entity)
                            .insert((lunco_usd_bevy_core::UsdInstanceRoot, projection));
                    }
                    if let Some(catalog_id) = catalog_id {
                        world
                            .entity_mut(entity)
                            .insert(lunco_core::CatalogEntryId(catalog_id));
                    }
                }
            }
            // ALREADY LIVE, AND RESYNCED — not "nothing to do".
            //
            // A descendant edit (for example adding `/Rover/Mission`) can report
            // `/Rover` as resynced even though no transform was authored. The
            // typed transform hint was applied above, so this structural pass
            // never overwrites a moving body with its authored spawn pose. The
            // physics bridge below likewise refreshes only a missing schema
            // projection, never an already admitted vehicle.
            (true, Some(_entity)) => {
                reproject_physics_if_needed(world, id, path);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

    #[test]
    fn transform_edits_extract_only_authored_channels() {
        let info_only = vec![
            "/World".to_string(),
            "/World/Rover".to_string(),
            "/World/Route/W0".to_string(),
            "/World/Route/W0.xformOp:translate".to_string(),
            "/World/Route/W0.xformOp:scale".to_string(),
            "/World/Rover/Mission.info:sourceCode".to_string(),
        ];

        assert_eq!(
            transform_edits(&info_only),
            HashMap::from([(
                "/World/Route/W0".to_string(),
                TransformEditChannels {
                    translate: true,
                    rotate: false,
                    scale: true,
                },
            )])
        );
    }

    /// **Moving an ALREADY-LIVE prim must move its entity.** Authoring
    /// `xformOp:translate` onto the live stage — the exact call
    /// `twin_projection::apply_incremental_op_to_stage` makes when it replays a
    /// `UsdOp::SetTranslate` — has to reach the projected `Transform`.
    ///
    /// Pinned because the failure is silent and looks like nothing happened: the
    /// op journals, saves and replicates while the object stays put. It is what
    /// the waypoint menu's `Move` hit, and it is invisible to the gizmo (whose
    /// drag moves the ECS first and authors afterwards, so its visual never came
    /// from this path at all).
    ///
    /// Whichever half of the sink reports the edit — `info_only` (attribute
    /// value) or `resynced` (the uniform `xformOpOrder` the same call appends
    /// to) — [`project_stage_changes`] must land the new value.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn authoring_a_translate_moves_an_already_live_entity() {
        use bevy::asset::AssetApp;
        use bevy::prelude::*;
        use lunco_usd_bevy_core::canonical::CanonicalStages;
        use lunco_usd_compose::recipe::StageRecipe;

        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<UsdStageAsset>()
            .init_non_send::<CanonicalStages>();

        let recipe = StageRecipe::from_source("scene.usda", TINY);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(UsdStageAsset::from_recipe(recipe.clone()).expect("prepare stage asset"));
        let id = handle.id();
        app.world_mut()
            .non_send_mut::<CanonicalStages>()
            .get_or_build(id, &recipe)
            .expect("stage builds");

        // A prim that is already on the stage AND already projected — the state a
        // second `SetTranslate` finds (the first one placed it at spawn).
        {
            let stages = app.world().non_send::<CanonicalStages>();
            let cs = stages.get(id).unwrap();
            cs.stage()
                .define_prim("/World/Pin")
                .unwrap()
                .set_type_name("Xform")
                .unwrap();
            cs.projector()
                .author_translate(&SdfPath::new("/World/Pin").unwrap(), [1.0, 0.0, 2.0])
                .expect("initial translate authors");
        }
        app.world_mut()
            .non_send_mut::<CanonicalStages>()
            .drain_all_changes();

        let pin = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: handle,
                    path: "/World/Pin".into(),
                },
                Transform::from_translation(Vec3::new(1.0, 0.0, 2.0)),
            ))
            .id();

        // THE MOVE.
        {
            let stages = app.world().non_send::<CanonicalStages>();
            stages
                .get(id)
                .unwrap()
                .projector()
                .author_translate(&SdfPath::new("/World/Pin").unwrap(), [5.0, 0.0, 7.0])
                .expect("move authors");
        }
        project_stage_changes(app.world_mut());

        let landed = app.world().get::<Transform>(pin).unwrap().translation;
        assert!(
            (landed - Vec3::new(5.0, 0.0, 7.0)).length() < 1e-4,
            "authored move must reach the live entity: Transform is {landed:?}, \
             authored (5, 0, 7)"
        );
    }

    /// **Scaling an ALREADY-LIVE USD preview must update its entity.** The
    /// document and canonical stage can advance independently of the render
    /// projection; this pins the sink bridge that keeps the preview's
    /// Inspector and geometry on the same composed transform.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn authoring_a_scale_updates_an_already_live_preview_entity() {
        use bevy::asset::AssetApp;
        use bevy::prelude::*;
        use lunco_usd_bevy_core::canonical::CanonicalStages;
        use lunco_usd_bevy_scene::UsdPreviewOnly;
        use lunco_usd_compose::recipe::StageRecipe;

        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<UsdStageAsset>()
            .init_non_send::<CanonicalStages>();

        let recipe = StageRecipe::from_source("scene.usda", TINY);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(UsdStageAsset::from_recipe(recipe.clone()).expect("prepare stage asset"));
        let id = handle.id();
        app.world_mut()
            .non_send_mut::<CanonicalStages>()
            .get_or_build(id, &recipe)
            .expect("stage builds");

        {
            let stages = app.world().non_send::<CanonicalStages>();
            let cs = stages.get(id).unwrap();
            cs.stage()
                .define_prim("/World/Panel")
                .unwrap()
                .set_type_name("Xform")
                .unwrap();
            cs.projector()
                .author_scale(&SdfPath::new("/World/Panel").unwrap(), [1.0, 2.0, 3.0])
                .expect("initial scale authors");
        }
        app.world_mut()
            .non_send_mut::<CanonicalStages>()
            .drain_all_changes();

        let preview_root = app.world_mut().spawn(UsdPreviewOnly).id();
        let panel = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: handle,
                    path: "/World/Panel".into(),
                },
                Transform::from_scale(Vec3::ONE),
                ChildOf(preview_root),
            ))
            .id();

        {
            let stages = app.world().non_send::<CanonicalStages>();
            stages
                .get(id)
                .unwrap()
                .projector()
                .author_scale(&SdfPath::new("/World/Panel").unwrap(), [4.0, 5.0, 6.0])
                .expect("preview scale authors");
        }
        project_stage_changes(app.world_mut());

        assert_eq!(
            app.world().get::<Transform>(panel).unwrap().scale,
            Vec3::new(4.0, 5.0, 6.0),
            "a composed SetScale must reach the already-live USD preview entity"
        );
    }

    /// A descendant structural edit may resync an existing ancestor in the
    /// OpenUSD sink. That notice is not a transform edit: re-seating the live
    /// entity from its authored spawn pose would teleport a moving physics body.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn unrelated_resync_preserves_an_already_live_pose() {
        use bevy::asset::AssetApp;
        use bevy::prelude::*;
        use lunco_usd_bevy_core::canonical::CanonicalStages;
        use lunco_usd_compose::recipe::StageRecipe;

        const SCENE: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n    metersPerUnit = 1.0\n    upAxis = \"Y\"\n)\ndef Xform \"World\"\n{\n    def Xform \"Rover\"\n    {\n        double3 xformOp:translate = (0, -1900, 0)\n        uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    }\n}\n";

        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<UsdStageAsset>()
            .init_non_send::<CanonicalStages>();
        let recipe = StageRecipe::from_source("scene.usda", SCENE);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(UsdStageAsset::from_recipe(recipe.clone()).expect("prepare stage asset"));
        let id = handle.id();
        app.world_mut()
            .non_send_mut::<CanonicalStages>()
            .get_or_build(id, &recipe)
            .expect("stage builds");
        app.world_mut()
            .non_send_mut::<CanonicalStages>()
            .drain_all_changes();

        let rover = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: handle,
                    path: "/World/Rover".into(),
                },
                Transform::from_translation(Vec3::new(10.0, -1901.0, 10.0)),
            ))
            .id();

        reconcile_structural_live(app.world_mut(), id, &["/World/Rover".to_string()]);
        assert_eq!(
            app.world().get::<Transform>(rover).unwrap().translation,
            Vec3::new(10.0, -1901.0, 10.0),
            "a descendant resync must not restore the authored spawn pose"
        );

        let authored_translate = HashMap::from([(
            "/World/Rover".to_string(),
            TransformEditChannels::translate(),
        )]);
        apply_transform_edits_live(app.world_mut(), id, &authored_translate);
        reconcile_structural_live(app.world_mut(), id, &["/World/Rover".to_string()]);
        assert_eq!(
            app.world().get::<Transform>(rover).unwrap().translation,
            Vec3::new(0.0, -1900.0, 0.0),
            "an explicit SetTranslate resync must still seat the authored pose"
        );
    }

    /// A structural notice for a child can include its already-admitted vehicle
    /// ancestor. That ancestor must keep the live simulation projection: clearing
    /// `UsdSimProcessed` here would refresh the rover and restart its wheels and
    /// Modelica participants even though no vehicle schema changed.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn descendant_resync_does_not_reproject_an_admitted_vehicle() {
        use bevy::asset::AssetApp;
        use bevy::prelude::*;
        use lunco_usd_bevy_core::canonical::CanonicalStages;
        use lunco_usd_compose::recipe::StageRecipe;

        const SCENE: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n    def Xform \"Rover\" (\n        prepend apiSchemas = [\"PhysicsRigidBodyAPI\", \"PhysxVehicleContextAPI\"]\n    )\n    {\n    }\n}\n";

        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<UsdStageAsset>()
            .init_non_send::<CanonicalStages>();
        let recipe = StageRecipe::from_source("vehicle.usda", SCENE);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(UsdStageAsset::from_recipe(recipe.clone()).expect("prepare stage asset"));
        let id = handle.id();
        app.world_mut()
            .non_send_mut::<CanonicalStages>()
            .get_or_build(id, &recipe)
            .expect("stage builds");
        app.world_mut()
            .non_send_mut::<CanonicalStages>()
            .drain_all_changes();

        let rover = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: handle,
                    path: "/World/Rover".into(),
                },
                avian3d::prelude::RigidBody::Dynamic,
                lunco_core::MobilityRoot,
                lunco_usd_sim_core::UsdSimProcessed,
            ))
            .id();

        assert!(
            !reproject_physics_if_needed(app.world_mut(), id, "/World/Rover"),
            "a descendant-only resync must not refresh an admitted vehicle"
        );
        assert!(
            app.world()
                .get::<lunco_usd_sim_core::UsdSimProcessed>(rover)
                .is_some(),
            "the admitted sim projection marker must remain live"
        );
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
        use lunco_usd_bevy_core::canonical::CanonicalStages;
        use lunco_usd_compose::recipe::StageRecipe;

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
            .add(UsdStageAsset::from_recipe(recipe.clone()).expect("prepare stage asset"));
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
                stage_handle: handle,
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
    /// - the prim path is what [`apply_transform_edits_live`] matches on (drop it and
    ///   gizmo moves stop projecting);
    /// - the property path is what names the CHANGED ATTRIBUTE, which is the only
    ///   way [`refresh_edited_prims_live`] can tell "the colour moved, re-project
    ///   the look" from "it was just a drag, use the cheap transform path".
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn info_only_reports_both_prim_and_property_paths() {
        use bevy::asset::AssetApp;
        use bevy::prelude::*;
        use lunco_usd_bevy_core::canonical::CanonicalStages;
        use lunco_usd_compose::recipe::StageRecipe;

        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<UsdStageAsset>()
            .init_non_send::<CanonicalStages>();

        let recipe = StageRecipe::from_source("scene.usda", TINY);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(UsdStageAsset::from_recipe(recipe.clone()).expect("prepare stage asset"));
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
