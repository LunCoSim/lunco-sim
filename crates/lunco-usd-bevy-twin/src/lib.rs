//! Render-free Twin-backed USD projection state for Bevy hosts.
//!
//! This package owns the identity and lifetime seam between a document in the
//! document registry and a `twin://` USD stage asset. It does not load stages,
//! compose layers, or project entities. Those runtime mechanisms remain in
//! `lunco-usd`; UI packages consume this contract without importing that
//! aggregate runtime crate.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bevy::asset::{AssetId, AssetServer};
use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_usd_bevy_core::UsdStageAsset;

/// A USD document transitioned from a Twin-only scene lease to a user-facing
/// session lease.
#[derive(Event, Clone, Copy, Debug)]
pub struct UsdDocumentUserOwned {
    /// Document whose user-session lease was created.
    pub doc: DocumentId,
}

/// Marks a live prim entity whose content is refreshed in place by its domain
/// owner, so Twin projection must not structurally reload that subtree for an
/// attribute-only document edit.
#[derive(Component)]
pub struct LiveRebuildExempt;

/// One `twin://` projection identity and its ownership/cursor state.
struct TwinSceneRef {
    roots: Vec<PathBuf>,
    name: String,
    rel: String,
    preview_leases: usize,
    applied_generation: Option<u64>,
    synced_generation: Option<u64>,
    stage_id: Option<AssetId<UsdStageAsset>>,
    overlay_synced_generation: Option<u64>,
}

/// Maps a document to the Twin scene that backs it.
///
/// The resource carries workspace Twin leases, editor preview leases, and the
/// cursors used by the live projection owner. It intentionally contains no UI
/// viewport type: a preview is represented only by a counted lease.
#[derive(Resource, Default)]
pub struct DocBackedTwinScenes {
    map: HashMap<DocumentId, TwinSceneRef>,
    user_owned: HashSet<DocumentId>,
}

impl DocBackedTwinScenes {
    /// Return the document backing `twin://<name>/<rel>`, if any.
    pub fn doc_for(&self, name: &str, rel: &str) -> Option<DocumentId> {
        self.map
            .iter()
            .find(|(_, scene)| scene.name == name && scene.rel == rel)
            .map(|(doc, _)| *doc)
    }

    /// Return the `twin://` coordinates currently assigned to `doc`.
    pub fn coords_of(&self, doc: DocumentId) -> Option<(String, String)> {
        self.map
            .get(&doc)
            .map(|scene| (scene.name.clone(), scene.rel.clone()))
    }

    /// Return the last document generation whose stage changes were consumed.
    pub fn synced_generation(&self, doc: DocumentId) -> Option<u64> {
        self.map.get(&doc).and_then(|scene| scene.synced_generation)
    }

    /// Return the generation last serialized into the Twin overlay.
    pub fn overlay_synced_generation(&self, doc: DocumentId) -> Option<u64> {
        self.map
            .get(&doc)
            .and_then(|scene| scene.overlay_synced_generation)
    }

