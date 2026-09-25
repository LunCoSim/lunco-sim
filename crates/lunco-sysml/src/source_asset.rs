//! SysML source as a Bevy asset.
//!
//! The loader only admits UTF-8 text. It does not parse: all semantic work is
//! routed through [`lunco_sysml_ast::SysmlAnalysis`] so editor, test, and
//! headless paths cannot drift into separate parser behavior.

use bevy::asset::{Asset, AssetLoader, LoadContext, io::Reader};
use bevy::prelude::*;

/// The text contents of a `.sysml` or `.kerml` source file.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct SysmlSource {
    /// UTF-8 source text.
    pub text: std::sync::Arc<str>,
}

/// Bevy loader for SysML source files.
#[derive(Default, TypePath)]
pub struct SysmlSourceLoader;

impl AssetLoader for SysmlSourceLoader {
    type Asset = SysmlSource;
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
        Ok(SysmlSource {
            text: std::sync::Arc::from(std::str::from_utf8(&bytes)?),
        })
    }

    fn extensions(&self) -> &[&str] {
        &["sysml", "kerml"]
    }
}

/// Registers the SysML text asset and loader.
pub struct SysmlSourceAssetPlugin;

impl Plugin for SysmlSourceAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<SysmlSource>()
            .init_asset_loader::<SysmlSourceLoader>();
    }
}
