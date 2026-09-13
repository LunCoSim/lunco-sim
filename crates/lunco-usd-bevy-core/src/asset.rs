//! Bevy asset boundary for resolver-backed composed USD stages.
//!
//! `UsdStageAsset` carries only the send-safe layer recipe and projection plan.
//! The live OpenUSD stage is intentionally owned by [`crate::canonical`], so
//! this asset can cross Bevy's asynchronous loading boundary without retaining
//! `Rc`-backed OpenUSD handles.

use std::sync::Arc;

use anyhow::Result;
use bevy::asset::{io::Reader, AssetLoader, LoadContext};
use bevy::prelude::{Asset, TypePath};

use crate::compose::fetch_layer_closure;
use crate::UsdStageProjectionPlan;

/// A Bevy asset representing a loaded, composed USD stage.
///
/// The prepared plan is used for initial projection. When authoring or
/// incremental projection needs a live stage, [`crate::canonical`] opens the
/// same recipe on the main thread.
#[derive(Asset, TypePath, Clone)]
pub struct UsdStageAsset {
    /// The send-safe layer closure shared by initial projection and the live
    /// canonical stage. It is absent for an externally composed stage.
    pub recipe: Option<lunco_usd_core::StageRecipe>,
    /// Structural hierarchy and default-time facts captured from the composed
    /// stage before the asset crosses the async boundary.
    pub projection_plan: Arc<UsdStageProjectionPlan>,
}

impl UsdStageAsset {
    /// Build an asset with the same prepared projection contract as the async
    /// loader.
    pub fn from_recipe(recipe: lunco_usd_core::StageRecipe) -> Result<Self> {
        let projection_plan = UsdStageProjectionPlan::from_recipe(&recipe)?;
        projection_plan.validate()?;
        Ok(Self {
            recipe: Some(recipe),
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
            projection_plan: Arc::new(projection_plan),
        })
    }
}

/// Bevy loader that fetches and composes the complete transitive USD layer
/// closure before publishing a [`UsdStageAsset`].
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

        // Preserve a named Bevy asset source when anchoring relative USD arcs.
        // `LoadContext::path()` does not include the source scheme by itself.
        let path = load_context.path();
        let root_asset_path = match path.source() {
            bevy::asset::io::AssetSourceId::Name(name) => {
                format!("{}://{}", name, path.path().to_string_lossy())
            }
            bevy::asset::io::AssetSourceId::Default => path.path().to_string_lossy().into_owned(),
        };

        let recipe = fetch_layer_closure(load_context, &root_asset_path, bytes).await?;
        UsdStageAsset::from_recipe(recipe)
    }

    fn extensions(&self) -> &[&str] {
        &["usda"]
    }
}