    /// Snapshot all tracked projection entries for the runtime owner.
    pub fn entries(
        &self,
    ) -> impl Iterator<Item = (DocumentId, String, String, Option<u64>, Option<u64>)> + '_ {
        self.map.iter().map(|(doc, scene)| {
            (
                *doc,
                scene.name.clone(),
                scene.rel.clone(),
                scene.applied_generation,
                scene.overlay_synced_generation,
            )
        })
    }

    /// Mark the canonical-stage sink for `stage_id` as consumed.
    pub fn mark_stage_projected(&mut self, stage_id: AssetId<UsdStageAsset>) {
        if let Some(scene) = self
            .map
            .values_mut()
            .find(|scene| scene.stage_id == Some(stage_id))
        {
            scene.synced_generation = scene.applied_generation;
        }
    }

    /// Record that the initial composed source already represents `generation`.
    pub fn mark_initial_projection(&mut self, doc: DocumentId, generation: u64) {
        if let Some(scene) = self.map.get_mut(&doc) {
            scene.applied_generation = Some(generation);
            scene.synced_generation = Some(generation);
            scene.overlay_synced_generation = Some(generation);
        }
    }

    /// Record a generation applied to a live canonical stage.
    pub fn mark_applied(
        &mut self,
        doc: DocumentId,
        stage_id: AssetId<UsdStageAsset>,
        generation: u64,
    ) {
        if let Some(scene) = self.map.get_mut(&doc) {
            scene.applied_generation = Some(generation);
            scene.stage_id = Some(stage_id);
        }
    }

    /// Record that `generation` has been serialized into the Twin overlay.
    pub fn mark_overlay_synced(&mut self, doc: DocumentId, generation: u64) {
        if let Some(scene) = self.map.get_mut(&doc) {
            scene.overlay_synced_generation = Some(generation);
        }
    }

    /// Claim a document for the user-facing document session.
    pub fn claim_user(&mut self, doc: DocumentId) -> bool {
        self.user_owned.insert(doc)
    }

    /// Whether the document has a user-facing lease.
    pub fn is_user_owned(&self, doc: DocumentId) -> bool {
        self.user_owned.contains(&doc)
    }

    /// Whether an editor preview currently keeps this document projected.
    pub fn has_preview_lease(&self, doc: DocumentId) -> bool {
        self.map
            .get(&doc)
            .is_some_and(|scene| scene.preview_leases != 0)
    }

    /// Track a document under a workspace Twin root.
    pub fn track(&mut self, doc: DocumentId, root: PathBuf, name: String, rel: String) {
        if let Some(scene) = self.map.get_mut(&doc) {
            if !scene
                .roots
                .iter()
                .any(|existing| lunco_doc::same_file(existing, &root))
            {
                scene.roots.push(root);
            }
            return;
        }
        self.map.insert(
            doc,
            TwinSceneRef {
                roots: vec![root],
                name,
                rel,
                preview_leases: 0,
                applied_generation: None,
                synced_generation: None,
                stage_id: None,
                overlay_synced_generation: None,
            },
        );
    }

    /// Track an editor preview without assigning a workspace Twin root.
    pub fn track_preview(&mut self, doc: DocumentId, name: String, rel: String) {
        if self.map.contains_key(&doc) {
            return;
        }
        self.map.insert(
            doc,
            TwinSceneRef {
                roots: Vec::new(),
                name,
                rel,
                preview_leases: 0,
                applied_generation: None,
                synced_generation: None,
                stage_id: None,
                overlay_synced_generation: None,
            },
        );
    }

    /// Acquire one editor preview lease for a tracked document.
    pub fn acquire_preview(&mut self, doc: DocumentId) {
        if let Some(scene) = self.map.get_mut(&doc) {
            scene.preview_leases = scene.preview_leases.saturating_add(1);
        }
    }

    /// Release one editor preview lease and return synthetic coordinates when
    /// the final preview was the document's last owner.
    pub fn release_preview(&mut self, doc: DocumentId) -> Option<(String, String)> {
        let scene = self.map.get_mut(&doc)?;
        if scene.preview_leases == 0 {
            return None;
        }
        scene.preview_leases -= 1;
        if scene.preview_leases == 0 && scene.roots.is_empty() {
            let scene = self.map.remove(&doc)?;
            return Some((scene.name, scene.rel));
        }
        None
    }

    /// Release a workspace Twin root and return documents with no remaining
    /// owner and no user-facing lease.
    pub fn release_root(&mut self, root: &Path) -> Vec<DocumentId> {
        let mut released = Vec::new();
        self.map.retain(|doc, scene| {
            scene
                .roots
                .retain(|existing| !lunco_doc::same_file(existing, root));
            if scene.roots.is_empty() && scene.preview_leases == 0 {
                released.push(*doc);
                false
            } else {
                true
            }
        });
        released
            .into_iter()
            .filter(|doc| !self.user_owned.contains(doc))
            .collect()
    }

    /// Forget a document after its registry host has been removed.
    pub fn forget_document(&mut self, doc: DocumentId) -> Option<(String, String)> {
        let synthetic = self
            .map
            .remove(&doc)
            .and_then(|scene| scene.roots.is_empty().then_some((scene.name, scene.rel)));
        self.user_owned.remove(&doc);
        synthetic
    }

    /// Drop current workspace projection coordinates while retaining preview
    /// leases so the preview can be rehomed under the same identity.
    pub fn detach_projection(&mut self, doc: DocumentId) {
        if let Some(scene) = self.map.get_mut(&doc) {
            if scene.preview_leases > 0 {
                scene.roots.clear();
                return;
            }
        }
        self.map.remove(&doc);
    }
}

