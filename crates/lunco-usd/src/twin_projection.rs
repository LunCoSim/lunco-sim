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

use bevy::prelude::*;
use lunco_assets::twin_source::TwinRoots;
use lunco_doc::{Document, DocumentId, DocumentOrigin};
use lunco_usd_bevy::{UsdSourceText, UsdStageAsset};

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
        // Current generation + composed payloads, or drop if the doc closed.
        let resolved = world.resource::<UsdDocumentRegistry>().host(doc).map(|h| {
            (
                h.document().generation(),
                h.document().composed_source(),
                h.document().composed(),
            )
        });
        let Some((cur_gen, composed_source, composed)) = resolved else {
            world.resource::<TwinRoots>().clear_overlay(&name, &rel);
            world.resource_mut::<DocBackedTwinScenes>().map.remove(&doc);
            continue;
        };
        if Some(cur_gen) == synced {
            continue;
        }

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

        // Structural changes still re-read the whole asset through the overlay.
        let needs_structural = batch.as_ref().map(|b| b.needs_structural).unwrap_or(true);
        if needs_structural {
            world.resource::<AssetServer>().reload(twin_path);
        }

        if let Some(s) = world.resource_mut::<DocBackedTwinScenes>().map.get_mut(&doc) {
            s.synced_generation = Some(cur_gen);
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
