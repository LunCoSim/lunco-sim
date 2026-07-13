//! Custom `modelica://` image loader for `egui_extras`.
//!
//! Modelica documentation embeds images with the
//! [`modelica://`](https://specification.modelica.org/master/annotations.html#documentation)
//! URI scheme:
//!
//! ```html
//! <img src="modelica://Modelica/Resources/Images/Rotational/PID.png"/>
//! ```
//!
//! The standard egui image loaders don't know this scheme. Until we
//! register ours they'd log `unsupported uri scheme`, leaving the
//! image placeholder empty. This loader rewrites the URI to a
//! filesystem path under [`lunco_assets::msl_dir`] (the on-disk MSL
//! tree) and hands the bytes off to egui's cached decoder.
//!
//! Non-`modelica://` URIs are left to the other loaders installed by
//! [`egui_extras::install_image_loaders`] — this one returns
//! `LoadError::NotSupported` for anything it doesn't handle.
//!
//! # One path, both targets
//!
//! Bytes are fetched through [`lunco_assets::msl::msl_read`] — the MSL
//! *virtual* filesystem — not `std::fs`. `msl_read` resolves a
//! bundle-relative path (`Modelica/Resources/Images/…`) against whichever MSL
//! root is installed: the on-disk tree on native, the in-memory bundle the web
//! fetcher unpacked on wasm. So this loader compiles and runs identically on
//! both targets, with no `#[cfg]` in the load path.
//!
//! **Known gap (not in this crate):** the *web* MSL bundle currently carries
//! only `.mo` sources — `crates/lunco-assets/src/bin/build_msl_assets.rs`
//! explicitly skips `Resources/` (images, matrix data). Until that bundler ships
//! them, `msl_read` returns `None` in the browser and Documentation images fall
//! back to their `alt` text, exactly as before. The fix is one step in the
//! bundler, and this loader needs no change when it lands.

use bevy_egui::egui;
use std::sync::{Arc, Mutex};

/// MIME type inferred from the URI's extension. Handed to egui's
/// image-decoder chain so it picks the right backend without having
/// to sniff the bytes. `None` for unknown extensions — the decoder
/// falls back to content sniffing.
fn mime_for(uri: &str) -> Option<String> {
    let ext = uri.rsplit('.').next().map(|s| s.to_ascii_lowercase())?;
    let m = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => return None,
    };
    Some(m.to_string())
}

/// Per-URI state. `Pending` entries carry a shared slot the worker
/// thread fills when the disk read completes; the loader checks it
/// on subsequent polls and promotes to `Ready` without ever
/// touching the filesystem from the UI thread.
enum Slot {
    /// Worker thread spawned; result not yet available.
    Pending(Arc<Mutex<Option<Result<Arc<[u8]>, String>>>>),
    /// Bytes ready — served on every future poll.
    Ready(Arc<[u8]>),
    /// Terminal error — served on every future poll (no retry).
    Failed(String),
}

/// Image loader that resolves `modelica://Package/sub/path.png`
/// against the on-disk MSL tree.
///
/// Every disk read runs on a background thread: first poll spawns
/// the worker and returns `BytesPoll::Pending`; egui's image loader
/// then re-polls each frame, and the first poll after the worker
/// completes promotes the slot to `Ready`. This keeps the UI thread
/// free — large SVGs, cold-cache Documentation assets, and burst
/// loads (e.g. re-layout after a window resize) don't stall the
/// next frame. The in-process cache covers hot reads so the same
/// image never hits disk twice.
pub struct ModelicaImageLoader {
    /// `modelica://…` URI → current load state.
    cache: Mutex<std::collections::HashMap<String, Slot>>,
}

impl ModelicaImageLoader {
    /// Singleton-style constructor. The returned value is cheap to
    /// clone (`Arc`); we register it once at startup.
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(Default::default()),
        }
    }

    /// Eagerly populate the cache with the bytes for every URI in
    /// `uris`. Spawned on `IoTaskPool` so it runs entirely in the
    /// background — no UI work, no main-thread blocking.
    ///
    /// Pre-warming the icon cache eliminates the visible "node has
    /// no icon for ~2 seconds after Add" gap that the optimistic
    /// synth path exhibits when a fresh MSL component first appears
    /// on the canvas. With the bytes already in `Slot::Ready`, the
    /// next paint just reads them — no async file load, no decode
    /// hitch, no perceived freeze.
    pub fn prewarm_uris(self: Arc<Self>, uris: Vec<String>) {
        bevy::tasks::IoTaskPool::get()
            .spawn(async move {
                let t0 = web_time::Instant::now();
                let mut loaded = 0usize;
                let mut failed = 0usize;
                let mut total_bytes = 0usize;
                for uri in uris.iter() {
                    // Skip if already cached (don't re-read).
                    if self.cache.lock().ok()
                        .and_then(|m| m.get(uri).map(|_| ()))
                        .is_some()
                    {
                        continue;
                    }
                    let Some(path) = Self::resolve_uri(uri) else {
                        continue;
                    };
                    match lunco_assets::msl::msl_read(&path) {
                        Some(bytes) => {
                            let arc: Arc<[u8]> = Arc::from(bytes);
                            total_bytes += arc.len();
                            loaded += 1;
                            if let Ok(mut cache) = self.cache.lock() {
                                cache.insert(
                                    uri.clone(),
                                    Slot::Ready(arc),
                                );
                            }
                        }
                        None => {
                            failed += 1;
                        }
                    }
                }
                bevy::log::info!(
                    "[ModelicaImageLoader] prewarm: {loaded} loaded ({} KB), {failed} failed, in {:.1}s",
                    total_bytes / 1024,
                    t0.elapsed().as_secs_f64(),
                );
            })
            .detach();
    }

    /// Resolve `modelica://Modelica/Resources/…` → an **MSL-root-relative**
    /// path (`Modelica/Resources/…`), the key
    /// [`lunco_assets::msl::msl_read`] takes. The root itself (a directory on
    /// native, the in-memory bundle on wasm) is the storage layer's business,
    /// not ours — which is what makes this one function correct on both
    /// targets.
    ///
    /// Returns `None` for non-`modelica://` URIs and for any path segment `..`
    /// (defence-in-depth: a malformed URI must not climb out of the MSL root).
    fn resolve_uri(uri: &str) -> Option<std::path::PathBuf> {
        let rest = uri.strip_prefix("modelica://")?;
        // Strip a leading "Modelica/" if present so both
        // `modelica://Modelica/Resources/…` (MSL-internal) and
        // `modelica:///Resources/…` resolve the same way.
        let rel: &str = rest.strip_prefix("Modelica/").unwrap_or(rest);
        if rel.split('/').any(|seg| seg == "..") {
            return None;
        }
        Some(std::path::Path::new("Modelica").join(rel))
    }
}

