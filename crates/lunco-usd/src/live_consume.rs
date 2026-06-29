//! Incremental change consumption (E2): apply `UsdChange` deltas to the live
//! world entity-by-entity instead of full-reloading the scene on every edit.
//!
//! `UsdDocument` records granular deltas (`document::UsdChange`): a **move** is
//! `InfoOnly{path, "xformOp:translate"}` (cheap — just an entity transform),
//! while spawns/removes/renames are `Resync` and a wholesale replace is
//! `FullReload`. The doc-backed refresh systems (E1 `refresh_live_doc_scenes`,
//! E1b `sync_twin_overlays`) used to full-reload the *whole* scene on any
//! generation bump — so dragging a gizmo re-instantiated every rover + terrain
//! every frame of the drag.
//!
//! E2-1 makes those systems change-aware via the helpers here: classify the
//! changes since the last-synced generation, apply transform-only edits in
//! place (no reload), and fall back to the structural reload **only** when a
//! spawn/remove/rename (or any non-transform edit) appears. Incremental
//! spawn/despawn (E2-2/E2-3) will later replace that structural fallback too.

use bevy::prelude::*;
use lunco_doc::{Document, DocumentId};
use lunco_usd_bevy::{UsdData, UsdPrimPath, UsdStageAsset};
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
    /// Whether any change needs a full structural reload: a `Resync`
    /// (spawn/remove/rename), a `FullReload`, an `InfoOnly` for some other
    /// attribute, or a change-ring overflow (we can't prove we saw every delta).
    pub needs_structural: bool,
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
    let mut needs_structural = false;
    let mut count = 0u64;
    for (_g, change) in host.document().changes_since(since) {
        count += 1;
        match change {
            UsdChange::InfoOnly { path, attr } if attr == TRANSLATE_ATTR => {
                translate_paths.push(path.clone());
            }
            _ => needs_structural = true,
        }
    }
    // Generations are consecutive (one per commit), so we expect exactly
    // `cur_gen - since` deltas; fewer means the ring overflowed → reload.
    if count < cur_gen.saturating_sub(since) {
        needs_structural = true;
    }
    Some(ChangeBatch { translate_paths, needs_structural })
}

/// Apply the composed `xformOp:translate` for each `path` to its live entity
/// (scoped to the scene's `stage_handle_id`), mirroring `instantiate_usd_prim`'s
/// decode via [`lunco_usd_bevy::get_attribute_as_vec3`]. Unlike instantiation we
/// apply even a zero translate — an explicit move to the origin is a real edit,
/// not a spawn-position default to preserve. Skips paths with no live entity yet
/// (not instantiated) — a following structural reload would cover them.
pub(crate) fn apply_translates(
    world: &mut World,
    stage_handle_id: AssetId<UsdStageAsset>,
    composed: &UsdData,
    paths: &[String],
) {
    for path in paths {
        let Ok(sdf_path) = SdfPath::new(path) else {
            continue;
        };
        let Some(v) = lunco_usd_bevy::get_attribute_as_vec3(composed, &sdf_path, TRANSLATE_ATTR) else {
            continue;
        };
        let target = {
            let mut q = world.query::<(Entity, &UsdPrimPath)>();
            q.iter(world)
                .find(|(_, upp)| upp.stage_handle.id() == stage_handle_id && upp.path == *path)
                .map(|(e, _)| e)
        };
        let Some(entity) = target else { continue };
        if let Some(mut tf) = world.entity_mut(entity).get_mut::<Transform>() {
            tf.translation = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{LayerId, UsdOp};
    use lunco_doc::DocumentOrigin;

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

        // The spawn itself is structural.
        let b = classify_changes_since(&registry, doc, 0, after_spawn).unwrap();
        assert!(b.needs_structural, "AddPrim is a Resync → structural");

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
    }
}
