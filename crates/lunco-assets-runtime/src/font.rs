//! Cross-platform font-byte loading.
//!
//! The workspace ships **DejaVu Sans** as its proportional fallback (math /
//! Greek / arrow coverage — see [`lunco_assets_core::dejavu_sans_path`]). Different crates
//! need its raw `.ttf` bytes at runtime: `lunco-theme` installs them into egui,
//! `lunco-usd-bevy` rasterises diagnostic labels from them, etc. Native reads
//! the cache file synchronously; web fetches the bundled copy over HTTP. This
//! shared loader keeps platform I/O out of each consumer.

use std::sync::mpsc::{Receiver, Sender};

/// Dataset registry identity for the shared application fallback font.
pub const DEJAVU_ASSET_ID: &str = "engine/fonts/dejavu_sans";

/// URL the web bundle serves the DejaVu Sans TTF at. `scripts/build_web.sh`
/// copies `<cache>/fonts/DejaVuSans.ttf` to `dist/<bin>/fonts/DejaVuSans.ttf`,
/// i.e. site-root `/fonts/DejaVuSans.ttf`.
pub const DEJAVU_WEB_URL: &str = "/fonts/DejaVuSans.ttf";

/// Start loading the DejaVu Sans TTF bytes, returning a receiver that yields
/// them exactly once.
///
/// * **Native** — reads [`lunco_assets_core::dejavu_sans_path`] synchronously; the bytes
///   are already queued on the channel when this returns, so the first poll
///   succeeds.
/// * **Web** — spawns an async `fetch` of [`DEJAVU_WEB_URL`]; the bytes arrive
///   on the channel a few frames later. Poll the receiver each frame
///   (`try_recv`) until it yields.
///
/// On failure the channel is simply left empty (a warning is logged), so
/// callers degrade gracefully — no font, no panic.
pub fn load_dejavu_sans_bytes(settings: &lunco_settings::DownloadSettings) -> Receiver<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    load_into(tx, settings);
    rx
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_methods)]
fn load_into(tx: Sender<Vec<u8>>, _settings: &lunco_settings::DownloadSettings) {
    // This crate owns native font-file reads; consumers start loading after
    // the engine asset registry reports the font installed.
    let path = match lunco_assets_core::dejavu_sans_path() {
        Ok(path) => path,
        Err(error) => {
            bevy::log::warn!("font root resolution failed: {error}");
            return;
        }
    };
    match std::fs::read(&path) {
        Ok(bytes) => {
            let _ = tx.send(bytes);
        }
        Err(e) => bevy::log::warn!(
            "[lunco-assets] DejaVu Sans not found at {}: {e} — text that needs \
             it will be missing",
            path.display()
        ),
    }
}

#[cfg(target_arch = "wasm32")]
fn load_into(tx: Sender<Vec<u8>>, settings: &lunco_settings::DownloadSettings) {
    let settings = settings.clone();
    wasm_bindgen_futures::spawn_local(async move {
        match lunco_assets_core::web_fetch::network_fetch_uncached(DEJAVU_WEB_URL, &settings).await
        {
            Ok(bytes) => {
                let _ = tx.send(bytes);
            }
            Err(error) => {
                bevy::log::warn!("[lunco-assets] font fetch {DEJAVU_WEB_URL}: {error}");
            }
        }
    });
}
