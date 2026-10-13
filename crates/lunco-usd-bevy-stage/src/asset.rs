//! Bevy asset boundary for resolver-backed composed USD stages.
//!
//! `UsdStageAsset` carries the send-safe layer recipe, source-read receipts,
//! and projection plan. The live OpenUSD stage is intentionally owned by
//! [`crate::canonical`], so this asset can cross Bevy's asynchronous loading
//! boundary without retaining `Rc`-backed OpenUSD handles.

use std::sync::Arc;

use anyhow::Result;
use bevy::asset::{AssetLoader, AssetServer, Handle, LoadContext, io::Reader};
use bevy::prelude::{Asset, TypePath};
#[cfg(not(target_arch = "wasm32"))]
use bevy::tasks::AsyncComputeTaskPool;

use crate::UsdStageProjectionPlan;
use crate::compose::{FetchedStageClosure, fetch_layer_closure};
use lunco_assets_core::asset_path::anchor_of;

/// A Bevy asset representing a loaded, composed USD stage.
///
/// The prepared plan is used for initial projection. When authoring or
/// incremental projection needs a live stage, [`crate::canonical`] opens the
/// same recipe on the main thread.
#[derive(Asset, TypePath, Clone)]
pub struct UsdStageAsset {
    /// The send-safe layer closure shared by initial projection and the live
    /// canonical stage. It is absent for an externally composed stage.
    /// Cloned handles share the immutable closure instead of copying its layer
    /// bytes on the app thread.
    pub recipe: Option<Arc<lunco_usd_compose::recipe::StageRecipe>>,
    /// Keeps source-read receipt assets alive so Bevy can reload this stage
    /// when one of its transitive USD layers changes.
    #[dependency]
    source_dependencies: Vec<Handle<UsdLayerReadReceipt>>,
    /// Structural hierarchy and default-time facts captured from the composed
    /// stage before the asset crosses the async boundary.
    pub projection_plan: Arc<UsdStageProjectionPlan>,
}

impl UsdStageAsset {
    /// Build an asset with the same prepared projection contract as the async
    /// loader.
    pub fn from_recipe(recipe: lunco_usd_compose::recipe::StageRecipe) -> Result<Self> {
        let projection_plan = UsdStageProjectionPlan::from_recipe(&recipe)?;
        Self::from_prepared_recipe(recipe, Vec::new(), projection_plan)
    }

    pub(crate) fn from_prepared_recipe(
        recipe: lunco_usd_compose::recipe::StageRecipe,
        source_dependencies: Vec<Handle<UsdLayerReadReceipt>>,
        projection_plan: UsdStageProjectionPlan,
    ) -> Result<Self> {
        projection_plan.validate()?;
        Ok(Self {
            recipe: Some(Arc::new(recipe)),
            source_dependencies,
            projection_plan: Arc::new(projection_plan),
        })
    }

    /// Build an asset from an already-composed native stage.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_composed_stage(stage: &openusd::usd::Stage) -> Result<Self> {
        let projection_plan = UsdStageProjectionPlan::from_stage(stage)?;
        projection_plan.validate()?;
        Ok(Self {
            recipe: None,
            source_dependencies: Vec::new(),
            projection_plan: Arc::new(projection_plan),
        })
    }
}

/// Labeled receipt that keeps a transitive layer read in Bevy's hot-reload graph.
/// Apps registering [`UsdLoader`] directly must also initialize this asset type.
#[derive(Asset, TypePath)]
pub struct UsdLayerReadReceipt;

/// Resolve a USD asset reference into its typed Bevy load address. Native
/// references require the stage's exact live Twin mount; externally composed
/// stages without an asset-server origin cannot admit native asset loads.
pub fn resolve_stage_asset_path(
    asset_server: &AssetServer,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    asset_path: &str,
    roots: Option<&lunco_assets_core::TwinRoots>,
    prepared: Option<&lunco_assets_core::asset_path::PreparedAssetPaths>,
) -> Result<bevy::asset::AssetPath<'static>, lunco_assets_core::TwinRootsError> {
    let origin = asset_server.get_path(stage_id);
    lunco_assets_core::asset_path::load_asset_path(asset_path, origin.as_ref(), roots, prepared)
}

/// Bevy loader that fetches and composes the available transitive USD layer
/// closure before publishing a [`UsdStageAsset`]. Missing transitive layers
/// remain unresolved USD arcs and are carried as runtime diagnostics.
#[derive(TypePath)]
pub struct UsdLoader {
    roots: Option<lunco_assets_core::TwinRoots>,
}

/// Native stage input at Bevy's asset-metadata adapter boundary. Root bytes
/// remain owned by the asset reader; a captured session participates in the
/// same bounded layer closure and source-read receipt graph.
#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsdStageLoadSettings {
    pub session_layer: Option<String>,
    /// Live document input is runtime metadata, never a serialized asset setting.
    #[serde(skip)]
    pub document_input: Option<DocumentStageInputReader>,
}

/// The document lease owns publication and retirement of immutable native input.
pub struct DocumentStageInputOwner(
    Arc<std::sync::Mutex<Option<Arc<lunco_usd_compose::recipe::StageRecipe>>>>,
);

/// A loader may capture input but cannot publish or retain its document lease.
#[derive(Clone)]
pub struct DocumentStageInputReader(
    Arc<std::sync::Mutex<Option<Arc<lunco_usd_compose::recipe::StageRecipe>>>>,
);

