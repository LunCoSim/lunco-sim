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
//! [`LoadError::NotSupported`] for anything it doesn't handle.
//!
//! # TODO: web target
//!
//! On `wasm32-unknown-unknown` the browser sandbox blocks
//! `std::fs::read`, and [`lunco_assets::msl_dir`] points at a
//! filesystem path that doesn't exist on the client. Options for
//! the web build:
//!
//! 1. **Bundle MSL image assets at build time** — `include_bytes!`
//!    each file into a static `HashMap<uri, &[u8]>`. Simplest, but
//!    adds a few MB to the wasm bundle. Worth it only for the
//!    frequently-opened examples.
//! 2. **Fetch over HTTP at runtime** — serve `.cache/msl/` under
//!    `/assets/msl/` on the static host, rewrite `modelica://` URIs
//!    to `/assets/msl/…` paths, let `egui_extras`'s http loader do
//!    the work. Zero bundle cost, needs CORS + hosting alignment.
//! 3. **Accept broken images on web for v1** — render the `alt`
//!    text, skip the raster.
//!
//! The `cfg(not(target_arch = "wasm32"))` gate on the body of
//! [`ModelicaImageLoader::load`] is the obvious place to branch
//! once we pick an approach. For now the loader is native-only; on
//! wasm it'll silently return `NotSupported` for every
//! `modelica://` URI, yielding the alt-text fallback (option 3).

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

/// Image loader that resolves `modelica://Package/sub/path.png`
/// against the on-disk MSL tree. Bytes are cached in-memory after the
/// first read — a typical Modelica doc reuses the same image in
/// multiple classes, and re-reading from disk per frame would stutter.
pub struct ModelicaImageLoader {
    /// `modelica://…` URI → file bytes, populated lazily.
    cache: Mutex<std::collections::HashMap<String, Arc<[u8]>>>,
}

impl ModelicaImageLoader {
    /// Singleton-style constructor. The returned value is cheap to
    /// clone (`Arc`); we register it once at startup.
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(Default::default()),
        }
    }

    /// Resolve `modelica://Modelica/Resources/…` → filesystem path.
    /// Returns `None` for URIs in unknown packages or paths that
    /// escape the MSL root (defence-in-depth against a malformed
    /// `..` traversal).
    fn resolve_uri(uri: &str) -> Option<std::path::PathBuf> {
        let rest = uri.strip_prefix("modelica://")?;
        // Strip a leading "Modelica/" if present so both
        // `modelica://Modelica/Resources/…` (MSL-internal) and
        // `modelica:///Resources/…` resolve the same way.
        let rel: &str = rest.strip_prefix("Modelica/").unwrap_or(rest);
        let root = lunco_assets::msl_dir().join("Modelica");
        let joined = root.join(rel);
        // Canonical-path check: refuse anything that climbed out of
        // the MSL root via `..`. Skip when canonicalisation fails
        // (file doesn't exist yet) — the subsequent read will surface
        // an NotFound error naturally.
        match (joined.canonicalize(), root.canonicalize()) {
            (Ok(j), Ok(r)) if j.starts_with(&r) => Some(j),
            (Ok(_), Ok(_)) => None,
            _ => Some(joined),
        }
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
        _ctx: &egui::Context,
        uri: &str,
    ) -> egui::load::BytesLoadResult {
        if !uri.starts_with("modelica://") {
            return Err(egui::load::LoadError::NotSupported);
        }
        // Web builds can't read the disk. See module docs for the
        // three options under review (bundle / fetch / alt-text).
        // Until we pick one, wasm falls through to the alt-text
        // fallback via NotSupported.
        #[cfg(target_arch = "wasm32")]
        {
            let _ = uri;
            return Err(egui::load::LoadError::NotSupported);
        }
        // Cache hit → return immediately.
        if let Some(bytes) = self.cache.lock().ok().and_then(|c| c.get(uri).cloned()) {
            return Ok(egui::load::BytesPoll::Ready {
                size: None,
                bytes: egui::load::Bytes::Shared(bytes),
                mime: mime_for(uri),
            });
        }
        // Miss: resolve + read. Small and sync — the cache covers
        // subsequent reads, and the first-frame-per-image cost is
        // bounded by disk I/O which is sub-millisecond for MSL's
        // small PNG assets.
        let path = match Self::resolve_uri(uri) {
            Some(p) => p,
            None => return Err(egui::load::LoadError::Loading(
                format!("modelica:// URI outside MSL root: {uri}"),
            )),
        };
        match std::fs::read(&path) {
            Ok(bytes) => {
                log::info!(
                    "[ModelicaImageLoader] loaded {} → {} ({} bytes)",
                    uri,
                    path.display(),
                    bytes.len(),
                );
                let arc: Arc<[u8]> = Arc::from(bytes);
                if let Ok(mut c) = self.cache.lock() {
                    c.insert(uri.to_string(), arc.clone());
                }
                Ok(egui::load::BytesPoll::Ready {
                    size: None,
                    bytes: egui::load::Bytes::Shared(arc),
                    mime: mime_for(uri),
                })
            }
            Err(e) => {
                log::warn!(
                    "[ModelicaImageLoader] read failed: {} → {}: {}",
                    uri,
                    path.display(),
                    e,
                );
                Err(egui::load::LoadError::Loading(format!(
                    "modelica:// image read failed ({}): {}",
                    path.display(),
                    e,
                )))
            }
        }
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
            .map(|c| c.values().map(|b| b.len()).sum::<usize>())
            .unwrap_or(0)
    }
}
