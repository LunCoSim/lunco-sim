//! Raw USDA source assets loaded through Bevy's asset resolver.
//!
//! This is deliberately separate from composed [`UsdStageAsset`] loading:
//! document authoring needs the bytes of one layer, while runtime projection
//! needs the composed stage and its prepared read surface. Keeping the raw
//! source asset in the headless USD substrate lets both paths use the same
//! `twin://` and storage-aware resolver without depending on visual projection.

use bevy::asset::{io::Reader, AssetLoader, LoadContext};
use bevy::prelude::{Asset, TypePath};

/// A USD layer's raw source text, without composition.
#[derive(Asset, TypePath, Clone)]
pub struct UsdSourceText(pub String);

/// Asset loader for raw USDA source text.
#[derive(Default, TypePath)]
pub struct UsdSourceTextLoader;

impl AssetLoader for UsdSourceTextLoader {
    type Asset = UsdSourceText;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(UsdSourceText(String::from_utf8(bytes)?))
    }

    fn extensions(&self) -> &[&str] {
        &["usda"]
    }
}
