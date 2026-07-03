//! Live-world document projection (E1).
//!
//! The live 3D sim world and the editable [`UsdDocument`](crate::document)
//! used to be two *parallel* USD stages: opening a scene file additively
//! imported it into the world straight from disk (`AssetServer` →
//! `compose_file`), while the document — with its `runtime` overlay of C4b
//! spawns and moved transforms — lived only in the registry and the egui
//! viewport preview. Runtime edits never reached the live world.
//!
//! E1 closes that gap for **doc-backed** scenes (the
//! `docs/architecture/18-…` Phase-E north-star, first slice): when a USD
//! file is opened, the live scene root is mounted from the document's
//! **composed** (`base ⊕ runtime`) stage — exactly the source the viewport
//! already renders — instead of the raw file. So a reopened document's
//! persisted runtime spawns appear in the world, and later runtime edits
//! refresh it live.
//!
//! Two systems, mirroring the viewport's `install_active_doc` /
//! `rebuild_active_asset` split:
//!
//! - [`project_pending_live_imports`] — first mount. Document allocation is
//!   *async* (`on_open_file_for_usd` → `drain_pending_usd_file_loads`), so
//!   [`on_open_file`](crate::commands) can't mount inline; it records the
//!   path in [`PendingLiveImports`] and this system mounts once the matching
//!   document exists in the registry.
//! - [`refresh_live_doc_scenes`] — generation-keyed re-mount. Every runtime
//!   edit bumps the document generation (`UsdDocument::commit`), so this
//!   keeps each projected root's stage in sync with later spawns/moves —
//!   including the open-time `restore_runtime`, which also bumps generation
//!   (so the order of restore vs. first mount doesn't matter).
//!
//! Scope: **doc-backed only**. Twin default scenes (loaded via `LoadScene`
//! with no document) and `mem://` / `bundled://` imports keep the file-backed
//! path. Headless-safe — no UI, no egui.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::prelude::*;
// `Document` brings the `generation()` trait method into scope.
use lunco_doc::{Document, DocumentId, DocumentOrigin};
use lunco_usd_bevy::{UsdData, UsdPrimPath, UsdStageAsset, UsdVisualSynced};

use crate::registry::UsdDocumentRegistry;

/// A live-world import requested by [`OpenFile`](lunco_doc_bevy::OpenFile),
/// waiting for its document to finish loading. `attempts` bounds the retry so
/// a path that never becomes a registered document (e.g. a read failure)
/// doesn't churn forever.
struct PendingImport {
    /// Original `OpenFile` path; matched against document origins (after
    /// stripping a `file://` scheme).
    path: String,
    attempts: u32,
}

/// USD files opened for live import, pending their (asynchronously allocated)
/// document. Drained by [`project_pending_live_imports`].
#[derive(Resource, Default)]
pub struct PendingLiveImports {
    imports: Vec<PendingImport>,
}

impl PendingLiveImports {
    /// Queue `path` for live-world projection once its document exists.
    pub fn push(&mut self, path: String) {
        self.imports.push(PendingImport { path, attempts: 0 });
    }
}

/// Marks a live scene-root entity as the projection of document [`doc`]. Carries
/// the last generation projected so [`refresh_live_doc_scenes`] only re-mounts
/// when the document actually changed.
///
/// [`doc`]: LiveDocScene::doc
#[derive(Component, Debug)]
pub struct LiveDocScene {
    /// The document this live scene root projects.
    pub doc: DocumentId,
    /// The document generation currently mounted (`None` until first read).
    pub generation: Option<u64>,
}

/// Give up on a pending import after this many frames without a matching
/// document — long enough to outlast a slow async read, short enough not to
/// spin forever on a path that never registers.
const MAX_IMPORT_ATTEMPTS: u32 = 600;

/// A `file://`-scheme path reduced to its filesystem form; other inputs pass
/// through unchanged. Document origins store the stripped path
/// (`on_open_file_for_usd`), so we strip here before matching.
fn strip_file_scheme(path: &str) -> &str {
    path.strip_prefix("file://").unwrap_or(path)
}

