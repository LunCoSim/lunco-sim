//! Render-free Twin-backed USD projection state for Bevy hosts.
//!
//! This package owns the identity and lifetime seam between a document in the
//! document registry and a `twin://` USD stage asset. It publishes immutable native
//! layer inputs and retains their asset handles for the projection lease.
//! Decoding and composition belong to `lunco-usd-bevy-stage`; entity projection
//! belongs to `lunco-usd-bevy-runtime-core`. UI packages consume this contract
//! without importing runtime visual adapters.

use lunco_workspace::TwinId;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::asset::{AssetId, AssetServer};
use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_usd_bevy_stage::{
    UsdStageAsset,
    canonical::{CanonicalStage, CanonicalStages},
};

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
    owners: Vec<TwinId>,
    name: String,
    rel: String,
    preview_leases: usize,
    applied_generation: Option<u64>,
    /// Last generation whose disposable view-layer ops reached the stage.
    /// The initial scene asset contains base and runtime layers, never view.
    applied_view_generation: Option<u64>,
    synced_generation: Option<u64>,
    stage_id: Option<AssetId<UsdStageAsset>>,
    source_stage: Option<Handle<UsdStageAsset>>,
    source_input: Option<lunco_usd_bevy_stage::asset::DocumentStageInputOwner>,
    overlay_synced_generation: Option<u64>,
}

/// Maps a document to the Twin scene that backs it.
///
/// The resource carries workspace Twin leases, editor preview leases, and the
/// cursors used by the live projection owner. It intentionally contains no UI
/// viewport type: a preview is represented only by a counted lease.
#[derive(Resource)]
pub struct DocBackedTwinScenes {
    map: HashMap<DocumentId, TwinSceneRef>,
    user_owned: HashSet<DocumentId>,
    private_mounts: HashMap<DocumentId, String>,
    inputs: lunco_usd_bevy_stage::asset::DocumentStageInputs,
}

impl FromWorld for DocBackedTwinScenes {
    fn from_world(world: &mut World) -> Self {
        Self::new(
            world
                .get_resource_or_init::<lunco_usd_bevy_stage::asset::DocumentStageInputs>()
                .clone(),
        )
    }
}

impl DocBackedTwinScenes {
    pub fn new(inputs: lunco_usd_bevy_stage::asset::DocumentStageInputs) -> Self {
        Self {
            map: HashMap::new(),
            user_owned: HashSet::new(),
            private_mounts: HashMap::new(),
            inputs,
        }
    }
    /// Publish one native source snapshot under its admitted document lease.
    /// Layer decoding and composition remain owned by the stage asset loader.
    pub fn publish_stage_input(
        &mut self,
        doc: DocumentId,
        roots: &lunco_assets_core::TwinRoots,
        server: &AssetServer,
        input: lunco_usd_document::document::DocumentStageInput,
    ) -> Result<(), String> {
        let (name, rel) = self
            .coords_of(doc)
            .ok_or_else(|| format!("document {doc} has no projection lease"))?;
        let root_id = lunco_assets_core::twin_uri(&name, &rel);
        if input.recipe.root_id != root_id {
            return Err(format!(
                "document input root {} does not match {root_id}",
                input.recipe.root_id
            ));
        }
        if let Some(owner) = self.source_input(doc) {
            roots
                .set_overlay(&name, &rel, input.root_bytes)
                .map_err(|error| error.to_string())?;
            return owner
                .publish(input.recipe)
                .map_err(|error| error.to_string());
        }
        let path = lunco_assets_core::asset_path::load_asset_path(&root_id, None, None, None)
            .map_err(|error| error.to_string())?;
        let already_requested = server.get_handle::<UsdStageAsset>(path.clone()).is_some();
        let owner = self
            .inputs
            .register(input.recipe)
            .map_err(|error| error.to_string())?;
        roots
            .set_overlay(&name, &rel, input.root_bytes)
            .map_err(|error| error.to_string())?;
        let handle = server.load::<UsdStageAsset>(path.clone());
        if already_requested {
            server.reload(path);
        }
        self.retain_source_stage(doc, handle, owner)
    }
    /// Retain the stage admitted with this document's explicit native inputs.
    /// Scene roots and source reloads share the same typed asset handle.
    fn retain_source_stage(
        &mut self,
        doc: DocumentId,
        handle: Handle<UsdStageAsset>,
        input: lunco_usd_bevy_stage::asset::DocumentStageInputOwner,
    ) -> Result<(), String> {
        let scene = self
            .map
            .get_mut(&doc)
            .ok_or_else(|| format!("document {doc} has no projection lease"))?;
        scene.stage_id = Some(handle.id());
        scene.source_stage = Some(handle);
        scene.source_input = Some(input);
        Ok(())
    }

