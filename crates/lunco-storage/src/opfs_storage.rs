//! Origin Private File System (OPFS) backend for wasm — the web counterpart to
//! [`crate::FileStorage`] for **binary assets** (meshes, textures, DEMs), where
//! [`crate::WebStorage`]'s `localStorage`+hex (2× size, string-only) is unusable.
//!
//! # Why inherent methods, not `impl Storage`
//!
//! The [`Storage`](crate::Storage) trait requires `Send` futures (it's
//! `#[async_trait]` over `Send + Sync`). OPFS calls hold non-`Send` JS values
//! (`Promise`, `JsValue`, the handle types) **across `.await`**, so an OPFS
//! future can't be `Send`. Rather than fork the trait, `OpfsStorage` exposes
//! **inherent** `async fn`s with the same shape; callers drive them with
//! `wasm_bindgen_futures::spawn_local` (which, unlike a thread pool, does not
//! require `Send`). Native code keeps using the `Send` [`Storage`] trait +
//! `AsyncComputeTaskPool`. The shared caller (`lunco-networking`'s asset sync)
//! therefore diverges only at a one-line `#[cfg]` backend pick.
//!
//! # Handle mapping
//!
//! Deliberately reuses [`StorageHandle::File`] rather than adding an `Opfs`
//! variant — the path's `Normal` components become the OPFS directory/file tree
//! (`getDirectoryHandle`/`getFileHandle` with `{create:true}`), so the same
//! `File(path)` handle addresses the native FS on desktop and OPFS on web with no
//! enum change (and no exhaustiveness churn across the workspace). A relative
//! path (`scenarios/<id>/rover.glb`) is the intended input on web; absolute /
//! prefix / `..` components are ignored (OPFS has no ambient root or parent
//! traversal).
//!
//! # Non-blocking
//!
//! Writes go through the **async** `createWritable()` stream — legal on the main
//! thread and non-blocking (the browser streams to disk off-thread). The
//! worker-only synchronous `FileSystemSyncAccessHandle` is intentionally NOT
//! used, so no Web Worker is required.

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::{StorageError, StorageHandle, StorageResult};

/// OPFS-backed storage for wasm binary assets. Zero-sized; all state lives in
/// the origin's private file system, shared across every instance.
#[derive(Default)]
pub struct OpfsStorage;

impl OpfsStorage {
    /// Construct a fresh backend handle.
    pub fn new() -> Self {
        Self
    }

    /// Write `bytes` to the OPFS path addressed by `handle` (a
    /// [`StorageHandle::File`]), creating intermediate directories. Async +
    /// non-blocking (`createWritable`). See the module docs for the handle
    /// mapping and the "inherent, not `impl Storage`" rationale.
    pub async fn write(&self, handle: &StorageHandle, bytes: &[u8]) -> StorageResult<()> {
        let (dirs, file) = split_handle(handle)?;
        let dir = resolve_dir(&segments_root().await?, &dirs, true).await?;
        let file_handle = get_file(&dir, &file, true).await?;
        let writable = JsFuture::from(file_handle.create_writable())
            .await
            .map_err(js_err)?
            .dyn_into::<web_sys::FileSystemWritableFileStream>()
            .map_err(|_| unsupported("createWritable did not return a writable stream"))?;
        // `write_with_u8_array` copies the bytes into a browser-owned buffer; the
        // actual disk write is streamed asynchronously by the UA.
        let write_promise = writable.write_with_u8_array(bytes).map_err(js_err)?;
        JsFuture::from(write_promise).await.map_err(js_err)?;
        JsFuture::from(writable.close()).await.map_err(js_err)?;
        Ok(())
    }

    /// Read the full contents of the OPFS file addressed by `handle`.
    pub async fn read(&self, handle: &StorageHandle) -> StorageResult<Vec<u8>> {
        let (dirs, file) = split_handle(handle)?;
        let dir = resolve_dir(&segments_root().await?, &dirs, false).await?;
        let file_handle = get_file(&dir, &file, false).await?;
        let file = JsFuture::from(file_handle.get_file())
            .await
            .map_err(|_| StorageError::NotFound)?
            .dyn_into::<web_sys::File>()
            .map_err(|_| unsupported("getFile did not return a File"))?;
        let buf = JsFuture::from(file.array_buffer())
            .await
            .map_err(js_err)?;
        let array = js_sys::Uint8Array::new(&buf);
        Ok(array.to_vec())
    }