impl Default for ModelicaImageLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl egui::load::BytesLoader for ModelicaImageLoader {
    fn id(&self) -> &str {
        "lunco.modelica.image_loader"
    }

    fn load(
        &self,
        ctx: &egui::Context,
        uri: &str,
    ) -> egui::load::BytesLoadResult {
        if !uri.starts_with("modelica://") {
            return Err(egui::load::LoadError::NotSupported);
        }

        // --- State machine, all transitions under one lock ---
        let mut cache = match self.cache.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };

        // Existing slot — promote Pending→Ready if the worker
        // finished, then serve.
        if let Some(slot) = cache.get(uri) {
            match slot {
                Slot::Ready(bytes) => {
                    return Ok(egui::load::BytesPoll::Ready {
                        size: None,
                        bytes: egui::load::Bytes::Shared(bytes.clone()),
                        mime: mime_for(uri),
                    });
                }
                Slot::Failed(msg) => {
                    return Err(egui::load::LoadError::Loading(msg.clone()));
                }
                Slot::Pending(slot_arc) => {
                    // Peek at the shared result slot. `try_lock` so
                    // we never block the UI on the worker's lock —
                    // if contended we just report Pending for this
                    // frame and check again next time.
                    let maybe_ready = slot_arc
                        .try_lock()
                        .ok()
                        .and_then(|guard| guard.clone());
                    match maybe_ready {
                        Some(Ok(bytes)) => {
                            cache.insert(uri.to_string(), Slot::Ready(bytes.clone()));
                            return Ok(egui::load::BytesPoll::Ready {
                                size: None,
                                bytes: egui::load::Bytes::Shared(bytes),
                                mime: mime_for(uri),
                            });
                        }
                        Some(Err(msg)) => {
                            cache.insert(uri.to_string(), Slot::Failed(msg.clone()));
                            return Err(egui::load::LoadError::Loading(msg));
                        }
                        None => {
                            return Ok(egui::load::BytesPoll::Pending { size: None });
                        }
                    }
                }
            }
        }

        // Fresh URI → resolve, spawn, return Pending.
        let path = match Self::resolve_uri(uri) {
            Some(p) => p,
            None => {
                let msg = format!("modelica:// URI outside MSL root: {uri}");
                cache.insert(uri.to_string(), Slot::Failed(msg.clone()));
                return Err(egui::load::LoadError::Loading(msg));
            }
        };

        // Shared result slot written by the worker once the read
        // completes. Spawned on Bevy's `IoTaskPool` so this compiles
        // on wasm32 (where `std::thread::spawn` is unsupported).
        let result: Arc<Mutex<Option<Result<Arc<[u8]>, String>>>> =
            Arc::new(Mutex::new(None));
        cache.insert(uri.to_string(), Slot::Pending(result.clone()));
        drop(cache);

        let uri_for_worker = uri.to_string();
        // Kick egui to repaint when the bytes land — otherwise a
        // pending image wouldn't drive any redraw and the user
        // would have to move the mouse to see it appear.
        let ctx = ctx.clone();
        bevy::tasks::IoTaskPool::get().spawn(async move {
            // MSL virtual FS: on-disk tree (native) or in-memory bundle (web).
            let read_result: Result<Arc<[u8]>, String> =
                match lunco_assets::msl::msl_read(&path) {
                    Some(bytes) => {
                        log::info!(
                            "[ModelicaImageLoader] loaded {} → {} ({} bytes)",
                            uri_for_worker,
                            path.display(),
                            bytes.len(),
                        );
                        Ok(Arc::from(bytes))
                    }
                    None => {
                        log::warn!(
                            "[ModelicaImageLoader] not found in any MSL root: {} → {}",
                            uri_for_worker,
                            path.display(),
                        );
                        Err(format!(
                            "modelica:// image not found in the MSL root ({})",
                            path.display(),
                        ))
                    }
                };
            if let Ok(mut guard) = result.lock() {
                *guard = Some(read_result);
            }
            ctx.request_repaint();
        }).detach();

        Ok(egui::load::BytesPoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        if let Ok(mut c) = self.cache.lock() {
            c.remove(uri);
        }
    }

    fn forget_all(&self) {
        if let Ok(mut c) = self.cache.lock() {
            c.clear();
        }
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .map(|c| {
                c.values()
                    .map(|s| match s {
                        Slot::Ready(b) => b.len(),
                        _ => 0,
                    })
                    .sum::<usize>()
            })
            .unwrap_or(0)
    }
}
