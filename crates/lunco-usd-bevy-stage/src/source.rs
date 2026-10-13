//! Raw USD layer assets loaded through Bevy's asset resolver.
//!
//! This is deliberately separate from composed [`UsdStageAsset`] loading:
//! document authoring needs the bytes of one layer, while runtime projection
//! needs the composed stage and its prepared read surface. Keeping the raw
//! source asset in the headless USD substrate lets both paths use the same
//! `twin://` and storage-aware resolver without depending on visual projection.

use bevy::asset::{AssetLoader, LoadContext, io::Reader};
use bevy::prelude::{Asset, TypePath};

/// A USD layer's exact source bytes, without composition.
#[derive(Asset, TypePath, Clone)]
pub struct UsdSourceBytes(pub Vec<u8>);

/// Asset loader for raw USD source bytes.
#[derive(Default, TypePath)]
pub struct UsdSourceBytesLoader;

impl AssetLoader for UsdSourceBytesLoader {
    type Asset = UsdSourceBytes;
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
        Ok(UsdSourceBytes(bytes))
    }

    fn extensions(&self) -> &[&str] {
        &["usd", "usda", "usdc"]
    }
}
