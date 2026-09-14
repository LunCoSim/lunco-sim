//! Modelica source as a Bevy `Asset`.
//!
//! Domain code consuming `.mo` files must go through `AssetServer::load(...)`;
//! this keeps native and wasm source loading on one platform-portable path and
//! gives callers hot reload and `AssetEvent`s without coupling them to the
//! compiler worker.

use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::prelude::*;
use lunco_modelica_ast::normalize_modelica_source;

/// The text contents of a `.mo` file, surfaced as an asset.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct ModelicaSource {
    /// Raw `.mo` text. UTF-8 (the loader rejects non-UTF-8 inputs).
    pub text: String,
}

#[derive(Default, TypePath)]
pub struct ModelicaSourceLoader;

impl AssetLoader for ModelicaSourceLoader {
    type Asset = ModelicaSource;
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
        let text = normalize_modelica_source(std::str::from_utf8(&bytes)?).into_owned();
        Ok(ModelicaSource { text })
    }

    fn extensions(&self) -> &[&str] {
        &["mo"]
    }
}

/// Read UTF-8 source through the platform-portable storage backend.
pub fn read_text_sync(path: &std::path::Path) -> Result<String, String> {
    let bytes = lunco_storage::read_file_sync(path)
        .map_err(|e| format!("read failed `{}`: {e}", path.display()))?;
    let text =
        String::from_utf8(bytes).map_err(|e| format!("non-utf8 text `{}`: {e}", path.display()))?;
    Ok(normalize_modelica_source(&text).into_owned())
}

/// Write UTF-8 source through the platform-portable storage backend.
pub fn write_text_sync(path: &std::path::Path, text: &str) -> Result<(), String> {
    lunco_storage::write_file_sync(path, text.as_bytes())
        .map_err(|e| format!("write failed `{}`: {e}", path.display()))
}

/// Registers the `.mo` asset type and loader.
pub struct ModelicaSourceAssetPlugin;

impl Plugin for ModelicaSourceAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<ModelicaSource>()
            .init_asset_loader::<ModelicaSourceLoader>();
    }
}
