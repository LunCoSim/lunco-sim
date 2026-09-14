//! Cross-platform font-byte loading.
//!
//! The workspace ships **DejaVu Sans** as its proportional fallback (math /
//! Greek / arrow coverage — see [`crate::dejavu_sans_path`]). Different crates
//! need its raw `.ttf` bytes at runtime: `lunco-theme` installs them into egui,
//! `lunco-usd-bevy` rasterises diagnostic labels from them, etc. The *loading
//! procedure* differs by platform — native reads the cache file synchronously,
//! web has no filesystem and must `fetch()` the bundled copy over HTTP — so it
//! lives here once instead of being re-implemented per consumer.

use std::sync::mpsc::{Receiver, Sender};

/// URL the web bundle serves the DejaVu Sans TTF at. `scripts/build_web.sh`
/// copies `<cache>/fonts/DejaVuSans.ttf` to `dist/<bin>/fonts/DejaVuSans.ttf`,
/// i.e. site-root `/fonts/DejaVuSans.ttf`.
pub const DEJAVU_WEB_URL: &str = "/fonts/DejaVuSans.ttf";

/// Start loading the DejaVu Sans TTF bytes, returning a receiver that yields
/// them exactly once.
///
/// * **Native** — reads [`crate::dejavu_sans_path`] synchronously; the bytes
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
    // One-shot startup font read — a direct `std::fs::read` is correct here
    // (this crate is the I/O boundary; the wasm path below replaces it).
    let path = crate::dejavu_sans_path();
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
        match crate::web_fetch::network_fetch_uncached(DEJAVU_WEB_URL, &settings).await {
            Ok(bytes) => {
                let _ = tx.send(bytes);
            }
            Err(error) => {
                bevy::log::warn!("[lunco-assets] font fetch {DEJAVU_WEB_URL}: {error}");
            }
        }
    });
}