impl DocumentStageInputOwner {
    pub fn new(recipe: lunco_usd_compose::recipe::StageRecipe) -> Self {
        Self(Arc::new(std::sync::Mutex::new(Some(Arc::new(recipe)))))
    }

    pub fn reader(&self) -> DocumentStageInputReader {
        DocumentStageInputReader(Arc::clone(&self.0))
    }

    pub fn publish(&self, recipe: lunco_usd_compose::recipe::StageRecipe) -> Result<()> {
        let mut current = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("document stage input lock poisoned"))?;
        let previous = current
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("document stage input lease retired"))?;
        anyhow::ensure!(
            previous.root_id == recipe.root_id,
            "document stage input cannot change its root identity"
        );
        *current = Some(Arc::new(recipe));
        Ok(())
    }
}

impl Drop for DocumentStageInputOwner {
    fn drop(&mut self) {
        if let Ok(mut current) = self.0.lock() {
            *current = None;
        }
    }
}

impl DocumentStageInputReader {
    pub fn capture(&self) -> Result<Arc<lunco_usd_compose::recipe::StageRecipe>> {
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("document stage input lock poisoned"))?
            .clone()
            .ok_or_else(|| anyhow::anyhow!("document stage input lease retired"))
    }

    fn validate_capture(
        &self,
        captured: &Arc<lunco_usd_compose::recipe::StageRecipe>,
    ) -> Result<()> {
        let current = self.capture()?;
        anyhow::ensure!(
            Arc::ptr_eq(&current, captured),
            "document stage input changed during preparation"
        );
        Ok(())
    }
}

impl bevy::prelude::FromWorld for UsdLoader {
    fn from_world(world: &mut bevy::prelude::World) -> Self {
        Self {
            roots: world
                .get_resource::<lunco_assets_core::TwinRoots>()
                .cloned(),
        }
    }
}

impl AssetLoader for UsdLoader {
    type Asset = UsdStageAsset;
    type Settings = UsdStageLoadSettings;
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        // Preserve the named Bevy asset source when anchoring relative USD
        // arcs. The asset-path module owns both scheme reconstruction and
        // platform separator normalization.
        let origin = load_context.path().clone().into_owned();
        let roots = self.roots.clone();
        let root_asset_path = anchor_of(load_context.path());
        let captured = settings
            .document_input
            .as_ref()
            .map(DocumentStageInputReader::capture)
            .transpose()?;
        let input = match &captured {
            Some(input) => {
                anyhow::ensure!(
                    input.root_id == root_asset_path,
                    "document input root does not match its asset source"
                );
                (**input).clone()
            }
            None => {
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes).await?;
                let mut input = lunco_usd_compose::recipe::StageRecipe::new(
                    root_asset_path.clone(),
                    std::collections::HashMap::from([(root_asset_path, bytes)]),
                );
                input.session_layer = settings.session_layer.clone();
                input
            }
        };
        let fetched = fetch_layer_closure(load_context, input, roots.as_ref()).await?;
        let FetchedStageClosure {
            recipe,
            source_dependencies,
        } = fetched;
        #[cfg(not(target_arch = "wasm32"))]
        let (recipe, projection_plan) = {
            AsyncComputeTaskPool::get()
                .spawn(async move {
                    let mut projection_plan = UsdStageProjectionPlan::from_recipe(&recipe)?;
                    projection_plan.prepare_native_asset_paths(Some(origin), roots.as_ref());
                    if let Some(paths) = projection_plan.native_asset_paths_snapshot() {
                        paths.validate_owner(roots.as_ref())?;
                    }
                    Ok::<_, anyhow::Error>((recipe, projection_plan))
                })
                .await?
        };
        #[cfg(target_arch = "wasm32")]
        let (recipe, projection_plan) = {
            let mut projection_plan = UsdStageProjectionPlan::from_recipe(&recipe)?;
            projection_plan.prepare_native_asset_paths(Some(origin), roots.as_ref());
            if let Some(paths) = projection_plan.native_asset_paths_snapshot() {
                paths.validate_owner(roots.as_ref())?;
            }
            (recipe, projection_plan)
        };
        if let (Some(owner), Some(captured)) = (&settings.document_input, &captured) {
            owner.validate_capture(captured)?;
        }
        UsdStageAsset::from_prepared_recipe(recipe, source_dependencies, projection_plan)
    }

    fn extensions(&self) -> &[&str] {
        &["usd", "usda", "usdc"]
    }
}

#[cfg(test)]
mod document_input_tests {
    use super::*;

    fn recipe(root: &str) -> lunco_usd_compose::recipe::StageRecipe {
        lunco_usd_compose::recipe::StageRecipe::from_source(root, "#usda 1.0\n")
    }

    #[test]
    fn layered_stage_input_fences_revisions_identity_and_retirement() {
        let owner = DocumentStageInputOwner::new(recipe("scene.usda"));
        let reader = owner.reader();
        let first = reader.capture().unwrap();
        reader.validate_capture(&first).unwrap();
        assert!(owner.publish(recipe("other.usda")).is_err());
        reader.validate_capture(&first).unwrap();
        owner.publish(recipe("scene.usda")).unwrap();
        assert!(reader.validate_capture(&first).is_err());
        let second = reader.capture().unwrap();
        reader.validate_capture(&second).unwrap();
        drop(owner);
        assert!(reader.capture().is_err());
        assert!(reader.validate_capture(&second).is_err());
    }
}
