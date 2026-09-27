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

/// Resolve a USD asset path relative to the stage that authored it.
///
/// The canonical asset-path rule is shared with USD layer composition, so a
/// texture, scenario, or layer reference spelled the same way resolves the
/// same way. An in-memory stage has no asset-server anchor and therefore uses
/// the root canonicalization rule.
pub fn resolve_stage_asset_path(
    asset_server: &AssetServer,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    asset_path: &str,
) -> String {
    use lunco_assets_core::asset_path::anchor_of;
    use lunco_assets_path::{canonicalize, canonicalize_root};

    match asset_server.get_path(stage_id) {
        Some(stage_path) => canonicalize(asset_path, &anchor_of(&stage_path)),
        None => canonicalize_root(asset_path),
    }
}

/// Bevy loader that fetches and composes the available transitive USD layer
/// closure before publishing a [`UsdStageAsset`]. Missing transitive layers
/// remain unresolved USD arcs and are carried as runtime diagnostics.
#[derive(Default, TypePath)]
pub struct UsdLoader;

impl AssetLoader for UsdLoader {
    type Asset = UsdStageAsset;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        // Preserve the named Bevy asset source when anchoring relative USD
        // arcs. The asset-path module owns both scheme reconstruction and
        // platform separator normalization.
        let root_asset_path = anchor_of(load_context.path());

        let fetched = fetch_layer_closure(load_context, &root_asset_path, bytes).await?;
        let FetchedStageClosure {
            recipe,
            source_dependencies,
        } = fetched;
        #[cfg(not(target_arch = "wasm32"))]
        let (recipe, projection_plan) = {
            AsyncComputeTaskPool::get()
                .spawn(async move {
                    let projection_plan = UsdStageProjectionPlan::from_recipe(&recipe)?;
                    Ok::<_, anyhow::Error>((recipe, projection_plan))
                })
                .await?
        };
        #[cfg(target_arch = "wasm32")]
        let (recipe, projection_plan) = {
            let projection_plan = UsdStageProjectionPlan::from_recipe(&recipe)?;
            (recipe, projection_plan)
        };
        UsdStageAsset::from_prepared_recipe(recipe, source_dependencies, projection_plan)
    }

    fn extensions(&self) -> &[&str] {
        &["usda"]
    }
}
