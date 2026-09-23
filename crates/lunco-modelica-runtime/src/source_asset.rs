//! Modelica source as a Bevy `Asset`.
//!
//! Domain code consuming `.mo` files must go through `AssetServer::load(...)`;
//! this keeps native and wasm source loading on one platform-portable path and
//! gives callers hot reload and `AssetEvent`s without coupling them to the
//! compiler worker.

use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use bevy::tasks::AsyncComputeTaskPool;
use lunco_modelica_ast::ast_extract::{parse_model_interface, ModelInterface};
use lunco_modelica_ast::normalize_modelica_source;

/// The text contents of a `.mo` file, surfaced as an asset.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct ModelicaSource {
    /// Raw `.mo` text. UTF-8 (the loader rejects non-UTF-8 inputs).
    pub text: String,
    /// Interface facts prepared from this exact source revision.
    pub interface: ModelInterface,
}

impl ModelicaSource {
    fn prepare(text: String) -> Self {
        let interface = parse_model_interface(&text, "modelica-source-asset.mo");
        Self { text, interface }
    }
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
        // Asset loaders run on Bevy's I/O pool. Native builds move CPU-heavy
        // parsing to the existing async-compute pool so a large Modelica file
        // cannot stall other asset I/O or the app schedule.
        #[cfg(not(target_arch = "wasm32"))]
        let source = AsyncComputeTaskPool::get()
            .spawn(async move { ModelicaSource::prepare(text) })
            .await;
        // Bevy's wasm task pools use the browser main thread. Keep this single
        // source-boundary parse shared by consumers; routing it through the
        // Modelica Web Worker remains necessary for fully non-blocking web loads.
        #[cfg(target_arch = "wasm32")]
        let source = ModelicaSource::prepare(text);
        Ok(source)
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

#[cfg(test)]
mod tests {
    use super::ModelicaSource;

    #[test]
    fn prepared_source_keeps_text_and_derives_interface_once() {
        let source = ModelicaSource::prepare(
            concat!(
                "within Demo;\n",
                "model Rover\n",
                "  parameter Real mass = 12.0;\n",
                "  input Real drive;\n",
                "  input Real gain = 2.5;\n",
                "  output Real speed;\n",
                "equation\n",
                "  speed = gain * drive;\n",
                "end Rover;\n",
            )
            .to_string(),
        );

        assert!(source.text.starts_with("within Demo;"));
        assert_eq!(source.interface.model_name.as_deref(), Some("Rover"));
        assert_eq!(source.interface.within.as_deref(), Some("Demo"));
        assert_eq!(source.interface.parameters.get("mass"), Some(&12.0));
        assert!(source.interface.inputs.contains_key("drive"));
        assert_eq!(source.interface.inputs.get("drive"), Some(&0.0));
        assert_eq!(source.interface.input_defaults.get("gain"), Some(&2.5));
        assert!(!source.interface.input_defaults.contains_key("drive"));
        assert!(source.interface.outputs.contains("speed"));
    }
}