    /// Current native input owner for this document projection lease.
    pub fn source_input(
        &self,
        doc: DocumentId,
    ) -> Option<&lunco_usd_bevy_stage::asset::DocumentStageInputOwner> {
        self.map.get(&doc)?.source_input.as_ref()
    }
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

    /// Return the generation applied to the canonical stage for this stage id.
    pub fn applied_stage_generation(
        &self,
        doc: DocumentId,
        stage: AssetId<UsdStageAsset>,
    ) -> Option<u64> {
        self.map.get(&doc).and_then(|scene| {
            (scene.stage_id == Some(stage))
                .then_some(scene.applied_generation)
                .flatten()
        })
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
    ) -> impl Iterator<
        Item = (
            DocumentId,
            String,
            String,
            Option<u64>,
            Option<u64>,
            Option<u64>,
        ),
    > + '_ {
        self.map.iter().map(|(doc, scene)| {
            (
                *doc,
                scene.name.clone(),
                scene.rel.clone(),
                scene.applied_generation,
                scene.applied_view_generation,
                scene.overlay_synced_generation,
            )
        })
    }

    /// Mark the canonical-stage sink for `stage_id` as consumed and return the
    /// document generation that is now visible through the live stage.
    pub fn mark_stage_projected(
        &mut self,
        stage_id: AssetId<UsdStageAsset>,
    ) -> Option<(DocumentId, u64)> {
        if let Some((doc, scene)) = self
            .map
            .iter_mut()
            .find(|(_, scene)| scene.stage_id == Some(stage_id))
        {
            scene.synced_generation = scene.applied_generation;
            return scene
                .applied_generation
                .map(|generation| (*doc, generation));
        }
        None
    }

    /// Close the projection cursor for a document after the runtime projector
    /// has reconciled the stage selected by the document projector.
    ///
    /// The document identity is the durable handoff key.  The stage id is
    /// carried as an integrity check, but the cursor is not rediscovered from
    /// an ECS entity or an asset-path lookup at this boundary.
    pub fn mark_document_projected(
        &mut self,
        doc: DocumentId,
        stage_id: AssetId<UsdStageAsset>,
        generation: u64,
    ) -> Option<(DocumentId, u64)> {
        let scene = self.map.get_mut(&doc)?;
        if scene.stage_id != Some(stage_id) || scene.applied_generation != Some(generation) {
            return None;
        }
        scene.synced_generation = Some(generation);
        Some((doc, generation))
    }

    /// Record that the initial composed source already represents `generation`.
    pub fn mark_initial_projection(&mut self, doc: DocumentId, generation: u64) {
        if let Some(scene) = self.map.get_mut(&doc) {
            scene.applied_generation = Some(generation);
            scene.synced_generation = Some(generation);
            scene.applied_view_generation = None;
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

    /// Record that the disposable view-layer operations through `generation`
    /// reached the same canonical stage as the persistent document projection.
    pub fn mark_view_applied(
        &mut self,
        doc: DocumentId,
        stage_id: AssetId<UsdStageAsset>,
        generation: u64,
    ) {
        if let Some(scene) = self.map.get_mut(&doc) {
            if scene.stage_id == Some(stage_id) {
                scene.applied_view_generation = Some(generation);
            }
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

    /// Track a document under its admitted local workspace Twin lifetime.
    pub fn track(&mut self, doc: DocumentId, twin: TwinId, name: String, rel: String) {
        if let Some(scene) = self.map.get_mut(&doc) {
            if !scene.owners.iter().any(|existing| *existing == twin) {
                scene.owners.push(twin);
            }
            return;
        }
        self.map.insert(
            doc,
            TwinSceneRef {
                owners: vec![twin],
                name,
                rel,
                preview_leases: 0,
                applied_generation: None,
                applied_view_generation: None,
                synced_generation: None,
                stage_id: None,
                source_stage: None,
                source_input: None,
                overlay_synced_generation: None,
            },
        );
    }

    /// Track an editor preview without assigning a workspace Twin lifetime.
    pub fn track_preview(&mut self, doc: DocumentId, name: String, rel: String) {
        if let Some(scene) = self.map.get_mut(&doc) {
            if scene.name != name || scene.rel != rel {
                scene.name = name;
                scene.rel = rel;
                scene.source_input = None;
                scene.source_stage = None;
                scene.stage_id = None;
                scene.applied_generation = None;
                scene.applied_view_generation = None;
                scene.synced_generation = None;
                scene.overlay_synced_generation = None;
            }
            return;
        }
        self.map.insert(
            doc,
            TwinSceneRef {
                owners: Vec::new(),
                name,
                rel,
                preview_leases: 0,
                applied_generation: None,
                applied_view_generation: None,
                synced_generation: None,
                stage_id: None,
                source_stage: None,
                source_input: None,
                overlay_synced_generation: None,
            },
        );
    }

    /// Record an asset authority created exclusively for this document.
    /// Workspace ownership and preview leases do not transfer this authority.
    pub fn track_private_mount(&mut self, doc: DocumentId, name: String) {
        self.private_mounts.insert(doc, name);
    }

    /// Acquire one editor preview lease for a tracked document.
    pub fn acquire_preview(&mut self, doc: DocumentId) {
        if let Some(scene) = self.map.get_mut(&doc) {
            scene.preview_leases = scene.preview_leases.saturating_add(1);
        }
    }

    /// Release one editor preview lease and return its private authority when
    /// the final preview was the document's last owner.
    pub fn release_preview(&mut self, doc: DocumentId) -> Option<String> {
        let scene = self.map.get_mut(&doc)?;
        if scene.preview_leases == 0 {
            return None;
        }
        scene.preview_leases -= 1;
        if scene.preview_leases == 0 && scene.owners.is_empty() {
            self.map.remove(&doc);
            return self.private_mounts.remove(&doc);
        }
        None
    }

    /// Release an exact workspace Twin lifetime and return documents with no remaining
    /// owner and no user-facing lease.
    pub fn release_twin(&mut self, twin: TwinId) -> Vec<DocumentId> {
        let mut released = Vec::new();
        self.map.retain(|doc, scene| {
            scene.owners.retain(|existing| *existing != twin);
            if scene.owners.is_empty() && scene.preview_leases == 0 {
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
    pub fn forget_document(&mut self, doc: DocumentId) -> Option<String> {
        self.map.remove(&doc);
        self.user_owned.remove(&doc);
        self.private_mounts.remove(&doc)
    }

    /// Drop current workspace projection coordinates while retaining preview
    /// leases so the preview can be rehomed under the same identity.
    pub fn detach_projection(&mut self, doc: DocumentId) {
        if let Some(scene) = self.map.get_mut(&doc) {
            if scene.preview_leases > 0 {
                scene.owners.clear();
                scene.source_input = None;
                scene.source_stage = None;
                scene.stage_id = None;
                scene.applied_generation = None;
                scene.applied_view_generation = None;
                scene.synced_generation = None;
                scene.overlay_synced_generation = None;
                return;
            }
        }
        self.map.remove(&doc);
    }
}

/// Resolve the document holding this exact projected stage lease.
/// Names and editor focus do not grant ownership of a retired stage.
pub fn scene_document_for(
    backed: &DocBackedTwinScenes,
    scene: AssetId<UsdStageAsset>,
) -> Option<DocumentId> {
    let mut owners = backed
        .map
        .iter()
        .filter_map(|(doc, source)| (source.stage_id == Some(scene)).then_some(*doc));
    let owner = owners.next()?;
    if owners.next().is_some() {
        error!(?scene, "projected USD stage has multiple document owners");
        return None;
    }
    Some(owner)
}

/// Read the stage identity committed by the document projection owner.
pub fn stage_asset_for_document(
    backed: &DocBackedTwinScenes,
    doc: DocumentId,
) -> Option<AssetId<UsdStageAsset>> {
    backed.map.get(&doc)?.stage_id
}

/// Resolve a document-backed Twin scene to its already-mounted canonical USD
/// stage.
///
/// The Twin map owns the document-to-`twin://` identity, while the USD Bevy
/// core owns canonical stage storage. This function only joins those existing
/// owners; it does not load, compose, or project a stage.
pub fn canonical_stage_for_document(world: &World, doc: DocumentId) -> Option<&CanonicalStage> {
    let stage_id = stage_asset_for_document(world.get_resource::<DocBackedTwinScenes>()?, doc)?;
    world.get_non_send::<CanonicalStages>()?.get(stage_id)
}

/// Event-driven invalidation state for the live Twin projection owner.
#[derive(Resource, Clone, Default)]
pub struct TwinProjectionWake {
    pending: Arc<AtomicBool>,
}

impl TwinProjectionWake {
    /// Mark projection work as pending.
    pub fn wake(&self) {
        self.pending.store(true, Ordering::Release);
    }

    /// Consume the pending invalidation.
    pub fn consume(&self) {
        self.pending.swap(false, Ordering::AcqRel);
    }

    /// Whether projection work is pending.
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }
}

/// Wake the document-backed projection after a presentation mount installs a
/// new preview root.
pub fn wake_twin_projection(world: &mut World) {
    world.resource_mut::<TwinProjectionWake>().wake();
}

#[cfg(test)]
mod tests {
    use bevy::asset::AssetIndex;

    use super::*;

    #[test]
    fn twin_scene_lease_closes_only_unclaimed_documents() {
        let twin = TwinId::new(1);
        let scene_only = DocumentId::new(1);
        let user_owned = DocumentId::new(2);
        let mut backed = DocBackedTwinScenes::new(Default::default());
        backed.track(scene_only, twin, "moonbase".into(), "scene.usda".into());
        backed.track(user_owned, twin, "moonbase".into(), "edited.usda".into());
        assert!(backed.claim_user(user_owned));

        assert_eq!(backed.release_twin(twin), vec![scene_only]);
        assert!(backed.coords_of(scene_only).is_none());
        assert!(backed.coords_of(user_owned).is_none());
    }

    #[test]
    fn closing_one_of_multiple_twin_leases_keeps_the_document_backed() {
        let twin_a = TwinId::new(1);
        let twin_b = TwinId::new(2);
        let doc = DocumentId::new(1);
        let mut backed = DocBackedTwinScenes::new(Default::default());
        backed.track(doc, twin_a, "shared".into(), "scene.usda".into());
        backed.track(doc, twin_b, "shared".into(), "scene.usda".into());

        assert!(backed.release_twin(twin_a).is_empty());
        assert!(backed.coords_of(doc).is_some());
        assert_eq!(backed.release_twin(twin_b), vec![doc]);
        assert!(backed.coords_of(doc).is_none());
    }

    #[test]
    fn closing_one_of_multiple_preview_leases_keeps_the_authority() {
        let doc = DocumentId::new(1);
        let mut backed = DocBackedTwinScenes::new(Default::default());
        backed.track_preview(doc, "assembly".into(), "scene.usda".into());
        backed.track_private_mount(doc, "assembly".into());
        backed.acquire_preview(doc);
        backed.acquire_preview(doc);

        assert!(backed.release_preview(doc).is_none());
        assert!(backed.coords_of(doc).is_some());
        assert_eq!(backed.release_preview(doc), Some("assembly".into()));
        assert!(backed.coords_of(doc).is_none());
    }

    #[test]
    fn private_mount_retirement_is_independent_of_projection_leases() {
        let doc = DocumentId::new(1);
        let twin = TwinId::new(1);
        let mut backed = DocBackedTwinScenes::new(Default::default());
        backed.track_private_mount(doc, "private".into());
        backed.track(doc, twin, "private".into(), "scene.usda".into());
        assert_eq!(backed.release_twin(twin), vec![doc]);
        assert_eq!(backed.forget_document(doc), Some("private".into()));
        assert!(backed.forget_document(doc).is_none());

        backed.track(doc, twin, "shared".into(), "scene.usda".into());
        backed.acquire_preview(doc);
        backed.release_twin(twin);
        assert!(backed.release_preview(doc).is_none());
        assert!(backed.forget_document(doc).is_none());

        backed.track_private_mount(doc, "private-again".into());
        backed.track(doc, twin, "private-again".into(), "scene.usda".into());
        assert_eq!(backed.forget_document(doc), Some("private-again".into()));
    }

    #[test]
    fn document_resolution_requires_the_exact_current_stage_lease() {
        let first = DocumentId::new(1);
        let replacement = DocumentId::new(2);
        let twin = TwinId::new(1);
        let old_stage: AssetId<UsdStageAsset> = AssetIndex::from_bits(11).into();
        let new_stage: AssetId<UsdStageAsset> = AssetIndex::from_bits(12).into();
        let mut backed = DocBackedTwinScenes::new(Default::default());
        backed.track(first, twin, "shared-name".into(), "scene.usda".into());
        backed.mark_applied(first, old_stage, 0);
        assert_eq!(scene_document_for(&backed, old_stage), Some(first));
        backed.release_twin(twin);
        backed.track(replacement, twin, "shared-name".into(), "scene.usda".into());
        assert_eq!(scene_document_for(&backed, old_stage), None);
        backed.mark_applied(replacement, new_stage, 0);
        assert_eq!(scene_document_for(&backed, old_stage), None);
        assert_eq!(scene_document_for(&backed, new_stage), Some(replacement));
    }

    #[test]
    fn standalone_preview_marks_typed_stage_generation_consumed() {
        let doc = DocumentId::new(7);
        let stage: AssetId<UsdStageAsset> = AssetIndex::from_bits(11).into();
        let mut backed = DocBackedTwinScenes::new(Default::default());
        backed.track_preview(doc, "assembly".into(), "component.usda".into());

        backed.mark_applied(doc, stage, 4);
        assert_eq!(backed.synced_generation(doc), None);

        // Preview stages do not pass through the live ECS sink.  The
        // projection owner therefore closes their cursor explicitly once the
        // canonical stage has been updated.
        backed.mark_stage_projected(stage);
        assert_eq!(backed.synced_generation(doc), Some(4));
    }
}
