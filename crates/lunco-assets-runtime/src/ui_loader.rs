//! Shared egui byte loading through registered asset sources. Language
//! adapters provide typed addresses, never a reader or filesystem search.

use bevy::asset::AssetServer;
use lunco_assets_core::registered_read::AssetReadTarget;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Bytes {
    data: Arc<[u8]>,
    mime: Option<String>,
}
type ResultSlot = Arc<Mutex<Option<Result<Bytes, String>>>>;
type AddressResolver = dyn Fn(&str) -> Result<AssetReadTarget, String> + Send + Sync;

fn poll_result(slot: &ResultSlot) -> Option<Result<Bytes, String>> {
    match slot.try_lock() {
        Ok(result) => result.clone(),
        Err(std::sync::TryLockError::WouldBlock) => None,
        Err(std::sync::TryLockError::Poisoned(_)) => Some(Err(
            "asset result is unavailable because its lock is poisoned".into(),
        )),
    }
}

enum Slot {
    Pending(ResultSlot),
    Ready(Result<Bytes, String>),
}

/// One bounded, asynchronous loader for every language's resource addresses.
/// Cache lifetime is the egui context; `forget` discards pending publications
/// as well as completed results because workers only retain their own slot.
pub struct AssetBytesLoader {
    scheme: String,
    server: AssetServer,
    resolve: Arc<AddressResolver>,
    max_bytes: usize,
    cache: Mutex<HashMap<String, Slot>>,
}

impl AssetBytesLoader {
    pub fn new(
        scheme: impl Into<String>,
        server: AssetServer,
        max_bytes: usize,
        resolve: impl Fn(&str) -> Result<AssetReadTarget, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            scheme: scheme.into(),
            server,
            resolve: Arc::new(resolve),
            max_bytes,
            cache: Mutex::new(HashMap::new()),
        }
    }
}

impl egui::load::BytesLoader for AssetBytesLoader {
    fn id(&self) -> &str {
        &self.scheme
    }

    fn load(&self, ctx: &egui::Context, uri: &str) -> egui::load::BytesLoadResult {
        if !lunco_assets_path::uri_scheme(uri)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case(&self.scheme))
        {
            return Err(egui::load::LoadError::NotSupported);
        }
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| egui::load::LoadError::Loading("asset UI cache unavailable".into()))?;
        if let Some(Slot::Pending(slot)) = cache.get(uri) {
            let result = poll_result(slot);
            if let Some(result) = result {
                cache.insert(uri.to_owned(), Slot::Ready(result));
            }
        }
        if let Some(slot) = cache.get(uri) {
            return match slot {
                Slot::Ready(Ok(bytes)) => Ok(egui::load::BytesPoll::Ready {
                    size: None,
                    bytes: egui::load::Bytes::Shared(bytes.data.clone()),
                    mime: bytes.mime.clone(),
                }),
                Slot::Ready(Err(error)) => Err(egui::load::LoadError::Loading(error.clone())),
                Slot::Pending(_) => Ok(egui::load::BytesPoll::Pending { size: None }),
            };
        }
        let slot: ResultSlot = Arc::new(Mutex::new(None));
        cache.insert(uri.to_owned(), Slot::Pending(slot.clone()));
        drop(cache);
        let server = self.server.clone();
        let resolve = self.resolve.clone();
        let uri = uri.to_owned();
        let max_bytes = self.max_bytes;
        let ctx = ctx.clone();
        bevy::tasks::IoTaskPool::get()
            .spawn(async move {
                let result = match resolve(&uri) {
                    Ok(path) => {
                        let mime = mime_guess2::from_path(path.path())
                            .first_raw()
                            .map(str::to_owned);
                        path.read(&server, max_bytes).await.map(|bytes| Bytes {
                            data: Arc::from(bytes),
                            mime,
                        })
                    }
                    Err(error) => Err(error),
                };
                if let Ok(mut slot) = slot.lock() {
                    *slot = Some(result);
                }
                ctx.request_repaint();
            })
            .detach();
        Ok(egui::load::BytesPoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.remove(uri);
        }
    }
    fn forget_all(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }
    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .map(|cache| {
                cache
                    .values()
                    .map(|slot| match slot {
                        Slot::Ready(Ok(bytes)) => bytes.data.len(),
                        _ => 0,
                    })
                    .sum()
            })
            .unwrap_or(0)
    }
}

/// Shared UI resource budget, persisted through the application settings owner.
#[derive(bevy::prelude::Resource, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AssetUiSettings {
    pub max_resource_bytes: usize,
    pub max_text_preview_bytes: usize,
}
impl Default for AssetUiSettings {
    fn default() -> Self {
        Self {
            max_resource_bytes: 64 * 1024 * 1024,
            max_text_preview_bytes: 64 * 1024,
        }
    }
}
impl lunco_settings::SettingsSection for AssetUiSettings {
    const KEY: &'static str = "asset_ui";
    fn validate_section(&self) -> Result<(), String> {
        if self.max_resource_bytes == 0 || self.max_text_preview_bytes == 0 {
            return Err("UI asset byte budget must be positive".into());
        }
        Ok(())
    }
}

/// Install the shared maintained raster decoder, independent of any language.
pub fn install_image_decoders(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
}

/// Retire bytes, textures and the decoder instance together. A completed native
/// decode retains only its old cache Arc, so URI reuse cannot publish old pixels
/// into the replacement context's cache.
pub fn invalidate_resource_images(ctx: &egui::Context) {
    ctx.forget_all_images();
    replace_raster_decoder(ctx);
}

/// Explicit user refresh retires this resource's cached bytes and texture.
pub fn invalidate_resource_image(ctx: &egui::Context, uri: &str) {
    ctx.forget_image(uri);
    replace_raster_decoder(ctx);
}

fn replace_raster_decoder(ctx: &egui::Context) {
    let loaders = ctx.loaders();
    let mut images = loaders.image.lock();
    images.retain(|loader| loader.id() != egui_extras::loaders::image_loader::ImageCrateLoader::ID);
    images.push(Arc::new(
        egui_extras::loaders::image_loader::ImageCrateLoader::default(),
    ));
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    #[test]
    fn failed_result_slot_is_terminal() {
        let slot: ResultSlot = Arc::new(Mutex::new(None));
        assert!(poll_result(&slot).is_none());
        let worker_slot = slot.clone();
        let worker = std::thread::spawn(move || {
            let _guard = worker_slot.lock().unwrap();
            panic!("injected worker failure while publishing");
        });
        assert!(worker.join().is_err());
        assert!(
            matches!(poll_result(&slot), Some(Err(message)) if message.contains("lock is poisoned"))
        );
    }
}