/// Find the registered USD document whose origin is the on-disk file `fs_path`.
fn find_doc_for_path(world: &World, fs_path: &Path) -> Option<DocumentId> {
    let reg = world.resource::<UsdDocumentRegistry>();
    reg.ids().find(|id| {
        reg.host(*id)
            .map(|h| match h.document().origin() {
                DocumentOrigin::File { path, .. } => path == fs_path,
                _ => false,
            })
            .unwrap_or(false)
    })
}

/// The live scene-root entity already projecting `doc`, if any.
fn live_scene_entity(world: &mut World, doc: DocumentId) -> Option<Entity> {
    let mut q = world.query::<(Entity, &LiveDocScene)>();
    q.iter(world).find(|(_, s)| s.doc == doc).map(|(e, _)| e)
}

/// Parse a composed `.usda` source into a stage, resolving composition arcs
/// (references/payloads) against `base_dir` when one is known — same as the
/// viewport's `parse_reader`, so referenced geometry (glTF/terrain) surfaces.
/// Falls back to the raw root layer when there's no base dir or the flatten
/// fails (Untitled / in-memory docs, or wasm).
fn parse_composed(source: &str, base_dir: Option<&Path>) -> Option<UsdData> {
    if let Some(dir) = base_dir {
        if let Some(flat) = lunco_usd_bevy::compose_native_fs(source, dir) {
            return Some(flat);
        }
        warn!("[usd-e1] compose failed for {dir:?} — falling back to raw layer");
    }
    openusd::usda::parse(source).ok()
}

/// Build the composed (`base ⊕ runtime`) stage for `doc`, ready to wrap in a
/// [`UsdStageAsset`]. `None` if the document is gone or unparseable.
fn composed_reader(world: &World, doc: DocumentId) -> Option<UsdData> {
    let (source, base) = {
        let host = world.resource::<UsdDocumentRegistry>().host(doc)?;
        let source = host.document().composed_source();
        let base = match host.document().origin() {
            DocumentOrigin::File { path, .. } => path.parent().map(Path::to_path_buf),
            _ => None,
        };
        (source, base)
    };
    parse_composed(&source, base.as_deref())
}

/// The document's current generation, or `None` if it's no longer registered.
fn doc_generation(world: &World, doc: DocumentId) -> Option<u64> {
    world
        .resource::<UsdDocumentRegistry>()
        .host(doc)
        .map(|h| h.document().generation())
}

/// First-mount system: drain [`PendingLiveImports`], and for each whose
/// document has now been allocated, mount that document's composed stage as a
/// live scene root (tagged [`LiveDocScene`]). Imports whose document hasn't
/// loaded yet are retried next frame, up to [`MAX_IMPORT_ATTEMPTS`].
pub(crate) fn project_pending_live_imports(world: &mut World) {
    // No `UsdStageAsset` store ⇒ no live world to project into (a pure-document /
    // headless context, e.g. the open-file unit tests). Leave pending untouched.
    if !world.contains_resource::<Assets<UsdStageAsset>>() {
        return;
    }
    let pending = std::mem::take(&mut world.resource_mut::<PendingLiveImports>().imports);
    if pending.is_empty() {
        return;
    }
    let mut still = Vec::new();
    for mut import in pending {
        let fs_path = PathBuf::from(strip_file_scheme(&import.path));
        let Some(doc) = find_doc_for_path(world, &fs_path) else {
            import.attempts += 1;
            if import.attempts < MAX_IMPORT_ATTEMPTS {
                still.push(import);
            } else {
                warn!("[usd-e1] no document for `{}` after {MAX_IMPORT_ATTEMPTS} frames — dropping live import", import.path);
            }
            continue;
        };
        // Already projected (idempotent re-open) — nothing to do.
        if live_scene_entity(world, doc).is_some() {
            continue;
        }
        let Some(reader) = composed_reader(world, doc) else {
            // Document exists but won't parse yet — retry.
            import.attempts += 1;
            if import.attempts < MAX_IMPORT_ATTEMPTS {
                still.push(import);
            }
            continue;
        };
        let generation = doc_generation(world, doc);
        let handle = world
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(UsdStageAsset { reader: Arc::new(reader), recipe: None });
        let label = fs_path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| import.path.clone());
        if let Some(root) =
            lunco_usd_sim::cosim::spawn_scene_root_with_stage(world, &label, "", handle)
        {
            world
                .entity_mut(root)
                .insert(LiveDocScene { doc, generation });
            info!("[usd-e1] projected document {doc} into live world (entity {root:?})");
        }
    }
    world.resource_mut::<PendingLiveImports>().imports.extend(still);
}

