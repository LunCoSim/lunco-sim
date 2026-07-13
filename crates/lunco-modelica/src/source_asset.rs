//! Modelica source as a Bevy `Asset`.
//!
//! Domain code consuming `.mo` files (USD cosim, the experiments runner,
//! scripted-test fixtures) must go through `AssetServer::load(...)` — not
//! `std::fs::read_to_string`. wasm32 has no filesystem; routing through
//! the asset pipeline gives us one call shape that works on both targets,
//! plus hot reload and `AssetEvent`s for free. See
//! `docs/architecture/40-asset-io.md` for the rationale and the
//! workspace-wide policy this loader is one half of.
//!
//! Usage:
//!
//! ```ignore
//! let h: Handle<ModelicaSource> = asset_server.load("models/Balloon.mo");
//! commands.entity(e).insert(PendingModelicaSource(h));
//! // ...later, in a drain system:
//! if let Some(src) = sources.get(&pending.0) {
//!     channels.tx.send(ModelicaCommand::Compile { source: src.text.clone(), .. });
//! }
//! ```

use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::prelude::*;

/// The text contents of a `.mo` file, surfaced as an asset.
///
/// Kept deliberately dumb — no parse here. The cosim dispatcher and the
/// experiments runner already invoke `rumoca_phase_parse` against the
/// text, often with different lenient/strict knobs; pre-parsing in the
/// loader would either duplicate that work or force a one-size-fits-all
/// configuration on every consumer.
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
        let text = String::from_utf8(bytes)?;
        Ok(ModelicaSource { text })
    }

    fn extensions(&self) -> &[&str] {
        &["mo"]
    }
}

/// Read a path's UTF-8 text through the platform-portable storage backend.
///
/// The synchronous counterpart of the asset loader above, for the paths that do
/// **not** come from the asset pipeline: a file the user picked, a workspace
/// `.mo` dependency, a small JSON ledger. `std::fs::read_to_string` is banned in
/// this crate because it silently `Err`s in a browser; [`lunco_storage`] routes
/// the same call to `FileStorage` on native and `WebStorage` (localStorage) on
/// wasm, so one call site is correct on both targets.
pub fn read_text_sync(path: &std::path::Path) -> Result<String, String> {
    let bytes = lunco_storage::read_file_sync(path)
        .map_err(|e| format!("read failed `{}`: {e}", path.display()))?;
    String::from_utf8(bytes).map_err(|e| format!("non-utf8 text `{}`: {e}", path.display()))
}

/// Write UTF-8 text to a path through the platform-portable storage backend.
/// See [`read_text_sync`] — same rationale, write side (native `FileStorage`
/// does an atomic tmp+rename; wasm maps the path onto a localStorage key).
pub fn write_text_sync(path: &std::path::Path, text: &str) -> Result<(), String> {
    lunco_storage::write_file_sync(path, text.as_bytes())
        .map_err(|e| format!("write failed `{}`: {e}", path.display()))
}

/// Plugin that registers the `.mo` asset loader. Add once at app build —
/// usually pulled in by `ModelicaCorePlugin`, which composes it
/// idempotently so binaries that also add it directly don't double-register.
pub struct ModelicaSourceAssetPlugin;

impl Plugin for ModelicaSourceAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<ModelicaSource>()
            .init_asset_loader::<ModelicaSourceLoader>();
    }
}
