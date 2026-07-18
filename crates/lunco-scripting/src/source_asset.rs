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

/// Publish every loaded `.rhai` asset into the registry that backs `import`.
///
/// Registration is keyed by the asset's own canonical id
/// (`lunco_assets::asset_path::anchor_of`) — the same identity the `AssetServer`
/// loaded it under. So a script is importable by exactly the path that names it,
/// through whatever source it came from: `lunco://` for engine assets, `twin://`
/// for a campaign repo outside the engine tree — including content synced from a
/// peer, which mounts as an ordinary Twin root over its cache dir.
///
/// This is why the resolver needs no loading logic of its own: whatever the asset
/// system can reach, an `import` can reach, with no second discovery pass to keep
/// in step with the first.
///
/// `Modified` re-registers, so a hot-edited module is picked up — the resolver's
/// memo stores the source text it compiled and recompiles when it differs.
fn publish_rhai_sources(
    mut events: MessageReader<AssetEvent<RhaiSource>>,
    assets: Res<Assets<RhaiSource>>,
    asset_server: Res<AssetServer>,
    sources: Res<lunco_assets::script_source::ScriptSources>,
) {
    for ev in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = ev else {
            continue;
        };
        let (Some(src), Some(path)) = (assets.get(*id), asset_server.get_path(*id)) else {
            continue;
        };
        let canonical = lunco_assets::asset_path::anchor_of(&path);
        info!("[rhai] script available for import: {canonical}");
        sources.insert(canonical, src.text.clone());
    }
}

/// Plugin that registers the `.rhai` asset loader. Pulled in by
/// `LunCoScriptingPlugin`.
pub struct RhaiSourceAssetPlugin;

impl Plugin for RhaiSourceAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<RhaiSource>()
            .init_asset_loader::<RhaiSourceLoader>()
            // `ScriptSources` is inserted by `LunCoScriptingPlugin`, from the rhai
            // runtime that owns it. Gated on the resource so this loader stays
            // usable in a build without the rhai backend.
            .add_systems(
                Update,
                publish_rhai_sources
                    .run_if(resource_exists::<lunco_assets::script_source::ScriptSources>),
            );
    }
}
