//! Doc-backed twin default scene (E1b) — web-ready via the twin asset source.
//!
//! E1 ([`live_projection`](crate::live_projection)) makes scenes opened via
//! `OpenFile` doc-backed, but it composes synchronously off the filesystem
//! (`compose_native_fs`) — native-only, and it loses the `twin://` scheme that
//! co-located refs (terrain `.glb`) need. The **default twin scene** loads
//! through the `twin://` asset source and the async [`UsdLoader`], which already
//! re-attaches the scheme so refs resolve correctly on every platform the source
//! supports. E1b keeps that path and makes it doc-backed by serving the scene
//! document's **composed** (`base ⊕ runtime`) source as a *byte-overlay* on the
//! twin source, so the live world composes from the editable document — and
//! reloaded runtime spawns/moves appear live.
//!
//! Flow (`open_usd_docs_on_twin_added` keeps firing `LoadScene` for the immediate
//! live mount; E1b runs alongside):
//! 1. On `TwinAdded` with a `[usd] default_scene`, kick an async
//!    [`UsdSourceText`] load of `twin://<name>/<scene>` (raw base layer, read
//!    through the twin source — web-ready) and record it in [`PendingTwinDocs`].
//! 2. [`drain_pending_twin_docs`] — once the source text is in hand, allocate a
//!    [`UsdDocument`](crate::document) for it (origin = the on-disk path, so Save
//!    and dedup work) and record it in [`DocBackedTwinScenes`].
//! 3. [`sync_twin_overlays`] — whenever the document generation moves (initial
//!    mount, open-time `restore_runtime`, or a later spawn/move), serialize the
//!    composed source into the twin **overlay** and `reload` the scene asset; the
//!    existing asset-reload → re-instantiate machinery refeeds the live world.
//!
//! Scope: the **default twin scene** only. Arbitrary `OpenFile` scenes stay on
//! E1's path; `mem://` / `bundled://` keep the file-backed import.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bevy::asset::AssetId;
use bevy::prelude::*;
use lunco_assets::twin_source::TwinRoots;
use lunco_doc::{Document, DocumentId, DocumentOrigin};
use lunco_usd_bevy::{UsdPrimPath, UsdSourceText, UsdStageAsset, UsdVisualSynced};

use crate::registry::UsdDocumentRegistry;

/// A default-twin-scene document waiting for its base source text to finish
/// loading through the twin source.
struct PendingTwinDoc {
    /// In-flight raw-source load of `twin://<name>/<rel>`.
    handle: Handle<UsdSourceText>,
    /// Twin name (the `twin://` first segment).
    name: String,
    /// Scene path relative to the twin root (the `twin://` remainder).
    rel: String,
    /// On-disk absolute path — the document origin (Save target + dedup key).
    abs_path: PathBuf,
    attempts: u32,
}

/// Default twin scenes whose base source is still loading. Drained by
/// [`drain_pending_twin_docs`].
#[derive(Resource, Default)]
pub struct PendingTwinDocs {
    items: Vec<PendingTwinDoc>,
}

impl PendingTwinDocs {
    /// Queue a default twin scene for doc-backed projection.
    pub fn push(&mut self, handle: Handle<UsdSourceText>, name: String, rel: String, abs_path: PathBuf) {
        self.items.push(PendingTwinDoc { handle, name, rel, abs_path, attempts: 0 });
    }
}

/// Make a twin scene **doc-backed** (E1b) outside the `TwinAdded` observer — for
/// binaries that mount a twin scene via a direct [`LoadScene`](crate::LoadScene)
/// (e.g. the sandbox `--scene`) rather than opening a workspace Twin. Reads the
/// scene's base layer through the twin source (`twin://<name>/<rel>`, web-ready)
/// and queues a document for it; once allocated, [`sync_twin_overlays`] serves the
/// composed (`base ⊕ runtime`) source so runtime USD edits project to the live
/// world. The immediate file-backed mount still comes from the caller's
/// `LoadScene` — exactly the dual path `open_usd_docs_on_twin_added` runs in
/// production. `abs_path` is the scene's on-disk path (the document origin → Save
/// target + dedup key).
pub fn doc_back_twin_scene(
    asset_server: &AssetServer,
    pending: &mut PendingTwinDocs,
    twin_name: &str,
    rel: &str,
    abs_path: PathBuf,
) {
    let handle = asset_server.load::<UsdSourceText>(format!("twin://{twin_name}/{rel}"));
    pending.push(handle, twin_name.to_string(), rel.to_string(), abs_path);
}

/// The twin-source coordinates + last-synced generation for a doc-backed twin
/// scene, so [`sync_twin_overlays`] re-serializes only when the document moved.
struct TwinSceneRef {
    name: String,
    rel: String,
    synced_generation: Option<u64>,
}