/// Resolve the editable USD document backing a `UsdStageAsset` loaded through
/// `twin://<name>/<rel>`.
pub fn scene_document_for(
    backed: &DocBackedTwinScenes,
    asset_server: &AssetServer,
    scene: AssetId<UsdStageAsset>,
) -> Option<DocumentId> {
    let asset_path = asset_server.get_path(scene)?;
    let rel_path = asset_path.path().to_string_lossy();
    let (name, rel) = lunco_assets::split_twin_rel(&rel_path)?;
    backed.doc_for(name, rel)
}

/// Event-driven invalidation state for the live Twin projection owner.
#[derive(Resource, Default)]
pub struct TwinProjectionWake {
    pending: bool,
}

impl TwinProjectionWake {
    /// Mark projection work as pending.
    pub fn wake(&mut self) {
        self.pending = true;
    }

    /// Consume the pending invalidation.
    pub fn consume(&mut self) {
        self.pending = false;
    }

    /// Whether projection work is pending.
    pub fn is_pending(&self) -> bool {
        self.pending
    }
}

/// Wake the document-backed projection after a presentation mount installs a
/// new preview root.
pub fn wake_twin_projection(world: &mut World) {
    world.resource_mut::<TwinProjectionWake>().wake();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twin_scene_lease_closes_only_unclaimed_documents() {
        let root = PathBuf::from("/twins/moonbase");
        let scene_only = DocumentId::new(1);
        let user_owned = DocumentId::new(2);
        let mut backed = DocBackedTwinScenes::default();
        backed.track(
            scene_only,
            root.clone(),
            "moonbase".into(),
            "scene.usda".into(),
        );
        backed.track(
            user_owned,
            root.clone(),
            "moonbase".into(),
            "edited.usda".into(),
        );
        assert!(backed.claim_user(user_owned));

        assert_eq!(backed.release_root(&root), vec![scene_only]);
        assert!(backed.coords_of(scene_only).is_none());
        assert!(backed.coords_of(user_owned).is_none());
    }

    #[test]
    fn closing_one_of_multiple_twin_leases_keeps_the_document_backed() {
        let root_a = PathBuf::from("/twins/a");
        let root_b = PathBuf::from("/twins/b");
        let doc = DocumentId::new(1);
        let mut backed = DocBackedTwinScenes::default();
        backed.track(doc, root_a.clone(), "shared".into(), "scene.usda".into());
        backed.track(doc, root_b.clone(), "shared".into(), "scene.usda".into());

        assert!(backed.release_root(&root_a).is_empty());
        assert!(backed.coords_of(doc).is_some());
        assert_eq!(backed.release_root(&root_b), vec![doc]);
        assert!(backed.coords_of(doc).is_none());
    }

    #[test]
    fn closing_one_of_multiple_preview_leases_keeps_the_authority() {
        let doc = DocumentId::new(1);
        let mut backed = DocBackedTwinScenes::default();
        backed.track_preview(doc, "assembly".into(), "scene.usda".into());
        backed.acquire_preview(doc);
        backed.acquire_preview(doc);

        assert!(backed.release_preview(doc).is_none());
        assert!(backed.coords_of(doc).is_some());
        assert_eq!(
            backed.release_preview(doc),
            Some(("assembly".into(), "scene.usda".into()))
        );
        assert!(backed.coords_of(doc).is_none());
    }
}