    /// Delete the OPFS file addressed by `handle`. [`StorageError::NotFound`]
    /// when the file (or any directory on its path) doesn't exist.
    pub async fn delete(&self, handle: &StorageHandle) -> StorageResult<()> {
        let (dirs, file) = split_handle(handle)?;
        let dir = resolve_dir(&segments_root().await?, &dirs, false).await?;
        JsFuture::from(dir.remove_entry(&file))
            .await
            .map_err(|_| StorageError::NotFound)?;
        Ok(())
    }

    /// Whether the OPFS file addressed by `handle` exists.
    pub async fn exists(&self, handle: &StorageHandle) -> bool {
        let Ok((dirs, file)) = split_handle(handle) else {
            return false;
        };
        let Ok(root) = segments_root().await else {
            return false;
        };
        let Ok(dir) = resolve_dir(&root, &dirs, false).await else {
            return false;
        };
        get_file(&dir, &file, false).await.is_ok()
    }
}

/// The OPFS root directory handle (`navigator.storage.getDirectory()`).
async fn segments_root() -> StorageResult<web_sys::FileSystemDirectoryHandle> {
    let navigator = web_sys::window()
        .map(|w| w.navigator())
        .ok_or_else(|| unsupported("no window (OPFS needs a browsing context)"))?;
    let manager = navigator.storage();
    JsFuture::from(manager.get_directory())
        .await
        .map_err(js_err)?
        .dyn_into::<web_sys::FileSystemDirectoryHandle>()
        .map_err(|_| unsupported("getDirectory did not return a directory handle"))
}

/// Split a `File` handle into `(intermediate dir names, final file name)`,
/// keeping only `Normal` path components (OPFS has no absolute root or `..`).
fn split_handle(handle: &StorageHandle) -> StorageResult<(Vec<String>, String)> {
    let path = handle
        .as_file_path()
        .ok_or_else(|| unsupported("OpfsStorage addresses File handles only"))?;
    let mut comps: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let file = comps.pop().ok_or(StorageError::NotFound)?;
    Ok((comps, file))
}

/// Walk (optionally creating) `segments` under `root`, returning the final dir.
async fn resolve_dir(
    root: &web_sys::FileSystemDirectoryHandle,
    segments: &[String],
    create: bool,
) -> StorageResult<web_sys::FileSystemDirectoryHandle> {
    let mut dir = root.clone();
    for seg in segments {
        let opts = get_options(create);
        let next = JsFuture::from(
            dir.get_directory_handle_with_options(seg, opts.unchecked_ref()),
        )
        .await
        .map_err(|e| if create { js_err(e) } else { StorageError::NotFound })?
        .dyn_into::<web_sys::FileSystemDirectoryHandle>()
        .map_err(|_| unsupported("getDirectoryHandle returned a non-directory"))?;
        dir = next;
    }
    Ok(dir)
}

/// Get (optionally creating) a file handle named `name` under `dir`.
async fn get_file(
    dir: &web_sys::FileSystemDirectoryHandle,
    name: &str,
    create: bool,
) -> StorageResult<web_sys::FileSystemFileHandle> {
    let opts = get_options(create);
    JsFuture::from(dir.get_file_handle_with_options(name, opts.unchecked_ref()))
        .await
        .map_err(|e| if create { js_err(e) } else { StorageError::NotFound })?
        .dyn_into::<web_sys::FileSystemFileHandle>()
        .map_err(|_| unsupported("getFileHandle returned a non-file"))
}

/// A `{create: bool}` options object, built via `Reflect` so it's independent of
/// web-sys's per-version option-setter API.
fn get_options(create: bool) -> JsValue {
    let o = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("create"),
        &JsValue::from_bool(create),
    );
    o.into()
}

fn js_err(e: JsValue) -> StorageError {
    StorageError::Io(std::io::Error::other(format!("opfs: {e:?}")))
}

fn unsupported(msg: &str) -> StorageError {
    StorageError::Unsupported(msg.to_string())
}

// NB: a `Storage` trait impl is intentionally absent — that trait's `Send`
// futures (`#[async_trait]` over `Send + Sync`) are incompatible with OPFS's
// non-`Send` JS futures. Callers use the inherent methods above under
// `spawn_local`. See the module docs.
