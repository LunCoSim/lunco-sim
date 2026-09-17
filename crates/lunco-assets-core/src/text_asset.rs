//! Generic UTF-8 text assets for small runtime-authored catalogs and policies.
//!
//! The asset type is intentionally language-neutral. Modelica and Rhai keep
//! their richer loaders, while JSON/TOML catalogs can use one asynchronous
//! Bevy path on native and wasm instead of a synchronous filesystem read or a
//! compiled-in snapshot.

use bevy::asset::{io::Reader, AssetLoader, LoadContext};
use bevy::prelude::*;

use crate::discovery::AssetManifest;

/// UTF-8 text loaded from the runtime asset tree.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct TextAsset {
    /// The decoded text.
    pub text: String,
}

/// Loader for small UTF-8 catalog/configuration files.
#[derive(Default, TypePath)]
pub struct TextAssetLoader;

impl AssetLoader for TextAssetLoader {
    type Asset = TextAsset;
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
        Ok(TextAsset {
            text: String::from_utf8(bytes)?,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["json", "toml"]
    }
}

/// One engine-library text asset discovered from the runtime manifest.
#[derive(Clone, Debug)]
pub struct TextAssetEntry {
    /// The logical asset path used with [`AssetServer::load`].
    pub asset_path: String,
    /// The handle for the asynchronously loaded text.
    pub handle: Handle<TextAsset>,
}

/// Runtime catalog of small text assets in the engine asset library.
///
/// This catalog answers only “which text assets exist?” and starts their
/// platform-neutral loads. Consumers inspect the loaded text and own their
/// semantic kind/schema, so this layer remains independent of Modelica,
/// tutorials, input, and policy types.
#[derive(Resource, Default)]
pub struct TextAssetCatalog {
    entries: Vec<TextAssetEntry>,
    ready: bool,
}

impl TextAssetCatalog {
    /// Whether the runtime manifest has been enumerated.
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// All discovered JSON/TOML text assets in the engine library.
    pub fn entries(&self) -> &[TextAssetEntry] {
        &self.entries
    }
}

fn discover_text_assets(
    mut catalog: ResMut<TextAssetCatalog>,
    manifest: Option<Res<AssetManifest>>,
    asset_server: Option<Res<AssetServer>>,
) {
    if catalog.ready {
        return;
    }
    let (Some(manifest), Some(asset_server)) = (manifest, asset_server) else {
        return;
    };
    if !manifest.ready() {
        return;
    }

    catalog.entries = manifest
        .rels()
        .iter()
        .filter(|path| {
            matches!(
                std::path::Path::new(path).extension().and_then(|ext| ext.to_str()),
                Some("json" | "toml")
            )
        })
        .map(|asset_path| TextAssetEntry {
            asset_path: asset_path.clone(),
            handle: asset_server.load(asset_path.clone()),
        })
        .collect();
    catalog.ready = true;
}

/// Registers the generic text asset type.
pub struct TextAssetPlugin;

impl Plugin for TextAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<TextAsset>()
            .init_asset_loader::<TextAssetLoader>()
            .init_resource::<TextAssetCatalog>()
            .add_systems(Update, discover_text_assets);
    }
}