/// Refresh system: for every projected [`LiveDocScene`] whose document
/// generation moved past the one mounted, rebuild its stage from the new
/// composed source in place — re-triggering instantiation the same way the
/// viewport's `rebuild_active_asset` does (mutate the asset, drop the synced
/// marker + children, re-insert `UsdPrimPath`). Keeps live geometry in step
/// with runtime spawns/moves and the open-time `restore_runtime`.
pub(crate) fn refresh_live_doc_scenes(world: &mut World) {
    if !world.contains_resource::<Assets<UsdStageAsset>>() {
        return;
    }
    let targets: Vec<(Entity, DocumentId, Option<u64>)> = {
        let mut q = world.query::<(Entity, &LiveDocScene)>();
        q.iter(world).map(|(e, s)| (e, s.doc, s.generation)).collect()
    };
    for (entity, doc, mounted_gen) in targets {
        let Some(cur_gen) = doc_generation(world, doc) else {
            continue; // document closed — leave the last-projected geometry standing
        };
        if Some(cur_gen) == mounted_gen {
            continue; // already current
        }
        let Some(handle) = world.get::<UsdPrimPath>(entity).map(|p| p.stage_handle.clone()) else {
            continue;
        };
        // E2-1: classify the changes since the mounted generation. A move
        // (`InfoOnly` translate) is applied in place; only a spawn/remove/rename
        // (or any other edit) needs the full in-place rebuild below.
        let batch = crate::live_consume::classify_changes_since(
            world.resource::<UsdDocumentRegistry>(),
            doc,
            mounted_gen.unwrap_or(0),
            cur_gen,
        );

        // Incremental transforms — read the cheap base⊕runtime merge (NOT the
        // full PCP flatten) for the new translate, apply to the live entity.
        if let Some(batch) = &batch {
            if !batch.translate_paths.is_empty() {
                let composed = world
                    .resource::<UsdDocumentRegistry>()
                    .host(doc)
                    .map(|h| h.document().composed());
                if let Some(composed) = composed {
                    crate::live_consume::apply_translates(
                        world,
                        handle.id(),
                        &composed,
                        &batch.translate_paths,
                    );
                }
            }
        }

        // Structural changes. E2-2/E2-3: when we have the exact set of changed
        // prim paths (a per-prim `Resync`, not a whole-source `FullReload` or a
        // ring overflow), refresh the asset reader and reconcile just those
        // subtrees — spawn the added, despawn the removed — leaving every
        // sibling rover/terrain entity untouched. Only a `full_reload` (or an
        // unknown batch) falls back to the whole-scene in-place rebuild.
        let needs_structural = batch.as_ref().map(|b| b.needs_structural).unwrap_or(true);
        if needs_structural {
            let full = batch.as_ref().map(|b| b.full_reload).unwrap_or(true);
            let resync = batch.as_ref().map(|b| b.resync_paths.clone()).unwrap_or_default();
            if let Some(reader) = composed_reader(world, doc) {
                // Refresh the shared stage reader either way — both the reconcile
                // observer and the full rebuild read it from the asset store.
                if let Some(asset) = world.resource_mut::<Assets<UsdStageAsset>>().get_mut(&handle) {
                    asset.reader = Arc::new(reader);
                }
                if !full && !resync.is_empty() {
                    // Incremental: diff only the changed prims against the fresh
                    // reader (E2-4 — no whole-scene re-instantiation).
                    if let Some(reader_arc) =
                        world.resource::<Assets<UsdStageAsset>>().get(&handle).map(|a| a.reader.clone())
                    {
                        crate::live_consume::reconcile_structural(
                            world,
                            handle.id(),
                            &reader_arc,
                            &resync,
                        );
                    }
                } else if let Ok(mut em) = world.get_entity_mut(entity) {
                    // Coarse fallback: drop the synced marker + children and
                    // re-insert `UsdPrimPath` to rebuild the whole subtree.
                    em.remove::<UsdVisualSynced>();
                    em.despawn_related::<Children>();
                    if let Some(pp) = em.take::<UsdPrimPath>() {
                        em.insert(pp);
                    }
                }
            }
        }
        if let Some(mut s) = world.entity_mut(entity).get_mut::<LiveDocScene>() {
            s.generation = Some(cur_gen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::UsdDocumentRegistry;
    use lunco_doc::DocumentOrigin;

    const TINY: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

    #[test]
    fn strip_file_scheme_only_removes_the_scheme() {
        assert_eq!(strip_file_scheme("file:///a/b.usda"), "/a/b.usda");
        assert_eq!(strip_file_scheme("/a/b.usda"), "/a/b.usda");
        assert_eq!(strip_file_scheme("twin://t/s.usda"), "twin://t/s.usda");
    }

    #[test]
    fn parse_composed_reads_inline_source_without_base_dir() {
        let data = parse_composed(TINY, None).expect("parses inline usda");
        let world = openusd::sdf::Path::new("/World").unwrap();
        assert!(data.spec(&world).is_some(), "World prim present");
    }

    #[test]
    fn find_doc_for_path_matches_file_origin_only() {
        let mut world = World::new();
        let mut reg = UsdDocumentRegistry::default();
        let path = PathBuf::from("/tmp/e1_scene.usda");
        let doc = reg.allocate(TINY.to_string(), DocumentOrigin::writable_file(path.clone()));
        // An untitled doc must NOT match by path.
        reg.allocate(TINY.to_string(), DocumentOrigin::untitled("Untitled.usda"));
        world.insert_resource(reg);

        assert_eq!(find_doc_for_path(&world, &path), Some(doc));
        assert_eq!(find_doc_for_path(&world, Path::new("/tmp/other.usda")), None);
    }

    #[test]
    fn composed_reader_includes_runtime_layer_spawns() {
        use crate::document::{LayerId, UsdOp};

        let mut world = World::new();
        let mut reg = UsdDocumentRegistry::default();
        // Untitled origin → no base dir → `composed_reader` takes the
        // deterministic `usda::parse(composed_source)` path (no filesystem
        // resolver). The reference-resolving `compose_native_fs` branch is the
        // viewport's already-proven production path; here we pin the
        // E1-specific property: the *composed overlay* carries runtime spawns.
        let doc = reg.allocate(TINY.to_string(), DocumentOrigin::untitled("e1.usda"));
        // Author a realistic C4b spawn (a runtime prim that references its asset)
        // into the runtime overlay.
        reg.host_mut(doc)
            .unwrap()
            .document_mut()
            .apply(UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path: "/World".into(),
                name: "rover_1".into(),
                type_name: Some("Xform".into()),
                reference: Some("vessels/rovers/skid_rover.usda".into()),
            })
            .unwrap();

        let spawned = openusd::sdf::Path::new("/World/rover_1").unwrap();
        // The pure base ⊕ runtime merge carries the spawn (independent of any
        // serialization round-trip).
        assert!(
            reg.host(doc).unwrap().document().composed().spec(&spawned).is_some(),
            "composed overlay (base ⊕ runtime) carries the runtime spawn"
        );
        world.insert_resource(reg);

        let reader = composed_reader(&world, doc).expect("composed parses");
        assert!(
            reader.spec(&spawned).is_some(),
            "runtime-layer spawn rides the composed stage projected into the live world"
        );
    }
}
