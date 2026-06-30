//! Python script source as a Bevy `Asset`.
//!
//! Symmetric to `lunco_modelica::source_asset::ModelicaSource`. Domain
//! code must route `.py` reads through `AssetServer::load(...)` rather
//! than `std::fs::read_to_string` — that path doesn't exist on wasm32.
//! See `docs/architecture/40-asset-io.md`.

use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::prelude::*;

/// Raw text of a `.py` file.
///
/// We don't pre-compile the script in the loader. `lunco_scripting::python`
/// owns Py bytecode and the compile happens lazily on first execution,
/// driven by `ScriptDocument`. Keeping the asset as a string keeps the
/// loader cheap and lets non-python consumers (linters, AI assistants)
/// share the same handle.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct PythonSource {
    /// Raw `.py` text. UTF-8.
    pub text: String,
}

#[derive(Default, TypePath)]
pub struct PythonSourceLoader;

impl AssetLoader for PythonSourceLoader {
    type Asset = PythonSource;
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
        let text = String::from_utf8(bytes)?;
        Ok(PythonSource { text })
    }

    fn extensions(&self) -> &[&str] {
        &["py"]
    }
}

/// Plugin that registers the `.py` asset loader. Pulled in by
/// `LunCoScriptingPlugin`.
pub struct PythonSourceAssetPlugin;

impl Plugin for PythonSourceAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<PythonSource>()
            .init_asset_loader::<PythonSourceLoader>();
    }
}

/// Raw text of a `.rhai` file — the file-backed twin of
/// [`lunco_core::EmbeddedScenarioSource`] (inline `lunco:script`). Lets a scene
/// reference a scenario by `lunco:scriptPath` and keep the source as an
/// editable, hot-reloadable `.rhai` file instead of a string baked into USD.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct RhaiSource {
    /// Raw `.rhai` text. UTF-8.
    pub text: String,
}

#[derive(Default, TypePath)]
pub struct RhaiSourceLoader;

impl AssetLoader for RhaiSourceLoader {
    type Asset = RhaiSource;
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
        let text = String::from_utf8(bytes)?;
        Ok(RhaiSource { text })
    }

    fn extensions(&self) -> &[&str] {
        &["rhai"]
    }
}

/// Plugin that registers the `.rhai` asset loader. Pulled in by
/// `LunCoScriptingPlugin`.
pub struct RhaiSourceAssetPlugin;

impl Plugin for RhaiSourceAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<RhaiSource>()
            .init_asset_loader::<RhaiSourceLoader>();
    }
}