/// Map of document → the twin scene it backs. Populated by
/// [`drain_pending_twin_docs`], consumed by [`sync_twin_overlays`].
#[derive(Resource, Default)]
pub struct DocBackedTwinScenes {
    map: HashMap<DocumentId, TwinSceneRef>,
}

/// A deferred structural reconcile for a twin scene, queued by
/// [`sync_twin_overlays`] when the document moved structurally and the
/// flattened reader is being refreshed **asynchronously** through the twin
/// source. Consumed by [`drain_twin_reconciles`] once that reload lands (so the
/// reconcile sees the fresh, `twin://`-resolved reader).
struct TwinReconcile {
    /// The reloaded scene asset this reconcile is waiting on.
    handle_id: AssetId<UsdStageAsset>,
    /// Changed prim paths to reconcile per-subtree (spawn added / despawn
    /// removed). Empty + `full` means whole-scene rebuild.
    resync_paths: Vec<String>,
    /// Whole-scene rebuild rather than per-prim reconcile (FullReload / overflow
    /// / a structural change with no concrete prim path to reconcile).
    full: bool,
}

/// Structural reconciles waiting on their twin asset's async reload to land.
/// Populated by [`sync_twin_overlays`], drained by [`drain_twin_reconciles`].
#[derive(Resource, Default)]
pub struct PendingTwinReconciles {
    items: Vec<TwinReconcile>,
}

impl PendingTwinReconciles {
    fn push(&mut self, handle_id: AssetId<UsdStageAsset>, resync_paths: Vec<String>, full: bool) {
        // A structural change with no concrete prim path can't be reconciled
        // per-prim — force the whole-scene rebuild.
        let full = full || resync_paths.is_empty();
        self.items.push(TwinReconcile { handle_id, resync_paths, full });
    }
}

/// Twin scene assets whose async reload finished this frame — buffered by
/// [`collect_reloaded_twin_assets`] (a cheap `MessageReader` system) so the
/// exclusive [`drain_twin_reconciles`] can match them against
/// [`PendingTwinReconciles`].
#[derive(Resource, Default)]
pub struct ReloadedTwinAssets {
    ids: Vec<AssetId<UsdStageAsset>>,
}

/// Give up on a pending twin doc after this many frames without its source
/// loading (a missing / unreadable scene), so it doesn't retry forever.
const MAX_TWIN_DOC_ATTEMPTS: u32 = 600;

/// The registered USD document whose origin is the on-disk file `abs`, if any.
fn find_doc_for_abs(registry: &UsdDocumentRegistry, abs: &std::path::Path) -> Option<DocumentId> {
    registry.ids().find(|id| {
        registry
            .host(*id)
            .map(|h| match h.document().origin() {
                DocumentOrigin::File { path, .. } => path == abs,
                _ => false,
            })
            .unwrap_or(false)
    })
}

/// Allocate the document for each pending twin scene once its base source text
/// has loaded through the twin source. Idempotent: reuses an existing document
/// for the same on-disk path (twin re-add) rather than double-allocating.
pub(crate) fn drain_pending_twin_docs(
    mut pending: ResMut<PendingTwinDocs>,
    mut registry: ResMut<UsdDocumentRegistry>,
    mut backed: ResMut<DocBackedTwinScenes>,
    sources: Res<Assets<UsdSourceText>>,
) {
    if pending.items.is_empty() {
        return;
    }
    let taken = std::mem::take(&mut pending.items);
    let mut still = Vec::new();
    for mut item in taken {
        let Some(UsdSourceText(source)) = sources.get(&item.handle) else {
            item.attempts += 1;
            if item.attempts < MAX_TWIN_DOC_ATTEMPTS {
                still.push(item);
            } else {
                warn!(
                    "[usd-e1b] base source for `twin://{}/{}` never loaded — no doc-backed projection",
                    item.name, item.rel
                );
            }
            continue;
        };
        let doc = find_doc_for_abs(&registry, &item.abs_path).unwrap_or_else(|| {
            registry.allocate(source.clone(), DocumentOrigin::writable_file(item.abs_path.clone()))
        });
        backed.map.entry(doc).or_insert(TwinSceneRef {
            name: item.name.clone(),
            rel: item.rel.clone(),
            synced_generation: None,
        });
        info!("[usd-e1b] default scene `twin://{}/{}` is now doc-backed ({doc})", item.name, item.rel);
    }
    pending.items.extend(still);
}

