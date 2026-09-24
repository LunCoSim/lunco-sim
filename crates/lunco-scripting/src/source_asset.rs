//! Python source assets as Bevy `Asset`s.
//!
//! Rhai source assets belong to `lunco-scripting-rhai-runtime`, where the
//! interpreter-backed asset graph is installed. This module keeps the
//! language-neutral scripting package's optional Python source boundary small.

#[cfg(feature = "python")]
use bevy::asset::{Asset, AssetLoader, LoadContext, io::Reader};
#[cfg(feature = "python")]
use bevy::prelude::*;

/// Raw text of a `.py` file.
///
/// The Python backend compiles lazily on first execution. Keeping the asset as
/// a string keeps loading cheap; execution remains behind the `python` feature.
#[cfg(feature = "python")]
#[derive(Asset, TypePath, Debug, Clone)]
pub struct PythonSource {
    /// Raw `.py` text. UTF-8.
    pub text: String,
}

#[cfg(feature = "python")]
#[derive(Default, TypePath)]
pub struct PythonSourceLoader;

#[cfg(feature = "python")]
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

/// Registers the `.py` asset loader for the optional Python backend.
#[cfg(feature = "python")]
pub struct PythonSourceAssetPlugin;

#[cfg(feature = "python")]
impl Plugin for PythonSourceAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<PythonSource>()
            .init_asset_loader::<PythonSourceLoader>();
    }
}