/// Keep each doc-backed twin scene's twin-source overlay in step with its
/// document: when the generation moves, serialize the composed (`base ⊕ runtime`)
/// source into the overlay and `reload` the scene asset so the live world
/// re-composes from the document (web-ready — the async loader anchors at the
/// `twin://` identity). Drops entries whose document has closed.
pub(crate) fn sync_twin_overlays(world: &mut World) {
    // Snapshot tracked scenes (owned) so no resource borrow is held across the
    // world mutations below.
    let entries: Vec<(DocumentId, String, String, Option<u64>)> = world
        .resource::<DocBackedTwinScenes>()
        .map
        .iter()
        .map(|(doc, s)| (*doc, s.name.clone(), s.rel.clone(), s.synced_generation))
        .collect();

    for (doc, name, rel, synced) in entries {
        // Cheap generation probe FIRST — then early-out. The expensive payloads
        // below (`composed_source()` re-serializes the whole composed stage to a
        // String; `composed()` recomposes it) must NOT be computed every frame:
        // the document is unchanged on the overwhelming majority of frames and we
        // `continue` right after this check, so computing them up-front was
        // ~212µs/frame of pure waste (profiled on the moonbase twin). Read only
        // the generation here; pay for the payloads only once it has moved.
        let cur_gen = match world.resource::<UsdDocumentRegistry>().host(doc) {
            Some(h) => h.document().generation(),
            None => {
                world.resource::<TwinRoots>().clear_overlay(&name, &rel);
                world.resource_mut::<DocBackedTwinScenes>().map.remove(&doc);
                continue;
            }
        };
        if Some(cur_gen) == synced {
            continue;
        }
        // Generation moved — now (and only now) pay for the composed payloads.
        let (composed_source, composed) = {
            let reg = world.resource::<UsdDocumentRegistry>();
            let h = reg
                .host(doc)
                .expect("doc host present: its generation was just read above (single-threaded exclusive system, no despawn between)");
            (h.document().composed_source(), h.document().composed())
        };

        // Always refresh the overlay so persistence / next load reflect the doc,
        // regardless of whether we reload the live asset.
        world
            .resource::<TwinRoots>()
            .set_overlay(&name, &rel, Arc::new(composed_source.into_bytes()));

        // E2-1: classify the changes. The twin scene's live stage handle is the
        // cached `twin://name/rel` UsdStageAsset handle (AssetServer dedups by
        // path), shared by every child prim entity — so transform-only edits can
        // be applied in place without re-reading the whole asset.
        let twin_path = format!("twin://{}/{}", name, rel);
        let batch = crate::live_consume::classify_changes_since(
            world.resource::<UsdDocumentRegistry>(),
            doc,
            synced.unwrap_or(0),
            cur_gen,
        );
        let handle = world
            .resource::<AssetServer>()
            .load::<UsdStageAsset>(twin_path.clone());

        if let Some(batch) = &batch {
            if !batch.translate_paths.is_empty() {
                crate::live_consume::apply_translates(
                    world,
                    handle.id(),
                    &composed,
                    &batch.translate_paths,
                );
            }
        }

        // Structural changes refresh the flattened reader through the overlay
        // (async — the twin loader resolves `twin://` / `lunco://` refs that a
        // synchronous compose can't), then reconcile once that reload lands
        // (`drain_twin_reconciles`): spawn the added subtrees, despawn the
        // removed, leaving siblings untouched (E2-2/E2-3/E2-4).
        //
        // On the very first mount (`synced == None`) the reload *is* the initial
        // build via `sync_usd_visuals`, so there's no baseline to diff — just
        // reload, no reconcile queued.
        let needs_structural = batch.as_ref().map(|b| b.needs_structural).unwrap_or(true);
        if needs_structural {
            if synced.is_some() {
                let full = batch.as_ref().map(|b| b.full_reload).unwrap_or(true);
                let resync = batch.as_ref().map(|b| b.resync_paths.clone()).unwrap_or_default();
                world
                    .resource_mut::<PendingTwinReconciles>()
                    .push(handle.id(), resync, full);
            }
            world.resource::<AssetServer>().reload(twin_path);
        }

        if let Some(s) = world.resource_mut::<DocBackedTwinScenes>().map.get_mut(&doc) {
            s.synced_generation = Some(cur_gen);
        }
    }
}

/// Buffer the ids of twin scene assets that finished (re)loading this frame, so
/// the exclusive [`drain_twin_reconciles`] can match them against
/// [`PendingTwinReconciles`]. Cheap `MessageReader` system — runs only on frames
/// an asset event actually fires.
pub(crate) fn collect_reloaded_twin_assets(
    mut ev: MessageReader<AssetEvent<UsdStageAsset>>,
    mut out: ResMut<ReloadedTwinAssets>,
) {
    for event in ev.read() {
        if let AssetEvent::LoadedWithDependencies { id } = event {
            out.ids.push(*id);
        }
    }
}

/// Run the structural reconciles whose twin asset reload has now landed: for a
/// per-prim batch, [`reconcile_structural`](crate::live_consume::reconcile_structural)
/// against the fresh reader; for a `full` batch, an in-place whole-scene
/// rebuild. Reconciles whose reload hasn't arrived yet are retried next frame.
pub(crate) fn drain_twin_reconciles(world: &mut World) {
    let loaded = std::mem::take(&mut world.resource_mut::<ReloadedTwinAssets>().ids);
    if loaded.is_empty() {
        return;
    }
    if world.resource::<PendingTwinReconciles>().items.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut world.resource_mut::<PendingTwinReconciles>().items);
    let mut still = Vec::new();
    for item in pending {
        if !loaded.contains(&item.handle_id) {
            still.push(item);
            continue;
        }
        // Fresh, fully-resolved reader from the asset store.
        let reader = world
            .resource::<Assets<UsdStageAsset>>()
            .get(item.handle_id)
            .map(|a| a.reader.clone());
        let Some(reader) = reader else {
            continue; // asset gone — drop the reconcile
        };
        if item.full {
            full_rebuild_twin_scene(world, item.handle_id);
        } else {
            crate::live_consume::reconcile_structural(
                world,
                item.handle_id,
                &reader,
                &item.resync_paths,
            );
        }
    }
    world.resource_mut::<PendingTwinReconciles>().items.extend(still);
}

/// Whole-scene in-place rebuild for a twin scene (the coarse fallback): for each
/// scene-root entity of `handle_id`, drop its `UsdVisualSynced` marker +
/// children and re-insert `UsdPrimPath` so `on_usd_prim_added` re-instantiates
/// the subtree against the freshly-reloaded reader. A scene root is an entity of
/// this scene whose parent is *not* itself a prim of the same scene (i.e. it
/// hangs off the world grid, not another USD prim).
fn full_rebuild_twin_scene(world: &mut World, handle_id: AssetId<UsdStageAsset>) {
    let scene: Vec<(Entity, Option<Entity>)> = {
        let mut q = world.query::<(Entity, &UsdPrimPath, Option<&ChildOf>)>();
        q.iter(world)
            .filter(|(_, upp, _)| upp.stage_handle.id() == handle_id)
            .map(|(e, _, parent)| (e, parent.map(|p| p.0)))
            .collect()
    };
    let members: std::collections::HashSet<Entity> = scene.iter().map(|(e, _)| *e).collect();
    let roots: Vec<Entity> = scene
        .iter()
        .filter(|(_, parent)| parent.map(|p| !members.contains(&p)).unwrap_or(true))
        .map(|(e, _)| *e)
        .collect();
    for root in roots {
        if let Ok(mut em) = world.get_entity_mut(root) {
            em.remove::<UsdVisualSynced>();
            em.despawn_related::<Children>();
            if let Some(pp) = em.take::<UsdPrimPath>() {
                em.insert(pp);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{LayerId, UsdOp};

    const TINY: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

    #[test]
    fn find_doc_for_abs_matches_file_origin_only() {
        let mut registry = UsdDocumentRegistry::default();
        let abs = PathBuf::from("/twins/moonbase/scene.usda");
        let doc = registry.allocate(TINY.to_string(), DocumentOrigin::writable_file(abs.clone()));
        registry.allocate(TINY.to_string(), DocumentOrigin::untitled("Untitled.usda"));

        assert_eq!(find_doc_for_abs(&registry, &abs), Some(doc));
        assert_eq!(find_doc_for_abs(&registry, std::path::Path::new("/twins/x.usda")), None);
    }

    /// The bytes pushed into the overlay are the document's *composed* source —
    /// so a runtime-layer spawn rides into the live world's composition.
    #[test]
    fn composed_source_overlay_carries_runtime_spawn() {
        let mut registry = UsdDocumentRegistry::default();
        let abs = PathBuf::from("/twins/moonbase/scene.usda");
        let doc = registry.allocate(TINY.to_string(), DocumentOrigin::writable_file(abs));
        registry
            .host_mut(doc)
            .unwrap()
            .document_mut()
            .apply(UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path: "/World".into(),
                name: "rover_1".into(),
                type_name: Some("Xform".into()),
                reference: Some("lunco://vessels/rovers/skid_rover.usda".into()),
            })
            .unwrap();

        let composed = registry.host(doc).unwrap().document().composed_source();
        assert!(
            composed.contains("rover_1"),
            "overlay bytes carry the runtime spawn:\n{composed}"
        );
        assert!(
            composed.contains("@lunco://vessels/rovers/skid_rover.usda@"),
            "and its asset reference (resolved by the async loader at the twin:// anchor)"
        );
    }
}
